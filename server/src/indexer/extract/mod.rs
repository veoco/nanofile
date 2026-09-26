//! Format dispatch: turning a file's bytes into the text the index stores.
//!
//! The indexer itself knows nothing about file formats. It asks this module
//! what a file is ([`plan`]), how much of it to read ([`budget`]), and then
//! hands the bytes back for extraction ([`extract`]). Adding a format means
//! adding an arm to [`plan`] (and a parser module), then bumping
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
//! details, the format table, and the size caps below — must bump the constant,
//! or already-indexed files keep their old content (or their old "skipped"
//! verdict) forever.
//!
//! # Three ways to read a file
//!
//! A plain-text file is indexed from a prefix: the first [`MAX_INDEXED_BYTES`]
//! are enough for search, and a prefix is always a valid text extract. A
//! container format is not — a ZIP's central directory and a PDF's
//! cross-reference table live at the *end* of the file, so a truncated document
//! either fails to parse or, worse, parses to nothing at all. Those formats are
//! read whole, up to [`MAX_STRUCTURED_BYTES`], and a file above that is skipped
//! without being read.
//!
//! # Text Detection
//!
//! A file with no known text extension is sniffed by content:
//! [`text_content_sniff`] accepts the first [`PLAN_SNIFF_BYTES`] when they
//! contain no NUL bytes and are valid UTF-8.
//!
//! # What bounds a document
//!
//! Four things, in the order they apply:
//!
//! * the read itself — a document is never larger than [`MAX_STRUCTURED_BYTES`],
//!   and a file above that cap is skipped without being read;
//! * the parsers' own limits — the PDF reader caps every stream it decodes and
//!   the page count, and the Office path counts what a package decompresses to
//!   before the parser sees it;
//! * this process's per-document text caps — [`MAX_INDEXED_CONTENT_BYTES`] of
//!   text, which is also what a search result can highlight;
//! * the extraction worker ([`worker`]) — a separate, confined process with an
//!   address-space limit, a CPU limit and a wall-clock timeout. That last one
//!   is not redundant: [`guard`] catches panics, and a stack overflow or a
//!   failed allocation is not a panic but an abort, which no boundary in this
//!   process could contain.
//!
//! Documents are extracted *only* in that worker. If it cannot be started, or
//! refuses to run without confinement, the file is failed and retried later;
//! nothing in the server parses a document itself.
//!
//! # Failure Handling
//!
//! [`extract`] never fails and never unwinds: a format the pipeline cannot read
//! comes back as [`Extracted::Unsupported`] with a reason, which the caller
//! records as a *skipped* document. That distinction matters — a parse error or
//! a panic is a property of the bytes, so retrying it on a cooldown would be
//! waste, whereas a failure to *read* the blocks is transient and stays a
//! failed document the backfill retries.

mod office;
mod pdf;

/// Version of the extraction pipeline as a whole.
///
/// Bumping this is what makes the backfill re-extract files that were already
/// processed. `1` is the pre-pipeline arrangement (plain text only, no
/// per-document version); `2` is the first version that records itself, and
/// covers everything this module extracts — plain text, PDF and Office
/// documents.
pub const EXTRACTOR_VERSION: u32 = 2;

/// How much of a file's content may be read for indexing as plain text.
///
/// The prefix is enough for searchable content and bounds both the block-store
/// read and the memory the extraction holds.
pub const MAX_INDEXED_BYTES: usize = 8 * 1024 * 1024;

/// How much extracted text is stored in an index document.
///
/// Content is a stored field, so this also bounds what a search highlights.
pub const MAX_INDEXED_CONTENT_BYTES: usize = 8 * 1024 * 1024;

/// How much of a container format's content may be read.
///
/// A document cannot be indexed from a prefix (see the module docs), so this is
/// the whole file — and therefore also the point above which a document is
/// skipped unread. It bounds the parser's input, and with it the memory a
/// single parse can occupy.
///
/// Raising this must bump [`EXTRACTOR_VERSION`], otherwise documents already
/// recorded as skipped for being too large are never reconsidered.
pub const MAX_STRUCTURED_BYTES: usize = 32 * 1024 * 1024;

