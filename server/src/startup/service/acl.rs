//! Directory access for the account the service runs under.
//!
//! A virtual or built-in service identity starts with no access to anything
//! outside its own profile, so the install grants the state directories a
//! modify ACE and refuses to finish if it cannot: a service that cannot write
//! its own database fails at the next boot with no console to say so.
//! `icacls` gives `(OI)(CI)` inheritance and the `*<SID>` form, so a virtual
//! account's SID needs no name lookup.

// `unsafe` is the SID conversion and `LocalFree` around it; the module doc explains
// the access this grants.
#![allow(unsafe_code)]

use std::path::Path;
use std::process::Command;

/// Resolve an account name to its SID string (`S-1-5-80-…`).
pub(crate) fn resolve_sid(name: &str) -> anyhow::Result<String> {
    let sid = crate::startup::win32::lookup_account_sid(name)
        .map_err(|_| anyhow::anyhow!("the account '{name}' could not be resolved to a SID"))?;
    crate::startup::win32::sid_to_string(&sid)
        .ok_or_else(|| anyhow::anyhow!("the account '{name}' has no printable SID"))
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

/// Remove the grant again, best effort: a path without the entry is the wanted state.
pub(crate) fn revoke(sid: &str, path: &Path) -> anyhow::Result<()> {
    icacls(path, &["/remove:g", &format!("*{sid}"), "/Q"])
}

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
