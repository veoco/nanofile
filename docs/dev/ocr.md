---
description: How Nanofile classifies PDF pages, renders scans and runs the OCR models.
---
# OCR internals

[Text in images and PDFs](/guide/use/ocr) covers setup, privacy and the operator-facing
limits. This page records the implementation details behind that behaviour.

## Page classification

There is no reliable document-level "scanned PDF" flag, so Nanofile decides per page:

- `lopdf` extracts the page's text layer. A page with at least 32 non-whitespace
  characters is a text page and keeps that text.
- A page below that threshold is scan-like. The confined OCR worker renders it with
  the pure-Rust `hayro` rasterizer and reads it with PP-OCRv6.
- The two kinds are merged back in page order. Text pages keep their text layer; scan
  pages use the OCR text, falling back to the short text layer when OCR finds nothing.

A PDF whose text layer was produced by an earlier OCR pass is trusted as text when
each page reaches the threshold, and is not OCR'd again. A text-heavy page with an
illustration also counts as a text page; words inside the illustration are not read.
That is a deliberate trade-off: there is no producer check, image-count heuristic or
separate flag.

Pages that contain only vector lettering and no text layer are not guaranteed: the
renderer is built without embedded fonts, so such a page may render without readable
glyphs. The target is raster scan pages.

## Rendering and limits

- `hayro` is asked only for pages that need pixels; the PDF text layer stays with
  `lopdf`.
- A PDF is read whole, at most 32 MiB. Its text layer is bounded by the existing
  2000-page extraction cap.
- OCR has no separate page cap: the worker renders candidate pages until the
  150-second soft deadline (below the profile's 180-second hard timeout) or the 8 MiB
  text cap. A long scan is indexed partially; the remaining pages wait for a re-index,
  not a later backfill pass.
- PNG, JPEG, WebP, BMP, GIF, TIFF and ICO are decoded in the OCR worker. HEIC, HEIF and
  AVIF are decoded to a still frame through the configured `storage.ffmpeg_path`
  first.
- One image may be at most 32 MiB, and at most 300 text lines are recognised per image
  or PDF page. The text is capped at the same 8 MiB as document extraction.
- A four-class model turns a page that was scanned or photographed a quarter turn off
  upright (90°, 180° or 270°). A two-class model flips an individual upside-down text
  line. Other angles, handwriting, very small print, low-contrast photos and decorative
  fonts reduce accuracy.
- Multi-column pages are read in a simple top-to-bottom order. Search is not affected,
  but the order in a highlight may differ from the page.
- Videos, images embedded in Office documents and archived files are not OCR'd.

## Models

PP-OCRv6 tiny (detection and recognition) and the PP-LCNet page and text-line
orientation classifiers run through the pure-Rust `tract` runtime in the confined OCR
worker. The models are compiled into the binary and never downloaded at runtime. They
are Apache-2.0; sources, revisions and limits are in `server/models/MODEL_CARD.md`.

PDF scan pages are rasterized by `hayro`, a pure-Rust renderer (Apache-2.0 OR MIT); the
PDF text layer is still read by `lopdf`.

## Stored data

Only the recognised **text** is stored, as the file's content in the full-text index.
No image crop, page bitmap, text box, confidence value, orientation decision or model
state is stored per file.

The index directory is **not encrypted**, and the text can contain names, addresses or
document numbers. It is searchable by anyone who can already read the library; library
permissions are unchanged. Disabling `[ocr]` stops new OCR but leaves the text already
in the index until it is rebuilt (re-index the library, or use `POST /api2/reindex/`).

## Related

- [Text in images and PDFs](/guide/use/ocr) — setup, privacy and operator limits.
- [Sandbox internals](/dev/sandbox) — the confined worker the OCR model runs in.
- [Architecture](/dev/architecture) — where the OCR service sits.
