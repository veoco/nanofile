//! Format dispatch: turning a file's bytes into the text the index stores.
//!
//! The indexer itself knows nothing about file formats. It asks this module
//! what a file contains, and gets one of two answers: text to index, or a
//! reason the format is not supported. Adding a format means adding an arm to
//! [`extract`] (and eventually a real parser), then bumping
//! [`EXTRACTOR_VERSION`] so the backfill re-runs over files it had previously
//! recorded as unsupported.
//!
//! # Versioning
//!
//! [`EXTRACTOR_VERSION`] is stamped into every index document. The backfill
//! treats a document whose stamped version is not the current one as stale, so
//! a format that becomes supported does not require a manual full rebuild: the
//! next quiet pass notices the version difference and picks the file up again.
//! Any change to what this module extracts — including the text-extraction
//! details below — must bump the constant, or already-indexed files keep their
//! old content forever.
//!
//! # Text Detection
//!
//! [`is_indexable_text`] determines whether a file should be indexed as plain
//! text:
//! 1. Known text-file extensions (`.txt .rs .py .md …`)
//! 2. Content sniffing — first 1024 bytes contain no NUL bytes and are valid
//!    UTF-8.

/// Version of the extraction pipeline as a whole.
///
/// Bumping this is what makes the backfill re-extract files that were already
/// processed. `1` is the pre-pipeline arrangement (plain text only, no
/// per-document version); the first version that records itself is `2`.
pub const EXTRACTOR_VERSION: u32 = 2;

/// How much of a file's content may be read for indexing.
///
/// The prefix is enough for searchable content and bounds both the block-store
/// read and the memory the extraction holds.
pub const MAX_INDEXED_BYTES: usize = 8 * 1024 * 1024;

/// How much extracted text is stored in an index document.
///
/// Content is a stored field, so this also bounds what a search highlights.
pub const MAX_INDEXED_CONTENT_BYTES: usize = 8 * 1024 * 1024;

/// What the extraction pipeline decided about a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extracted {
    /// Index this text.
    Text(String),
    /// Do not index; the reason is recorded so the file is not re-read every
    /// backfill pass until the extractor version changes.
    Unsupported(&'static str),
}

/// Extraction failed for a reason that may be transient or format-specific.
///
/// Kept separate from [`Extracted::Unsupported`] so the caller can record a
/// failure with a retry cooldown instead of "permanently not indexable".
#[derive(Debug, Clone)]
pub struct ExtractError(pub String);

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExtractError {}

/// File extensions considered indexable plain text.
const TEXT_EXTENSIONS: &[&str] = &[
    // Code
    "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "rb", "php", "c", "cpp", "h", "hpp", "cs",
    "swift", "kt", "scala", "dart", "lua", "pl", "pm", "r", "m", "mm", "clj", "cljs", "coffee",
    "groovy", "erl", "hrl", "fs", "fsx", "hs", "lhs", "nim", "zig", "v", "vhdl", "asm", "s", "awk",
    "cbl", "cc", "cfc", "cfm", "cob", "cpy", "d", "e", "el", "ex", "exs", "f", "f90", "f95", "for",
    "frag", "fsh", "geo", "glsl", "gml", "gql", "graphql", "gyp", "hbs", "hxx", "ino", "ipp", "j",
    "jl", "kt", "kts", "lagda", "lisp", "ll", "lm", "lpr", "ls", "m4", "mak", "ml", "mli", "mll",
    "mly", "mo", "mod", "ms", "mt", "nix", "njk", "nqp", "ox", "oxh", "oxo", "p6", "p7s", "pas",
    "pck", "pd", "pdd", "pkh", "pig", "pl6", "pls", "pm6", "pod", "pod6", "pp", "prc", "prefs",
    "pro", "proto", "ps", "ps1", "psd1", "psm1", "pt", "purs", "pxd", "pxi", "pyx", "qbs", "qml",
    "r2", "r3", "rake", "rbw", "rbx", "rhtml", "rkt", "rmd", "rno", "roff", "rpy", "rq", "rsx",
    "ru", "sage", "sas", "sass", "sc", "scad", "scm", "scss", "sed", "sfd", "sh", "sjs", "sls",
    "sml", "sps", "sqf", "sr", "ss", "st", "styl", "sv", "t", "tcc", "tcl", "tex", "textile",
    "tla", "tlx", "tpl", "tpp", "tst", "ttl", "twig", "uc", "udf", "vala", "vbs", "vhd", "vim",
    "vm", "vsh", "w", "wast", "wat", "webidl", "xib", "xl", "xqy", "xquery", "xsd", "xsl", "xslt",
    "xul", "yang", "yaws", "yxx", "yy", "zep", // Scripts / config
    "sh", "bash", "zsh", "fish", "bat", "cmd", "ps1", "gradle", "cmake", "make", "mk",
    // Web
    "html", "htm", "xhtml", "css", "scss", "less", "sass", "vue", "svelte", "ejs", "erb", "hbs",
    "mustache", "haml", "slim", "jade", "pug", // Data / markup
    "json", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf", "env", "csv", "tsv", "sql",
    "graphql", // Docs
    "txt", "text", "md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "rst", "rtf", "tex", "bib",
    "log", "org", "pod", "wiki", "creole", "rest", "asc", "adoc", "asciidoc", "docbook",
    // Other
    "diff", "patch", "po", "pot", "spec",
];

