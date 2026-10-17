//! PDF text extraction.
//!
//! Loaded from memory, then walked page by page through `lopdf`'s bounded API:
//! [`LoadOptions::max_decompressed_size`] caps every loader stream (a few hundred
//! crafted bytes would otherwise decode into gigabytes);
//! [`Document::extract_text_chunks_with_limit`] caps each page, and the reader's
//! depth limit refuses a deeply nested document instead of overflowing the stack
//! (an `abort`, not an unwind, so [`super::guard`] cannot contain it). The worker's
//! address-space limit backstops what the format leaves: one page, one string.
//! # Form XObjects
//! `lopdf` reads a page's own content streams only; [`Forms`] also renders each
//! `/Subtype /Form` XObject a page references.

use std::collections::HashSet;

use lopdf::{Document, LoadOptions, Object, ObjectId};

use super::{Extracted, MAX_INDEXED_CONTENT_BYTES, MAX_STRUCTURED_BYTES, finish, guard, reason};

/// How many pages of one document are read.
///
/// A stop, not a failure: the text of the first pages is still worth indexing,
/// and the file is stored whole in the block store.
const MAX_PAGES: usize = 2000;

/// Most bytes a single stream may decode to.
///
/// The loader's per-stream limit and the limit for each page and each form
/// rendered: a stream that decodes past the file's own cap is a bomb, not a
/// document.
const MAX_PDF_STREAM_BYTES: usize = MAX_STRUCTURED_BYTES;

/// How deep a chain of Form XObjects nested inside other forms is followed.
///
/// Cycles are refused by [`Forms::seen`] regardless; this bounds a merely long
/// chain.
const MAX_FORM_DEPTH: usize = 8;

/// Most text the form walk may produce for one document.
///
/// The [`MAX_INDEXED_CONTENT_BYTES`] check happens *between* pages, and one page
/// can reference any number of forms; this bounds the work one page can cause.
const MAX_FORM_WALK_BYTES: usize = 64 * 1024 * 1024;

/// Load options bounded for untrusted input.
fn load_options() -> LoadOptions {
    LoadOptions {
        max_decompressed_size: Some(MAX_PDF_STREAM_BYTES),
        ..LoadOptions::default()
    }
}

/// Extract the text of a PDF, or why it cannot be read.
pub(super) fn extract_text(data: Vec<u8>) -> Extracted {
    guard(
        "pdf",
        |_| Extracted::Unsupported(reason::PANIC),
        move || {
            let mut doc = match Document::load_mem_with_options(&data, load_options()) {
                Ok(doc) => doc,
                Err(e) => {
                    tracing::debug!("pdf: cannot parse: {e}");
                    return Extracted::Unsupported(reason::PARSE);
                }
            };

            // Open with an empty user password so the caller records "encrypted"
            // rather than "unparseable"; the content is unreachable either way.
            if doc.is_encrypted() && doc.decrypt("").is_err() {
                return Extracted::Unsupported(reason::ENCRYPTED);
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
                let Some(&page_id) = pages.get(&page_num) else {
                    continue;
                };
                append_page(&mut text, &mut doc, page_id, page_num);
                if text.len() >= MAX_INDEXED_CONTENT_BYTES {
                    break;
                }
            }

            // No text layer at all (a scan) means skipped, not indexed as empty.
            finish(text)
        },
    )
}

/// Append everything one page draws: its own content streams, then the Form
/// XObjects it references.
fn append_page(text: &mut String, doc: &mut Document, page_id: ObjectId, page_num: u32) {
    append_chunks(text, doc, page_num);

    let Some(resources) = page_resources(doc, page_id) else {
        return;
    };
    let forms = form_xobjects(doc, &resources);
    if forms.is_empty() {
        return;
    }

    let mut walk = Forms {
        doc,
        page_id,
        page_num,
        seen: HashSet::new(),
        budget: MAX_FORM_WALK_BYTES,
        text,
    };
    for form_id in forms {
        walk.render(form_id, &resources, 0);
        if walk.budget == 0 || walk.text.len() >= MAX_INDEXED_CONTENT_BYTES {
            break;
        }
    }
}

