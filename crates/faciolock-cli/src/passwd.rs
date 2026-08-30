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

    #[test]
    fn root_round_trips_and_an_invented_user_does_not_resolve() {
        assert_eq!(uid_of("root"), Some(0));
        assert_eq!(name_of(0).as_deref(), Some("root"));
        assert_eq!(uid_of("no-such-user-3f9a2c"), None);
    }

    #[test]
    fn an_embedded_nul_does_not_truncate_into_another_account() {
        assert_eq!(uid_of("root\0evil"), None);
    }

    #[test]
    fn a_uid_with_no_account_is_none_rather_than_an_error() {
        assert_eq!(name_of(4_000_000_000), None);
    }
}