/// How much of a file's head is used to decide whether it is text.
pub const PLAN_SNIFF_BYTES: usize = 1024;

/// What the extraction pipeline decided about a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extracted {
    /// Index this text.
    Text(String),
    /// Do not index; the reason is recorded so the file is not re-read every
    /// backfill pass until the extractor version changes.
    Unsupported(&'static str),
}

/// A structured document format this module can parse.
///
/// Kept as our own type so the pipeline never has to name the parser crates'
/// enums at its call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Document {
    Pdf,
    Office(office_oxide::DocumentFormat),
}

/// What the pipeline must read for one file, decided from its name alone.
///
/// Deciding before the read is what keeps a container format from being fed a
/// truncated prefix, and what lets an oversized document be skipped without
/// costing a block read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// A known text extension: index a prefix of the bytes.
    Text,
    /// Nothing claims the file: read a prefix and sniff it.
    Sniff,
    /// Read the whole file and parse it.
    Document(Document),
}

/// Why the extractor did not index a file.
///
/// Named constants because these reasons cross a process boundary: the worker's
/// reply carries the text, and [`intern`] maps it back onto one of them so the
/// caller keeps its `&'static str` and the index records something it can
/// compare against.
pub mod reason {
    pub const PARSE: &str = "could not parse document";
    pub const ENCRYPTED: &str = "encrypted pdf";
    pub const NO_TEXT: &str = "no extractable text";
    pub const PANIC: &str = "extractor panicked";
    pub const NOT_TEXT: &str = "not a text file";
    pub const BUDGET: &str = "document expands past the extraction budget";
    pub const PARTS: &str = "document has too many parts";
    pub const FAILED: &str = "extractor failed";
    pub const TIMED_OUT: &str = "extractor timed out";
    pub const SANDBOX_OFF: &str = "the sandbox is switched off";

    /// Every reason the extractor can produce.
    pub const ALL: &[&str] = &[
        PARSE,
        ENCRYPTED,
        NO_TEXT,
        PANIC,
        NOT_TEXT,
        BUDGET,
        PARTS,
        FAILED,
        TIMED_OUT,
        SANDBOX_OFF,
    ];

    /// Map a reason that crossed the process boundary back onto a constant.
    ///
    /// Anything unrecognised becomes [`FAILED`]: the point of the table is that
    /// the caller never has to allocate a reason per document, and a worker
    /// that says something this process does not know is a failed extraction
    /// rather than a skipped one with a made-up label.
    pub fn intern(value: &str) -> &'static str {
        ALL.iter()
            .copied()
            .find(|known| *known == value)
            .unwrap_or(FAILED)
    }
}

impl Plan {
    /// The byte that names this plan in the worker protocol.
    ///
    /// The whole enum is covered rather than only the documents: the parent
    /// decides what it sends, and a protocol that can already carry every plan
    /// does not have to change the day a text prefix moves into the child too.
    pub fn tag(self) -> u8 {
        use office_oxide::DocumentFormat;

        match self {
            Plan::Text => 0,
            Plan::Sniff => 1,
            Plan::Document(Document::Pdf) => 2,
            Plan::Document(Document::Office(DocumentFormat::Docx)) => 3,
            Plan::Document(Document::Office(DocumentFormat::Xlsx)) => 4,
            Plan::Document(Document::Office(DocumentFormat::Pptx)) => 5,
            Plan::Document(Document::Office(DocumentFormat::Doc)) => 6,
            Plan::Document(Document::Office(DocumentFormat::Xls)) => 7,
            Plan::Document(Document::Office(DocumentFormat::Ppt)) => 8,
        }
    }

    /// The plan a worker request names, or `None` for an unknown tag.
    pub fn from_tag(tag: u8) -> Option<Plan> {
        use office_oxide::DocumentFormat;

        Some(match tag {
            0 => Plan::Text,
            1 => Plan::Sniff,
            2 => Plan::Document(Document::Pdf),
            3 => Plan::Document(Document::Office(DocumentFormat::Docx)),
            4 => Plan::Document(Document::Office(DocumentFormat::Xlsx)),
            5 => Plan::Document(Document::Office(DocumentFormat::Pptx)),
            6 => Plan::Document(Document::Office(DocumentFormat::Doc)),
            7 => Plan::Document(Document::Office(DocumentFormat::Xls)),
            8 => Plan::Document(Document::Office(DocumentFormat::Ppt)),
            _ => return None,
        })
    }
}

