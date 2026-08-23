//! Username → uid, once, at the daemon's boundary.
//!
//! wiki/04 keys everything the daemon persists by **uid, not username**: usernames get
//! reused, uids are what PAM and `SO_PEERCRED` actually deal in. The wire carries a name
//! because that is what `PAM_USER` gives the module (wiki/03 R2 — the module states who you
//! *claim* to be), so exactly one place has to translate, and this is it. Below this line
//! nothing takes a username: not the template store, not the rate limiter, not the audit
//! log.
//!
//! A name that does not resolve is `None` rather than an error. An unknown user is a normal
//! thing for an authentication stack to be asked about, and it is indistinguishable from an
//! unenrolled one by design — the daemon must not become an oracle for which accounts exist.

/// Look up a user's numeric id.
///
/// `getpwnam_r` rather than `getpwnam`: the daemon is serial today but a warm-up thread
/// already shares the process, and the non-`_r` form hands back a pointer into a static
/// buffer that the next caller overwrites.
pub fn uid_of(username: &str) -> Option<u32> {
    // A NUL in the middle would truncate the name inside libc and resolve a *different*
    // user. The wire's charset check already forbids one; this is the layer that does not
    // depend on the caller having run it.
    let name = std::ffi::CString::new(username).ok()?;

    // `sysconf` is a suggestion, not a bound — glibc returns `ERANGE` when it is wrong, so
    // the buffer grows until it fits or the size stops being credible. 64 KiB is far past
    // any real passwd entry and is there so a broken NSS module cannot spin this loop.
    let mut cap = match unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) } {
        n if n > 0 => n as usize,
        _ => 1024,
    };

    while cap <= 64 * 1024 {
        let mut buf = vec![0u8; cap];
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();

        let rc = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                cap,
                &mut found,
            )
        };

        // `rc == 0` with a null result is the documented "no such user". A non-zero `rc`
        // is a lookup *failure* — NSS unreachable, for instance — and answering "no such
        // user" to that would silently disable face unlock rather than report a fault.
        // Both end in the same `None` here, and the caller's `NoEnrolment` is the honest
        // answer for both: the daemon cannot verify this user right now.
        match rc {
            0 if found.is_null() => return None,
            0 => return Some(pwd.pw_uid),
            e if e == libc::ERANGE => cap *= 2,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `root` is uid 0 on every system this will ever run on, which makes it the one
    /// lookup whose answer can be asserted rather than merely observed.
    #[test]
    fn a_real_user_resolves_and_an_invented_one_does_not() {
        assert_eq!(uid_of("root"), Some(0));
        assert_eq!(uid_of("no-such-user-3f9a2c"), None);
        assert_eq!(uid_of(""), None);
    }

    /// An embedded NUL must not truncate into a *different* real user. `CString::new`
    /// refuses it, which is the whole reason the conversion is fallible here.
    #[test]
    fn an_embedded_nul_does_not_truncate_into_another_account() {
        assert_eq!(uid_of("root\0evil"), None);
    }
}