/// Append the text of the page's own content streams.
/// One page's failure is not the document's: an over-limit page is skipped and
/// the remaining pages may still carry what was searched for.
fn append_chunks(text: &mut String, doc: &Document, page_num: u32) {
    for chunk in doc.extract_text_chunks_with_limit(&[page_num], MAX_PDF_STREAM_BYTES) {
        match chunk {
            Ok(chunk) => text.push_str(&chunk),
            Err(e) => tracing::debug!("pdf: page {page_num}: {e}"),
        }
    }
}

/// The resources in effect for a page.
/// `/Resources` is inheritable (PDF 32000-1 §7.7.3.4), so a page without its own
/// falls back to the nearest ancestor's. Owned because the caller keeps using it
/// while the document is borrowed mutably to render forms.
fn page_resources(doc: &Document, page_id: ObjectId) -> Option<Object> {
    let (inline, ids) = doc.get_page_resources(page_id).ok()?;
    if let Some(dict) = inline {
        return Some(Object::Dictionary(dict.clone()));
    }
    for id in ids {
        if let Ok(dict) = doc.get_dictionary(id) {
            return Some(Object::Dictionary(dict.clone()));
        }
    }
    None
}

/// Every Form XObject reachable from a resource dictionary.
///
/// Only `/Subtype /Form` entries qualify: an `/Image` XObject carries no text.
fn form_xobjects(doc: &Document, resources: &Object) -> Vec<ObjectId> {
    let Some(dict) = dictionary(doc, resources) else {
        return Vec::new();
    };
    let Some(xobjects) = dict.get(b"XObject").ok().and_then(|o| dictionary(doc, o)) else {
        return Vec::new();
    };

    xobjects
        .iter()
        .filter_map(|(_, value)| {
            let id = value.as_reference().ok()?;
            let stream = doc.get_object(id).ok()?.as_stream().ok()?;
            let subtype = stream.dict.get(b"Subtype").ok()?.as_name().ok()?;
            (subtype == b"Form").then_some(id)
        })
        .collect()
}

/// Resolve an object that may be a dictionary or a reference to one.
fn dictionary<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a lopdf::Dictionary> {
    match object {
        Object::Dictionary(dict) => Some(dict),
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        _ => None,
    }
}

/// The `/Resources` a form declares, if any; without one the form is drawn in
/// the resources of whatever referenced it.
fn declared_resources(doc: &Document, form_id: ObjectId) -> Option<Object> {
    let stream = doc.get_object(form_id).ok()?.as_stream().ok()?;
    stream.dict.get(b"Resources").ok().cloned()
}

/// Walks the Form XObjects of one page.
struct Forms<'a> {
    doc: &'a mut Document,
    page_id: ObjectId,
    page_num: u32,
    /// Forms already rendered for this page, so a cycle terminates.
    seen: HashSet<ObjectId>,
    /// Remaining text budget for the walk.
    budget: usize,
    text: &'a mut String,
}

impl Forms<'_> {
    /// Render one form, then the forms it draws itself.
    /// `context` is the dictionary the form is drawn in: its own when declared,
    /// otherwise whatever referenced it.
    fn render(&mut self, form_id: ObjectId, context: &Object, depth: usize) {
        if depth > MAX_FORM_DEPTH || self.budget == 0 || !self.seen.insert(form_id) {
            return;
        }

        // Read what the form draws before redirecting: the walk needs the document
        // immutably, and the redirect holds it mutably.
        let declared = declared_resources(self.doc, form_id);
        let effective = declared.clone().unwrap_or_else(|| context.clone());
        let nested = form_xobjects(self.doc, &effective);

        let before = self.text.len();
        {
            let redirect = PageRedirect::apply(self.doc, self.page_id, form_id, declared);
            append_chunks(self.text, redirect.document(), self.page_num);
        }
        self.budget = self.budget.saturating_sub(self.text.len() - before);

        for nested_id in nested {
            self.render(nested_id, &effective, depth + 1);
            if self.budget == 0 || self.text.len() >= MAX_INDEXED_CONTENT_BYTES {
                return;
            }
        }
    }
}