/// Decide from the filename how a file must be read.
///
/// Detection is by extension only. Magic bytes cannot disambiguate the
/// containers that matter here — `PK\x03\x04` is shared by three Office
/// formats and `D0CF11E0` by the three legacy ones — and a `.docx` renamed to
/// `.bin` is no more indexable than any other misnamed file.
pub fn plan(filename: &str) -> Plan {
    let ext = extension(filename);
    if let Some(document) = structured_format(&ext) {
        return Plan::Document(document);
    }
    if is_text_extension(&ext) {
        return Plan::Text;
    }
    Plan::Sniff
}

/// One document the worker's self-test parses, in the confined process.
pub(crate) struct ProbeDocument {
    /// The name its verdict is reported under.
    pub(crate) name: &'static str,
    pub(crate) plan: Plan,
    /// A word the extracted text must contain.
    pub(crate) word: &'static str,
    pub(crate) bytes: &'static [u8],
}

/// What the self-test parses, and why it is part of the self-test.
///
/// The report says the confinement was installed; this says the confined
/// process can still do the work it exists for. The two are different failures:
/// a profile or filter that denies a syscall a parser needs shows up here as
/// `parse=pdf-unsupported`, instead of as a deployment where every document
/// quietly becomes a skipped file.
///
/// The bytes are the fixtures the test suite parses, carried into the binary
/// because the child is denied every path — it has nothing to open them from.
pub(crate) const PROBE_DOCUMENTS: &[ProbeDocument] = &[
    ProbeDocument {
        name: "pdf",
        plan: Plan::Document(Document::Pdf),
        word: "nanofile",
        bytes: include_bytes!("../../../tests/fixtures/probe.pdf"),
    },
    ProbeDocument {
        name: "docx",
        plan: Plan::Document(Document::Office(office_oxide::DocumentFormat::Docx)),
        word: "nanofile",
        bytes: include_bytes!("../../../tests/fixtures/probe.docx"),
    },
];

/// How many bytes of a file the plan needs, or why it cannot be indexed.
///
/// The caller reads at most this much; for a document the value is the whole
/// file, so a short read is an error rather than a partial extract.
pub fn budget(plan: Plan, size: u64) -> Result<usize, &'static str> {
    match plan {
        Plan::Text | Plan::Sniff => Ok(MAX_INDEXED_BYTES),
        Plan::Document(_) => {
            if size > MAX_STRUCTURED_BYTES as u64 {
                Err("file too large to index")
            } else {
                Ok(size as usize)
            }
        }
    }
}

/// Extract the indexable text of a file the caller read according to `plan`.
///
/// Infallible by construction: see the module docs on failure handling.
pub fn extract(plan: Plan, data: Vec<u8>) -> Extracted {
    match plan {
        Plan::Text => extract_text(&data),
        Plan::Sniff => {
            if text_content_sniff(&data) {
                extract_text(&data)
            } else {
                Extracted::Unsupported(reason::NOT_TEXT)
            }
        }
        Plan::Document(Document::Pdf) => pdf::extract_text(data),
        Plan::Document(Document::Office(format)) => office::extract_text(data, format),
    }
}

/// Bound the document parser's own per-document text budget.
///
/// `office_oxide` renders a spreadsheet's text *before* this module can cap it,
/// and the output can dwarf the file: the crate documents a 110 KB `.xlsx`
/// holding one large shared string referenced from twelve thousand cells
/// rendering to hundreds of megabytes. Its default budget is 256 Mi characters,
/// far above anything the index stores, so it is lowered to
/// [`MAX_INDEXED_CONTENT_BYTES`]: the work then stops roughly where truncation
/// would have, and a crafted file cannot make the process allocate text nobody
/// reads. The setting is process-global, so it is applied once at startup.
pub(crate) fn configure_limits() {
    office_oxide::limits::set_max_text_chars(MAX_INDEXED_CONTENT_BYTES);
}

