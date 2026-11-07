# Embedded inference models

`server/models/` holds the models the server runs in confined child
processes. Every file is checked in, hash-pinned in `manifest.json` and embedded
into the binary with `include_bytes!`/`include_str!`; nothing is downloaded at
runtime and no inference framework other than the pure-Rust `tract` CPU runtime
is used. `tools/models/fetch_models.py` reproduces the files from their pinned
upstream revisions.

## YuNet face detector

`yunet.onnx` is OpenCV Zoo's `face_detection_yunet_2023mar.onnx`, used only to
count faces in images.

- **Task:** face detection (boxes and five landmarks); the automatic people tags
  use only the box count and best score.
- **Architecture:** YuNet, a 53K-parameter CNN with a three-level FPN and
  cls/obj/bbox/kps heads.
- **Input:** fixed 640×640 BGR, 0–255, letterboxed with black padding.
- **Output:** the ONNX graph returns 12 tensors: cls/obj (`[1, N, 1]`, already
  sigmoid), bbox (`[1, N, 4]`, raw centre offsets and log-size) and kps
  (`[1, N, 10]`) for strides 8, 16 and 32. The Rust decoder applies NMS (IoU 0.3,
  top 100).
- **Parameters:** 53,104; **file size:** 232,589 bytes.
- **Accuracy (upstream WIDER Face val):** AP 0.884 / 0.866 / 0.750
  (easy / medium / hard).

### What it is not

The scan stores a **count** and the best detection score. It does not create
face crops, embeddings, landmarks, identities or templates, and it cannot
recognise a person across photos. No biometric data is persisted.

### Source and licence

- Upstream: <https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet>
- File: `face_detection_yunet_2023mar.onnx`, upstream commit
  `f12e12798e8314f7c074a6656816c048dcc95b7a`.
- Licence: **MIT**, copyright (c) 2020 Shiqi Yu. Full text in
  [`LICENSES/YuNet-LICENSE`](LICENSES/YuNet-LICENSE).
- Paper: <https://doi.org/10.1007/s11633-023-1423-y>
- SHA-256: `8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4`

The file is the upstream ONNX graph unchanged; there is no conversion step.

## PP-OCRv6 tiny (image and PDF text for the full-text index)

`ppocr_v6_tiny_det.onnx` and `ppocr_v6_tiny_rec.onnx` read text from images and
from rendered PDF scan pages for the optional `[ocr]` full-text indexing feature.
`ppocr_keys_v6.txt` is the recognition alphabet extracted from the recognition
model's `inference.yml`.

- **Task:** text detection (DB) and text-line recognition (CTC), 49 languages in
  the tiny tier.
- **Architecture:** PPLCNetV4 + RepLKFPN detection; PPLCNetV4 + LightSVTR
  recognition with a CTC head.
- **Detection input:** dynamic `[1, 3, H, W]`, H/W multiples of 32, BGR with
  ImageNet mean/std. The engine resizes the long edge to 960 by default.
- **Detection output:** one probability map `[1, 1, H, W]`; the Rust
  post-processor thresholds it (`0.2`), labels 8-connected components, takes
  each component's minimum-area rectangle, scores it (`≥ 0.4`) and expands it by
  the DB unclip distance (`1.4`).
- **Recognition input:** `[1, 3, 48, W]`, W bucketed at 160/320/640/960/1280,
  BGR normalised by `(x/255 - 0.5) / 0.5` and padded on the right.
- **Recognition output:** `[1, T, 6906]`, already softmaxed. Index 0 is the CTC
  blank, 1–6904 are the dictionary characters and 6905 is a space; the decoder
  collapses repeats and drops low-confidence lines.
- **File sizes:** detection 1,780,590 bytes; recognition 4,462,639 bytes;
  dictionary 27,156 bytes (6,904 characters).
- **Accuracy (upstream, internal multi-scene set):** detection Hmean 80.6,
  recognition average 73.5.
- **Licence:** **Apache-2.0**. Full text in
  [`LICENSES/PaddleOCR-LICENSE`](LICENSES/PaddleOCR-LICENSE).

### Sources

| File | Repository | Revision | SHA-256 |
| --- | --- | --- | --- |
| `ppocr_v6_tiny_det.onnx` | `PaddlePaddle/PP-OCRv6_tiny_det_onnx` | `2ba1506c0380b8f0b03dd142459aac66d4421f6c` | `193bab7a04fca699a6c82e6abb5b81bdb28177f0abd4062552b04908dafb19f8` |
| `ppocr_v6_tiny_rec.onnx` | `PaddlePaddle/PP-OCRv6_tiny_rec_onnx` | `2612ab37152ae0a677521bae4e1e3d4fb4cf7c30` | `9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6` |
| `ppocr_keys_v6.txt` | the recognition model's `inference.yml` | same as above | `c5cbe34ef40c29c4df07ed012bf96569cb69a2d2a01a07027e9f13cb832bd9cd` |

