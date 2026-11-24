//! Small Win32 helpers the auto-start code shares: the process-level questions
//! ("am I elevated?", "which account?") and UTF-16 conversion for `…W` calls.

// `unsafe` is the Win32 token/SID lookups; this module is compiled only on Windows.
#![allow(unsafe_code)]

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Resolve an account name to its SID bytes.
///
/// The two-call `LookupAccountNameW` dance (size query, then fill) is the same
/// unsafe sequence whichever caller needs the SID; the service-logon grant and the
/// directory ACL both used to inline it. A context-specific error is the caller's
/// to add.
pub(crate) fn lookup_account_sid(name: &str) -> anyhow::Result<Vec<u8>> {
    use windows_sys::Win32::Security::{LookupAccountNameW, SID_NAME_USE};

    let name_w = wide(name);
    let mut sid_len = 0u32;
    let mut domain_len = 0u32;
    let mut use_: SID_NAME_USE = 0;
    unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut sid_len,
            std::ptr::null_mut(),
            &mut domain_len,
            &mut use_,
        );
    }
    if sid_len == 0 {
        anyhow::bail!("the account {name:?} has no SID on this machine");
    }
    let mut sid = vec![0u8; sid_len as usize];
    let mut domain = vec![0u16; domain_len.max(1) as usize];
    let found = unsafe {
        LookupAccountNameW(
            std::ptr::null(),
            name_w.as_ptr(),
            sid.as_mut_ptr() as *mut core::ffi::c_void,
            &mut sid_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut use_,
        )
    };
    if found == 0 {
        anyhow::bail!("the account {name:?} has no SID on this machine");
    }
    Ok(sid)
}

/// Printable `S-1-5-…` form of a SID, or `None` when Windows cannot format it.
pub(crate) fn sid_to_string(sid: &[u8]) -> Option<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

    let mut text: *mut u16 = std::ptr::null_mut();
    let mut sid = sid.to_vec();
    if unsafe { ConvertSidToStringSidW(sid.as_mut_ptr() as *mut core::ffi::c_void, &mut text) } == 0
    {
        return None;
    }
    let mut len = 0usize;
    unsafe {
        while *text.add(len) != 0 {
            len += 1;
        }
    }
    let rendered = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(text, len)) };
    unsafe { LocalFree(text as *mut core::ffi::c_void) };
    Some(rendered)
}

/// True when the current process token is elevated ("Run as administrator").
#[cfg(feature = "tray")]
pub(crate) fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// The account this process runs as, in `DOMAIN\name` form, read from the
/// process token: a service started by the SCM has no `USERNAME`.
pub(crate) fn current_account_name() -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut size = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
        if size == 0 {
            CloseHandle(token);
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            size,
            &mut size,
        );
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        name_of_sid(user.User.Sid)
    }
}

/// `DOMAIN\name` for a SID, or `None` when it cannot be resolved.
///
/// # Safety
/// `sid` must point at a valid SID.
pub(crate) unsafe fn name_of_sid(sid: *mut core::ffi::c_void) -> Option<String> {
    use windows_sys::Win32::Security::{LookupAccountSidW, SID_NAME_USE};

    let mut name_len = 0u32;
    let mut domain_len = 0u32;
    let mut use_: SID_NAME_USE = 0;
    unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            std::ptr::null_mut(),
            &mut name_len,
            std::ptr::null_mut(),
            &mut domain_len,
            &mut use_,
        );
    }
    if name_len == 0 {
        return None;
    }
    let mut name = vec![0u16; name_len as usize];
    let mut domain = vec![0u16; domain_len.max(1) as usize];
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut use_,
        )
    };
    if ok == 0 {
        return None;
    }
    let trim = |buf: &[u16]| {
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    };
    let name = trim(&name);
    let domain = trim(&domain);
    if name.is_empty() {
        return None;
    }
    Some(if domain.is_empty() {
        name
    } else {
        format!("{domain}\\{name}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SID a service install writes into `icacls *<SID>` must render in the
    /// canonical form; `LocalFree` is part of the helper's contract.
    #[test]
    fn a_well_known_sid_renders_canonically() {
        // S-1-5-18 (LocalSystem): revision 1, one sub-authority, NT authority, 18.
        let mut sid = vec![1u8, 1, 0, 0, 0, 0, 0, 5];
        sid.extend_from_slice(&18u32.to_le_bytes());
        assert_eq!(sid_to_string(&sid).as_deref(), Some("S-1-5-18"));
    }

    #[test]
    fn an_invalid_sid_has_no_string_form() {
        assert_eq!(sid_to_string(&[]), None);
    }
}
