# Model assets

The server embeds every inference graph at compile time. `fetch_models.py`
reproduces `server/models/` from the upstream projects; it is the only step that
needs the network.

```bash
python3 tools/models/fetch_models.py
```

The script:

- downloads OpenCV Zoo's YuNet `face_detection_yunet_2023mar.onnx`;
- downloads PaddlePaddle's PP-OCRv6 tiny detection and recognition ONNX files
  from pinned Hugging Face revisions;
- downloads PaddlePaddle's PP-LCNet_x1_0 document orientation classifier from a
  pinned Hugging Face revision;
- downloads PaddlePaddle's PP-LCNet_x0_25 text line orientation classifier from
  a pinned Hugging Face revision;
- extracts the 6,904-character recognition dictionary from the recognition
  model's `inference.yml`;
- verifies every SHA-256 and byte count before writing a file;
- prints the manifest entries it expects.

The files are the upstream ONNX graphs unchanged. The server runs them on its own
fixed-model runtime: `export_plan.py` (also stdlib-only, no `onnx` package needed)
reads each ONNX file, folds its static shape subgraph, assigns workspace slots and
rewrites `server/src/inference/generated/`. The generated plans are committed; the
shipped binary has no ONNX parser. After changing a model, run both scripts and
commit the regenerated plans:

```bash
python3 tools/models/fetch_models.py
python3 tools/models/export_plan.py            # or --check to only verify
```

`server/models/MODEL_CARD.md` records the sources, revisions, sizes, licences and
limitations; `manifest.json` is what the compliance test reads. `LICENSES/` holds
the MIT text for YuNet and the Apache-2.0 text for the PaddleOCR models.

If an upstream file changes, update the URL, revision, byte count and hash in
`fetch_models.py`, run it, and update `manifest.json` and the model card in the
same change.
