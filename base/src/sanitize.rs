//! Validation for filenames and virtual paths.
//!
//! Defense in depth: tree resolution already keeps paths inside a library;
//! these checks reject path confusion within a repo before it reaches a client.

/// Characters that confuse path, JSON or HTTP-header handling everywhere.
///
/// This is the set a *stored* path must not contain: `\0`, the newlines and the
/// tab are control characters; the quotes and angle brackets are shell/HTML
/// delimiters; a backslash is a separator on Windows. `canonicalize_path` applies
/// this set when a path is *addressable*, so an existing library whose entries
/// were created by another Seafile server keeps resolving.
const PATH_CONFUSING_CHARS: &[char] = &['\0', '\n', '\r', '\t', '\'', '"', '<', '>', '\\'];

/// Characters Win32 forbids in a *new* file name (`< > : " / \ | ? *`, minus the
/// separators handled separately).
///
/// Only [`validate_filename`] applies this set: it runs on the creation boundary
/// (upload, rename, mkdir, sync object insertion, WebDAV). It is deliberately not
/// part of path canonicalization, so a name that predates the rule can still be
/// read and deleted; creating a new one would give Windows clients a path they
/// cannot materialize.
const WINDOWS_FORBIDDEN_FILENAME_CHARS: &[char] = &[':', '*', '?', '|'];

/// Longest filename accepted, in UTF-8 bytes: Windows' `MAX_PATH` component
/// limit is 255 UTF-16 code units, and every SMB/NTFS client nanofile targets
/// breaks at the same ballpark. Shared with the conflict/uniquify helpers so a
/// generated name cannot exceed what a stored name may be.
pub const MAX_FILENAME_LEN: usize = 255;
const MAX_PATH_LEN: usize = 4096;

/// Windows reserves these device names regardless of extension or trailing
/// dots/spaces — Win32 treats `CON.txt` as the device `CON`, so a Windows
/// desktop client can never materialize such an entry on NTFS.
const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5",
    "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
    "LPT9",
];

/// True if `name` would collide with a Windows device name (case-insensitive,
/// ignoring extension and trailing dots/spaces).
fn is_windows_reserved_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name);
    let base = base.trim_end_matches([' ', '.']);
    let upper = base.to_ascii_uppercase();
    WINDOWS_RESERVED_NAMES.contains(&upper.as_str())
}

/// Validate a single filename or directory name.
pub fn validate_filename(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("invalid filename");
    }
    if name.len() > MAX_FILENAME_LEN {
        return Err("filename too long");
    }
    if name.contains(PATH_CONFUSING_CHARS) || name.contains(WINDOWS_FORBIDDEN_FILENAME_CHARS) {
        return Err("filename contains invalid characters");
    }
    // Beyond the four control characters in `PATH_CONFUSING_CHARS`: no new name
    // may carry any control character (log/terminal/JSON confusion).
    if name.chars().any(char::is_control) {
        return Err("filename contains control characters");
    }
    if name.contains('/') {
        return Err("filename must not contain path separators");
    }
    // Win32 silently strips trailing dots and spaces (`CON.`, `report.txt `),
    // so such an entry exists under one name on Windows and another everywhere
    // else — a race that makes a synced tree disagree with itself. Reject the
    // name instead of creating an entry a Windows client cannot address.
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("filename must not end with a dot or space");
    }
    if is_windows_reserved_name(name) {
        return Err("filename is reserved by Windows");
    }
    Ok(())
}

/// The filename rule that predates the Windows-only tightening.
///
/// A sync client re-sends whole directory objects, and a directory object may
/// legitimately contain entries named under the old rule (created before the
/// tightening, by this server or another Seafile server). Rejecting that object
/// would make the directory un-syncable forever, so the sync ingest keeps
/// accepting those names. Every interactive creation surface (upload, rename,
/// mkdir, WebDAV) applies [`validate_filename`] instead, so new entries get the
/// stricter rule.
pub fn validate_legacy_filename(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("invalid filename");
    }
    if name.len() > MAX_FILENAME_LEN {
        return Err("filename too long");
    }
    if name.contains(PATH_CONFUSING_CHARS) {
        return Err("filename contains invalid characters");
    }
    if name.contains('/') {
        return Err("filename must not contain path separators");
    }
    if is_windows_reserved_name(name) {
        return Err("filename is reserved by Windows");
    }
    Ok(())
}

