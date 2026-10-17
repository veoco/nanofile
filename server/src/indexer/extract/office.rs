//! Office document text extraction (DOCX, XLSX, PPTX and legacy DOC, XLS, PPT).
//!
//! The parser takes a `Read + Seek` source, so a document never touches the
//! filesystem on its way from the block store to the index.
//!
//! `office_oxide` bounds each OPC part at 512 MiB but charges its rendered-text
//! budget on spreadsheets only, so a package of many parts can expand far past
//! the file. [`within_budget`] decodes every part into a fixed 64 KiB scratch
//! buffer and refuses the package past [`MAX_OFFICE_UNCOMPRESSED_BYTES`], which
//! also bounds what the parser can decode: a declared size is attacker-controlled
//! and the Deflate reader runs to the end of the stream. Counting decoded bytes
//! is the only sound bound; past it a package is skipped, not failed.

use std::io::{Cursor, Read};

use office_oxide::{Document, DocumentFormat};
use zip::ZipArchive;

use super::{Extracted, MAX_STRUCTURED_BYTES, finish, guard, reason};

/// Most bytes a whole Office package may decompress to: eight times the size a
/// document may have on disk ([`MAX_STRUCTURED_BYTES`]), which leaves room for
/// real Office XML and still bounds a bomb.
const MAX_OFFICE_UNCOMPRESSED_BYTES: u64 = 8 * MAX_STRUCTURED_BYTES as u64;

/// Most parts (ZIP entries) a package may hold; every entry costs the archive
/// reader a name and metadata block, so this bounds that fixed cost too.
const MAX_OFFICE_ENTRIES: usize = 10_000;

/// Scratch buffer for counting a part's decoded bytes; fixed, so the pre-check's
/// own memory does not grow with the package.
const BUDGET_SCRATCH_BYTES: usize = 64 * 1024;

/// Extract the text of an Office document, or why it cannot be read.
///
/// `format` comes from the extraction plan; the parser validates the package
/// against it and fails loudly on a mismatch.
pub(super) fn extract_text(data: Vec<u8>, format: DocumentFormat) -> Extracted {
    guard(
        "office",
        |_| Extracted::Unsupported(reason::PANIC),
        move || {
            // The OOXML formats are ZIP packages; the legacy ones are compound
            // files, whose streams are uncompressed and bounded by the file.
            if is_package(format)
                && let Err(reason) =
                    within_budget(&data, MAX_OFFICE_UNCOMPRESSED_BYTES, MAX_OFFICE_ENTRIES)
            {
                tracing::debug!("office: not indexable: {reason}");
                return Extracted::Unsupported(reason);
            }

            let document = match Document::from_reader(std::io::Cursor::new(data), format) {
                Ok(document) => document,
                Err(e) => {
                    tracing::debug!("office: cannot parse {format:?}: {e}");
                    return Extracted::Unsupported(reason::PARSE);
                }
            };
            finish(document.plain_text())
        },
    )
}

/// Whether a format is a ZIP-based OOXML package.
fn is_package(format: DocumentFormat) -> bool {
    matches!(
        format,
        DocumentFormat::Docx | DocumentFormat::Xlsx | DocumentFormat::Pptx
    )
}

