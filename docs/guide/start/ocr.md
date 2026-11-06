# Text in images (OCR)

Nanofile can read the text in images and add it to the full-text index. A
screenshot, a photographed sign or a scanned page becomes findable by the words
it contains, not only by its file name. The feature is off by default.

## Turning it on

OCR feeds the full-text index, so **the search index must be on first**
(`[index] enabled`). Then:

```toml
[ocr]
enabled = true
min_score = 0.55
max_side = 960
```

The same keys are on **System Settings → Maintenance → Text in images (OCR)**,
or in `NANOFILE_OCR_ENABLED`, `NANOFILE_OCR_MIN_SCORE` and
`NANOFILE_OCR_MAX_SIDE`. They apply without a restart. The config file also
accepts `[ocr]` as its startup value, but the settings page is the reference.

- `min_score` (0–1) is the recognition confidence a text line must reach to be
  indexed. Raise it to drop noisy lines; lower it to keep faint print.
- `max_side` is the longest edge the detector resizes a picture to, rounded up
  to a multiple of 32 (320–1600). Larger values read smaller print and cost more
  CPU.

Before detection, a small four-class model turns a page that was scanned or
photographed a quarter turn off upright (90°, 180° or 270°). This is part of OCR
and has no separate switch.

New uploads and syncs are read in the background. Existing images are picked up
by the low-load backfill pass; enabling OCR later does not need a full index
rebuild. The backfill examines images once, so a picture with no text is not
re-read on every pass.

## What is stored

Only the recognised **text**, as the file's content in the full-text index. No
image crop, text box, confidence value, page angle or model state is stored per
file.

The index directory is **not encrypted**, and the text can contain names,
addresses or document numbers. It is searchable by anyone who can already read
the repository; repository permissions are unchanged. Disabling `[ocr]` stops
new OCR but leaves the text already in the index until it is rebuilt (re-index
the repository, or use `POST /api2/reindex/`).

## Formats and limits

- PNG, JPEG, WebP, BMP, GIF, TIFF and ICO are decoded in the OCR worker.
- HEIC, HEIF and AVIF are decoded to a still frame through the configured
  `storage.ffmpeg_path` first; without `ffmpeg` those files are not OCR'd.
- One image may be at most 32 MiB, and at most 300 text lines are recognised.
  The text is capped at the same 8 MiB as document extraction.
- Pages turned by 90°, 180° or 270° are corrected before detection. Other
  angles, handwriting, very small print, low-contrast photos and decorative
  fonts reduce accuracy.
- Multi-column pages are read in a simple top-to-bottom order. Search is not
  affected, but the order in a highlight may differ from the page.
- Scanned PDFs and videos are **not** OCR'd. PDF text still comes from the PDF
  text layer.

## Search

Search works exactly as before: the OCR text is the document's content, so a
content search matches it and the result carries a highlighted snippet. The
"full-text index" section of a file's details shows the same text. Tags, file
names and permissions are unchanged.

## Models and licence

PP-OCRv6 tiny (detection and recognition) and the PP-LCNet_x1_0 page
orientation classifier run through the pure-Rust `tract` runtime in the confined
OCR worker. The models are compiled into the binary and never downloaded at
runtime. They are Apache-2.0; see `server/models/MODEL_CARD.md` in the
repository for sources, revisions and limits.

## Not the Seafile OCR endpoints

Seafile's per-repository OCR metadata endpoints are not implemented. This page
describes automatic OCR for Nanofile's own full-text index; it does not turn on
the Seafile OCR feature or create the Seafile metadata records.

## Troubleshooting

- **Images are not searchable.** Check that both `index.enabled` and
  `ocr.enabled` are on, and that the sandbox is on. See
  [Sandbox](/guide/admin/sandbox); the OCR band shows whether this host can load
  the models.
- **HEIC/AVIF images are not read.** Configure `storage.ffmpeg_path`; the backfill
  retries those files once the helper is available.
- **Everything is slow.** Lower `max_side`, raise `min_score`, or turn OCR off.
  The OCR worker is separate from document parsing and thumbnails, so it does not
  hold their permits while it runs.
- **A file still shows old text after disabling OCR.** The text is in the index
  until the repository is re-indexed.