/// Validate a Seafile-relative path (e.g. "/dir/file.txt").
pub fn validate_path(path: &str) -> Result<(), &'static str> {
    canonicalize_path(path)?;
    Ok(())
}

/// Canonicalize a virtual path by resolving `.` and `..` components.
///
/// Returns the normalized path with all `.` components removed and `..`
/// components resolved. Returns an error if the path attempts to traverse
/// beyond the root directory.
///
/// # Examples
///
/// ```
/// use base::sanitize::canonicalize_path;
///
/// assert_eq!(canonicalize_path("/").unwrap(), "/");
/// assert_eq!(canonicalize_path("/foo/bar").unwrap(), "/foo/bar");
/// assert_eq!(canonicalize_path("/foo/../bar").unwrap(), "/bar");
/// assert_eq!(canonicalize_path("/foo/./bar").unwrap(), "/foo/bar");
/// assert!(canonicalize_path("/../etc").is_err());
/// ```
pub fn canonicalize_path(path: &str) -> Result<String, &'static str> {
    if path.is_empty() {
        return Err("path must not be empty");
    }
    if !path.starts_with('/') {
        return Err("path must start with /");
    }
    if path.len() > MAX_PATH_LEN {
        return Err("path too long");
    }
    if path.contains('\0') {
        return Err("path contains null byte");
    }

    use std::path::Component;

    let mut normalized = Vec::new();

    for component in std::path::Path::new(path).components() {
        match component {
            Component::RootDir => {}
            Component::CurDir => {}
            Component::Normal(name) => {
                if let Some(name_str) = name.to_str() {
                    if name_str == "." || name_str == ".." {
                        return Err("path contains unnormalized components");
                    }
                    for ch in PATH_CONFUSING_CHARS {
                        if name_str.contains(*ch) {
                            return Err("path contains invalid characters");
                        }
                    }
                    normalized.push(name_str.to_string());
                } else {
                    return Err("path contains invalid unicode");
                }
            }
            Component::ParentDir => {
                if normalized.pop().is_none() {
                    return Err("path traversal beyond root");
                }
            }
            _ => {
                return Err("unsupported path component");
            }
        }
    }

    if normalized.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", normalized.join("/")))
    }
}

/// Join a base path with a relative path, rejecting escapes from the root.
///
/// # Examples
///
/// ```
/// use base::sanitize::safe_join_path;
///
/// assert_eq!(safe_join_path("/", "foo").unwrap(), "/foo");
/// assert_eq!(safe_join_path("/bar", "foo").unwrap(), "/bar/foo");
/// assert_eq!(safe_join_path("/bar", "../foo").unwrap(), "/foo");
/// assert!(safe_join_path("/bar", "../../../etc").is_err());
/// ```
pub fn safe_join_path(base: &str, relative: &str) -> Result<String, &'static str> {
    let base = canonicalize_path(base)?;
    if relative.trim().is_empty() {
        return Ok(base);
    }
    let relative = relative.trim_start_matches('/');
    let combined = if base == "/" {
        format!("/{}", relative)
    } else {
        format!("{}/{}", base, relative)
    };
    canonicalize_path(&combined)
}

