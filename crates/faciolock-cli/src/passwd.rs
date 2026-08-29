//! Username ↔ uid, both directions.
//!
//! The daemon has `users.rs` and it goes one way only, because that is the only direction it
//! needs: the wire carries a name, everything below the socket is keyed by uid, and nothing
//! in the daemon ever turns a uid back into a name. The CLI needs both — `remove` is given a
//! name and has to find `<uid>.tpl`, `list` finds `<uid>.tpl` and has to print a name.
//!
//! `_r` forms in both directions: the non-reentrant ones hand back a pointer into a static
//! buffer that the next call overwrites, and `list` calls this in a loop.

/// Look up a user's numeric id. `None` for an unknown user *and* for a lookup that failed —
/// the caller's answer is the same either way, and the daemon's `users.rs` makes the same
/// choice for the same reason.
pub fn uid_of(username: &str) -> Option<u32> {
    let name = std::ffi::CString::new(username).ok()?;
    with_buffer(|buf, cap| {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe { libc::getpwnam_r(name.as_ptr(), &mut pwd, buf, cap, &mut found) };
        if found.is_null() {
            return (rc, None);
        }
        (rc, Some(pwd.pw_uid))
    })
}

/// The name behind a uid, for display only.
///
/// A uid with no passwd entry is a real and unremarkable state — the user was deleted and
/// their template outlived them — so `None` here is something `list` prints rather than an
/// error it raises. That orphan is exactly what a person runs `list` to find.
pub fn name_of(uid: u32) -> Option<String> {
    with_buffer(|buf, cap| {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf, cap, &mut found) };
        if found.is_null() {
            return (rc, None);
        }
        let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
        (rc, name.to_str().ok().map(str::to_owned))
    })
}

/// `sysconf` is a suggestion, not a bound: glibc returns `ERANGE` when it is wrong, so the
/// buffer grows until it fits. 64 KiB is far past any real passwd entry and is the cap that
/// stops a broken NSS module from spinning this loop.
fn with_buffer<T>(mut lookup: impl FnMut(*mut libc::c_char, usize) -> (i32, Option<T>)) -> Option<T> {
    let mut cap = match unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) } {
        n if n > 0 => n as usize,
        _ => 1024,
    };
    while cap <= 64 * 1024 {
        let mut buf = vec![0u8; cap];
        match lookup(buf.as_mut_ptr() as *mut libc::c_char, cap) {
            (0, answer) => return answer,
            (e, _) if e == libc::ERANGE => cap *= 2,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `root` is uid 0 on every system this will run on, which makes it the one lookup whose
    /// answer can be asserted rather than merely observed — and it round-trips, which is the
    /// property `list` and `remove` rely on jointly.
    #[test]
    fn root_round_trips_and_an_invented_user_does_not_resolve() {
        assert_eq!(uid_of("root"), Some(0));
        assert_eq!(name_of(0).as_deref(), Some("root"));
        assert_eq!(uid_of("no-such-user-3f9a2c"), None);
    }

    /// An embedded NUL must not truncate into a *different* real user.
    #[test]
    fn an_embedded_nul_does_not_truncate_into_another_account() {
        assert_eq!(uid_of("root\0evil"), None);
    }

    /// A template left behind by a deleted user is a normal thing for `list` to find, and it
    /// has to be printable rather than fatal.
    #[test]
    fn a_uid_with_no_account_is_none_rather_than_an_error() {
        assert_eq!(name_of(4_000_000_000), None);
    }
}
