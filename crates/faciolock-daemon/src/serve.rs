// ponytail: serial; one camera

use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Instant;

use faciolock_ipc::{Op, REQUEST_MAX, Request, Response, Verdict};

use crate::Daemon;

const BACKLOG: libc::c_int = 8;

pub fn listen(path: &Path) -> Result<UnixListener, String> {
    let mut staging = path.as_os_str().to_os_string();
    staging.push(format!(".bind.{}", std::process::id()));
    let staging = std::path::PathBuf::from(staging);
    let _ = std::fs::remove_file(&staging);

    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(format!("socket: {}", std::io::Error::last_os_error()));
    }
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

const LISTEN_FDS_START: libc::c_int = 3;

pub fn inherited() -> Result<Option<UnixListener>, String> {
    let Ok(pid) = std::env::var("LISTEN_PID") else {
        return Ok(None);
    };

    let me = std::process::id();
    if pid.parse::<u32>() != Ok(me) {
        return Err(format!("LISTEN_PID is {pid:?}, not this process ({me})"));
    }
    match std::env::var("LISTEN_FDS").as_deref() {
        Ok("1") => {}
        other => {
            return Err(format!(
                "LISTEN_FDS is {other:?}; this daemon takes exactly one socket. \
                 Check the .socket unit has one ListenSequentialPacket= line"
            ));
        }
    }

    let fd = LISTEN_FDS_START;
    conforms(fd)?;

    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(format!(
            "FD_CLOEXEC on the inherited socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(Some(unsafe { UnixListener::from_raw_fd(fd) }))
}

fn conforms(fd: libc::c_int) -> Result<(), String> {
    if sockopt(fd, libc::SO_ACCEPTCONN)? != 1 {
        return Err(format!("the inherited fd {fd} is not a listening socket"));
    }
    if sockopt(fd, libc::SO_DOMAIN)? != libc::AF_UNIX {
        return Err("the inherited socket is not AF_UNIX".into());
    }
    let kind = sockopt(fd, libc::SO_TYPE)?;
    if kind != libc::SOCK_SEQPACKET {
        return Err(format!(
            "the inherited socket is type {kind}, not SOCK_SEQPACKET ({}). \
             The unit needs ListenSequentialPacket=, not ListenStream=",
            libc::SOCK_SEQPACKET
        ));
    }
    Ok(())
}

fn sockopt(fd: libc::c_int, name: libc::c_int) -> Result<libc::c_int, String> {
    let mut value: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(fd, libc::SOL_SOCKET, name, &mut value as *mut _ as *mut libc::c_void, &mut len)
    };
    match rc {
        0 => Ok(value),
        _ => Err(format!("getsockopt on fd {fd}: {}", std::io::Error::last_os_error())),
    }
}

// ponytail: capped so a hammering client cannot spin us
const DRAIN_MAX: usize = 64;

// ponytail: close, not reply — same PAM code
pub fn drain(listener: &UnixListener) -> usize {
    if listener.set_nonblocking(true).is_err() {
        return 0;
    }
    let mut closed = 0;
    while closed < DRAIN_MAX && listener.accept().is_ok() {
        closed += 1;
    }
    let _ = listener.set_nonblocking(false);
    closed
}

pub fn run(listener: &UnixListener, daemon: &Daemon, idle: Option<std::time::Duration>) {
    loop {
        if let Some(timeout) = idle {
            match after(wait(listener, timeout), daemon.may_idle_exit()) {
                Next::Accept => {}
                Next::Poll => continue,
                Next::Exit => {
                    eprintln!(
                        "faciolockd: idle for {} s, exiting — the socket unit reactivates on the next connect",
                        timeout.as_secs()
                    );
                    return;
                }
            }
        }
        match listener.accept() {
            Ok((stream, _)) => handle(stream, daemon),
            Err(e) => eprintln!("faciolockd: accept: {e}"),
        }
    }
}

fn after(wait: Wait, may_exit: bool) -> Next {
    match wait {
        Wait::Ready => Next::Accept,
        Wait::Retry => Next::Poll,
        Wait::Idle if may_exit => Next::Exit,
        Wait::Idle => Next::Poll,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Next {
    Accept,
    Poll,
    Exit,
}

enum Wait {
    Ready,
    Idle,
    Retry,
}

fn wait(listener: &UnixListener, timeout: std::time::Duration) -> Wait {
    let mut fd =
        libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ms = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    match unsafe { libc::poll(&mut fd, 1, ms) } {
        0 => Wait::Idle,
        n if n > 0 => Wait::Ready,
        _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {
            Wait::Retry
        }
        _ => Wait::Ready,
    }
}

pub struct Peer<'a> {
    stream: &'a UnixStream,
    uid: u32,
    pid: i32,
}

impl<'a> Peer<'a> {
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

    pub fn uid(&self) -> u32 {
        self.uid
    }

    // ponytail: stage boundaries only
    pub fn hung_up(&self) -> bool {
        let mut poll = libc::pollfd {
            fd: self.stream.as_raw_fd(),
            events: libc::POLLRDHUP,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut poll, 1, 0) };
        rc > 0 && poll.revents & (libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR) != 0
    }

    pub fn describe(&self) -> String {
        format!("pid {} uid {}", self.pid, self.uid)
    }
}

const PEER_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

fn handle(stream: UnixStream, daemon: &Daemon) {
    let peer = match Peer::of(&stream) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("faciolockd: no peer credentials ({e}); closing");
            return;
        }
    };
    if let Err(e) = stream
        .set_read_timeout(Some(PEER_IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(PEER_IO_TIMEOUT)))
    {
        eprintln!("faciolockd: could not bound I/O on the connection ({e}); closing");
        return;
    }

    let mut buf = [0u8; REQUEST_MAX + 1];
    let n = match (&stream).read(&mut buf) {
        Ok(0) => return,
        Ok(n) => n,
        Err(e) => {
            eprintln!("faciolockd: {} sent nothing ({e}); closing", peer.describe());
            return;
        }
    };
    if n > REQUEST_MAX {
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
    let Some(verdict) = verdict else { return };

    let elapsed = started.elapsed().as_millis().min(u16::MAX as u128) as u16;
    if let Err(e) = (&stream).write_all(&Response::new(verdict, elapsed).encode()) {
        eprintln!("faciolockd: {} left before the answer ({e})", peer.describe());
    }
}

