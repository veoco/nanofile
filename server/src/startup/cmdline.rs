//! Windows command-line text shared by the two auto-start mechanisms.
//!
//! A login entry (`HKCU\…\Run`) and a service registration (`ImagePath`) both
//! record an executable plus a `--config <absolute path>`, and both have to be
//! read back and compared against the copy that is running. That text is
//! generated and parsed in exactly one place.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// Quotes one argument for a Windows command line.
///
/// Only double quotes need escaping — backslashes are literal except
/// immediately before a quote, and doubling them would corrupt plain paths.
pub(crate) fn win_cmd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// Normalize a Windows path for comparison: no surrounding quotes, one folder
/// separator, no case difference (Windows paths are case-insensitive).
pub(crate) fn normalize_win_path(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .replace('/', "\\")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_only_inner_quotes() {
        assert_eq!(win_cmd_quote(r"C:\a b\x.exe"), r#""C:\a b\x.exe""#);
        assert_eq!(
            win_cmd_quote(r"C:\apps\Na nofile\nanofile.exe"),
            r#""C:\apps\Na nofile\nanofile.exe""#
        );
        assert_eq!(win_cmd_quote(r#"C:\we"ird\x.exe"#), r#""C:\we\"ird\x.exe""#);
    }

    #[test]
    fn normalization_ignores_quotes_case_and_separators() {
        assert_eq!(
            normalize_win_path(r#"  "C:/Apps/Nanofile/nanofile.exe"  "#),
            r"c:\apps\nanofile\nanofile.exe"
        );
    }
}
