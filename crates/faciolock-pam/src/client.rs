//! The whole of what the module does: connect, send one message, read one message.
//!
//! ## Why this is not `UnixStream::connect`
//!
//! wiki/04 specifies `SOCK_SEQPACKET` so message boundaries come from the kernel. `std`
//! only ever creates `SOCK_STREAM`, and a `SOCK_STREAM` client cannot connect to a
//! `SOCK_SEQPACKET` listener at all — it fails with `EPROTOTYPE`. So the socket is created
//! through `libc` and handed to `std`, which owns the fd from that moment and closes it on
//! every path out. This mirrors `faciolock-daemon`'s `serve::listen` on the other side,
//! deliberately duplicated rather than shared: the eight lines are cheaper than a crate
//! that both a root-loaded module and the daemon would have to depend on.
//!
//! ## Every failure here ends at a password prompt
//!
//! There is no retry, no fallback path and no second socket. The daemon is either there
//! and answering or the user types their password, which is wiki/04's failure table read
//! as code.

use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use faciolock_ipc::{RESPONSE_LEN, Request, Response, Verdict};

#[derive(Debug)]
pub enum Failure {
    /// No socket, wrong permissions, nothing listening, or the daemon closed on us without
    /// answering — which is what its `SO_PEERCRED` gate does to a non-root peer.
    Unreachable(std::io::Error),
    /// The daemon accepted the request and did not answer inside the deadline.
    Timeout,
    /// Something answered, and it was not the daemon speaking our protocol.
    Malformed(&'static str),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Unreachable(e) => write!(f, "daemon unreachable: {e}"),
            Failure::Timeout => write!(f, "daemon did not answer in time"),
            Failure::Malformed(what) => write!(f, "malformed reply: {what}"),
        }
    }
}

/// Ask the daemon one question and take one answer.
///
/// `deadline` bounds **the whole exchange stage by stage** — connect, write and read each
/// get it — and that includes the connect for a reason that is not obvious. An `AF_UNIX`
/// `connect` does not always return immediately: if the listener's backlog is full the
/// kernel *sleeps* the caller until the daemon accepts. A daemon wedged in a stuck camera
/// ioctl stops accepting, eight queued PAM clients fill `BACKLOG`, and the ninth would
/// block in `connect` with no timeout on it at all — a hung lock screen, which wiki/03
/// names as the failure that matters most. Linux bounds that sleep by `SO_SNDTIMEO`, so
/// the timeouts are set on the socket **before** `connect` rather than after it.
pub fn ask(path: &Path, req: &Request, deadline: Duration) -> Result<Verdict, Failure> {
    let stream = connect(path, deadline).map_err(classify)?;

    let mut out = [0u8; faciolock_ipc::REQUEST_MAX];
    let n = req.encode(&mut out);
    // `write_all`'s loop never runs twice here: a `SOCK_SEQPACKET` write is atomic — the
    // whole message or an error, never a prefix. That is the property wiki/04 chose this
    // socket type for, and it is why nothing on either side does length-prefix framing.
    (&stream).write_all(&out[..n]).map_err(classify)?;

    // One byte over the legal reply, so an oversized one is *detectable* rather than
    // silently truncated into something that parses. The daemon reads requests the same way.
    let mut reply = [0u8; RESPONSE_LEN + 1];
    let got = (&stream).read(&mut reply).map_err(classify)?;
    if got == 0 {
        // A clean EOF: the daemon read the request and closed without answering, which is
        // what it does to an oversized datagram. The `SO_PEERCRED` refusal arrives here as
        // `ECONNRESET` instead — measured, not assumed — because the daemon closes with the
        // request still unread and the kernel sends a reset. Both land on `Unreachable` and
        // both end at a password prompt; the distinction only matters for the log.
        return Err(Failure::Unreachable(std::io::ErrorKind::UnexpectedEof.into()));
    }
    if got > RESPONSE_LEN {
        return Err(Failure::Malformed("longer than a response"));
    }
    Response::decode(&reply[..got]).map(|r| r.verdict).map_err(|_| Failure::Malformed("undecodable"))
}

