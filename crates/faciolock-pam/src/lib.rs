//! `pam_faciolock.so` — the root-privileged half of Faciolock, which is deliberately the
//! half that does nothing.
//!
//! It reads a username, refuses remote sessions, sends one datagram to
//! [`faciolockd`](../faciolock_daemon/index.html) and turns one datagram back into a PAM
//! return code. No camera, no image decoding, no inference, no config file, no template
//! store — wiki/03's non-negotiable #1, enforced by this crate having nothing in it to do
//! any of that with.
//!
//! ## The rules this file exists to hold
//!
//! - **One `PAM_SUCCESS` construction site**, in [`code_for`], reached only from
//!   `Verdict::Allow`. Every other path — every error, every unknown byte, every panic —
//!   is a failure code, and under `auth sufficient` every failure code means the user types
//!   their password (wiki/04's failure table).
//! - **`catch_unwind` at the boundary.** Unwinding across an `extern "C"` frame is
//!   undefined behaviour, and the caller is `sudo`. `panic = "abort"` is the wrong fix here
//!   and must never be set on this crate: it would turn a bug in the module into a killed
//!   `sudo` rather than a password prompt.
//! - **No global state.** KDE runs the password and biometric stacks concurrently on
//!   purpose, so the module must be reentrant; holding nothing is the cheapest way to be.
//! - **No environment-derived socket path.** See [`options`].

mod client;
mod pam;

use core::ffi::{CStr, c_char, c_int};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Duration;

use faciolock_ipc::{Op, Request, Verdict};

/// wiki/04's data layout. Overridable only from the pam.d line — see [`options`].
const DEFAULT_SOCKET: &str = "/run/faciolock.sock";

/// What the module is willing to wait for, and therefore the worst stall a wedged daemon
/// can add to a lock screen before the password prompt appears (decision P3-c).
///
/// The daemon's own hard cap is a 1500 ms capture deadline plus ~350 ms of inference, so
/// 3 s is that worst case with margin, and nothing about it is a target: the warm path is
/// measured at ~673 ms (wiki/06). Shorter would start cutting off honest slow answers;
/// longer would be a user staring at a frozen screen.
const DEADLINE: Duration = Duration::from_millis(3_000);

/// The timeout the daemon is *asked* for. It applies its own regardless (wiki/04), so this
/// is a hint, set below [`DEADLINE`] so the daemon gives up first and we hear about it.
const REQUEST_TIMEOUT_MS: u16 = 2_000;

/// PAM's authentication entry point.
///
/// # Safety
/// Called by libpam with a live handle and its own `argv`. Nothing here dereferences either
/// beyond the length libpam states.
#[unsafe(no_mangle)]
pub extern "C" fn pam_sm_authenticate(
    pamh: *mut pam::Handle,
    _flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    // The whole point of this wrapper: a panic anywhere below becomes a password prompt
    // rather than undefined behaviour inside `sudo`.
    catch_unwind(AssertUnwindSafe(|| authenticate(pamh, argc, argv))).unwrap_or(pam::AUTH_ERR)
}

/// Required by libpam even though face authentication establishes no credentials.
///
/// A missing `pam_sm_setcred` makes the whole module unloadable, so this is not optional;
/// wiki/03 says it can simply succeed, and there is nothing here that could fail.
#[unsafe(no_mangle)]
pub extern "C" fn pam_sm_setcred(
    _pamh: *mut pam::Handle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    pam::SUCCESS
}

fn authenticate(pamh: *mut pam::Handle, argc: c_int, argv: *const *const c_char) -> c_int {
    let opts = unsafe { options(argc, argv) };
    let Some(handle) = (unsafe { pamh.as_mut() }) else {
        return pam::AUTH_ERR;
    };

    // **Before any IPC**, wiki/03 R4. A `sudo` over SSH has no camera at the far end, and
    // asking the daemon would light up an emitter in an empty room and burn a rate-limiter
    // slot on somebody who was never going to be looked at.
    if let Some(reason) = remote_session(handle) {
        if opts.debug {
            pam::debug(&format!("faciolock: refusing a remote session ({reason})"));
        }
        return pam::AUTHINFO_UNAVAIL;
    }

    let Some(user) = handle.user() else {
        return pam::AUTH_ERR;
    };
    // The charset and length rules live in the wire type, so a username that could never
    // travel is refused here rather than being trimmed into one that could.
    let Ok(request) = Request::new(Op::Auth, user, REQUEST_TIMEOUT_MS) else {
        return pam::AUTH_ERR;
    };

    match client::ask(opts.socket, &request, DEADLINE) {
        Ok(verdict) => code_for(verdict),
        Err(e) => {
            if opts.debug {
                pam::debug(&format!("faciolock: {e}"));
            }
            pam::AUTHINFO_UNAVAIL
        }
    }
}

