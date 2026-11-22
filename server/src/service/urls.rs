//! Absolute URL construction for every capability and share link.
//!
//! The pieces used to be spelled out at each mint point (`/download-api/{token}`,
//! `/blks/{token}/{file_id}/{block_id}`, `/f/{token}/`, `/u/{token}/`, …). They
//! live here so a shape change is one edit, and so the "always trim the base"
//! rule cannot be forgotten.

/// `{base}/{op}/{token}` — the shape of the upload, update, download and block
/// capability URLs.
pub fn capability(base: &str, op: &str, token: &str) -> String {
    format!("{}/{op}/{token}", base.trim_end_matches('/'))
}

/// `{base}/{op}/{token}/{rest}` — the block URL adds the file and block ids.
pub fn capability_rest(base: &str, op: &str, token: &str, rest: &str) -> String {
    format!(
        "{}/{op}/{token}/{}",
        base.trim_end_matches('/'),
        rest.trim_start_matches('/')
    )
}

/// `{base}/f/{token}/` for a file share and `{base}/d/{token}/` for a folder
/// share, matching seahub's `gen_shared_link()`.
pub fn share_page(base: &str, token: &str, is_dir: bool) -> String {
    let kind = if is_dir { "d" } else { "f" };
    format!("{}/{kind}/{token}/", base.trim_end_matches('/'))
}

/// `{base}/u/{token}/` — nanofile's upload-link page. This deliberately stays
/// on `/u/{token}/` (seahub uses `/u/d/{token}/`); the router serves it at that
/// path and compatibility.md records the difference.
pub fn upload_page(base: &str, token: &str) -> String {
    format!("{}/u/{token}/", base.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_trims_the_base_and_keeps_the_op() {
        assert_eq!(
            capability("http://h:8082/", "upload-api", "abc"),
            "http://h:8082/upload-api/abc"
        );
        assert_eq!(
            capability("http://h:8082", "download-api", "abc"),
            "http://h:8082/download-api/abc"
        );
    }

    #[test]
    fn capability_rest_joins_the_extra_path() {
        assert_eq!(
            capability_rest("http://h/", "blks", "t", "file-id/block-id"),
            "http://h/blks/t/file-id/block-id"
        );
        assert_eq!(
            capability_rest("http://h", "blks", "t", "/file-id/block-id"),
            "http://h/blks/t/file-id/block-id"
        );
    }

    #[test]
    fn share_page_uses_the_folder_and_file_prefixes() {
        assert_eq!(share_page("https://h/", "tok", false), "https://h/f/tok/");
        assert_eq!(share_page("https://h", "tok", true), "https://h/d/tok/");
    }

    #[test]
    fn upload_page_keeps_the_current_u_token_shape() {
        assert_eq!(
            upload_page("https://h/", "tok"),
            "https://h/u/tok/",
            "the /u/d/ seahub shape is deliberately not used"
        );
    }
}
