//! Input validation utilities for filenames and paths.
//!
//! These checks are defense-in-depth — the Seafile FS tree resolution
//! already prevents server filesystem traversal, but we reject obviously
//! malicious input early to prevent path confusion within repos.

const INVALID_FILENAME_CHARS: &[char] = &['\0', '\n', '\r', '\t'];
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

/// Validate a Seafile-relative path (e.g. "/dir/file.txt").
///
/// Must start with `/`, contain no null bytes, and have no
/// traversal components (`..`, `.` as segments).
pub fn validate_path(path: &str) -> Result<(), &'static str> {
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
    for segment in path.split('/') {
        if segment == ".." || segment == "." {
            return Err("path traversal detected");
        }
    }
    Ok(())
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
        assert!(validate_path("/./foo").is_err());
        assert!(validate_path("/foo\0bar").is_err());
    }
}
