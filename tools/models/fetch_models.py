#!/usr/bin/env python3
"""Fetch and verify the embedded inference models.

The runtime never downloads anything: this script is the one offline step that
puts the upstream files in `server/models/`. It uses only the Python standard
library and pins every source to a revision and a SHA-256.

Usage:
  python3 tools/models/fetch_models.py [--output server/models]

The upstream models are:
  * YuNet (OpenCV Zoo, MIT)
  * PP-OCRv6 tiny detection and recognition (PaddlePaddle, Apache-2.0)
  * PP-LCNet_x1_0 document orientation classification (PaddlePaddle,
    Apache-2.0)
  * the recognition character dictionary, extracted from the recognition
    model's inference.yml

Licences and the model card live beside the files they describe.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys
import urllib.request

YUNET = {
    "name": "yunet",
    "file": "yunet.onnx",
    "url": (
        "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/"
        "models/face_detection_yunet/face_detection_yunet_2023mar.onnx"
    ),
    "bytes": 232589,
    "sha256": "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
}

DET = {
    "name": "ppocr_v6_tiny_det",
    "file": "ppocr_v6_tiny_det.onnx",
    "url": (
        "https://huggingface.co/PaddlePaddle/PP-OCRv6_tiny_det_onnx/resolve/"
        "2ba1506c0380b8f0b03dd142459aac66d4421f6c/inference.onnx"
    ),
    "bytes": 1780590,
    "sha256": "193bab7a04fca699a6c82e6abb5b81bdb28177f0abd4062552b04908dafb19f8",
}

REC = {
    "name": "ppocr_v6_tiny_rec",
    "file": "ppocr_v6_tiny_rec.onnx",
    "url": (
        "https://huggingface.co/PaddlePaddle/PP-OCRv6_tiny_rec_onnx/resolve/"
        "2612ab37152ae0a677521bae4e1e3d4fb4cf7c30/inference.onnx"
    ),
    "bytes": 4462639,
    "sha256": "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6",
}

DOC_ORI = {
    "name": "pp_lcnet_x1_0_doc_ori",
    "file": "pp_lcnet_x1_0_doc_ori.onnx",
    "url": (
        "https://huggingface.co/PaddlePaddle/PP-LCNet_x1_0_doc_ori_onnx/resolve/"
        "7330ab7039123e46af2dc03154b9969aa412c61d/inference.onnx"
    ),
    "bytes": 6788069,
    "sha256": "af9a0a4f317ff0709ce752067807f819cb15d883f8ecad89f28df1c6ee2d9c92",
}

KEYS = {
    "name": "ppocr_v6_tiny_keys",
    "file": "ppocr_keys_v6.txt",
    "url": (
        "https://huggingface.co/PaddlePaddle/PP-OCRv6_tiny_rec_onnx/resolve/"
        "2612ab37152ae0a677521bae4e1e3d4fb4cf7c30/inference.yml"
    ),
    "bytes": 27156,
    "sha256": "c5cbe34ef40c29c4df07ed012bf96569cb69a2d2a01a07027e9f13cb832bd9cd",
}


def _fetch(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=120) as response:
        return response.read()


def _verify(spec: dict, data: bytes) -> None:
    if len(data) != spec["bytes"]:
        raise SystemExit(
            f"{spec['name']}: {len(data)} bytes, expected {spec['bytes']} "
            "(the pinned upstream file changed)"
        )
    digest = hashlib.sha256(data).hexdigest()
    if digest != spec["sha256"]:
        raise SystemExit(
            f"{spec['name']}: sha256 {digest}, expected {spec['sha256']} "
            "(the pinned upstream file changed)"
        )


def _dictionary(inference_yml: bytes) -> bytes:
    """Extract `character_dict` from a Paddle inference.yml.

    The values are one per line under `character_dict:`. The Paddle export
    quotes some entries; the file this project stores is one raw UTF-8
    character per line, and the runtime appends the CTC blank and a space.
    """
    text = inference_yml.decode("utf-8")
    lines = text.splitlines()
    try:
        start = next(
            index for index, line in enumerate(lines) if line.strip() == "character_dict:"
        )
    except StopIteration as error:
        raise SystemExit("the recognition config has no character_dict") from error

    characters: list[str] = []
    for line in lines[start + 1 :]:
        if not line.startswith("  - "):
            break
        value = line[4:]
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
            quote = value[0]
            value = value[1:-1]
            if quote == "'":
                value = value.replace("''", "'")
        if len(value) != 1:
            raise SystemExit(f"dictionary entry is not one character: {value!r}")
        characters.append(value)
    if len(characters) != 6904:
        raise SystemExit(f"dictionary has {len(characters)} characters, expected 6904")
    return ("\n".join(characters) + "\n").encode("utf-8")


def _write(path: pathlib.Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    print(f"wrote {path} ({len(data)} bytes)")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        default=pathlib.Path(__file__).resolve().parents[2] / "server" / "models",
        help="destination directory (default: server/models)",
    )
    args = parser.parse_args()

    yunet = _fetch(YUNET["url"])
    _verify(YUNET, yunet)
    _write(args.output / YUNET["file"], yunet)

    det = _fetch(DET["url"])
    _verify(DET, det)
    _write(args.output / DET["file"], det)

    rec = _fetch(REC["url"])
    _verify(REC, rec)
    _write(args.output / REC["file"], rec)

    doc_ori = _fetch(DOC_ORI["url"])
    _verify(DOC_ORI, doc_ori)
    _write(args.output / DOC_ORI["file"], doc_ori)

    inference_yml = _fetch(KEYS["url"])
    keys = _dictionary(inference_yml)
    _verify(KEYS, keys)
    _write(args.output / KEYS["file"], keys)

    manifest = {"models": {}}
    for spec, url in [
        (YUNET, YUNET["url"]),
        (DET, DET["url"]),
        (REC, REC["url"]),
        (DOC_ORI, DOC_ORI["url"]),
        (KEYS, KEYS["url"]),
    ]:
        manifest["models"][spec["name"]] = {
            "file": spec["file"],
            "bytes": spec["bytes"],
            "sha256": spec["sha256"],
            "source": url,
        }
    print(json.dumps(manifest, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