/// The only place in this crate that can produce `PAM_SUCCESS`.
///
/// Exhaustive rather than a lookup or an `is_allow` test, so a verdict added to the
/// protocol later fails to compile instead of falling into whichever arm it happens to
/// resemble. `RateLimited` deliberately returns the same code as `Deny`: telling the caller
/// which of the two it was is telling an attacker whether to keep going.
fn code_for(verdict: Verdict) -> c_int {
    match verdict {
        Verdict::Allow => pam::SUCCESS,
        Verdict::Deny | Verdict::RateLimited => pam::AUTH_ERR,
        Verdict::NoEnrolment => pam::USER_UNKNOWN,
        Verdict::Unavailable => pam::AUTHINFO_UNAVAIL,
    }
}

/// Why this authentication is not happening in front of the camera, if it is not.
///
/// `PAM_RHOST` is the authoritative signal and the SSH variables are the belt: `sudo` under
/// an SSH session inherits them, and a `sudo` stack that never set `PAM_RHOST` would
/// otherwise look local. Cheap enough to do all four.
fn remote_session(handle: &pam::Handle) -> Option<&'static str> {
    if handle.rhost().is_some_and(|h| !h.is_empty()) {
        return Some("PAM_RHOST");
    }
    remote_from_environment()
}

/// Split out from [`remote_session`] because it is the half a test can reach without a live
/// `pam_handle_t`, and it is the half with a list in it that could lose an entry.
fn remote_from_environment() -> Option<&'static str> {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .into_iter()
        .find(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
}

struct Options<'a> {
    socket: &'a Path,
    debug: bool,
}

