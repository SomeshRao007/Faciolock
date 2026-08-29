//! Connect, send one message, read one message.
//!
//! ## Why this is a second copy
//!
//! `faciolock-pam/src/client.rs` does the same thing and its header says why it is not
//! shared: `std` only ever creates `SOCK_STREAM`, a `SOCK_STREAM` client cannot connect to a
//! `SOCK_SEQPACKET` listener at all, and the eight lines of `libc` that fix it are cheaper
//! than a crate a root-loaded PAM module would have to depend on. That argument gets
//! *stronger* with a third consumer, not weaker — the shared crate would now be in the tree
//! of a root module, a daemon and a CLI, and wiki/04's dependency rule is the thing it would
//! be putting at risk.
//!
//! What is genuinely shared is the part that matters: `faciolock_ipc` owns the wire format,
//! so the bytes cannot drift even though the socket boilerplate is written twice.
//!
//! ## The CLI's deadline is not the PAM module's
//!
//! PAM is behind a lock screen and gives up fast, because a slow answer is a hung lock
//! screen. `faciolockctl` is a person at a terminal who just typed a command and expects the
//! camera to be used — an enrolment pose is a full burst plus inference and can take well
//! over a second on a cold daemon. So the deadline here is generous where the module's is
//! tight, and that difference is the whole reason the two files do not want the same
//! constants either.

use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use faciolock_ipc::{Op, RESPONSE_LEN, REQUEST_MAX, Request, Response, Verdict};

/// Generous on purpose — see the module header. A cold daemon warms the camera on its first
/// request, which measured 339 ms for the warm alone plus the burst behind it.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// A verdict and how long the daemon said it took.
#[derive(Debug)]
pub struct Answer {
    pub verdict: Verdict,
    /// The daemon's own measurement, which excludes connect and scheduling. `bench` reports
    /// the round trip it measured itself instead; this is here so the two can be compared.
    pub daemon_ms: u16,
    /// What the CLI measured around the whole exchange.
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

    // One byte over the legal reply, so an oversized one is detectable rather than silently
    // truncated into something that parses. Both the daemon and the PAM module read this way.
    let mut reply = [0u8; RESPONSE_LEN + 1];
    let got = (&stream).read(&mut reply).map_err(|e| describe(socket, e))?;
    let round_trip = started.elapsed();

    if got == 0 {
        // The daemon read the request and closed without answering. Against this CLI that is
        // almost always the `SO_PEERCRED` gate, which is what running unprivileged looks like
        // from the client side — `main` checks for root first so the message stays accurate.
        return Err("the daemon closed the connection without answering".into());
    }
    if got > RESPONSE_LEN {
        return Err("the daemon sent something longer than a response".into());
    }
    let response = Response::decode(&reply[..got]).map_err(|e| format!("undecodable reply: {e}"))?;
    Ok(Answer { verdict: response.verdict, daemon_ms: response.elapsed_ms, round_trip })
}

/// Errors a person can act on. `ENOENT` on this path means one specific thing and saying
/// "No such file or directory" instead would send them to read the source.
fn describe(socket: &Path, e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            format!("no socket at {} — is faciolockd running?", socket.display())
        }
        std::io::ErrorKind::ConnectionRefused => {
            format!("{} exists but nothing is listening — faciolockd is not running", socket.display())
        }
        std::io::ErrorKind::PermissionDenied => {
            format!("{} is not readable by this user — run as root", socket.display())
        }
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            "the daemon did not answer in time".into()
        }
        _ => format!("{}: {e}", socket.display()),
    }
}

/// Mirrors `faciolock-daemon`'s `serve::listen` on the other side of the socket.
fn connect(path: &Path, deadline: Duration) -> Result<UnixStream, std::io::Error> {
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_os_str().as_bytes();
    // Checked before the socket is created. A path that does not fit would otherwise be
    // truncated into a *different* path, which is the one failure here that could connect to
    // the wrong thing.
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
    // From here `std` owns the fd and closes it on every path out.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    // Before `connect`: an `AF_UNIX` connect against a listener whose backlog is full does
    // not fail, it sleeps, and `SO_SNDTIMEO` is what bounds that sleep.
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

    /// The daemon refuses a name outside the POSIX portable set at the wire boundary. The CLI
    /// must not be the thing that discovers this by getting a connection refused — it takes
    /// argv, which is where a hostile name would arrive from.
    #[test]
    fn a_hostile_username_never_reaches_a_socket() {
        for bad in ["ro ot", "../root", "a;b", ""] {
            let e = ask(Path::new("/nonexistent.sock"), Op::Auth, bad, DEADLINE)
                .expect_err("must not encode");
            assert!(e.contains("not a usable username"), "{bad:?} failed for the wrong reason: {e}");
        }
    }

    /// An absent daemon is the single most common thing to hit, and the message has to say so
    /// rather than repeat errno at somebody.
    #[test]
    fn an_absent_daemon_says_so_in_words() {
        let e = ask(Path::new("/nonexistent/faciolock.sock"), Op::Status, "root", DEADLINE)
            .expect_err("nothing is listening");
        assert!(e.contains("is faciolockd running?"), "{e}");
    }
}