pub fn to_wire(verdict: &faciolock_vision::Verdict) -> Verdict {
    use faciolock_vision::Verdict as V;
    match verdict {
        V::Match { .. } => Verdict::Allow,
        V::NoMatch { .. } | V::NotLive { .. } | V::NoFace => Verdict::Deny,
        V::Environment { .. } | V::Unusable(_) | V::Failed(_) => Verdict::Unavailable,
    }
}

// The old rule was "only root may ask" and it lived at
// connect(); it cannot stay there, because the KDE greeter that has to ask is not root.
//
// Root asks about anybody. Anybody else asks only about themselves, and only the two
// questions that cannot move the budget in their favour: Clear would hand a session process
// an unlimited supply of face attempts, and Enroll would let one write a template.
//
// `subject` is None when the username did not resolve, and that case belongs in here rather
// than in the caller: answered separately it is a user-existence oracle, because "no such
// user" would come back as NoEnrolment where a real other user comes back as silence.
pub fn may_ask(peer_uid: u32, subject: Option<u32>, op: Op) -> bool {
    peer_uid == 0 || (subject == Some(peer_uid) && matches!(op, Op::Auth | Op::Status))
}

pub fn status(enrolled: bool) -> Verdict {
    if enrolled { Verdict::Deny } else { Verdict::NoEnrolment }
}