/// Whether a file should be indexed as plain text.
///
/// A structured document is not text, whatever its bytes look like: it is
/// parsed, not stored verbatim.
pub fn is_indexable_text(filename: &str, data: &[u8]) -> bool {
    match plan(filename) {
        Plan::Text => true,
        Plan::Sniff => text_content_sniff(data),
        Plan::Document(_) => false,
    }
}

/// Sniff whether `data` looks like plain text by checking the first
/// [`PLAN_SNIFF_BYTES`] for NUL bytes and UTF-8 validity.
pub fn text_content_sniff(data: &[u8]) -> bool {
    let head = if data.len() > PLAN_SNIFF_BYTES {
        &data[..PLAN_SNIFF_BYTES]
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

/// Filename → structured format, decided by this table.
///
/// Deliberately *not* `DocumentFormat::from_extension`: that table grows with
/// the crate, and a format the crate claims is a file the pipeline then reads
/// in full. `office_oxide` 0.1.12 already claims `.pot` (a gettext template
/// this module lists as text) and `.dot` (Graphviz), so inheriting the table
/// would quietly repurpose both. Macro-enabled and template extensions are left
/// out because the pinned readers reject their content types.
fn structured_format(ext: &str) -> Option<Document> {
    use office_oxide::DocumentFormat;

    Some(match ext {
        "pdf" => Document::Pdf,
        "docx" => Document::Office(DocumentFormat::Docx),
        "xlsx" => Document::Office(DocumentFormat::Xlsx),
        "pptx" => Document::Office(DocumentFormat::Pptx),
        "doc" => Document::Office(DocumentFormat::Doc),
        "xls" => Document::Office(DocumentFormat::Xls),
        "ppt" => Document::Office(DocumentFormat::Ppt),
        _ => return None,
    })
}

/// Lowercased extension of a filename, without the dot.
fn extension(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

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

/// Whether a lowercased extension, without the dot, is a known text format.
fn is_text_extension(ext: &str) -> bool {
    TEXT_EXTENSIONS.contains(&ext)
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
    cap_text(&mut text);
    Extracted::Text(text)
}

/// Turn a document parser's output into the index's verdict.
///
/// Shared by the PDF and Office paths: normalises the separators those parsers
/// emit, caps the length, and treats "parsed successfully but produced nothing"
/// as not indexable. That last rule is what keeps a scanned PDF from being
/// recorded as an indexed document with empty content.
pub(super) fn finish(text: String) -> Extracted {
    let mut text = text;
    normalize(&mut text);
    cap_text(&mut text);
    if text.trim().is_empty() {
        return Extracted::Unsupported(reason::NO_TEXT);
    }
    Extracted::Text(text)
}

/// Run a step with a panic boundary.
///
/// Both parser crates panic on some corrupted input (measured: roughly 1 in 600
/// for PDF), and a panic here would otherwise take down the blocking task and
/// the batch with it. Catching it keeps the module's contract — [`extract`]
/// always returns, never unwinds — and turns the file into a skipped document.
/// `fallback` builds the caller's own verdict from the panic message; the
/// default panic hook still prints it, which is the diagnostic we want.
///
/// The index pipeline reuses this around the blocking call, so a panic in its
/// own dispatch cannot fail a whole run either. It catches panics only: a stack
/// overflow or a failed allocation aborts, which is why documents are parsed in
/// a separate process (see [`worker`]).
pub(crate) fn guard<T, F>(label: &str, fallback: impl FnOnce(&str) -> T, f: F) -> T
where
    F: FnOnce() -> T,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            tracing::warn!("{label}: extractor panicked: {detail}");
            fallback(&detail)
        }
    }
}

/// Replace the separators a document parser emits with the ones the index
/// expects: a non-breaking space between words, a form feed between pages.
///
/// Tokenization is unaffected either way — both are non-alphanumeric, so the
/// tokenizer already splits on them — but the stored content field is what a
/// client highlights against, and a form feed renders as nothing useful there.
fn normalize(text: &mut String) {
    if !text.contains(['\u{a0}', '\u{c}', '\u{0}']) {
        return;
    }
    *text = text
        .chars()
        .filter(|c| *c != '\u{0}')
        .map(|c| match c {
            '\u{a0}' => ' ',
            '\u{c}' => '\n',
            other => other,
        })
        .collect();
}