## PP-LCNet_x1_0_doc_ori (page orientation)

`pp_lcnet_x1_0_doc_ori.onnx` classifies a whole picture as 0°, 90°, 180° or
270° before text detection, so a sideways or upside-down page is turned upright
first. It runs in the same OCR child as the PP-OCRv6 graphs.

- **Task:** four-class document image orientation.
- **Architecture:** PP-LCNet_x1_0 with a four-way classification head.
- **Input:** fixed `[1, 3, 224, 224]` BGR. The engine scales the shorter edge to
  256, centre-crops 224×224 and normalises with ImageNet mean/std. The crop is
  sampled directly from the source, so a very wide or tall picture allocates no
  intermediate resize buffer.
- **Output:** `[1, 4]`, already softmaxed. Labels 0/90/180/270 name the
  counter-clockwise turn that makes the page upright; label 0 leaves the
  picture unchanged. No score threshold is applied.
- **Accuracy (upstream, internal multi-scene set):** Top-1 99.06%.
- **File size:** 6,788,069 bytes.
- **Licence:** **Apache-2.0**. Full text in
  [`LICENSES/PaddleOCR-LICENSE`](LICENSES/PaddleOCR-LICENSE).
- Upstream: <https://huggingface.co/PaddlePaddle/PP-LCNet_x1_0_doc_ori_onnx>,
  revision `7330ab7039123e46af2dc03154b9969aa412c61d`.
- SHA-256: `af9a0a4f317ff0709ce752067807f819cb15d883f8ecad89f28df1c6ee2d9c92`

The file is the upstream ONNX graph unchanged; there is no conversion step.

## PP-LCNet_x0_25_textline_ori (text line orientation)

`pp_lcnet_x0_25_textline_ori.onnx` classifies each detected text line as upright
or upside down before recognition. It runs after detection and after the page
orientation classifier, so a line that is inverted inside an otherwise upright
page is still read.

- **Task:** binary text line orientation (`0_degree` / `180_degree`).
- **Architecture:** PP-LCNet_x0_25 with a two-way classification head.
- **Input:** fixed `[1, 3, 80, 160]` BGR. The engine resizes the whole line crop
  to 160×80 and normalises with ImageNet mean/std.
- **Output:** `[1, 2]`, already softmaxed. A `180_degree` result flips the crop
  before recognition; `0_degree` leaves it unchanged. No score threshold is
  applied.
- **Accuracy (upstream, internal multi-scene set):** Top-1 98.85%.
- **File size:** 1,019,454 bytes.
- **Licence:** **Apache-2.0**. Full text in
  [`LICENSES/PaddleOCR-LICENSE`](LICENSES/PaddleOCR-LICENSE).
- Upstream: <https://huggingface.co/PaddlePaddle/PP-LCNet_x0_25_textline_ori_onnx>,
  revision `aea1f18d97338aaf84b463f90192f2820524a00a`.
- SHA-256: `94a6a0a0425f2b5f08b5df72086f2d72abe40f1d22f6d12d2cd83674f11f2ff3`

The file is the upstream ONNX graph unchanged; there is no conversion step.

### What it stores

The recognised text is written into the Tantivy full-text index as the file's
content. It can be searched by anyone who can already read the repository. The
index directory is not encrypted; OCR text may contain names, addresses or
document numbers. Disabling `[ocr]` stops new OCR but leaves existing text in
the index until it is rebuilt. No image crops, boxes, confidence values,
orientation decisions or page angle are persisted.

### Limitations

- Pages turned by 90°, 180° or 270°, and individual text lines turned upside
  down inside an upright page, are corrected before recognition. Arbitrary
  angles other than quarter turns, curved pages, handwriting, very small print,
  low-contrast photos and artistic fonts reduce accuracy.
- Multi-column pages are read in a simple top-to-bottom order; search results are
  unaffected, but the highlight order may differ from the page.
- A picture or PDF page can produce at most 300 text boxes; the text is capped
  at the same 8 MiB limit as document extraction.
- A PDF's text layer is read by `lopdf`; only pages below 32 non-whitespace
  characters are rendered by `hayro` and passed to these models. PDF requests
  stop at the 150-second soft deadline (under the profile's 180-second hard
  timeout) or the 8 MiB text cap.
- HEIC/HEIF/AVIF are decoded through the optional `ffmpeg` helper first; without
  one those files are not OCR'd.
- Pages of a PDF that have a text layer are read by `lopdf`, not OCR'd again.
  Pages below the text threshold are rendered by `hayro` and OCR'd; videos are
  not OCR'd.