/// Points a page at a Form XObject's stream, and puts it back on drop.
///
/// Restoring matters: `/Contents` and `/Resources` are read by the very next
/// extraction and by the nested renders inside this one.
struct PageRedirect<'a> {
    doc: &'a mut Document,
    page_id: ObjectId,
    contents: Option<Object>,
    resources: Option<Object>,
    /// Whether `/Resources` was replaced; a form without its own keeps the caller's.
    replaced_resources: bool,
}

impl<'a> PageRedirect<'a> {
    fn apply(
        doc: &'a mut Document,
        page_id: ObjectId,
        form_id: ObjectId,
        resources: Option<Object>,
    ) -> Self {
        let mut contents = None;
        let mut saved = None;
        let mut replaced_resources = false;

        if let Ok(page) = doc.get_object_mut(page_id).and_then(Object::as_dict_mut) {
            contents = page.get(b"Contents").ok().cloned();
            saved = page.get(b"Resources").ok().cloned();
            page.set("Contents", Object::Reference(form_id));
            if let Some(resources) = resources {
                page.set("Resources", resources);
                replaced_resources = true;
            }
        }

        Self {
            doc,
            page_id,
            contents,
            resources: saved,
            replaced_resources,
        }
    }

    /// The document, for the extraction that runs while the redirect is alive.
    fn document(&self) -> &Document {
        self.doc
    }
}

