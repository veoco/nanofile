use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entity::{commit, repo};
use base::AppError;
use base::common::DirEntryData;

/// Canonical absolute form of `path`, falling back to the input when it does not
/// exist yet. On Windows the `\\?\` prefix is stripped: its `?` would split a
/// `sqlite:` URL into file name and query, and mangle Explorer `/select` arguments.
pub fn absolute_path(path: &Path) -> PathBuf {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        let s = resolved.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    resolved
}

/// Directory of the running binary, or `None`. Every relative path the server owns
/// resolves against it: a login-started desktop runs in an unrelated working
/// directory, and a Windows `Run` entry cannot carry a start directory.
pub fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = absolute_path(&exe);
    exe.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

/// The current user's home directory, or `None` when the environment does not say.
///
/// Read from the environment rather than a platform crate on purpose: the only use
/// is the per-user config path, and `dirs` is in the graph only through the
/// optional `tray` feature. Windows falls back to `HOMEDRIVE` + `HOMEPATH` for
/// accounts whose `USERPROFILE` is unset (e.g. a virtual service account).
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(profile) = non_empty_var("USERPROFILE") {
            return Some(profile);
        }
        let drive = non_empty_var("HOMEDRIVE")?;
        let path = non_empty_var("HOMEPATH")?;
        // `PathBuf::push` treats a rooted path as a replacement, and `HOMEPATH` is rooted.
        let mut home = drive.into_os_string();
        home.push("\\");
        home.push(path.to_string_lossy().trim_start_matches(['\\', '/']));
        Some(PathBuf::from(home))
    }
    #[cfg(not(windows))]
    {
        non_empty_var("HOME")
    }
}

/// A non-empty environment variable as a path.
fn non_empty_var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Create `dir` (and parents) and, on Unix, restrict `dir` to its owner.
///
/// Every private directory — blocks, temp uploads, thumbnails, avatars, index, log —
/// is created here so the mode does not depend on the umask; a default `022` would
/// leave them world-readable. Only the leaf is chmod'ed (it gates traversal), and
/// pre-existing directories too, since nanofile may adopt one from an older build.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Create a directory owner-only, but leave a **pre-existing** one alone: chmodding
/// a shared directory (e.g. `/var/log`) would break every other service writing
/// there.
pub fn ensure_private_dir_only_if_created(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Extract a field from a JSON, form-urlencoded or multipart body, in that order.
pub fn extract_body_field(bytes: &[u8], field_name: &str) -> Option<String> {
    if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes)
        && let Some(val) = map.get(field_name)
    {
        return Some(val.clone());
    }
    if let Ok(map) = serde_urlencoded::from_bytes::<HashMap<String, String>>(bytes)
        && let Some(val) = map.get(field_name)
    {
        return Some(val.clone());
    }
    extract_multipart_field(bytes, field_name)
}

/// Extract a named field from a multipart body by scanning for `name="<field>"` and
/// returning the value after the header-terminating `\r\n\r\n`.
pub fn extract_multipart_field(bytes: &[u8], field_name: &str) -> Option<String> {
    let body_str = String::from_utf8_lossy(bytes);
    let pattern = format!("name=\"{}\"", field_name);
    let rest = body_str.split(&pattern).nth(1)?;
    let val_block = rest.split("\r\n\r\n").nth(1)?;
    let value = val_block.split("\r\n").next().unwrap_or("").trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Get the root_fs_id from the repo's head commit for path resolution.
pub async fn get_head_root_id(db: &DatabaseConnection, repo_id: &str) -> Result<String, AppError> {
    get_head_root_id_opt(db, repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))
}

/// Like `get_head_root_id`, but `Ok(None)` when there is no head commit yet.
pub async fn get_head_root_id_opt(
    db: &DatabaseConnection,
    repo_id: &str,
) -> Result<Option<String>, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    let Some(head_commit_id) = repo_record.head_commit_id else {
        return Ok(None);
    };
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".to_string()))?;
    Ok(Some(head.root_id))
}

/// Head commit id of a repo; errors when the repo or commit is missing.
pub async fn get_head_commit_id(
    db: &DatabaseConnection,
    repo_id: &str,
) -> Result<String, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))
}

/// Parent directory: `/dir/file.txt` → `/dir`, `/file.txt` → `/`.
pub fn parent_path_from(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    }
}

/// Final path segment; a trailing slash or `/` yields `""`.
pub fn basename(path: &str) -> &str {
    path.rsplit_once('/').map(|(_, name)| name).unwrap_or("")
}

/// Join parent and name without doubling the slash at the root.
pub fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// RFC3339 timestamp, or `""` when it cannot be represented.
pub fn timestamp_rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

/// Human-readable size in decimal units, matching the frontend's `formatFileSize`.
pub fn format_size(size: i64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut s = size as f64;
    let mut unit = 0;
    while s >= 1000.0 && unit < UNITS.len() - 1 {
        s /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", size, UNITS[unit])
    } else {
        format!("{:.1} {}", s, UNITS[unit])
    }
}

/// Append " (N)" before the extension until the name is unused.
pub fn generate_unique_filename(existing: &[DirEntryData], name: &str) -> String {
    let base = if let Some(dot) = name.rfind('.') {
        let (stem, ext) = name.split_at(dot);
        (stem.to_string(), ext.to_string())
    } else {
        (name.to_string(), String::new())
    };

    let mut i = 1;
    loop {
        let candidate = if base.1.is_empty() {
            format!("{} ({})", base.0, i)
        } else {
            format!("{} ({}){}", base.0, i, base.1)
        };
        if !existing.iter().any(|d| d.name == candidate) {
            return candidate;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[cfg(unix)]
    use super::ensure_private_dir;

    #[cfg(unix)]
    #[test]
    fn ensure_private_dir_restricts_to_owner() {
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("nanofile-private-dir-{}", std::process::id()));
        let nested = root.join("blocks");
        let _ = std::fs::remove_dir_all(&root);

        // Only the leaf is restricted; parents keep the process default.
        ensure_private_dir(&nested).unwrap();
        assert!(nested.is_dir());
        let mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "new dir should be 0700, got {mode:o}");

        // A pre-existing loose directory is tightened as well.
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&nested).unwrap();
        let mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "existing dir should be tightened to 0700");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn format_size_uses_decimal_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1000), "1.0 KB");
        assert_eq!(format_size(999_999), "1000.0 KB");
        assert_eq!(format_size(1_000_000), "1.0 MB");
        assert_eq!(format_size(1_000_000_000), "1.0 GB");
        assert_eq!(format_size(1_000_000_000_000), "1.0 TB");
    }
}
