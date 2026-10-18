use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use faciolock_ipc::{Op, RESPONSE_LEN, REQUEST_MAX, Request, Response, Verdict};

pub const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct Answer {
    pub verdict: Verdict,
    pub daemon_ms: u16,
    pub round_trip: Duration,
}

pub fn ask(socket: &Path, op: Op, user: &str, deadline: Duration) -> Result<Answer, String> {
    let req = Request::new(op, user, 0)
        .map_err(|e| format!("{user:?} is not a usable username: {e}"))?;

    let started = Instant::now();
    let stream = connect(socket, deadline).map_err(|e| describe(socket, e))?;

    let mut out = [0u8; REQUEST_MAX];
    let n = req.encode(&mut out);
    (&stream).write_all(&out[..n]).map_err(|e| describe(socket, e))?;

    let mut reply = [0u8; RESPONSE_LEN + 1];
    let got = (&stream).read(&mut reply).map_err(|e| describe(socket, e))?;
    let round_trip = started.elapsed();

    if got == 0 {
        return Err("the daemon closed the connection without answering".into());
    }
    if got > RESPONSE_LEN {
        return Err("the daemon sent something longer than a response".into());
    }
    let response = Response::decode(&reply[..got]).map_err(|e| format!("undecodable reply: {e}"))?;
    Ok(Answer { verdict: response.verdict, daemon_ms: response.elapsed_ms, round_trip })
}

fn describe(socket: &Path, e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            format!("no socket at {} — is faciolockd running?", socket.display())
        }
        std::io::ErrorKind::ConnectionRefused => {
            format!("{} exists but nothing is listening — faciolockd is not running", socket.display())
        }
        // The socket went from 0600 root:root to 0660 root:faciolock-auth, so "run as
        // root" stopped being the whole answer -- and it is the wrong half to lead with for
        // anything a session runs.
        std::io::ErrorKind::PermissionDenied => format!(
            "{} refused this user — you are not root and not in the faciolock-auth group.\n\
             Add yourself with:  sudo gpasswd -a $USER faciolock-auth  (then log out and in)",
            socket.display()
        ),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            "the daemon did not answer in time".into()
        }
        _ => format!("{}: {e}", socket.display()),
    }
}

fn connect(path: &Path, deadline: Duration) -> Result<UnixStream, std::io::Error> {
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= addr.sun_path.len() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;

    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    stream.set_write_timeout(Some(deadline))?;
    stream.set_read_timeout(Some(deadline))?;
    let rc =
        unsafe { libc::connect(stream.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostile_username_never_reaches_a_socket() {
        for bad in ["ro ot", "../root", "a;b", ""] {
            let e = ask(Path::new("/nonexistent.sock"), Op::Auth, bad, DEADLINE)
                .expect_err("must not encode");
            assert!(e.contains("not a usable username"), "{bad:?} failed for the wrong reason: {e}");
        }
    }

    #[test]
    fn an_absent_daemon_says_so_in_words() {
        let e = ask(Path::new("/nonexistent/faciolock.sock"), Op::Status, "root", DEADLINE)
            .expect_err("nothing is listening");
        assert!(e.contains("is faciolockd running?"), "{e}");
    }
}
