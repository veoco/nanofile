//! Content types for served files.
//!
//! One table decides the type a served file is sent with for both the web file
//! endpoints and WebDAV.

/// Extension → content type: the one place a served file's type comes from, used
/// by both `/repos/…` and WebDAV.
///
/// A missing row is not cosmetic: `nosniff` makes the content type the only thing
/// that lets an `<img>` paint. `.html` and the other markup formats are
/// `text/plain` on purpose — an uploaded document must never be served as a
/// document in this origin — while `.svg` *is* an image type, which is why its
/// response policy is `sandbox` (see `middleware::apply_security_headers`).
const MIME_BY_EXT: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
    ("ico", "image/x-icon"),
    ("tiff", "image/tiff"),
    ("tif", "image/tiff"),
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("avif", "image/avif"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("3gp", "video/3gpp"),
    ("mkv", "video/x-matroska"),
    ("avi", "video/x-msvideo"),
    ("wmv", "video/x-ms-wmv"),
    ("flv", "video/x-flv"),
    ("mp3", "audio/mpeg"),
    ("flac", "audio/flac"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("opus", "audio/ogg"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("wma", "audio/x-ms-wma"),
    ("pdf", "application/pdf"),
    ("txt", "text/plain; charset=utf-8"),
    ("md", "text/plain; charset=utf-8"),
    ("rs", "text/plain; charset=utf-8"),
    ("py", "text/plain; charset=utf-8"),
    ("js", "text/plain; charset=utf-8"),
    ("ts", "text/plain; charset=utf-8"),
    ("html", "text/plain; charset=utf-8"),
    ("css", "text/plain; charset=utf-8"),
    ("go", "text/plain; charset=utf-8"),
    ("java", "text/plain; charset=utf-8"),
    ("c", "text/plain; charset=utf-8"),
    ("cpp", "text/plain; charset=utf-8"),
    ("h", "text/plain; charset=utf-8"),
    ("rb", "text/plain; charset=utf-8"),
    ("php", "text/plain; charset=utf-8"),
    ("sh", "text/plain; charset=utf-8"),
    ("toml", "text/plain; charset=utf-8"),
    ("json", "text/plain; charset=utf-8"),
    ("yaml", "text/plain; charset=utf-8"),
    ("yml", "text/plain; charset=utf-8"),
    ("csv", "text/plain; charset=utf-8"),
    ("xml", "text/plain; charset=utf-8"),
    ("sql", "text/plain; charset=utf-8"),
    ("conf", "text/plain; charset=utf-8"),
    ("ini", "text/plain; charset=utf-8"),
    ("log", "text/plain; charset=utf-8"),
];

const OCTET_STREAM: &str = "application/octet-stream";

pub fn ext_of(name: &str) -> Option<&str> {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
}

/// A lookup, case-insensitively: a camera writes `IMG_0001.SVG` as readily as
/// `.svg`. `None` for anything unmapped.
fn mime_of(ext: &str) -> Option<&'static str> {
    MIME_BY_EXT
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(ext))
        .map(|(_, mime)| *mime)
}
/// The content type a served file is sent with: a case-insensitive [`MIME_BY_EXT`]
/// lookup, falling back to `application/octet-stream` next to `nosniff`.
pub fn mime_guess(filename: &str) -> &'static str {
    ext_of(filename).and_then(mime_of).unwrap_or(OCTET_STREAM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_guess_ignores_extension_case() {
        assert_eq!(mime_guess("clip.mp4"), "video/mp4");
        assert_eq!(mime_guess("CLIP.MP4"), "video/mp4");
        assert_eq!(mime_guess("IMG_0001.MOV"), "video/quicktime");
        assert_eq!(mime_guess("PHOTO.JPG"), "image/jpeg");
        assert_eq!(mime_guess("logo.SVG"), "image/svg+xml");
        assert_eq!(mime_guess("notes.TXT"), "text/plain; charset=utf-8");
        assert_eq!(mime_guess("archive.bin"), "application/octet-stream");
    }

    /// The table is the only map, so one extension must not carry two answers.
    #[test]
    fn the_mime_table_has_no_duplicate_extension() {
        let mut seen = std::collections::BTreeSet::new();
        for (ext, _) in MIME_BY_EXT {
            assert!(seen.insert(*ext), "{ext} is listed twice");
        }
    }

    /// An uploaded document is never served as a document: that would be
    /// same-origin markup.
    #[test]
    fn markup_is_served_as_text() {
        assert_eq!(mime_guess("page.html"), "text/plain; charset=utf-8");
        assert_eq!(mime_guess("feed.xml"), "text/plain; charset=utf-8");
        assert_eq!(mime_guess("script.js"), "text/plain; charset=utf-8");
    }
}
