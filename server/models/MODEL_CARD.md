# YuNet face detector

`yunet.safetensors` is a converted copy of OpenCV Zoo's
`face_detection_yunet_2023mar.onnx`, used only to count faces in images.

## What it is

- **Task:** face detection (boxes and five landmarks); this project uses only
  the box count.
- **Architecture:** YuNet, a 53K-parameter CNN with a three-level FPN and
  cls/obj/bbox/kps heads.
- **Input:** fixed 640×640 BGR, 0–255, letterboxed with black padding.
- **Output:** 6,400 + 1,600 + 400 anchor predictions, decoded with the same
  rules as OpenCV's `FaceDetectorYN`, then NMS (IoU 0.3, top 100).
- **Parameters:** 53,104; **file size:** 217 KB.
- **Accuracy (upstream WIDER Face val):** AP 0.884 / 0.866 / 0.750
  (easy / medium / hard).

## What it is not

The scan stores a **count** and the best detection score. It does not create
face crops, embeddings, landmarks, identities or templates, and it cannot
recognise a person across photos. No biometric data is persisted.

## Source and licence

- Upstream: <https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet>
- Model file: `face_detection_yunet_2023mar.onnx`
- Upstream commit for that file: `f12e12798e8314f7c074a6656816c048dcc95b7a`
  (2023-06-06)
- Licence: **MIT**, copyright (c) 2020 Shiqi Yu. Full text in
  [`LICENSES/YuNet-LICENSE`](LICENSES/YuNet-LICENSE).
- Paper: <https://doi.org/10.1007/s11633-023-1423-y>

## Conversion

`tools/face_tags/convert_yunet.py` reads the ONNX initializers with the Python
standard library only and writes F32 safetensors under canonical Candle names.
`yunet.safetensors` SHA-256:
`c6379b3fef1d75acb4d7469527e97ce340779a15d9ae902233572132490d36ef`.

## Limitations

- Small faces (short edge under 32 px in the original image) are filtered out
  by default; large group photos are tiled, but a 20 MP photo of a crowd will
  still miss faces.
- Side profiles, heavy occlusion, extreme blur and very low light reduce recall.
- Cartoons, masks and statues are not guaranteed to be rejected.
- The model was trained on WIDER Face; demographic performance is not uniform.
  It is used only for a coarse "there is a face" tag, not for a decision about
  a person.
