//! The socket: `AF_UNIX` `SOCK_SEQPACKET`, one request at a time.
//!
//! ## Why `libc` for the listener
//!
//! wiki/04 specifies `SOCK_SEQPACKET` so message boundaries come from the kernel and there
//! is no length-prefix framing bug to write. `std`'s `UnixListener` only ever creates
//! `SOCK_STREAM`, so the socket is created and bound through `libc` and then handed to
//! `std` — which owns the fd from that moment and closes it on every error path. `accept`,
//! `read` and `write` are type-agnostic, so nothing below this function needs `unsafe`.
//!
//! ## Serial by design
//!
//! ponytail: one connection at a time. There is one camera; two concurrent authentications
//! would queue on it anyway, and a thread per connection is a denial-of-service knob for
//! anyone who can reach the socket. Upgrade path if it is ever needed is a bounded pool,
//! not a thread per peer.
//!
//! ## What is deliberately **not** here
//!
//! `SO_PEERCRED`, remote-session rejection and request cancellation are Phase 3 (wiki/09),
//! and they belong on this side of the socket, not in the PAM module. Until then the file
//! mode is the whole access control: 0600, set through `umask` around `bind` so the socket
//! never exists at a looser mode even briefly.

use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Instant;

use faciolock_ipc::{REQUEST_MAX, Request, Response, Verdict};

use crate::Daemon;

const BACKLOG: libc::c_int = 8;

pub fn listen(path: &Path) -> Result<UnixListener, String> {
    // A socket file left by a previous run makes `bind` fail with `EADDRINUSE`. Phase 6's
    // systemd socket unit removes this whole path by passing an already-bound fd.
    let _ = std::fs::remove_file(path);

    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(format!("socket: {}", std::io::Error::last_os_error()));
    }
    // From here the fd is owned by `listener`, so every `?` below closes it.
    let listener = unsafe { UnixListener::from_raw_fd(fd) };

    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= addr.sun_path.len() {
        return Err(format!("socket path is longer than {} bytes", addr.sun_path.len() - 1));
    }
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;

    // 0600 from the instant it exists. `chmod` after `bind` would leave a window at the
    // process umask, and the window is the interesting part for anything watching /run.
    let previous = unsafe { libc::umask(0o177) };
    let bound =
        unsafe { libc::bind(listener.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len) };
    unsafe { libc::umask(previous) };
    if bound < 0 {
        return Err(format!("bind {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    if unsafe { libc::listen(listener.as_raw_fd(), BACKLOG) } < 0 {
        return Err(format!("listen: {}", std::io::Error::last_os_error()));
    }
    Ok(listener)
}

/// Accept forever. Never returns on a per-connection failure — a peer that hangs up mid
/// request must not take the daemon down with it.
pub fn run(listener: &UnixListener, daemon: &mut Daemon) {
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => handle(stream, daemon),
            Err(e) => eprintln!("faciolockd: accept: {e}"),
        }
    }
}

fn handle(mut stream: UnixStream, daemon: &mut Daemon) {
    // One byte over the legal maximum, so an oversized datagram is *detectable*. `read`
    // truncates silently on `SOCK_SEQPACKET` and `std` exposes no `MSG_TRUNC`, so without
    // the extra byte a 200-byte message would arrive looking like a valid 37-byte one.
    let mut buf = [0u8; REQUEST_MAX + 1];
    let n = match stream.read(&mut buf) {
        Ok(0) => return, // peer hung up before sending anything
        Ok(n) => n,
        Err(e) => {
            eprintln!("faciolockd: read: {e}");
            return;
        }
    };
    if n > REQUEST_MAX {
        // Not a request. Close without answering; wiki/04 already defines socket EOF as
        // `PAM_AUTHINFO_UNAVAIL` on the caller's side.
        eprintln!("faciolockd: refused a {n}-byte datagram (max {REQUEST_MAX})");
        return;
    }

    let started = Instant::now();
    let verdict = match Request::decode(&buf[..n]) {
        Ok(req) => daemon.answer(&req),
        Err(e) => {
            eprintln!("faciolockd: malformed request: {e}");
            Verdict::Deny
        }
    };

    let elapsed = started.elapsed().as_millis().min(u16::MAX as u128) as u16;
    // `std` sets `SIGPIPE` to `SIG_IGN`, so a peer that vanished mid-answer surfaces here
    // as `EPIPE` rather than as a dead daemon.
    if let Err(e) = stream.write_all(&Response::new(verdict, elapsed).encode()) {
        eprintln!("faciolockd: write: {e}");
    }
}