/// Extract the indexable text of a file.
///
/// `filename` decides the format; `data` is the (possibly truncated) content.
/// An unknown extension falls back to content sniffing, which is what makes a
/// README with no suffix searchable.
pub fn extract(filename: &str, data: &[u8]) -> Result<Extracted, ExtractError> {
    if is_text_extension(filename) {
        return Ok(extract_text(data));
    }
    if text_content_sniff(data) {
        return Ok(extract_text(data));
    }
    Ok(Extracted::Unsupported("not a text file"))
}

/// Whether a file should be indexed as plain text.
///
/// Uses a two-phase check:
/// 1. Extension whitelist — fast, file-specific.
/// 2. Content sniffing — for unknown extensions, check the first 1024 bytes
///    contain no NUL bytes and are valid UTF-8.
pub fn is_indexable_text(filename: &str, data: &[u8]) -> bool {
    matches!(extract(filename, data), Ok(Extracted::Text(_)))
}

/// Whether the file's extension is a known text format.
fn is_text_extension(filename: &str) -> bool {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    TEXT_EXTENSIONS.contains(&ext.as_str())
}

/// Lossy-decode `data` into the text the index stores, capped and NUL-free.
fn extract_text(data: &[u8]) -> Extracted {
    let lossy = String::from_utf8_lossy(data);
    let trimmed = lossy.trim_end_matches('\u{0}');
    let mut text = String::with_capacity(trimmed.len().min(MAX_INDEXED_CONTENT_BYTES));
    // Drop interior NULs: they carry no meaning in a search index and some
    // clients render them as a replacement character.
    for chunk in trimmed.split('\u{0}') {
        text.push_str(chunk);
        if text.len() >= MAX_INDEXED_CONTENT_BYTES {
            break;
        }
    }
    if text.len() > MAX_INDEXED_CONTENT_BYTES {
        let cut = floor_char_boundary(&text, MAX_INDEXED_CONTENT_BYTES);
        text.truncate(cut);
    }
    Extracted::Text(text)
}

/// Largest index `<= i` that is a UTF-8 char boundary of `s`.
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Sniff whether `data` looks like plain text by checking the first 1024
/// bytes for NUL bytes and UTF-8 validity.
pub fn text_content_sniff(data: &[u8]) -> bool {
    let head = if data.len() > 1024 {
        &data[..1024]
    } else {
        data
    };

    // NUL byte → binary.
    if head.contains(&0) {
        return false;
    }

    // Must be valid UTF-8.
    std::str::from_utf8(head).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_indexable_text_known_extensions() {
        assert!(is_indexable_text("hello.rs", b"fn main() {}"));
        assert!(is_indexable_text("main.py", b"print('hello')"));
        assert!(is_indexable_text("readme.md", b"# Title"));
        assert!(is_indexable_text("config.toml", b"[server]"));
        assert!(is_indexable_text("index.html", b"<html>"));
        assert!(is_indexable_text("style.css", b"body {}"));
        assert!(is_indexable_text("data.json", b"{}"));
        assert!(is_indexable_text("script.sh", b"#!/bin/bash"));
        assert!(is_indexable_text("README.txt", b"plain text"));
    }

    #[test]
    fn test_is_indexable_text_binary() {
        assert!(!is_indexable_text("image.png", b"\x89PNG\r\n\x1a\n"));
        // PDF starts with %PDF-1.4 which looks like text, so it's caught by
        // extension check. Binary detection via content: has null bytes.
        assert!(!is_indexable_text("binary.bin", &[0, 1, 2, 3, 4, 5]));
        assert!(!is_indexable_text("data.raw", b"text\x00binary"));
    }

    #[test]
    fn test_is_indexable_text_sniffing() {
        // File with no extension but valid text content.
        assert!(is_indexable_text("README", b"Hello World"));
        // File with no extension and binary content.
        assert!(!is_indexable_text("data", &[0x00, 0x01, 0x02]));
    }

    #[test]
    fn test_is_indexable_text_empty() {
        // Empty file should be valid.
        assert!(is_indexable_text("notes.txt", b""));
    }

    #[test]
    fn a_known_extension_yields_its_content() {
        assert_eq!(
            extract("notes.txt", b"hello").unwrap(),
            Extracted::Text("hello".to_string())
        );
    }

    /// An unsupported file says why, so the index can record it and the
    /// backfill does not re-read it every pass.
    #[test]
    fn an_unknown_binary_is_unsupported_not_an_error() {
        assert_eq!(
            extract("image.png", b"\x89PNG\r\n\x1a\n").unwrap(),
            Extracted::Unsupported("not a text file")
        );
    }

    /// Interior NULs are dropped rather than stored in the index.
    #[test]
    fn interior_nuls_are_removed() {
        let Extracted::Text(text) = extract("notes.txt", b"a\x00b").unwrap() else {
            panic!("text expected");
        };
        assert!(!text.contains('\u{0}'));
        assert_eq!(text, "ab");
    }

    /// Content longer than the cap is truncated on a char boundary.
    #[test]
    fn oversize_content_is_truncated_on_a_char_boundary() {
        let mut data = vec![b'a'; MAX_INDEXED_CONTENT_BYTES + 10];
        data.extend_from_slice("中".as_bytes());
        let Extracted::Text(text) = extract("big.txt", &data).unwrap() else {
            panic!("text expected");
        };
        assert!(text.len() <= MAX_INDEXED_CONTENT_BYTES);
        assert!(text.is_char_boundary(text.len()));
    }
}