/// Refuse a package whose parts decompress past `budget`, or that holds more
/// than `max_entries` parts.
///
/// A part that fails to decode ends its own count rather than failing the
/// package: the parser reads through the same decoder, so it cannot reach bytes
/// this loop could not, and one corrupt checksum does not refuse the file.
fn within_budget(data: &[u8], budget: u64, max_entries: usize) -> Result<(), &'static str> {
    let mut archive = ZipArchive::new(Cursor::new(data)).map_err(|_| reason::PARSE)?;
    if archive.len() > max_entries {
        return Err(reason::PARTS);
    }

    let mut total: u64 = 0;
    let mut scratch = vec![0u8; BUDGET_SCRATCH_BYTES];
    for index in 0..archive.len() {
        let Ok(mut part) = archive.by_index(index) else {
            continue;
        };
        // Free to check, and it stops an honest bomb before it is decoded.
        if part.size() > budget {
            return Err(reason::BUDGET);
        }
        loop {
            match part.read(&mut scratch) {
                Ok(0) => break,
                Ok(read) => {
                    total += read as u64;
                    if total > budget {
                        return Err(reason::BUDGET);
                    }
                }
                Err(_) => break,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The offset of the uncompressed-size field inside a central directory
    /// header (`PK\x01\x02`), which the archive reader trusts.
    const CENTRAL_UNCOMPRESSED_SIZE: usize = 24;

    #[test]
    fn a_small_package_fits_the_budget() {
        let package = package(&[("word/document.xml", 1024)]);
        assert_eq!(within_budget(&package, 64 * 1024, 10), Ok(()));
    }

    /// The declared size is checked without decoding, so an absurd part is refused cheaply.
    #[test]
    fn an_honest_oversized_part_is_refused() {
        let package = package(&[("word/document.xml", 64 * 1024)]);
        assert_eq!(
            within_budget(&package, 4 * 1024, 10),
            Err(reason::BUDGET),
            "the declared size must be enough to refuse it"
        );
    }

    /// The declaration can lie, but the reader refuses to deliver the part: the
    /// bytes never reach the count — and office_oxide reads the package through
    /// the same reader, so it cannot reach those bytes either.
    #[test]
    fn a_lying_declaration_delivers_nothing() {
        let mut package = package(&[("word/document.xml", 64 * 1024)]);
        patch_declared_size(&mut package, 16);

        assert_eq!(
            within_budget(&package, 4 * 1024, 10),
            Ok(()),
            "a part that delivers nothing must contribute nothing to the count"
        );

        let mut archive =
            ZipArchive::new(Cursor::new(package.as_slice())).expect("the package is a valid zip");
        let mut part = archive.by_index(0).expect("the entry is indexed");
        let mut scratch = vec![0u8; 4096];
        let mut decoded = 0usize;
        loop {
            match part.read(&mut scratch) {
                Ok(0) | Err(_) => break,
                Ok(read) => decoded += read,
            }
        }
        assert_eq!(
            decoded, 0,
            "a part whose declared size lies must not deliver its 64 KiB"
        );
    }

    /// Parts that each fit still have to be counted as they decode.
    #[test]
    fn the_decoded_total_refuses_a_sum_of_small_parts() {
        let package = package(&[
            ("word/one.xml", 2048),
            ("word/two.xml", 2048),
            ("word/three.xml", 2048),
        ]);

        assert_eq!(
            within_budget(&package, 4 * 1024, 10),
            Err(reason::BUDGET),
            "each part fits, the decoded total does not"
        );
    }

    /// A declaration far above the production budget is refused without
    /// building anything near that size.
    #[test]
    fn a_declared_bomb_is_refused_against_the_production_budget() {
        let mut package = package(&[("word/document.xml", 1024)]);
        patch_declared_size(&mut package, (MAX_OFFICE_UNCOMPRESSED_BYTES + 1) as u32);

        assert_eq!(
            within_budget(&package, MAX_OFFICE_UNCOMPRESSED_BYTES, MAX_OFFICE_ENTRIES),
            Err(reason::BUDGET)
        );
    }

    #[test]
    fn too_many_parts_is_refused() {
        let names: Vec<String> = (0..12)
            .map(|index| format!("word/part{index}.xml"))
            .collect();
        let parts: Vec<(&str, usize)> = names.iter().map(|name| (name.as_str(), 16)).collect();
        let package = package(&parts);
        assert_eq!(within_budget(&package, 1024 * 1024, 10), Err(reason::PARTS));
    }

    #[test]
    fn something_that_is_not_a_zip_is_a_parse_failure() {
        assert_eq!(
            within_budget(b"not an office document", 1024, 10),
            Err(reason::PARSE)
        );
    }

    /// The budget has to stay above the largest document that may be read at all.
    #[test]
    fn the_budget_is_a_multiple_of_the_document_cap() {
        assert!(MAX_OFFICE_UNCOMPRESSED_BYTES >= MAX_STRUCTURED_BYTES as u64);
    }

    /// A ZIP holding `parts`, each `size` zero bytes of Deflated content.
    fn package(parts: &[(&str, usize)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let chunk = vec![0u8; 8 * 1024];
        for (name, size) in parts {
            writer.start_file(*name, options).expect("start part");
            let mut left = *size;
            while left > 0 {
                let take = left.min(chunk.len());
                writer.write_all(&chunk[..take]).expect("write part");
                left -= take;
            }
        }
        writer.finish().expect("finish package").into_inner()
    }

    /// Rewrite the declared uncompressed size of every central directory entry,
    /// leaving the parts themselves untouched — the shape a bomb takes when it
    /// lies about its size.
    fn patch_declared_size(package: &mut [u8], declared: u32) {
        let mut patched = 0;
        let mut index = 0;
        while let Some(found) = find_central_header(&package[index..]) {
            let at = index + found + CENTRAL_UNCOMPRESSED_SIZE;
            package[at..at + 4].copy_from_slice(&declared.to_le_bytes());
            patched += 1;
            index = at + 4;
        }
        assert!(patched > 0, "the fixture must have a central directory");
    }

    /// Offset of the next `PK\x01\x02` signature in `data`.
    fn find_central_header(data: &[u8]) -> Option<usize> {
        data.windows(4).position(|w| w == b"PK\x01\x02")
    }
}
