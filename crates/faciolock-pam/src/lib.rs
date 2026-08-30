mod client;
mod pam;

use core::ffi::{CStr, c_char, c_int};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Duration;

use faciolock_ipc::{Op, Request, Verdict};

const DEFAULT_SOCKET: &str = "/run/faciolock.sock";

const DEADLINE: Duration = Duration::from_millis(3_000);

const REQUEST_TIMEOUT_MS: u16 = 2_000;

const CLEAR_DEADLINE: Duration = Duration::from_millis(300);
const CLEAR_TIMEOUT_MS: u16 = 200;

#[unsafe(no_mangle)]
pub extern "C" fn pam_sm_authenticate(
    pamh: *mut pam::Handle,
    _flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    catch_unwind(AssertUnwindSafe(|| authenticate(pamh, argc, argv))).unwrap_or(pam::AUTH_ERR)
}

#[unsafe(no_mangle)]
pub extern "C" fn pam_sm_setcred(
    _pamh: *mut pam::Handle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    pam::SUCCESS
}

#[unsafe(no_mangle)]
pub extern "C" fn pam_sm_acct_mgmt(
    pamh: *mut pam::Handle,
    _flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    let _panicked = catch_unwind(AssertUnwindSafe(|| clear_lockout(pamh, argc, argv)));
    pam::IGNORE
}

fn clear_lockout(pamh: *mut pam::Handle, argc: c_int, argv: *const *const c_char) {
    let opts = unsafe { options(argc, argv) };
    let Some(handle) = (unsafe { pamh.as_mut() }) else {
        return;
    };
    if remote_session(handle).is_some() {
        return;
    }
    let Some(user) = handle.user() else {
        return;
    };
    let Ok(request) = Request::new(Op::Clear, user, CLEAR_TIMEOUT_MS) else {
        return;
    };

    if let Err(e) = client::ask(opts.socket, &request, CLEAR_DEADLINE) {
        if opts.debug {
            pam::debug(&format!("faciolock: could not clear the lockout: {e}"));
        }
    }
}

fn authenticate(pamh: *mut pam::Handle, argc: c_int, argv: *const *const c_char) -> c_int {
    let opts = unsafe { options(argc, argv) };
    let Some(handle) = (unsafe { pamh.as_mut() }) else {
        return pam::AUTH_ERR;
    };

    if let Some(reason) = remote_session(handle) {
        if opts.debug {
            pam::debug(&format!("faciolock: refusing a remote session ({reason})"));
        }
        return pam::AUTHINFO_UNAVAIL;
    }

    let Some(user) = handle.user() else {
        return pam::AUTH_ERR;
    };
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

fn code_for(verdict: Verdict) -> c_int {
    match verdict {
        Verdict::Allow => pam::SUCCESS,
        Verdict::Deny | Verdict::RateLimited => pam::AUTH_ERR,
        Verdict::NoEnrolment => pam::USER_UNKNOWN,
        Verdict::Unavailable => pam::AUTHINFO_UNAVAIL,
        Verdict::Ack => pam::AUTH_ERR,
    }
}

fn remote_session(handle: &pam::Handle) -> Option<&'static str> {
    if handle.rhost().is_some_and(|h| !h.is_empty()) {
        return Some("PAM_RHOST");
    }
    remote_from_environment()
}

fn remote_from_environment() -> Option<&'static str> {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .into_iter()
        .find(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
}

struct Options<'a> {
    socket: &'a Path,
    debug: bool,
}

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

    #[test]
    fn allow_is_the_only_verdict_that_authenticates() {
        assert_eq!(code_for(Verdict::Allow), pam::SUCCESS);
        for verdict in [
            Verdict::Deny,
            Verdict::Unavailable,
            Verdict::NoEnrolment,
            Verdict::RateLimited,
            Verdict::Ack,
        ] {
            assert_ne!(code_for(verdict), pam::SUCCESS, "{verdict:?} must not authenticate");
        }
    }

    #[test]
    fn the_code_map_agrees_with_the_protocols_own_notion_of_success() {
        for verdict in [
            Verdict::Allow,
            Verdict::Deny,
            Verdict::Unavailable,
            Verdict::NoEnrolment,
            Verdict::RateLimited,
            Verdict::Ack,
        ] {
            assert_eq!(
                code_for(verdict) == pam::SUCCESS,
                verdict.is_allow(),
                "{verdict:?} is classified differently by the two rules"
            );
        }
    }

    #[test]
    fn account_management_ignores_everything_including_a_hostile_daemon() {
        let dir = std::env::temp_dir().join("faciolock-acct-mgmt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        assert_eq!(
            pam_sm_acct_mgmt(std::ptr::null_mut(), 0, 0, std::ptr::null()),
            pam::IGNORE,
            "a null handle must still be ignored"
        );

        let request = Request::new(Op::Clear, "csr", CLEAR_TIMEOUT_MS).expect("valid");
        let missing = dir.join("absent.sock");
        assert!(
            client::ask(&missing, &request, CLEAR_DEADLINE).is_err(),
            "an absent daemon is an error, not a hang"
        );
    }

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
        assert_eq!(parse(&["sockett=/x", "socket="]), (DEFAULT_SOCKET.into(), false));
    }

    #[test]
    fn no_arguments_at_all_is_not_a_dereference() {
        let opts = unsafe { options(0, std::ptr::null()) };
        assert_eq!(opts.socket, Path::new(DEFAULT_SOCKET));
    }

    #[test]
    fn ssh_variables_refuse_and_the_environment_never_chooses_the_daemon() {
        if remote_from_environment().is_some() {
            eprintln!("running under SSH; the negative half of this test is not meaningful");
            return;
        }
        for key in ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"] {
            unsafe { std::env::set_var(key, "10.0.0.1 51000 10.0.0.2 22") };
            assert_eq!(remote_from_environment(), Some(key), "{key} must refuse on its own");
            unsafe { std::env::set_var(key, "") };
            assert_eq!(remote_from_environment(), None, "an empty {key} is not a session");
            unsafe { std::env::remove_var(key) };
        }

        unsafe { std::env::set_var("FACIOLOCK_SOCKET", "/tmp/attacker.sock") };
        assert_eq!(parse(&[]).0, std::path::PathBuf::from(DEFAULT_SOCKET));
        unsafe { std::env::remove_var("FACIOLOCK_SOCKET") };
    }
}