/// Normalize a path, rejecting traversal beyond the root.
///
/// # Examples
///
/// ```
/// use base::sanitize::safe_normalize_path;
///
/// assert_eq!(safe_normalize_path("/foo/bar").unwrap(), "/foo/bar");
/// assert_eq!(safe_normalize_path("foo/bar").unwrap(), "/foo/bar");
/// assert_eq!(safe_normalize_path("").unwrap(), "/");
/// assert!(safe_normalize_path("/../etc").is_err());
/// assert!(safe_normalize_path("/foo/../../bar").is_err());
/// ```
pub fn safe_normalize_path(path: &str) -> Result<String, &'static str> {
    let path = path.trim();
    if path.is_empty() {
        return Ok("/".to_string());
    }
    let path_with_slash = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    canonicalize_path(&path_with_slash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_filenames() {
        assert!(validate_filename("file.txt").is_ok());
        assert!(validate_filename("a").is_ok());
        assert!(validate_filename("hello world.txt").is_ok());
    }

    #[test]
    fn test_invalid_filenames() {
        assert!(validate_filename("").is_err());
        assert!(validate_filename(".").is_err());
        assert!(validate_filename("..").is_err());
        assert!(validate_filename("a/b").is_err());
        assert!(validate_filename("a\0b").is_err());
        assert!(validate_filename("a'b").is_err());
        assert!(validate_filename("a\"b").is_err());
        assert!(validate_filename("a<b").is_err());
        assert!(validate_filename("a>b").is_err());
        assert!(validate_filename("a\\b").is_err());
    }

    /// Win32 forbids `: * ? |` in file names, so syncing such an entry would
    /// give Windows clients a path they cannot create or open.
    #[test]
    fn test_windows_forbidden_characters_rejected() {
        for n in ["a:b", "a*b", "a?b", "a|b", "C:report.txt", "what?.txt"] {
            assert!(validate_filename(n).is_err(), "{n} should be rejected");
        }
        // Any control character, not just the four in the path-confusion set.
        for n in ["bad\u{1}name", "x\u{7f}y", "a\u{85}b"] {
            assert!(validate_filename(n).is_err(), "{n:?} should be rejected");
        }
        // Punctuation Win32 accepts stays accepted (parentheses are used by the
        // conflict renamer, so they must not be caught by this rule).
        for n in [
            "a,b.txt", "a;b.txt", "a#b.txt", "a@b.txt", "a!b.txt", "a(b).txt", "a[b].txt",
            "a=b.txt", "a&b.txt", "a+b.txt", "a%b.txt", "a^b.txt", "a~b.txt",
        ] {
            assert!(validate_filename(n).is_ok(), "{n} should be accepted");
        }
    }

    /// Win32 strips trailing dots and spaces, so the server refuses to create an
    /// entry whose name would not round-trip through a Windows client.
    #[test]
    fn test_trailing_dot_or_space_rejected() {
        for n in ["a.", "a ", "report.txt.", "report.txt ", "a. ."] {
            assert!(validate_filename(n).is_err(), "{n:?} should be rejected");
        }
        // A leading dot (dotfile) and embedded dots/spaces are still fine.
        assert!(validate_filename(".profile").is_ok());
        assert!(validate_filename(".profile.").is_err());
        assert!(validate_filename("a.b").is_ok());
        assert!(validate_filename("a b").is_ok());
    }

    #[test]
    fn test_filename_length_boundary() {
        assert!(validate_filename(&"x".repeat(MAX_FILENAME_LEN)).is_ok());
        assert!(validate_filename(&"x".repeat(MAX_FILENAME_LEN + 1)).is_err());
        // The limit is bytes (as stored and as Windows counts the component),
        // not characters: 84 × 3-byte chars fit in 255 bytes.
        let multibyte = "文".repeat(84);
        assert_eq!(multibyte.len(), MAX_FILENAME_LEN - 3);
        assert!(validate_filename(&multibyte).is_ok());
        // 85 characters are exactly 255 bytes: still allowed.
        assert!(validate_filename(&format!("{multibyte}文")).is_ok());
        // 86 characters are 258 bytes: over the limit, unlike a char count that
        // would still be far below 255.
        assert!(validate_filename(&format!("{multibyte}文文")).is_err());
    }

    /// The sync ingest keeps the pre-tightening rule so a directory object with a
    /// legacy name can be re-sent; interactive creation uses the stricter rule.
    #[test]
    fn test_legacy_filename_rule_stays_permissive_for_sync() {
        for name in ["a:b", "a*b", "a?b", "a|b", "report.txt "] {
            assert!(
                validate_filename(name).is_err(),
                "{name:?} is refused for a new entry"
            );
            assert!(
                validate_legacy_filename(name).is_ok(),
                "{name:?} must stay acceptable in a re-sent sync object"
            );
        }
        // The path-confusion rules the sync ingest already enforced still apply.
        for name in ["a/b", "a\\b", "a\0b", "a\nb", "a'b", "a\"b", "a<b", "a>b"] {
            assert!(validate_legacy_filename(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn test_windows_reserved_names_rejected() {
        for n in [
            "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM9", "LPT1", "LPT9",
        ] {
            assert!(validate_filename(n).is_err(), "{n} should be rejected");
        }
        assert!(validate_filename("con").is_err());
        assert!(validate_filename("Com2").is_err());
        assert!(validate_filename("lpt8").is_err());
        assert!(validate_filename("CON.txt").is_err());
        assert!(validate_filename("nul.log").is_err());
        assert!(validate_filename("COM3.dat").is_err());
        assert!(validate_filename("CON.").is_err());
        assert!(validate_filename("LPT1. ").is_err());
    }

    #[test]
    fn test_non_reserved_names_accepted() {
        for n in [
            "context.txt",
            "console",
            "COM10",
            "com0",
            "LPT0",
            "control",
            "autumn",
            "normal.txt",
        ] {
            assert!(validate_filename(n).is_ok(), "{n} should be accepted");
        }
    }

    #[test]
    fn test_valid_paths() {
        assert!(validate_path("/").is_ok());
        assert!(validate_path("/file.txt").is_ok());
        assert!(validate_path("/dir/file.txt").is_ok());
    }

    #[test]
    fn test_invalid_paths() {
        assert!(validate_path("").is_err());
        assert!(validate_path("file.txt").is_err());
        assert!(validate_path("/../etc").is_err());
        assert!(validate_path("/foo\0bar").is_err());
    }

    /// Path components share the *stored-path* rule, not the creation rule: a
    /// library may still contain entries created by an older Seafile server whose
    /// names use characters Win32 forbids, and those entries must stay readable
    /// and deletable. Only creating a new such name is rejected.
    #[test]
    fn test_path_component_rules_are_the_stored_path_rules() {
        for path in ["/foo\0bar", "/foo'bar", "/foo\"bar", "/foo<bar", "/foo>bar"] {
            assert!(validate_path(path).is_err(), "{path:?} should be rejected");
        }
        // A path addresses an existing entry even when a *new* name with these
        // characters would be refused by `validate_filename`.
        for path in ["/foo:bar", "/foo*bar", "/foo?bar", "/foo|bar", "/a/b|c"] {
            assert!(validate_path(path).is_ok(), "{path} must stay addressable");
        }
        assert!(validate_filename("foo:bar").is_err());
        assert!(validate_filename("foo|bar").is_err());
    }

    #[test]
    fn test_canonicalize_path_basic() {
        assert_eq!(canonicalize_path("/").unwrap(), "/");
        assert_eq!(canonicalize_path("/foo").unwrap(), "/foo");
        assert_eq!(canonicalize_path("/foo/bar").unwrap(), "/foo/bar");
        assert_eq!(canonicalize_path("/foo/bar/baz").unwrap(), "/foo/bar/baz");
    }

    #[test]
    fn test_canonicalize_path_current_dir() {
        assert_eq!(canonicalize_path("/foo/./bar").unwrap(), "/foo/bar");
        assert_eq!(canonicalize_path("/./foo/bar").unwrap(), "/foo/bar");
        assert_eq!(canonicalize_path("/foo/bar/.").unwrap(), "/foo/bar");
        assert_eq!(canonicalize_path("/././foo").unwrap(), "/foo");
    }

    #[test]
    fn test_canonicalize_path_parent_dir() {
        assert_eq!(canonicalize_path("/foo/../bar").unwrap(), "/bar");
        assert_eq!(canonicalize_path("/a/b/../c").unwrap(), "/a/c");
        assert_eq!(canonicalize_path("/a/b/c/../../d").unwrap(), "/a/d");
        assert_eq!(canonicalize_path("/a/../b/../c").unwrap(), "/c");
        assert_eq!(canonicalize_path("/a/b/c/../../../d").unwrap(), "/d");
    }

    #[test]
    fn test_canonicalize_path_mixed() {
        assert_eq!(canonicalize_path("/a/./b/../c/./d").unwrap(), "/a/c/d");
        assert_eq!(canonicalize_path("/a/b/c/../.././d").unwrap(), "/a/d");
    }

    #[test]
    fn test_canonicalize_path_traversal_beyond_root() {
        assert!(canonicalize_path("/../etc").is_err());
        assert!(canonicalize_path("/foo/../../../etc").is_err());
        assert!(canonicalize_path("/a/b/c/../../../../d").is_err());
    }

    #[test]
    fn test_canonicalize_path_empty_and_invalid() {
        assert!(canonicalize_path("").is_err());
        assert!(canonicalize_path("foo/bar").is_err()); // No leading /
        assert!(canonicalize_path("/foo\0bar").is_err());
        assert!(canonicalize_path(&format!("/{}", "x".repeat(MAX_PATH_LEN + 1))).is_err());
    }

    #[test]
    fn test_safe_join_path_basic() {
        assert_eq!(safe_join_path("/", "foo").unwrap(), "/foo");
        assert_eq!(safe_join_path("/bar", "foo").unwrap(), "/bar/foo");
        assert_eq!(safe_join_path("/bar/", "foo/").unwrap(), "/bar/foo");
        assert_eq!(safe_join_path("/a/b", "c/d").unwrap(), "/a/b/c/d");
    }

    #[test]
    fn test_safe_join_path_with_parent_dir() {
        assert_eq!(safe_join_path("/bar", "../foo").unwrap(), "/foo");
        assert_eq!(safe_join_path("/a/b", "../c").unwrap(), "/a/c");
        assert_eq!(safe_join_path("/a/b/c", "../../d").unwrap(), "/a/d");
    }

    #[test]
    fn test_safe_join_path_traversal_blocked() {
        assert!(safe_join_path("/bar", "../../../etc").is_err());
        assert!(safe_join_path("/a", "../../b").is_err());
        assert!(safe_join_path("/", "../etc").is_err());
    }

    #[test]
    fn test_safe_join_path_empty_relative() {
        assert_eq!(safe_join_path("/foo", "").unwrap(), "/foo");
        assert_eq!(safe_join_path("/", "").unwrap(), "/");
        assert_eq!(safe_join_path("/foo", "   ").unwrap(), "/foo");
    }

    #[test]
    fn test_safe_join_path_absolute_relative() {
        assert_eq!(safe_join_path("/bar", "/foo").unwrap(), "/bar/foo");
        assert_eq!(safe_join_path("/a/b", "/c/d").unwrap(), "/a/b/c/d");
    }

    #[test]
    fn test_safe_normalize_path_basic() {
        assert_eq!(safe_normalize_path("/foo/bar").unwrap(), "/foo/bar");
        assert_eq!(safe_normalize_path("foo/bar").unwrap(), "/foo/bar");
        assert_eq!(safe_normalize_path("/").unwrap(), "/");
        assert_eq!(safe_normalize_path("").unwrap(), "/");
        assert_eq!(safe_normalize_path("   ").unwrap(), "/");
    }

    #[test]
    fn test_safe_normalize_path_normalization() {
        assert_eq!(safe_normalize_path("/foo/./bar").unwrap(), "/foo/bar");
        assert_eq!(safe_normalize_path("/foo/../bar").unwrap(), "/bar");
        assert_eq!(safe_normalize_path("a/b/../c").unwrap(), "/a/c");
    }

    #[test]
    fn test_safe_normalize_path_traversal_blocked() {
        assert!(safe_normalize_path("/../etc").is_err());
        assert!(safe_normalize_path("../etc").is_err());
        assert!(safe_normalize_path("/foo/../../bar").is_err());
        assert!(safe_normalize_path("../../etc/passwd").is_err());
    }

    #[test]
    fn test_safe_normalize_path_invalid_characters() {
        assert!(safe_normalize_path("/foo\0bar").is_err());
        assert!(safe_normalize_path("/foo'bar").is_err()); // single quote blocked
        assert!(safe_normalize_path("/foo\"bar").is_err()); // double quote blocked
        assert!(safe_normalize_path("/foo<bar").is_err()); // < blocked
        assert!(safe_normalize_path("/foo>bar").is_err()); // > blocked
        // A backslash is a separator on Windows, so `components` consumes it and
        // the character check never sees it. That is the right answer there: what
        // is left is a relative path, and nothing in it reaches outside the root.
        #[cfg(windows)]
        assert_eq!(safe_normalize_path("/foo\\bar").unwrap(), "/foo/bar");
        #[cfg(not(windows))]
        assert!(safe_normalize_path("/foo\\bar").is_err()); // backslash blocked
    }

    #[test]
    fn test_safe_normalize_path_max_length() {
        let long_path = format!("/{}", "x".repeat(MAX_PATH_LEN + 1));
        assert!(safe_normalize_path(&long_path).is_err());
    }
}
