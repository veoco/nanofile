//! Directory access for the account the service runs under.
//!
//! A virtual or built-in service identity starts with no access to anything
//! outside its own profile, and the state directories live next to the
//! installation (or wherever the administrator pointed them). Registering a
//! service that cannot write its own database is a registration that fails at
//! the next boot, with no console and only a log file to say so — so the
//! install grants what the service needs, and refuses to finish if it cannot.
//!
//! `icacls` does the work: it understands `(OI)(CI)` inheritance and the
//! `*<SID>` form (so a virtual account's SID is used directly, without a name
//! lookup that depends on the account being cached).

use std::path::Path;
use std::process::Command;

/// Resolve an account name to its SID string (`S-1-5-80-…`).
pub(crate) fn resolve_sid(name: &str) -> anyhow::Result<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{LookupAccountNameW, SID_NAME_USE};

    let name_w = crate::startup::win32::wide(name);
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
        anyhow::bail!("the account '{name}' could not be resolved to a SID");
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
        anyhow::bail!("the account '{name}' could not be resolved to a SID");
    }

    let mut text: *mut u16 = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_mut_ptr() as *mut core::ffi::c_void, &mut text) } == 0
    {
        anyhow::bail!("the account '{name}' has no printable SID");
    }
    let mut len = 0usize;
    unsafe {
        while *text.add(len) != 0 {
            len += 1;
        }
    }
    let sid = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(text, len)) };
    unsafe { LocalFree(text as *mut core::ffi::c_void) };
    Ok(sid)
}

/// Grant `sid` modify access to `path`, inheritable when `path` is a directory.
pub(crate) fn grant(sid: &str, path: &Path, inheritable: bool) -> anyhow::Result<()> {
    let rights = if inheritable {
        format!("*{sid}:(OI)(CI)M")
    } else {
        format!("*{sid}:M")
    };
    icacls(path, &["/grant", &rights, "/Q"])
}

/// Remove the grant again, best effort: a path without the access control entry
/// is the state the caller wants.
pub(crate) fn revoke(sid: &str, path: &Path) -> anyhow::Result<()> {
    icacls(path, &["/remove:g", &format!("*{sid}"), "/Q"])
}

/// Run `icacls <path> <args…>`, reporting what it said when it fails.
fn icacls(path: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("icacls")
        .arg(path)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("running icacls on {} failed: {e}", path.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    anyhow::bail!(
        "icacls {} {} failed: {detail}",
        path.display(),
        args.join(" ")
    )
}
