# Text in images and PDFs (OCR)

Nanofile can read the text in images and in the scanned pages of a PDF and add
it to the full-text index. A screenshot, a photographed sign or a scanned page
becomes findable by the words it contains, not only by its file name. The
feature is off by default.

## Turning it on

OCR feeds the full-text index, so **the search index must be on first**
(`[index] enabled`). Then:

```toml
[ocr]
enabled = true
min_score = 0.55
max_side = 960
```

The same keys are on **System Settings → Maintenance → Text in images and PDFs
(OCR)**, or in `NANOFILE_OCR_ENABLED`, `NANOFILE_OCR_MIN_SCORE` and
`NANOFILE_OCR_MAX_SIDE`. They apply without a restart. The config file also
accepts `[ocr]` as its startup value, but the settings page is the reference.

- `min_score` (0–1) is the recognition confidence a text line must reach to be
  indexed. Raise it to drop noisy lines; lower it to keep faint print.
- `max_side` is the longest edge the detector resizes a page to, rounded up to
  a multiple of 32 (320–1600). Larger values read smaller print and cost more
  CPU.

Before detection, a small four-class model turns a page that was scanned or
photographed a quarter turn off upright (90°, 180° or 270°). After detection, a
two-class model flips any individual line that is still upside down — for
example a rotated margin note or stamp. Both are part of OCR and have no
separate switch.

New uploads and syncs are read in the background. Existing images, and scanned
PDFs the index skipped earlier, are picked up by the low-load backfill pass.
Enabling OCR later does not need a full index rebuild. A mixed PDF that was
already indexed through its text layer is not re-read automatically; re-index
that repository to add its scan pages. The backfill examines each file once, so
one with no text is not re-read on every pass.

## How a PDF is handled

There is no reliable document-level "scanned PDF" flag. Nanofile decides **per
page**:

- `lopdf` extracts the page's text layer. A page with at least 32
  non-whitespace characters is treated as a text page and keeps that text.
- A page below that threshold is treated as scan-like. The confined OCR worker
  renders that page with the pure-Rust `hayro` rasterizer and reads it with
  PP-OCRv6.
- Both kinds are merged back in page order. Text pages keep their text layer;
  scan pages use the OCR text, falling back to the short text layer when OCR
  finds nothing.

A PDF whose text layer was produced by an earlier OCR pass is therefore trusted
as text — provided each page reaches the threshold — and is not OCR'd again. A text-heavy page with an illustration is also kept
as text: words inside the illustration are not read. This is a deliberate
trade-off; there is no separate flag, producer check or image-count heuristic.

Pages that contain only vector lettering and no text layer are not guaranteed:
the renderer is built without embedded fonts, so such a page may render without
readable glyphs. The target is raster scan pages.

## What is stored

Only the recognised **text**, as the file's content in the full-text index. No
image crop, page bitmap, text box, confidence value, orientation decision or
model state is stored per file.

The index directory is **not encrypted**, and the text can contain names,
addresses or document numbers. It is searchable by anyone who can already read
the repository; repository permissions are unchanged. Disabling `[ocr]` stops
new OCR but leaves the text already in the index until it is rebuilt (re-index
the repository, or use `POST /api2/reindex/`).

## Formats and limits

- PNG, JPEG, WebP, BMP, GIF, TIFF and ICO are decoded in the OCR worker.
- HEIC, HEIF and AVIF are decoded to a still frame through the configured
  `storage.ffmpeg_path` first; without `ffmpeg` those files are not OCR'd.
- One image may be at most 32 MiB, and at most 300 text lines are recognised
  per image or PDF page. The text is capped at the same 8 MiB as document
  extraction.
- A PDF is read whole, at most 32 MiB. Its text layer is limited by the existing
  2000-page extraction cap. There is no separate OCR page cap: the worker
  renders candidate pages until the 150-second soft deadline (below the
  profile's 180-second hard timeout) or the 8 MiB text cap. A long scan is
  therefore indexed partially; the remaining pages wait for a re-index, not a
  later backfill pass.
- Pages turned by 90°, 180° or 270°, and individual upside-down text lines, are
  corrected before recognition. Other angles, handwriting, very small print,
  low-contrast photos and decorative fonts reduce accuracy.
- Multi-column pages are read in a simple top-to-bottom order. Search is not
  affected, but the order in a highlight may differ from the page.
- Videos, images embedded in Office documents and archived files are **not**
  OCR'd.

## Search

Search works exactly as before: the OCR text is the document's content, so a
content search matches it and the result carries a highlighted snippet. The
"full-text index" section of a file's details shows the same text. Tags, file
names and permissions are unchanged.

## Models and licence

PP-OCRv6 tiny (detection and recognition) and the PP-LCNet page and text-line
orientation classifiers run through the pure-Rust `tract` runtime in the
confined OCR worker. The models are compiled into the binary and never
downloaded at runtime. They are Apache-2.0; see `server/models/MODEL_CARD.md`
in the repository for sources, revisions and limits.

PDF scan pages are rasterized by `hayro`, a pure-Rust renderer (Apache-2.0 OR
MIT); the PDF text layer is still read by `lopdf`. `hayro` is asked only for
pages that need pixels, never for text extraction.

## Not the Seafile OCR endpoints

Seafile's per-repository OCR metadata endpoints are not implemented. This page
describes automatic OCR for Nanofile's own full-text index; it does not turn on
the Seafile OCR feature or create the Seafile metadata records.

## Troubleshooting

- **Images or PDFs are not searchable.** Check that both `index.enabled` and
  `ocr.enabled` are on, and that the sandbox is on. See
  [Sandbox](/guide/admin/sandbox); the OCR band shows whether this host can load
  the models.
- **A scanned PDF is not read.** A page is only treated as a scan when it has
  fewer than 32 non-whitespace characters; a text layer, even a poor one, is
  trusted. A scan page with no text layer should be read; check that the file is
  under 32 MiB and that the sandbox is on.
- **A mixed PDF still lacks its scan pages.** It was indexed before OCR was
  enabled; re-index the repository. Enabling OCR later only re-examines files
  the index had skipped.
- **HEIC/AVIF images are not read.** Configure `storage.ffmpeg_path`; the
  backfill retries those files once the helper is available.
- **Everything is slow.** Lower `max_side`, raise `min_score`, or turn OCR off.
  A PDF scan stops at the 150-second soft deadline; the OCR worker is separate
  from document parsing and thumbnails, so it does not hold their permits while
  it runs.
- **A file still shows old text after disabling OCR.** The text is in the index
  until the repository is re-indexed.