/// The single place a `Match` becomes an `Allow`.
///
/// Exhaustive on purpose, in both directions: every non-`Match` pipeline verdict is named
/// here, so a variant added to `faciolock_vision::Verdict` later fails to compile rather
/// than falling into a catch-all next to the one arm that grants access.
///
/// The split between `Deny` and `Unavailable` is not cosmetic. `Deny` means somebody was
/// looked at and refused; `Unavailable` means the question could not be asked — a bright
/// room ([B18]), a camera in a video call, a pipeline failure. PAM falls through to the
/// password either way, and Phase 4's rate limiter reads
/// [`counts_as_an_attempt`](faciolock_vision::Verdict::counts_as_an_attempt) on the
/// *pipeline* verdict rather than this one, which is why no counting policy crosses the
/// wire (wiki/13).
///
/// [B18]: ../../../wiki/11-bug-log.md
pub fn to_wire(verdict: &faciolock_vision::Verdict) -> Verdict {
    use faciolock_vision::Verdict as V;
    match verdict {
        V::Match { .. } => Verdict::Allow,
        V::NoMatch { .. } | V::NotLive { .. } | V::NoFace => Verdict::Deny,
        V::Environment { .. } | V::Unusable(_) | V::Failed(_) => Verdict::Unavailable,
    }
}

/// `Status` asks whether authentication *could* work, and answers without touching the
/// camera or the models.
///
/// **It cannot return `Allow`.** wiki/09 wants one construction site for `PAM_SUCCESS`;
/// the same discipline says one construction site for `Allow`, and that site is
/// [`to_wire`]'s `Match` arm. A `Status` that answered `Allow` would mean a PAM module with
/// the wrong `op` byte could be granted access having done no biometric work at all.
pub fn status(enrolled: bool) -> Verdict {
    if enrolled { Verdict::Deny } else { Verdict::NoEnrolment }
}

/// Enrolment does not happen over this socket in Phase 2.
///
/// An unauthenticated enrolment endpoint is a write to the template store from anyone who
/// can reach the socket — an unlock, not a feature. It needs `SO_PEERCRED` (Phase 3) and
/// polkit (Phase 5) underneath it. Until then enrolment is `faciolockd --enroll`, which
/// requires local execution and write access to the store already.
pub fn enroll_over_the_wire() -> Verdict {
    Verdict::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use faciolock_vision::{BurstError, Check, Verdict as V};

    /// The one property this file exists to hold: nothing but a `Match` is an `Allow`, on
    /// any path a peer can reach.
    #[test]
    fn allow_has_exactly_one_source() {
        assert_eq!(to_wire(&V::Match { similarity: 0.9, agreeing: 2 }), Verdict::Allow);
        for denied in [
            V::NoMatch { best: 0.9 },
            V::NotLive { check: Check::DarkFrame, worst: 1.0, threshold: 1e-3 },
            V::NoFace,
            V::Environment { ambient_floor: 51.74 },
            V::Unusable(BurstError::Malformed),
            V::Failed("camera vanished".into()),
        ] {
            assert!(!to_wire(&denied).is_allow(), "{denied:?} must not reach Allow");
        }
        assert!(!status(true).is_allow(), "Status must never grant");
        assert!(!status(false).is_allow());
        assert!(!enroll_over_the_wire().is_allow());
    }

    /// A `Deny` for an unreadable request and an `Unavailable` for an unanswerable one are
    /// different facts, and Phase 4's audit log needs them to stay different.
    #[test]
    fn unavailable_means_the_question_was_never_asked() {
        assert_eq!(to_wire(&V::Environment { ambient_floor: 51.74 }), Verdict::Unavailable);
        assert_eq!(to_wire(&V::NoFace), Verdict::Deny);
    }

    /// The listener must actually be `SOCK_SEQPACKET`. `std` would happily hand back a
    /// working `SOCK_STREAM` listener, and the framing-free decode in `faciolock-ipc`
    /// depends on the difference.
    #[test]
    fn the_listener_is_seqpacket_and_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join("faciolockd-serve-test.sock");
        let listener = listen(&path).expect("binds");

        let mut kind: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                listener.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                &mut kind as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        assert_eq!(rc, 0, "getsockopt failed");
        assert_eq!(kind, libc::SOCK_SEQPACKET, "wiki/04 specifies SOCK_SEQPACKET");

        let mode = std::fs::metadata(&path).expect("socket exists").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the socket must never be group- or world-reachable");

        drop(listener);
        let _ = std::fs::remove_file(&path);
    }
}