/// Truncate to [`MAX_INDEXED_CONTENT_BYTES`] on a char boundary.
fn cap_text(text: &mut String) {
    if text.len() > MAX_INDEXED_CONTENT_BYTES {
        let cut = floor_char_boundary(text, MAX_INDEXED_CONTENT_BYTES);
        text.truncate(cut);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The formats the pipeline parses rather than stores verbatim.
    const DOCUMENT_EXTENSIONS: &[&str] = &["pdf", "docx", "xlsx", "pptx", "doc", "xls", "ppt"];

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

    /// A document is parsed, not stored verbatim, so it is never "text" — even
    /// when its header happens to be printable ASCII.
    #[test]
    fn a_document_is_not_indexable_text() {
        for ext in DOCUMENT_EXTENSIONS {
            let name = format!("file.{ext}");
            assert!(
                !is_indexable_text(&name, b"%PDF-1.4\n1 0 obj\n"),
                "{name} must be parsed, not indexed as text"
            );
            assert!(matches!(plan(&name), Plan::Document(_)), "{name} plan");
        }
    }

    /// The format table is ours, so extensions that mean something else in this
    /// codebase must not be captured by a document parser.
    #[test]
    fn formats_owned_by_other_extensions_are_not_documents() {
        // Gettext catalogue: listed as text above.
        assert_eq!(plan("messages.pot"), Plan::Text);
        // Graphviz source: no text extension, so it is sniffed — but it must
        // not be handed to the legacy Word parser.
        assert_eq!(plan("graph.dot"), Plan::Sniff);
        // Macro-enabled and template Office files are not in the pinned
        // readers' accepted content types, so they are not claimed either.
        for name in ["a.docm", "a.xlsm", "a.pptm", "a.dotx", "a.potx", "a.xlsb"] {
            assert!(
                matches!(plan(name), Plan::Sniff),
                "{name} should not be claimed"
            );
        }
    }

    #[test]
    fn plan_picks_a_reader_per_extension() {
        assert_eq!(plan("report.pdf"), Plan::Document(Document::Pdf));
        assert_eq!(plan("Report.PDF"), Plan::Document(Document::Pdf));
        assert_eq!(plan("notes.txt"), Plan::Text);
        assert_eq!(plan("README"), Plan::Sniff);
        assert_eq!(plan("archive.tar.gz"), Plan::Sniff);
    }

    #[test]
    fn budget_reads_text_as_a_prefix_and_documents_whole() {
        assert_eq!(budget(Plan::Text, 1_000).unwrap(), MAX_INDEXED_BYTES);
        assert_eq!(budget(Plan::Sniff, 1_000).unwrap(), MAX_INDEXED_BYTES);

        let pdf = Plan::Document(Document::Pdf);
        assert_eq!(budget(pdf, 1_000).unwrap(), 1_000);
        assert_eq!(
            budget(pdf, MAX_STRUCTURED_BYTES as u64).unwrap(),
            MAX_STRUCTURED_BYTES
        );

        // One byte over the cap is refused rather than truncated: a partial
        // container does not parse.
        assert_eq!(
            budget(pdf, MAX_STRUCTURED_BYTES as u64 + 1),
            Err("file too large to index")
        );
    }

    #[test]
    fn a_known_extension_yields_its_content() {
        assert_eq!(
            extract(plan("notes.txt"), b"hello".to_vec()),
            Extracted::Text("hello".to_string())
        );
    }

    /// An unsupported file says why, so the index can record it and the
    /// backfill does not re-read it every pass.
    #[test]
    fn an_unknown_binary_is_unsupported_not_an_error() {
        assert_eq!(
            extract(plan("image.png"), b"\x89PNG\r\n\x1a\n".to_vec()),
            Extracted::Unsupported(reason::NOT_TEXT)
        );
    }

    /// Interior NULs are dropped rather than stored in the index.
    #[test]
    fn interior_nuls_are_removed() {
        let Extracted::Text(text) = extract(plan("notes.txt"), b"a\x00b".to_vec()) else {
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
        let Extracted::Text(text) = extract(plan("big.txt"), data) else {
            panic!("text expected");
        };
        assert!(text.len() <= MAX_INDEXED_CONTENT_BYTES);
        assert!(text.is_char_boundary(text.len()));
    }

    /// A parser that finds nothing is not an indexed document with empty
    /// content — that is what keeps a scanned PDF out of the index.
    #[test]
    fn an_empty_document_result_is_unsupported() {
        assert_eq!(
            finish(String::new()),
            Extracted::Unsupported("no extractable text")
        );
        assert_eq!(
            finish("  \n\t ".to_string()),
            Extracted::Unsupported("no extractable text")
        );
    }

    /// Document parsers separate words with U+00A0 and pages with U+000C.
    #[test]
    fn document_separators_are_normalised() {
        assert_eq!(
            finish("a\u{a0}b\u{c}c".to_string()),
            Extracted::Text("a b\nc".to_string())
        );
    }

    #[test]
    fn document_text_is_capped_on_a_char_boundary() {
        let mut long = "中".repeat(MAX_INDEXED_CONTENT_BYTES);
        long.push_str("tail");
        let Extracted::Text(text) = finish(long) else {
            panic!("text expected");
        };
        assert!(text.len() <= MAX_INDEXED_CONTENT_BYTES);
        assert!(text.is_char_boundary(text.len()));
    }

    /// The panic boundary turns a panicking parser into a skipped document.
    #[test]
    fn a_panicking_parser_is_contained() {
        let result = guard(
            "test",
            |_| Extracted::Unsupported(reason::PANIC),
            || panic!("boom"),
        );
        assert_eq!(result, Extracted::Unsupported(reason::PANIC));
    }

    #[test]
    fn a_parser_that_returns_normally_is_untouched() {
        assert_eq!(
            guard(
                "test",
                |_| Extracted::Unsupported(reason::PANIC),
                || Extracted::Text("ok".to_string())
            ),
            Extracted::Text("ok".to_string())
        );
    }

    /// The fallback sees the panic's message, which is what lets the caller
    /// log something more useful than "it failed".
    #[test]
    fn the_fallback_receives_the_panic_message() {
        let seen = std::cell::RefCell::new(String::new());
        let result = guard(
            "test",
            |detail| {
                seen.replace(detail.to_string());
                Extracted::Unsupported(reason::PANIC)
            },
            || panic!("the reason"),
        );
        assert_eq!(result, Extracted::Unsupported(reason::PANIC));
        assert_eq!(seen.into_inner(), "the reason");
    }

    /// Every plan survives the worker protocol unchanged, or a document would
    /// be parsed as something it is not.
    #[test]
    fn a_plan_round_trips_through_its_protocol_tag() {
        let plans = [
            Plan::Text,
            Plan::Sniff,
            Plan::Document(Document::Pdf),
            Plan::Document(Document::Office(office_oxide::DocumentFormat::Docx)),
            Plan::Document(Document::Office(office_oxide::DocumentFormat::Xlsx)),
            Plan::Document(Document::Office(office_oxide::DocumentFormat::Pptx)),
            Plan::Document(Document::Office(office_oxide::DocumentFormat::Doc)),
            Plan::Document(Document::Office(office_oxide::DocumentFormat::Xls)),
            Plan::Document(Document::Office(office_oxide::DocumentFormat::Ppt)),
        ];
        let mut tags: Vec<u8> = plans.iter().map(|plan| plan.tag()).collect();
        assert_eq!(tags.len(), 9);
        for plan in plans {
            assert_eq!(Plan::from_tag(plan.tag()), Some(plan), "{plan:?}");
        }
        // The tags are unique, so a wire byte names exactly one plan.
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), 9);
    }

    /// A reason that crossed the process boundary has to come back as one of
    /// the constants; anything else is a failed extraction.
    #[test]
    fn a_reason_is_interned_back_to_a_constant() {
        for known in reason::ALL {
            assert_eq!(reason::intern(known), *known);
        }
        assert_eq!(reason::intern("something else"), reason::FAILED);
        assert_eq!(reason::intern(""), reason::FAILED);
    }

    #[test]
    fn a_leading_dot_filename_has_no_text_extension() {
        assert_eq!(extension(".bashrc"), "bashrc");
        assert!(!is_text_extension(&extension(".bashrc")));
    }
}
