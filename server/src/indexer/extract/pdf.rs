//! PDF text extraction.
//!
//! Loaded from memory and walked page by page. `pdf-extract`'s own
//! whole-document helpers are not usable here: `extract_text_from_mem` has no
//! page bound, and `extract_text_from_mem_by_pages` materialises every page's
//! text before returning, so the content budget could not stop it. A document
//! with a very large page count would run to completion regardless — long
//! enough to outlast the index job's no-progress timeout and take the batch
//! down with it. Walking `Document::get_pages()` ourselves keeps the cap and
//! lets the loop stop as soon as it has text enough to index.

use pdf_extract::{Document, PlainTextOutput, output_doc_page};

use super::{Extracted, MAX_INDEXED_CONTENT_BYTES, finish, guard};

/// How many pages of one document are read.
///
/// A stop, not a failure: the text of the first pages is still worth indexing,
/// and the file is stored whole in the block store if a later release wants to
/// raise this.
const MAX_PAGES: usize = 2000;

/// Extract the text of a PDF, or why it cannot be read.
pub(super) fn extract_text(data: Vec<u8>) -> Extracted {
    guard("pdf", move || {
        let mut doc = match Document::load_mem(&data) {
            Ok(doc) => doc,
            Err(e) => {
                tracing::debug!("pdf: cannot parse: {e}");
                return Extracted::Unsupported("could not parse document");
            }
        };

        // A PDF encrypted with a user password is opened with an empty one,
        // which is the attempt `pdf-extract` makes internally. Doing it here is
        // what lets the caller record "encrypted" rather than lumping it in
        // with "unparseable"; without a password to offer the content is out of
        // reach either way.
        if doc.is_encrypted() && doc.decrypt("").is_err() {
            return Extracted::Unsupported("encrypted pdf");
        }

        // `get_pages` returns an owned map keyed by 1-based page number.
        let pages = doc.get_pages();
        if pages.len() > MAX_PAGES {
            tracing::debug!(
                "pdf: reading the first {MAX_PAGES} of {} pages",
                pages.len()
            );
        }

        let mut text = String::new();
        for page_num in pages.keys().copied().take(MAX_PAGES) {
            let mut page = String::new();
            {
                let mut output = PlainTextOutput::new(&mut page);
                if let Err(e) = output_doc_page(&doc, &mut output, page_num) {
                    // One unreadable page does not condemn the document; the
                    // remaining pages may still carry what was searched for.
                    tracing::debug!("pdf: page {page_num}: {e}");
                }
            }
            text.push_str(&page);
            text.push('\n');
            if text.len() >= MAX_INDEXED_CONTENT_BYTES {
                break;
            }
        }

        // Nothing at all means no text layer — a scan — which is recorded as
        // skipped rather than as an indexed document with empty content.
        finish(text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 618-byte truncated PDF: the cross-reference table and everything after
    /// it are gone.
    ///
    /// Checked in so the "an unreadable file still produces a verdict" rule is
    /// exercised against a real parser failure rather than a synthesised one.
    const TRUNCATED_PDF: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/corrupt.pdf"
    ));

    /// A 589-byte input that makes `pdf-extract 0.12.1` panic (a `deref` on a
    /// missing object, `lib.rs:286`).
    ///
    /// This is the reason [`guard`] exists: without it the panic would unwind
    /// out of the blocking task and fail the batch that carried the file. The
    /// assertion is deliberately loose — a later release returning an error
    /// instead would keep the file just as unindexable, and the test would
    /// still pass. What must never happen is an unwind reaching the caller,
    /// which is exactly what dropping `guard` would produce.
    const PANICKING_PDF: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/panics.pdf"
    ));

    #[test]
    fn an_unparseable_pdf_is_unsupported() {
        assert_eq!(
            extract_text(TRUNCATED_PDF.to_vec()),
            Extracted::Unsupported("could not parse document")
        );
    }

    #[test]
    fn a_panicking_input_is_contained() {
        assert!(
            matches!(
                extract_text(PANICKING_PDF.to_vec()),
                Extracted::Unsupported(_)
            ),
            "a panicking parser must still produce a verdict"
        );
    }

    /// A PDF with no text operators parses fine and yields nothing, which must
    /// read as "not indexable" rather than as an indexed empty document.
    #[test]
    fn a_pdf_without_text_is_unsupported() {
        assert_eq!(
            extract_text(minimal_pdf_without_text()),
            Extracted::Unsupported("no extractable text")
        );
    }

    #[test]
    fn a_short_pdf_is_unsupported_not_a_panic() {
        let truncated = &TRUNCATED_PDF[..40];
        assert!(matches!(
            extract_text(truncated.to_vec()),
            Extracted::Unsupported(_)
        ));
    }

    #[test]
    fn an_empty_input_is_unsupported_not_a_panic() {
        assert!(matches!(
            extract_text(Vec::new()),
            Extracted::Unsupported(_)
        ));
    }

    /// A one-page PDF whose content stream draws nothing.
    fn minimal_pdf_without_text() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << >> /Contents 4 0 R >>"
                .to_string(),
            "<< /Length 0 >>\nstream\n\nendstream".to_string(),
        ];

        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
        out.push_str("0000000000 65535 f \n");
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }
}
