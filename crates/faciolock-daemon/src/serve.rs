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
//! ## Access control
//!
//! Two layers, and the second exists because the first is one `chmod` away from being
//! wrong. The socket is 0600 root-owned and only ever appears at its real path already at
//! that mode ([`listen`]); and [`Peer`] checks `SO_PEERCRED` **before the request body is
//! read**, so a non-root connection is closed having caused no parsing, no store lookup and
//! no camera activity (wiki/03 R2).
//!
//! Remote-session rejection is the PAM module's job rather than this one's: `PAM_RHOST` and
//! the SSH variables exist in the *caller's* process and never cross the socket.

use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Instant;

use faciolock_ipc::{REQUEST_MAX, Request, Response, Verdict};

use crate::Daemon;

const BACKLOG: libc::c_int = 8;

/// Bind, then `rename` into place — **not** `umask` around `bind`, and not `chmod` after it.
///
/// The socket must never be reachable at a looser mode, not even briefly, so `chmod` after
/// `bind` is out. `umask` closes that window but opens a worse one: umask is *process*
/// state, so for the duration of the `bind` every other thread's file creation inherits
/// 0177 too. That is not hypothetical — it corrupted an unrelated test's directory to
/// `drw-------` on the first run that called this function twice concurrently (B23).
///
/// Binding to a private name and renaming has neither problem. The socket is a filesystem
/// inode; `rename` is atomic, so the path either does not exist or is a 0600 listening
/// socket, with no third state. It also replaces the `remove_file` this used to need: a
/// stale socket from a crashed run is overwritten by the rename rather than unlinked in a
/// separate step that something else could win.
pub fn listen(path: &Path) -> Result<UnixListener, String> {
    // Per-process, so two daemons racing to start cannot collide on the staging name.
    let mut staging = path.as_os_str().to_os_string();
    staging.push(format!(".bind.{}", std::process::id()));
    let staging = std::path::PathBuf::from(staging);
    let _ = std::fs::remove_file(&staging);

    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(format!("socket: {}", std::io::Error::last_os_error()));
    }
    // From here the fd is owned by `listener`, so every `?` below closes it.
    let listener = unsafe { UnixListener::from_raw_fd(fd) };

    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = staging.as_os_str().as_bytes();
    if bytes.len() >= addr.sun_path.len() {
        return Err(format!("socket path is longer than {} bytes", addr.sun_path.len() - 1));
    }
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;

    let cleanup = |r: Result<(), String>| {
        // The staging inode outlives every failure below unless it is cleaned up here; a
        // /run littered with half-bound sockets is its own small operational problem.
        if r.is_err() {
            let _ = std::fs::remove_file(&staging);
        }
        r
    };

    if unsafe { libc::bind(listener.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len) }
        < 0
    {
        return Err(format!("bind {}: {}", staging.display(), std::io::Error::last_os_error()));
    }
    cleanup(
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {}: {e}", staging.display())),
    )?;
    cleanup(match unsafe { libc::listen(listener.as_raw_fd(), BACKLOG) } {
        0 => Ok(()),
        _ => Err(format!("listen: {}", std::io::Error::last_os_error())),
    })?;
    cleanup(
        std::fs::rename(&staging, path)
            .map_err(|e| format!("rename to {}: {e}", path.display())),
    )?;
    Ok(listener)
}

/// Accept forever. Never returns on a per-connection failure — a peer that hangs up mid
/// request must not take the daemon down with it.
pub fn run(listener: &UnixListener, daemon: &Daemon) {
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => handle(stream, daemon),
            Err(e) => eprintln!("faciolockd: accept: {e}"),
        }
    }
}

/// Who is on the other end, and whether they are still there.
///
/// `SO_PEERCRED` is the kernel's answer, not the peer's: it is recorded at `connect` time
/// and cannot be forged by anything the peer sends. That is what makes it worth having on
/// top of the file mode — wiki/03 R2's "the daemon never trusts a username from a non-root
/// peer" needs a fact about the connection, and this is the only one available.
pub struct Peer<'a> {
    stream: &'a UnixStream,
    uid: u32,
    pid: i32,
}

impl<'a> Peer<'a> {
    /// `std`'s `UnixStream::peer_cred` would be the ladder's answer here, but it is still
    /// unstable (rust-lang#42839) and this crate builds on stable. `getsockopt` is what it
    /// does anyway, and this file already reaches for `libc` to get `SOCK_SEQPACKET`.
    fn of(stream: &'a UnixStream) -> Result<Peer<'a>, std::io::Error> {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Peer { stream, uid: cred.uid, pid: cred.pid })
    }

    /// wiki/03 R2: only root may ask. The socket's 0600 mode says the same thing, and this
    /// is the layer that survives someone getting the mode wrong.
    fn is_root(&self) -> bool {
        self.uid == 0
    }

