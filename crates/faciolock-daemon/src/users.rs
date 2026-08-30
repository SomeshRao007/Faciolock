pub fn uid_of(username: &str) -> Option<u32> {
    let name = std::ffi::CString::new(username).ok()?;

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

    #[test]
    fn a_real_user_resolves_and_an_invented_one_does_not() {
        assert_eq!(uid_of("root"), Some(0));
        assert_eq!(uid_of("no-such-user-3f9a2c"), None);
        assert_eq!(uid_of(""), None);
    }

    #[test]
    fn an_embedded_nul_does_not_truncate_into_another_account() {
        assert_eq!(uid_of("root\0evil"), None);
    }
}
