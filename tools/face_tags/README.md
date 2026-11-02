# YuNet conversion

`convert_yunet.py` turns OpenCV Zoo's
`face_detection_yunet_2023mar.onnx` into the F32 safetensors file embedded by
`server/models/yunet.safetensors`. It uses only the Python standard library:
it parses the ONNX protobuf enough to read initializers, maps the publisher's
numeric tensor names to the canonical Candle names used by
`server/src/face_tags/yunet.rs`, and writes the safetensors header plus raw
little-endian F32 payload.

```bash
python3 tools/face_tags/convert_yunet.py \
  --input /path/to/face_detection_yunet_2023mar.onnx \
  --output server/models/yunet.safetensors
sha256sum server/models/yunet.safetensors
```

Update `server/models/manifest.json` and `MODEL_CARD.md` if the upstream file
changes. The model directory's MIT licence is in
`server/models/LICENSES/YuNet-LICENSE`.
