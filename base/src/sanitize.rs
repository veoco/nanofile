//! Input validation utilities for filenames and paths.
//!
//! These checks are defense-in-depth — the Seafile FS tree resolution
//! already prevents server filesystem traversal, but we reject obviously
//! malicious input early to prevent path confusion within repos.

const INVALID_FILENAME_CHARS: &[char] = &['\0', '\n', '\r', '\t', '\'', '"', '<', '>', '\\'];
const MAX_FILENAME_LEN: usize = 255;
const MAX_PATH_LEN: usize = 4096;

/// Validate a single filename or directory name.
///
/// Returns `Ok(())` if the name is safe, or an error description.
pub fn validate_filename(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("invalid filename");
    }
    if name.len() > MAX_FILENAME_LEN {
        return Err("filename too long");
    }
    if name.contains(INVALID_FILENAME_CHARS) {
        return Err("filename contains invalid characters");
    }
    if name.contains('/') {
        return Err("filename must not contain path separators");
    }
    Ok(())
}

/// Validate a name that may contain `/` for nested paths (used internally
/// by `FileOps::create_file` for batch directory copy).
///
/// Rejects the same dangerous characters as `validate_filename`, but allows
/// `/` since it represents nested virtual FS entries, not filesystem paths.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("invalid name");
    }
    if name.len() > MAX_FILENAME_LEN {
        return Err("name too long");
    }
    if name.contains('\0') {
        return Err("name contains null byte");
    }
    // Only reject XSS-dangerous characters, allow '/' for nested entries.
    for segment in name.split('/') {
        for ch in INVALID_FILENAME_CHARS {
            if *ch != '\0' && segment.contains(*ch) {
                return Err("name contains invalid characters");
            }
        }
    }
    Ok(())
}

/// Validate a Seafile-relative path (e.g. "/dir/file.txt").
///
/// Must start with `/`, contain no null bytes, and have no
/// traversal components (`..`, `.` as segments).
pub fn validate_path(path: &str) -> Result<(), &'static str> {
    // Use canonicalize_path for validation - if it succeeds, path is valid
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
            Component::RootDir => {
                // Root is implicit in our format
            }
            Component::CurDir => {
                // Ignore current directory
            }
            Component::Normal(name) => {
                // Validate the component does not contain dangerous patterns
                if let Some(name_str) = name.to_str() {
                    if name_str == "." || name_str == ".." {
                        return Err("path contains unnormalized components");
                    }
                    // Check for invalid filename characters in each component
                    for ch in INVALID_FILENAME_CHARS {
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
                // Handle parent directory traversal
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

/// Safely join a base path with a relative path component.
///
/// Both paths are canonicalized, and the result is guaranteed to be a valid
/// path within the repository. Path traversal attempts that would escape
/// the root are detected and rejected.
///
/// # Arguments
///
/// * `base` - The base directory path (must start with `/`)
/// * `relative` - The relative path to join (may contain `/` separators)
///
/// # Returns
///
/// Returns the canonicalized combined path, or an error if:
/// - The base path is invalid
/// - The combined path attempts to traverse beyond the root
/// - The path contains invalid characters
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
    // Validate and canonicalize the base path
    let base = canonicalize_path(base)?;

    // Handle empty relative path
    if relative.trim().is_empty() {
        return Ok(base);
    }

    // Ensure relative does not start with / (treat it as relative)
    let relative = relative.trim_start_matches('/');

    // Build combined path
    let combined = if base == "/" {
        format!("/{}", relative)
    } else {
        format!("{}/{}", base, relative)
    };

    // Validate and canonicalize the combined path
    canonicalize_path(&combined)
}

/// Safely normalize a path, validating it for security.
///
/// This function validates the path and rejects any attempts
/// at path traversal (`..` components that escape root).
///
/// # Arguments
///
/// * `path` - The path to normalize (may or may not start with `/`)
///
/// # Returns
///
/// Returns the canonicalized path starting with `/`, or an error if:
/// - The path contains `..` components that would traverse beyond root
/// - The path contains null bytes or other invalid characters
/// - The path exceeds the maximum length (4096)
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
    // Add leading slash if missing, then canonicalize
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
        // Note: "/./foo" is now valid because canonicalize_path normalizes it to "/foo"
        assert!(validate_path("/foo\0bar").is_err());
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
        // When relative starts with /, we strip it and treat as relative to base
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
        // Normalizes . and .. components
        assert_eq!(safe_normalize_path("/foo/./bar").unwrap(), "/foo/bar");
        assert_eq!(safe_normalize_path("/foo/../bar").unwrap(), "/bar");
        assert_eq!(safe_normalize_path("a/b/../c").unwrap(), "/a/c");
    }

    #[test]
    fn test_safe_normalize_path_traversal_blocked() {
        // Blocks traversal beyond root
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
        assert!(safe_normalize_path("/foo\\bar").is_err()); // backslash blocked
    }

    #[test]
    fn test_safe_normalize_path_max_length() {
        let long_path = format!("/{}", "x".repeat(MAX_PATH_LEN + 1));
        assert!(safe_normalize_path(&long_path).is_err());
    }
}