    /// Has the caller gone away?
    ///
    /// PAM gives up after its own deadline and the user types their password; the daemon
    /// should not then spend a camera and 300 ms of inference answering nobody. `POLLRDHUP`
    /// is the cheap way to ask — no read, no blocking, no state.
    ///
    /// ponytail: checked at stage boundaries, so an already-running burst still finishes
    /// and the camera can stay held up to ~400 ms past a cancel (decision P3-b). Making the
    /// capture itself interruptible needs a worker thread and a cancellation flag, and
    /// nothing has yet measured that hold as a problem.
    pub fn hung_up(&self) -> bool {
        let mut poll = libc::pollfd {
            fd: self.stream.as_raw_fd(),
            events: libc::POLLRDHUP,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut poll, 1, 0) };
        rc > 0 && poll.revents & (libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR) != 0
    }

    /// For the log. Phase 4's audit entry wants both fields.
    pub fn describe(&self) -> String {
        format!("pid {} uid {}", self.pid, self.uid)
    }
}

/// wiki/04: "daemon-side hard timeout regardless of what the client asked for".
///
/// The request is one datagram already in flight by the time the connection is accepted, so
/// this is not a budget for the peer to spend — it is the bound that stops a peer from not
/// spending it. The daemon is serial, so a client that connects and never sends would
/// otherwise park it in `read` forever and every later authentication would fail while the
/// camera sat held. Only root can reach the socket, so this is robustness rather than
/// access control; it is still two lines.
const PEER_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// `&UnixStream` reads and writes, so the connection can be borrowed by [`Peer`] for the
/// whole call rather than moved into each I/O.
fn handle(stream: UnixStream, daemon: &Daemon) {
    // **Before the body is read.** A peer that is not root is closed here having caused no
    // decode, no store lookup and no camera activity. No reply either: wiki/04 already
    // defines socket EOF as `PAM_AUTHINFO_UNAVAIL` on the caller's side, and answering
    // would turn the socket into an oracle that tells an unprivileged process whether a
    // given user is enrolled.
    let peer = match Peer::of(&stream) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("faciolockd: no peer credentials ({e}); closing");
            return;
        }
    };
    if !peer.is_root() {
        eprintln!("faciolockd: refused {} — only root may ask", peer.describe());
        return;
    }
    if let Err(e) = stream
        .set_read_timeout(Some(PEER_IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(PEER_IO_TIMEOUT)))
    {
        eprintln!("faciolockd: could not bound I/O on the connection ({e}); closing");
        return;
    }

    // One byte over the legal maximum, so an oversized datagram is *detectable*. `read`
    // truncates silently on `SOCK_SEQPACKET` and `std` exposes no `MSG_TRUNC`, so without
    // the extra byte a 200-byte message would arrive looking like a valid 37-byte one.
    let mut buf = [0u8; REQUEST_MAX + 1];
    let n = match (&stream).read(&mut buf) {
        Ok(0) => return, // peer hung up before sending anything
        Ok(n) => n,
        // A timeout here is a peer that connected and said nothing. Closing is the whole
        // recovery: the daemon goes back to accepting, and the next caller is answered.
        Err(e) => {
            eprintln!("faciolockd: {} sent nothing ({e}); closing", peer.describe());
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
        Ok(req) => daemon.answer(&req, &peer),
        Err(e) => {
            eprintln!("faciolockd: malformed request: {e}");
            Some(Verdict::Deny)
        }
    };
    // `None` is a cancelled request: the caller left, [`Daemon::answer`] logged it, and
    // there is nobody to tell. Phase 4's rate limiter must not count these — a user who
    // walks away from a lock screen has not failed an authentication.
    let Some(verdict) = verdict else { return };

    let elapsed = started.elapsed().as_millis().min(u16::MAX as u128) as u16;
    // `std` sets `SIGPIPE` to `SIG_IGN`, so a peer that vanished mid-answer surfaces here
    // as `EPIPE` rather than as a dead daemon. That is the same event as above, caught one
    // stage later, and it is logged the same way.
    if let Err(e) = (&stream).write_all(&Response::new(verdict, elapsed).encode()) {
        eprintln!("faciolockd: {} left before the answer ({e})", peer.describe());
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

    /// Both halves of [`Peer`] against a real accepted connection, because both are
    /// syscall behaviour rather than logic and a unit test of the logic would prove nothing.
    ///
    /// **The `uid == 0` accept path is not exercised here and cannot be**: the test process
    /// is unprivileged, which is exactly why the refusal half tests itself. What covers the
    /// accept path is the pam.d harness in wiki/09, where `sudo ./pam-try` connects as root.
    #[test]
    fn a_non_root_peer_is_refused_and_a_departed_one_is_noticed() {
        let path = std::env::temp_dir().join("faciolockd-peer-test.sock");
        let listener = listen(&path).expect("binds");

        let fd =
            unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
        assert!(fd >= 0, "socket");
        let client = unsafe { UnixStream::from_raw_fd(fd) };
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let bytes = path.as_os_str().as_bytes();
        for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
            *dst = *src as libc::c_char;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;
        let rc = unsafe {
            libc::connect(client.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len)
        };
        assert_eq!(rc, 0, "connect: {}", std::io::Error::last_os_error());

        let (server, _) = listener.accept().expect("accept");
        let peer = Peer::of(&server).expect("SO_PEERCRED");
        assert_eq!(peer.uid, unsafe { libc::getuid() }, "the kernel reports the real peer");
        assert!(!peer.is_root(), "an unprivileged peer must be refused");

        assert!(!peer.hung_up(), "the client is still connected");
        drop(client);
        assert!(peer.hung_up(), "a departed client must be detected, or the camera is held");

        drop(listener);
        let _ = std::fs::remove_file(&path);
    }
}
