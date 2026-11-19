use core::ffi::{CStr, c_char, c_int, c_void};

pub const SUCCESS: c_int = 0;
pub const AUTH_ERR: c_int = 7;
pub const AUTHINFO_UNAVAIL: c_int = 9;
pub const USER_UNKNOWN: c_int = 10;
pub const IGNORE: c_int = 25;

pub const SILENT: c_int = 0x8000;

pub const ITEM_SERVICE: c_int = 1;
pub const ITEM_RHOST: c_int = 4;

const TEXT_INFO: c_int = 4;

#[repr(C)]
pub struct Handle {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn pam_get_user(pamh: *mut Handle, user: *mut *const c_char, prompt: *const c_char) -> c_int;
    fn pam_get_item(pamh: *const Handle, item_type: c_int, item: *mut *const c_void) -> c_int;
    fn pam_prompt(
        pamh: *mut Handle,
        style: c_int,
        response: *mut *mut c_char,
        fmt: *const c_char,
        ...
    ) -> c_int;
}

impl Handle {
    pub fn user(&mut self) -> Option<&str> {
        let mut raw: *const c_char = core::ptr::null();
        let rc = unsafe { pam_get_user(self, &mut raw, core::ptr::null()) };
        if rc != SUCCESS {
            return None;
        }
        unsafe { borrow(raw) }
    }

    pub fn item(&self, item_type: c_int) -> Option<&str> {
        let mut raw: *const c_void = core::ptr::null();
        let rc = unsafe { pam_get_item(self, item_type, &mut raw) };
        if rc != SUCCESS {
            return None;
        }
        unsafe { borrow(raw as *const c_char) }
    }

    pub fn info(&mut self, text: &CStr) {
        unsafe {
            pam_prompt(self, TEXT_INFO, core::ptr::null_mut(), c"%s".as_ptr(), text.as_ptr());
        }
    }
}

unsafe fn borrow<'a>(raw: *const c_char) -> Option<&'a str> {
    if raw.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(raw) }.to_str().ok()
}

pub fn debug(message: &str) {
    let Ok(text) = std::ffi::CString::new(message) else {
        return;
    };
    // ponytail: libc::syslog, not pam_syslog
    unsafe {
        libc::syslog(libc::LOG_AUTHPRIV | libc::LOG_DEBUG, c"%s".as_ptr(), text.as_ptr());
    }
}