pub fn enrol_failure(verdict: &faciolock_vision::Verdict) -> Verdict {
    use faciolock_vision::Verdict as V;
    match verdict {
        V::Match { .. } | V::NoMatch { .. } => Verdict::Deny,
        V::NotLive { .. } | V::NoFace => Verdict::Deny,
        V::Environment { .. } | V::Unusable(_) | V::Failed(_) => Verdict::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faciolock_vision::{BurstError, Check, Verdict as V};

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
    }

    #[test]
    fn a_session_peer_may_ask_about_itself_and_nothing_else() {
        const ME: u32 = 1000;
        const SOMEBODY_ELSE: u32 = 1001;

        for op in [Op::Auth, Op::Status, Op::Enroll, Op::Clear] {
            assert!(may_ask(0, Some(SOMEBODY_ELSE), op), "root asks about anybody: {op:?}");
            assert!(may_ask(0, None, op), "root still gets NoEnrolment for a bad name: {op:?}");
            assert!(!may_ask(ME, Some(SOMEBODY_ELSE), op), "{ME} must not reach 1001: {op:?}");
            assert!(!may_ask(ME, Some(0), op), "uid {ME} must not reach root: {op:?}");

            // Or the socket is a user-existence oracle: silence for a real account you may
            // not ask about, NoEnrolment for one that does not exist.
            assert!(!may_ask(ME, None, op), "an unknown name must not answer either: {op:?}");
        }

        assert!(may_ask(ME, Some(ME), Op::Auth), "the lock screen's own question");
        assert!(may_ask(ME, Some(ME), Op::Status));

        // The two that would make the failure budget worthless. Clear is the sharp one: a
        // session process that can reset its own counter has unlimited face attempts, and
        // liveness does not reliably reject a printed photo, so the budget is the defence rather
        // than a backstop behind liveness.
        assert!(!may_ask(ME, Some(ME), Op::Clear), "clearing your own budget defeats the limiter");
        assert!(!may_ask(ME, Some(ME), Op::Enroll), "a session process must not write a template");
    }

    #[test]
    fn no_enrolment_failure_is_a_success_of_any_kind() {
        for refused in [
            V::NoMatch { best: 0.9 },
            V::Match { similarity: 0.9, agreeing: 2 },
            V::NotLive { check: Check::DarkFrame, worst: 1.0, threshold: 1e-3 },
            V::NoFace,
            V::Environment { ambient_floor: 51.74 },
            V::Unusable(BurstError::Malformed),
            V::Failed("camera vanished".into()),
        ] {
            let wire = enrol_failure(&refused);
            assert!(!wire.is_allow(), "{refused:?} must not reach Allow");
            assert_ne!(wire, Verdict::Ack, "{refused:?} must not read as a captured pose");
        }
    }

    #[test]
    fn an_unjudgeable_pose_is_distinguishable_from_a_refused_one() {
        assert_eq!(enrol_failure(&V::NoFace), Verdict::Deny);
        assert_eq!(enrol_failure(&V::Environment { ambient_floor: 85.44 }), Verdict::Unavailable);
    }

    #[test]
    fn unavailable_means_the_question_was_never_asked() {
        assert_eq!(to_wire(&V::Environment { ambient_floor: 51.74 }), Verdict::Unavailable);
        assert_eq!(to_wire(&V::NoFace), Verdict::Deny);
    }

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
        assert_eq!(kind, libc::SOCK_SEQPACKET, "the protocol requires SOCK_SEQPACKET");

        let mode = std::fs::metadata(&path).expect("socket exists").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the socket must never be group- or world-reachable");

        drop(listener);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_inherited_socket_of_the_wrong_type_is_refused() {
        let seqpacket = std::env::temp_dir().join("faciolockd-inherit-test.sock");
        let listener = listen(&seqpacket).expect("binds");
        assert!(conforms(listener.as_raw_fd()).is_ok(), "the real thing must pass");

        let stream = std::env::temp_dir().join("faciolockd-inherit-stream.sock");
        let _ = std::fs::remove_file(&stream);
        let wrong = UnixListener::bind(&stream).expect("std binds SOCK_STREAM");
        let e = conforms(wrong.as_raw_fd()).expect_err("SOCK_STREAM must not be accepted");
        assert!(e.contains("ListenSequentialPacket"), "the error must name the fix: {e}");

        let (a, _b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        assert!(conforms(a.as_raw_fd()).is_err(), "a connected socket is not a listener");
        assert!(conforms(2).is_err(), "stderr is not a socket at all");

        drop(listener);
        drop(wrong);
        let _ = std::fs::remove_file(&seqpacket);
        let _ = std::fs::remove_file(&stream);
    }

    #[test]
    fn a_daemon_that_cannot_exit_yet_re_polls_instead_of_blocking_in_accept() {
        assert_eq!(after(Wait::Idle, false), Next::Poll, "this one shipped as Next::Accept");
        assert_eq!(after(Wait::Idle, true), Next::Exit);

        assert_eq!(after(Wait::Ready, true), Next::Accept);
        assert_eq!(after(Wait::Ready, false), Next::Accept);

        assert_eq!(after(Wait::Retry, true), Next::Poll);
        assert_eq!(after(Wait::Retry, false), Next::Poll);
    }

    #[test]
    fn a_quiet_listener_times_out_and_a_waiting_peer_does_not() {
        let path = std::env::temp_dir().join("faciolockd-idle-test.sock");
        let listener = listen(&path).expect("binds");

        let started = Instant::now();
        assert!(
            matches!(wait(&listener, std::time::Duration::from_millis(50)), Wait::Idle),
            "an unused socket must go idle, or the daemon never exits"
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(45), "it must actually wait");

        let fd =
            unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
        assert!(fd >= 0, "socket");
        let client = unsafe { UnixStream::from_raw_fd(fd) };
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (dst, src) in addr.sun_path.iter_mut().zip(path.as_os_str().as_bytes()) {
            *dst = *src as libc::c_char;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + path.as_os_str().len() + 1)
            as libc::socklen_t;
        assert_eq!(
            unsafe {
                libc::connect(client.as_raw_fd(), &addr as *const _ as *const libc::sockaddr, len)
            },
            0,
            "connect: {}",
            std::io::Error::last_os_error()
        );

        assert!(
            matches!(wait(&listener, std::time::Duration::from_millis(50)), Wait::Ready),
            "a queued connection must read as ready, or an unlock is answered by an exit"
        );

        drop(client);
        drop(listener);
        let _ = std::fs::remove_file(&path);
    }

    fn connect_seqpacket(path: &Path) -> UnixStream {
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
        client
    }

    #[test]
    fn a_daemon_that_cannot_start_empties_the_queue_that_would_restart_it() {
        let path = std::env::temp_dir().join("faciolockd-drain-test.sock");
        let listener = listen(&path).expect("binds");

        // Two clients queued behind a daemon that is about to fail models::verify. Left
        // there, each one is a fresh activation into the same failure.
        let mut waiting: Vec<UnixStream> = (0..2).map(|_| connect_seqpacket(&path)).collect();

        let started = Instant::now();
        assert_eq!(drain(&listener), 2, "every queued request must be taken");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "drain must not block on an empty queue — it runs on the way out"
        );

        for client in &mut waiting {
            let mut buf = [0u8; 8];
            let read = client.read(&mut buf);
            assert!(
                matches!(read, Ok(0)) || read.is_err(),
                "a drained client must see the connection go, not wait for a verdict"
            );
        }

        assert_eq!(drain(&listener), 0, "nothing is left to reactivate on");
        assert!(
            matches!(wait(&listener, std::time::Duration::from_millis(50)), Wait::Idle),
            "and the socket reads idle, which is what stops the flap"
        );

        drop(listener);
        let _ = std::fs::remove_file(&path);
    }

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
        assert!(!may_ask(peer.uid(), Some(0), Op::Auth), "an unprivileged peer cannot reach root");

        assert!(!peer.hung_up(), "the client is still connected");
        drop(client);
        assert!(peer.hung_up(), "a departed client must be detected, or the camera is held");

        drop(listener);
        let _ = std::fs::remove_file(&path);
    }
}
