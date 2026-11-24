use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Split an absolute path into its parent directory and final segment in one pass.
///
/// The same rule was open-coded at more than a dozen call sites, each deciding
/// root/trailing-slash cases slightly differently. `("/a/b")` yields `("/a",
/// "b")`, a top-level `("a")` yields `("/", "a")`, and `"/"` yields `("/", "")`.
pub fn parent_and_name(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some(("", name)) => ("/", name),
        Some((parent, name)) => (parent, name),
        None => ("/", path),
    }
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
///
/// The result never exceeds [`base::sanitize::MAX_FILENAME_LEN`] UTF-8 bytes:
/// when the counter pushes the name past the limit, the stem is shortened on a
/// UTF-8 character boundary and the extension and the " (N)" marker are kept.
/// Without this, repeatedly uploading the same long name grew the name past
/// what the server itself accepts and what a Windows client can store.
pub fn generate_unique_filename(existing: &[DirEntryData], name: &str) -> String {
    let max_len = base::sanitize::MAX_FILENAME_LEN;
    let (stem, extension) = match name.rfind('.') {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    };

    let mut i = 1;
    loop {
        let suffix = format!(" ({i})");
        let candidate = fit_unique_name(stem, &suffix, extension, max_len);
        if !existing.iter().any(|d| d.name == candidate) {
            return candidate;
        }
        i += 1;
    }
}

/// `stem + suffix + extension`, shortened to at most `max_len` bytes.
///
/// The suffix (the " (N)" marker) is always kept intact and the extension is
/// kept whenever it fits next to it; only the stem shrinks. A genuinely
/// pathological extension longer than the whole limit is itself truncated, so
/// the caller still gets a bounded, usable name.
fn fit_unique_name(stem: &str, suffix: &str, extension: &str, max_len: usize) -> String {
    let fixed = suffix.len() + extension.len();
    if fixed < max_len {
        let stem = truncate_utf8(stem, max_len - fixed);
        format!("{stem}{suffix}{extension}")
    } else {
        let extension = truncate_utf8(extension, max_len.saturating_sub(suffix.len()));
        format!("{suffix}{extension}")
    }
}

/// Prefix of `s` at most `max_bytes` long, never splitting a UTF-8 character.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::{
        basename, format_size, generate_unique_filename, parent_and_name, parent_path_from,
    };

    use base::common::DirEntryData;

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
    fn parent_and_name_matches_the_separate_helpers() {
        for path in ["/a.txt", "/dir/a.txt", "/", "", "/dir/"] {
            let (parent, name) = parent_and_name(path);
            assert_eq!(parent, parent_path_from(path), "parent of {path:?}");
            assert_eq!(name, basename(path), "name of {path:?}");
        }
        assert_eq!(parent_and_name("/dir/a.txt"), ("/dir", "a.txt"));
        assert_eq!(parent_and_name("/a.txt"), ("/", "a.txt"));
        // A relative name has no directory part; callers used to fall back to the
        // whole input as the name for this case.
        assert_eq!(parent_and_name("a.txt"), ("/", "a.txt"));
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

    fn dirent(name: &str) -> DirEntryData {
        DirEntryData {
            id: "0".repeat(40),
            mode: 0o100644,
            modifier: "u@example.com".to_string(),
            mtime: 0,
            name: name.to_string(),
            size: 0,
        }
    }

    /// The de-dup format other clients and tests rely on: " (N)" before the
    /// last extension, or at the end for a name without one.
    #[test]
    fn generate_unique_filename_keeps_the_existing_format() {
        assert_eq!(generate_unique_filename(&[], "keep.txt"), "keep (1).txt");
        assert_eq!(generate_unique_filename(&[], "report"), "report (1)");
        assert_eq!(generate_unique_filename(&[], ".bashrc"), " (1).bashrc");
        assert_eq!(
            generate_unique_filename(&[dirent("keep (1).txt")], "keep.txt"),
            "keep (2).txt"
        );
        assert_eq!(
            generate_unique_filename(
                &[dirent("keep (1).txt"), dirent("keep (2).txt")],
                "keep.txt"
            ),
            "keep (3).txt"
        );
    }

    /// A long name plus a counter must stay within `MAX_FILENAME_LEN`, keep the
    /// extension and the counter, and be a name `validate_filename` accepts.
    #[test]
    fn generate_unique_filename_truncates_to_the_limit() {
        let max = base::sanitize::MAX_FILENAME_LEN;

        let name = format!("{}.txt", "x".repeat(max - 5));
        assert_eq!(name.len(), max - 1);
        let unique = generate_unique_filename(&[], &name);
        assert_eq!(unique.len(), max, "the counter fills the budget exactly");
        assert!(unique.ends_with(".txt"), "extension must survive: {unique}");
        assert!(unique.contains(" (1)"), "counter must survive: {unique}");
        assert!(base::sanitize::validate_filename(&unique).is_ok());

        // Even without an extension the bound holds.
        let long = "y".repeat(max + 100);
        let unique = generate_unique_filename(&[], &long);
        assert_eq!(unique.len(), max);
        assert!(unique.ends_with(" (1)"));
    }

    /// Truncation must not split a UTF-8 character: 3-byte characters with a
    /// limit that lands mid-character have to back off to the boundary.
    #[test]
    fn generate_unique_filename_truncates_on_a_utf8_boundary() {
        let name = format!("{}.txt", "文".repeat(83));
        let unique = generate_unique_filename(&[], &name);
        assert!(unique.len() <= base::sanitize::MAX_FILENAME_LEN);
        assert!(unique.is_char_boundary(unique.len()));
        assert!(unique.starts_with(&"文".repeat(82)), "got {unique}");
        assert!(unique.ends_with(".txt"), "got {unique}");
        assert!(base::sanitize::validate_filename(&unique).is_ok());

        // A collision keeps growing the counter; the name stays bounded and
        // remains a different candidate each round.
        let existing = vec![dirent(&unique)];
        let next = generate_unique_filename(&existing, &name);
        assert!(next.len() <= base::sanitize::MAX_FILENAME_LEN);
        assert_ne!(next, unique);
        assert!(next.contains(" (2)"), "got {next}");
    }
}