impl Drop for PageRedirect<'_> {
    fn drop(&mut self) {
        let Ok(page) = self
            .doc
            .get_object_mut(self.page_id)
            .and_then(Object::as_dict_mut)
        else {
            return;
        };
        match self.contents.take() {
            Some(contents) => page.set("Contents", contents),
            None => {
                page.remove(b"Contents");
            }
        }
        if self.replaced_resources {
            match self.resources.take() {
                Some(resources) => page.set("Resources", resources),
                None => {
                    page.remove(b"Resources");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 618-byte truncated PDF: the cross-reference table and everything after it
    /// are gone, so this is a real parser failure rather than a synthesised one.
    const TRUNCATED_PDF: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/corrupt.pdf"
    ));

    /// A 589-byte PDF whose font dictionary misspells `/Subtype`; `lopdf` still
    /// reads the page text, which is what this pins.
    const MALFORMED_FONT_PDF: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/malformed_font.pdf"
    ));

    #[test]
    fn a_malformed_font_still_yields_text() {
        let extracted = extract_text(MALFORMED_FONT_PDF.to_vec());
        let Extracted::Text(text) = &extracted else {
            panic!("text expected, got {}", describe(&extracted));
        };
        assert!(text.contains("Hello"), "extracted {text:?}");
    }

    #[test]
    fn a_truncated_pdf_is_unsupported() {
        assert!(
            matches!(
                extract_text(TRUNCATED_PDF.to_vec()),
                Extracted::Unsupported(_)
            ),
            "a truncated container must be skipped, not an error"
        );
    }

    /// Input that is not a PDF at all is refused by the loader.
    #[test]
    fn a_file_that_is_not_a_pdf_is_unsupported() {
        assert_eq!(
            extract_text(b"this file is not a PDF, whatever its name says".to_vec()),
            Extracted::Unsupported(reason::PARSE)
        );
    }

    /// A PDF with no text operators parses and yields nothing: unsupported, not
    /// indexed-empty.
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

    #[test]
    fn text_is_extracted() {
        let Extracted::Text(text) = extract_text(pdf_with_text("zebraquartz")) else {
            panic!(
                "text expected: {}",
                describe(&extract_text(pdf_with_text("zebraquartz")))
            );
        };
        assert!(text.contains("zebraquartz"), "extracted {text:?}");
    }

    /// Text drawn by a Form XObject is reached, which page-only extraction misses.
    #[test]
    fn text_inside_a_form_is_extracted() {
        let pdf = pdf_with_form("wombatpuzzle");
        let Extracted::Text(text) = extract_text(pdf.clone()) else {
            panic!("text expected: {}", describe(&extract_text(pdf)));
        };
        assert!(text.contains("wombatpuzzle"), "extracted {text:?}");
    }

    /// A self-referential form is visited once, so the walk terminates.
    #[test]
    fn a_self_referential_form_terminates() {
        let Extracted::Text(text) = extract_text(self_referential_form_pdf()) else {
            panic!("text expected");
        };
        assert_eq!(text.matches("narwhal").count(), 1, "extracted {text:?}");
    }

    /// The per-stream cap is per page: the pages after an oversized one are read.
    #[test]
    fn an_oversized_stream_costs_only_its_own_page() {
        let Extracted::Text(text) = extract_text(flate_bomb_pdf("xylophone")) else {
            panic!("the second page must still be extracted");
        };
        assert!(text.contains("xylophone"), "extracted {} bytes", text.len());
    }

    /// A deeply nested content stream is refused by the reader's nesting bound
    /// rather than overflowing the stack.
    #[test]
    fn deeply_nested_content_is_refused() {
        let Extracted::Unsupported(reason) = extract_text(deeply_nested_pdf()) else {
            panic!("a nested-only document has no text");
        };
        assert_eq!(reason, "no extractable text");
    }

    fn describe(extracted: &Extracted) -> String {
        match extracted {
            Extracted::Text(text) => format!("Text({} bytes)", text.len()),
            Extracted::Unsupported(reason) => format!("Unsupported({reason})"),
        }
    }

    /// A one-page PDF whose content stream draws nothing.
    fn minimal_pdf_without_text() -> Vec<u8> {
        assemble(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Contents 4 0 R >>"
                .to_vec(),
            stream_object("", b""),
        ])
    }

    /// A one-page PDF whose content stream draws `text` in Helvetica.
    fn pdf_with_text(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
        assemble(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_vec(),
            stream_object("", content.as_bytes()),
            helvetica(),
        ])
    }

    /// A one-page PDF whose resource dictionary holds a single Form drawing `text`.
    fn pdf_with_form(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
        assemble(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Fm0 5 0 R >> >> /Contents 4 0 R >>"
                .to_vec(),
            stream_object("", b""),
            stream_object(
                " /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> >>",
                content.as_bytes(),
            ),
            helvetica(),
        ])
    }

    /// A form that lists itself in its own resource dictionary.
    fn self_referential_form_pdf() -> Vec<u8> {
        assemble(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Fm0 5 0 R >> >> /Contents 4 0 R >>"
                .to_vec(),
            stream_object("", b""),
            stream_object(
                " /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> /XObject << /Fm0 5 0 R >> >>",
                b"BT /F1 24 Tf 72 700 Td (narwhal) Tj ET",
            ),
            helvetica(),
        ])
    }

    /// Two pages: the first draws a Flate stream that expands far past
    /// [`MAX_PDF_STREAM_BYTES`], the second draws `text`.
    fn flate_bomb_pdf(text: &str) -> Vec<u8> {
        use std::io::Write;

        // Past the per-stream cap, so the cap stops it, not the encoded size.
        let inflated = vec![b' '; MAX_PDF_STREAM_BYTES + 1024 * 1024];
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&inflated).expect("compress");
        let compressed = encoder.finish().expect("finish");

        let content = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
        assemble(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Contents 4 0 R >>"
                .to_vec(),
            stream_object(" /Filter /FlateDecode", &compressed),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>"
                .to_vec(),
            stream_object("", content.as_bytes()),
            helvetica(),
        ])
    }

    /// One page whose content stream nests brackets past the reader's limit.
    fn deeply_nested_pdf() -> Vec<u8> {
        let mut content = vec![b'['; 200_000];
        content.extend(std::iter::repeat_n(b']', 200_000));
        assemble(&[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Contents 4 0 R >>"
                .to_vec(),
            stream_object("", &content),
        ])
    }

    fn helvetica() -> Vec<u8> {
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()
    }

    /// A stream object whose `/Length` matches `body` exactly.
    fn stream_object(extra: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!("<< /Length {}{extra} >>\nstream\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendstream");
        out
    }

    /// Assemble numbered objects 1..=n into a PDF with a matching xref table.
    fn assemble(objects: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::from(&b"%PDF-1.4\n"[..]);
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}
