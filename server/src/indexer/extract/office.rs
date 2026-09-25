//! Office document text extraction (DOCX, XLSX, PPTX and the legacy DOC, XLS,
//! PPT).
//!
//! The parser takes a `Read + Seek` source rather than a path, so the file
//! never has to touch the filesystem on its way from the block store into the
//! index.

use office_oxide::{Document, DocumentFormat};

use super::{Extracted, finish, guard};

/// Extract the text of an Office document, or why it cannot be read.
///
/// `format` comes from the extraction plan, which decided it from the filename;
/// the parser validates the package against it and fails loudly on a mismatch.
pub(super) fn extract_text(data: Vec<u8>, format: DocumentFormat) -> Extracted {
    guard("office", move || {
        let document = match Document::from_reader(std::io::Cursor::new(data), format) {
            Ok(document) => document,
            Err(e) => {
                tracing::debug!("office: cannot parse {format:?}: {e}");
                return Extracted::Unsupported("could not parse document");
            }
        };
        finish(document.plain_text())
    })
}