/// Module arguments from the pam.d line.
///
/// **`socket=` is readable from here and from nowhere else — never from the environment.**
/// This code runs as root inside a process whose environment an unprivileged caller
/// influences; a socket path taken from `$FACIOLOCK_SOCKET` would let that caller point the
/// module at a daemon they wrote, which answers `Allow`. That is not a configuration bug,
/// it is a root shell. A pam.d file is root-owned, which is the whole reason the knob is
/// safe there (decision P3-a).
///
/// Unknown arguments are ignored rather than fatal: PAM has no way to report a
/// configuration error except by failing the authentication, and failing every
/// authentication over a typo is worse than ignoring it.
///
/// # Safety
/// `argv` points at `argc` NUL-terminated strings that outlive the call, per the PAM module
/// interface.
unsafe fn options<'a>(argc: c_int, argv: *const *const c_char) -> Options<'a> {
    let mut opts = Options { socket: Path::new(DEFAULT_SOCKET), debug: false };
    if argv.is_null() || argc <= 0 {
        return opts;
    }
    for i in 0..argc as isize {
        let raw = unsafe { *argv.offset(i) };
        if raw.is_null() {
            continue;
        }
        let bytes = unsafe { CStr::from_ptr(raw) }.to_bytes();
        match bytes.strip_prefix(b"socket=") {
            Some(path) if !path.is_empty() => {
                opts.socket = Path::new(OsStr::from_bytes(path));
            }
            _ if bytes == b"debug" => opts.debug = true,
            _ => {}
        }
    }
    opts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wiki/09's single-construction-site rule, as an assertion rather than a comment.
    #[test]
    fn allow_is_the_only_verdict_that_authenticates() {
        assert_eq!(code_for(Verdict::Allow), pam::SUCCESS);
        for verdict in
            [Verdict::Deny, Verdict::Unavailable, Verdict::NoEnrolment, Verdict::RateLimited]
        {
            assert_ne!(code_for(verdict), pam::SUCCESS, "{verdict:?} must not authenticate");
        }
    }

    /// The map and `Verdict::is_allow` are two statements of the same rule and must not be
    /// able to drift apart — one of them is what the daemon uses.
    #[test]
    fn the_code_map_agrees_with_the_protocols_own_notion_of_success() {
        for verdict in [
            Verdict::Allow,
            Verdict::Deny,
            Verdict::Unavailable,
            Verdict::NoEnrolment,
            Verdict::RateLimited,
        ] {
            assert_eq!(
                code_for(verdict) == pam::SUCCESS,
                verdict.is_allow(),
                "{verdict:?} is classified differently by the two rules"
            );
        }
    }

    /// wiki/03's table, spelled out. These are the codes that decide whether the user gets
    /// a password prompt or a lockout, and they are ABI.
    #[test]
    fn the_failure_codes_are_the_ones_wiki_03_specifies() {
        assert_eq!(code_for(Verdict::Deny), 7, "PAM_AUTH_ERR");
        assert_eq!(code_for(Verdict::NoEnrolment), 10, "PAM_USER_UNKNOWN");
        assert_eq!(code_for(Verdict::Unavailable), 9, "PAM_AUTHINFO_UNAVAIL");
        assert_eq!(pam::SUCCESS, 0);
    }

    fn parse(args: &[&str]) -> (std::path::PathBuf, bool) {
        let owned: Vec<std::ffi::CString> =
            args.iter().map(|a| std::ffi::CString::new(*a).expect("no NUL")).collect();
        let ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        let opts = unsafe { options(ptrs.len() as c_int, ptrs.as_ptr()) };
        (opts.socket.to_path_buf(), opts.debug)
    }

    #[test]
    fn the_socket_path_defaults_and_is_overridable_from_the_pam_d_line() {
        assert_eq!(parse(&[]), (DEFAULT_SOCKET.into(), false));
        assert_eq!(parse(&["debug"]), (DEFAULT_SOCKET.into(), true));
        assert_eq!(
            parse(&["socket=/run/user/1000/t.sock", "debug"]),
            ("/run/user/1000/t.sock".into(), true)
        );
        // A typo must not fail every authentication on the machine, and must not silently
        // become an empty path either.
        assert_eq!(parse(&["sockett=/x", "socket="]), (DEFAULT_SOCKET.into(), false));
    }

    /// A null `argv` is legal when `argc` is 0 and must not be dereferenced.
    #[test]
    fn no_arguments_at_all_is_not_a_dereference() {
        let opts = unsafe { options(0, std::ptr::null()) };
        assert_eq!(opts.socket, Path::new(DEFAULT_SOCKET));
    }

    /// Two properties in one test because both need the environment, and mutating it from
    /// two tests that `cargo` may run concurrently is a race.
    ///
    /// 1. Each SSH variable is enough on its own to refuse — this is wiki/03 R4, and the
    ///    list is the kind of thing that loses an entry in a refactor.
    /// 2. The environment cannot move the socket. If that assertion ever fails, somebody
    ///    has given an unprivileged caller a say in which daemon a root process trusts.
    #[test]
    fn ssh_variables_refuse_and_the_environment_never_chooses_the_daemon() {
        // Skip if this session is genuinely remote rather than pretend the fixture works.
        if remote_from_environment().is_some() {
            eprintln!("running under SSH; the negative half of this test is not meaningful");
            return;
        }
        for key in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
            unsafe { std::env::set_var(key, "10.0.0.1 51000 10.0.0.2 22") };
            assert_eq!(remote_from_environment(), Some(key), "{key} must refuse on its own");
            // Set but empty is not a remote session: some shells export the name alone.
            unsafe { std::env::set_var(key, "") };
            assert_eq!(remote_from_environment(), None, "an empty {key} is not a session");
            unsafe { std::env::remove_var(key) };
        }

        unsafe { std::env::set_var("FACIOLOCK_SOCKET", "/tmp/attacker.sock") };
        assert_eq!(parse(&[]).0, std::path::PathBuf::from(DEFAULT_SOCKET));
        unsafe { std::env::remove_var("FACIOLOCK_SOCKET") };
    }
}