fn classify(e: std::io::Error) -> Failure {
    match e.kind() {
        // `SO_RCVTIMEO` surfaces as one or the other depending on the platform's errno.
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Failure::Timeout,
        _ => Failure::Unreachable(e),
    }
}

fn connect(path: &Path, deadline: Duration) -> Result<UnixStream, std::io::Error> {
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_os_str().as_bytes();
    // Checked before the socket is created, so an over-long path costs nothing. A path that
    // does not fit would otherwise be silently truncated into a *different* path, which is
    // the one failure mode here that could connect to the wrong thing.
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
    // From here `std` owns the fd and closes it on the error path below.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    // Before `connect`, so `SO_SNDTIMEO` bounds the sleep a full backlog causes.
    stream.set_write_timeout(Some(deadline))?;
    stream.set_read_timeout(Some(deadline))?;
    let rc = unsafe {
        libc::connect(stream.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len)
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use faciolock_ipc::Op;
    use std::os::unix::net::UnixListener;

    /// A daemon that says whatever the test tells it to, on a real `SOCK_SEQPACKET` socket.
    ///
    /// Not a mock of the client: the client under test is the real one, doing real syscalls
    /// against a real kernel socket. Only the far end is ours.
    struct FakeDaemon {
        path: std::path::PathBuf,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    /// A `SOCK_SEQPACKET` listener at `path`. The daemon's own `serve::listen` is not
    /// reachable from here — different crate — so the eight lines are repeated once.
    fn bind_listener(path: &std::path::Path, backlog: libc::c_int) -> UnixListener {
        let _ = std::fs::remove_file(path);
        let fd =
            unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
        assert!(fd >= 0, "socket");
        let listener = unsafe { UnixListener::from_raw_fd(fd) };

        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let bytes = path.as_os_str().as_bytes();
        for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
            *dst = *src as libc::c_char;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;
        let rc = unsafe {
            libc::bind(listener.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len)
        };
        assert_eq!(rc, 0, "bind {}: {}", path.display(), std::io::Error::last_os_error());
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), backlog) }, 0, "listen");
        listener
    }

    impl FakeDaemon {
        /// `reply` gets the bytes of the request and returns what to send back, or `None`
        /// to close without answering.
        fn spawn(
            tag: &str,
            reply: impl FnOnce(&[u8]) -> Option<Vec<u8>> + Send + 'static,
        ) -> FakeDaemon {
            let path = std::env::temp_dir().join(format!("faciolock-pam-test-{tag}.sock"));
            let listener = bind_listener(&path, 4);

            let thread = std::thread::spawn(move || {
                let Ok((mut conn, _)) = listener.accept() else { return };
                let mut buf = [0u8; 64];
                let Ok(n) = conn.read(&mut buf) else { return };
                if let Some(out) = reply(&buf[..n]) {
                    let _ = conn.write_all(&out);
                }
            });
            FakeDaemon { path, thread: Some(thread) }
        }

        fn ask(&self) -> Result<Verdict, Failure> {
            let req = Request::new(Op::Auth, "csr", 2_000).expect("valid");
            ask(&self.path, &req, Duration::from_millis(500))
        }
    }

    impl Drop for FakeDaemon {
        fn drop(&mut self) {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn every_verdict_survives_the_round_trip() {
        for (i, verdict) in [
            Verdict::Allow,
            Verdict::Deny,
            Verdict::Unavailable,
            Verdict::NoEnrolment,
            Verdict::RateLimited,
        ]
        .into_iter()
        .enumerate()
        {
            let daemon = FakeDaemon::spawn(&format!("verdict{i}"), move |req| {
                assert_eq!(&req[..2], &[faciolock_ipc::VERSION, Op::Auth as u8]);
                Some(Response::new(verdict, 7).encode().to_vec())
            });
            assert_eq!(daemon.ask().expect("answered"), verdict);
        }
    }

    /// The daemon accepted and then said nothing. Without `SO_RCVTIMEO` this is a hung lock
    /// screen, which wiki/03 calls out as the failure mode that matters most.
    #[test]
    fn silence_becomes_a_timeout_not_a_hang() {
        let daemon = FakeDaemon::spawn("silent", |_| {
            std::thread::sleep(Duration::from_millis(1_500));
            None
        });
        let started = std::time::Instant::now();
        assert!(matches!(daemon.ask(), Err(Failure::Timeout)));
        assert!(started.elapsed() < Duration::from_millis(1_200), "did not honour the deadline");
    }

    /// What a non-root peer gets from the real daemon's `SO_PEERCRED` gate.
    #[test]
    fn a_close_without_an_answer_is_unreachable() {
        let daemon = FakeDaemon::spawn("eof", |_| None);
        assert!(matches!(daemon.ask(), Err(Failure::Unreachable(_))));
    }

    #[test]
    fn nothing_listening_is_unreachable_immediately() {
        let started = std::time::Instant::now();
        let req = Request::new(Op::Auth, "csr", 2_000).expect("valid");
        let out = ask(
            Path::new("/nonexistent/faciolock.sock"),
            &req,
            Duration::from_millis(5_000),
        );
        assert!(matches!(out, Err(Failure::Unreachable(_))));
        assert!(started.elapsed() < Duration::from_millis(100), "connect must not stall");
    }

    /// Garbage, an oversized datagram, and a truncated one all have to fail closed. Each is
    /// something a process that got hold of the socket path could send.
    #[test]
    fn a_reply_that_is_not_a_response_is_never_a_verdict() {
        for (tag, body) in [
            ("garbage", vec![0xff; 4]),
            ("oversized", vec![1u8; RESPONSE_LEN + 1]),
            ("truncated", vec![faciolock_ipc::VERSION, 1]),
            ("wrongversion", vec![faciolock_ipc::VERSION + 1, 1, 0, 0]),
            ("unknownverdict", vec![faciolock_ipc::VERSION, 9, 0, 0]),
        ] {
            let daemon = FakeDaemon::spawn(tag, move |_| Some(body));
            let out = daemon.ask();
            assert!(matches!(out, Err(Failure::Malformed(_))), "{tag}: got {out:?}");
        }
    }

    /// A daemon that is **alive but not accepting** — the case `set_read_timeout` alone
    /// does not cover.
    ///
    /// The daemon is serial (one connection at a time) with a backlog of 8, so a camera
    /// ioctl stuck in D-state stops it accepting and eight queued clients fill the queue.
    /// Without `SO_SNDTIMEO` set *before* `connect`, the ninth blocks in the kernel with no
    /// deadline on it and the lock screen hangs — which is the one failure mode wiki/03
    /// says must not exist.
    #[test]
    fn a_full_backlog_times_out_instead_of_blocking_forever() {
        let path = std::env::temp_dir().join("faciolock-pam-test-backlog.sock");
        let listener = bind_listener(&path, 1);

        // Fill the queue. Nothing ever accepts, so a connect eventually has to wait.
        let mut held = Vec::new();
        let mut the_queue_filled = false;
        for _ in 0..16 {
            match connect(&path, Duration::from_millis(150)) {
                Ok(s) => held.push(s),
                Err(e) => {
                    the_queue_filled = matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    );
                    break;
                }
            }
        }
        assert!(the_queue_filled, "the backlog never filled, so this proves nothing");

        let req = Request::new(Op::Auth, "csr", 0).expect("valid");
        let started = std::time::Instant::now();
        let out = ask(&path, &req, Duration::from_millis(200));
        assert!(matches!(out, Err(Failure::Timeout)), "got {out:?}");
        assert!(started.elapsed() < Duration::from_millis(900), "connect was not bounded");

        drop(held);
        drop(listener);
        let _ = std::fs::remove_file(&path);
    }

    /// A path too long for `sun_path` must be refused, not truncated into a different one.
    #[test]
    fn an_overlong_socket_path_is_refused() {
        let long = std::path::PathBuf::from(format!("/tmp/{}", "x".repeat(200)));
        let req = Request::new(Op::Auth, "csr", 0).expect("valid");
        assert!(matches!(
            ask(&long, &req, Duration::from_millis(10)),
            Err(Failure::Unreachable(_))
        ));
    }
}
