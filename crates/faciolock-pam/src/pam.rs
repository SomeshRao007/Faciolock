//! The libpam side of the FFI boundary: the four constants we return, the two items we
//! read, and nothing else.
//!
//! Declared by hand rather than pulled from a crate. `pam-sys` and friends would be a
//! dependency inside a root process for the sake of eight lines of `extern "C"`, and
//! wiki/04's rule — `faciolock-pam` depends on exactly `libc` and `faciolock-ipc` — is
//! checked in CI precisely so that trade never gets made quietly.
//!
//! **Every number below was read from `security/_pam_types.h` of `libpam0g-dev`
//! 1.7.0-5ubuntu3.1**, with the header line cited, rather than recalled. They are ABI: PAM
//! modules are loaded by a libpam the distribution built separately, so a wrong constant
//! here is not a compile error, it is a wrong authentication result.
//!
//! `IGNORE` was added in Phase 4 on a machine where that header was no longer installed, so
//! it was read from the *shipped* `libpam.so.0` instead — which is the stronger source, and
//! cross-checked the four that were already here:
//!
//! ```text
//! pam_strerror(NULL, 0)  -> "Success"
//! pam_strerror(NULL, 7)  -> "Authentication failure"
//! pam_strerror(NULL, 9)  -> "Authentication service cannot retrieve authentication info"
//! pam_strerror(NULL, 10) -> "User not known to the underlying authentication module"
//! pam_strerror(NULL, 25) -> "The return value should be ignored by PAM dispatch"
//! ```

use core::ffi::{CStr, c_char, c_int, c_void};

// _pam_types.h:29
pub const SUCCESS: c_int = 0;
// _pam_types.h:37 — "Authentication failure"
pub const AUTH_ERR: c_int = 7;
// _pam_types.h:40 — "Underlying authentication service could not retrieve information"
pub const AUTHINFO_UNAVAIL: c_int = 9;
// _pam_types.h:43 — "User not known to the underlying authentication module"
pub const USER_UNKNOWN: c_int = 10;
// "The return value should be ignored by PAM dispatch" — the only code `pam_sm_acct_mgmt`
// returns, on every path. See [`crate::pam_sm_acct_mgmt`] for why that is not a shortcut.
pub const IGNORE: c_int = 25;

// _pam_types.h:135 — the only item we read. `PAM_USER` (133) is reached through
// `pam_get_user` instead, which is the documented accessor for it.
const ITEM_RHOST: c_int = 4;

/// The application's PAM context. Opaque: we hold a pointer to it and pass it back.
#[repr(C)]
pub struct Handle {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn pam_get_user(pamh: *mut Handle, user: *mut *const c_char, prompt: *const c_char) -> c_int;
    fn pam_get_item(pamh: *const Handle, item_type: c_int, item: *mut *const c_void) -> c_int;
}

impl Handle {
    /// The user being authenticated.
    ///
    /// `pam_get_user` with a null prompt returns whatever the application already set —
    /// `sudo`, `login` and `sddm` all set it before the auth stack runs. It can prompt
    /// through the conversation function if nothing set it; that is the application's
    /// business, and by the time we are called it has happened.
    ///
    /// The returned string is libpam's storage, borrowed for as long as the handle is.
    pub fn user(&mut self) -> Option<&str> {
        let mut raw: *const c_char = core::ptr::null();
        let rc = unsafe { pam_get_user(self, &mut raw, core::ptr::null()) };
        if rc != SUCCESS {
            return None;
        }
        unsafe { borrow(raw) }
    }

    /// The remote host, if the application recorded one. Non-empty means this
    /// authentication is happening for somebody who is not in front of the camera.
    pub fn rhost(&self) -> Option<&str> {
        let mut raw: *const c_void = core::ptr::null();
        let rc = unsafe { pam_get_item(self, ITEM_RHOST, &mut raw) };
        if rc != SUCCESS {
            return None;
        }
        unsafe { borrow(raw as *const c_char) }
    }
}

/// Borrow a NUL-terminated string libpam owns.
///
/// Non-UTF-8 is treated as absent rather than lossily converted: the only two strings we
/// read are a username, which the wire format restricts to ASCII anyway, and a hostname.
///
/// # Safety
/// `raw` is null or points at a NUL-terminated string that outlives `'a`.
unsafe fn borrow<'a>(raw: *const c_char) -> Option<&'a str> {
    if raw.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(raw) }.to_str().ok()
}

/// One line to `authpriv`, only when the pam.d line says `debug`.
///
/// ponytail: `libc::syslog` rather than `pam_syslog`, because it is the same journal entry
/// without a second symbol to declare. Off by default — wiki/03 R8 says the module's only
/// observable effect is its return code, and a module that narrated every failed unlock
/// into the log of a machine at a lock screen would be a nuisance and a small oracle.
///
/// The message is passed through `%s`. A format string built from a username is how this
/// kind of code grows a hole.
pub fn debug(message: &str) {
    let Ok(text) = std::ffi::CString::new(message) else {
        return;
    };
    unsafe {
        libc::syslog(libc::LOG_AUTHPRIV | libc::LOG_DEBUG, c"%s".as_ptr(), text.as_ptr());
    }
}
