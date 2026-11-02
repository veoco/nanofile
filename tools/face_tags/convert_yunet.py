#!/usr/bin/env python3
"""Convert the OpenCV Zoo YuNet 2023mar ONNX weights to a small safetensors file.

This deliberately uses only the Python standard library: the build must not
depend on protoc, numpy, onnx, or TensorFlow.  The ONNX graph is parsed just far
enough to read initializers; the Rust model in `server/src/face_tags/yunet.rs`
mirrors the graph and uses the canonical tensor names emitted here.

Usage:
  python3 tools/face_tags/convert_yunet.py \
      --input /tmp/face_detection_yunet_2023mar.onnx \
      --output server/models/yunet.safetensors

The upstream model is MIT licensed; see server/models/LICENSES/YuNet-LICENSE.
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
import urllib.request

DEFAULT_URL = (
    "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/"
    "models/face_detection_yunet/face_detection_yunet_2023mar.onnx"
)


def _varint(buf: bytes, pos: int) -> tuple[int, int]:
    value = 0
    shift = 0
    while True:
        byte = buf[pos]
        pos += 1
        value |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return value, pos
        shift += 7


def _fields(buf: bytes):
    pos = 0
    while pos < len(buf):
        key, pos = _varint(buf, pos)
        field = key >> 3
        wire = key & 7
        if wire == 0:
            value, pos = _varint(buf, pos)
            yield field, wire, value
        elif wire == 2:
            length, pos = _varint(buf, pos)
            yield field, wire, buf[pos : pos + length]
            pos += length
        elif wire == 5:
            yield field, wire, buf[pos : pos + 4]
            pos += 4
        elif wire == 1:
            yield field, wire, buf[pos : pos + 8]
            pos += 8
        else:
            raise ValueError(f"unsupported protobuf wire type {wire}")


def _text(buf: bytes) -> str:
    return buf.decode("utf-8", "replace")


def _parse_graph(model: bytes):
    graph = None
    for field, wire, value in _fields(model):
        if field == 7 and wire == 2:
            graph = value
    if graph is None:
        raise ValueError("not an ONNX ModelProto with a graph")

    nodes = []
    initializers = {}
    for field, wire, value in _fields(graph):
        if field == 1 and wire == 2:
            op = None
            inputs = []
            outputs = []
            for f, w, v in _fields(value):
                if f == 1 and w == 2:
                    inputs.append(_text(v))
                elif f == 2 and w == 2:
                    outputs.append(_text(v))
                elif f == 4 and w == 2:
                    op = _text(v)
            nodes.append((op, inputs, outputs))
        elif field == 5 and wire == 2:
            dims = []
            dtype = None
            name = None
            raw = None
            floats = []
            for f, w, v in _fields(value):
                if f == 1 and w == 0:
                    dims.append(v)
                elif f == 1 and w == 2:
                    pos = 0
                    while pos < len(v):
                        dim, pos = _varint(v, pos)
                        dims.append(dim)
                elif f == 2 and w == 0:
                    dtype = v
                elif f == 4 and w == 2:
                    floats = list(struct.unpack("<" + "f" * (len(v) // 4), v))
                elif f == 8 and w == 2:
                    name = _text(v)
                elif f == 9 and w == 2:
                    raw = v
            initializers[name] = (dtype, dims, floats, raw)
    return nodes, initializers


# Node index -> canonical prefix.  A Conv node's initializer inputs are always
# [weight, bias]; the numeric initializer names in the ONNX file are opaque, so
# the mapping is by graph order, which is stable for this published model.
CONV_NAMES = {
    0: ("stem.conv", True),
    2: ("backbone.0.conv1", True),
    3: ("backbone.0.dw", True),
    6: ("backbone.1.conv1", True),
    7: ("backbone.1.dw1", True),
    9: ("backbone.1.conv2", True),
    10: ("backbone.1.dw2", True),
    12: ("backbone.2.conv1", True),
    13: ("backbone.2.dw1", True),
    15: ("backbone.2.conv2", True),
    16: ("backbone.2.dw2", True),
    19: ("backbone.3.conv1", True),
    20: ("backbone.3.dw1", True),
    22: ("backbone.3.conv2", True),
    23: ("backbone.3.dw2", True),
    26: ("backbone.4.conv1", True),
    27: ("backbone.4.dw1", True),
    29: ("backbone.4.conv2", True),
    30: ("backbone.4.dw2", True),
    33: ("backbone.5.conv1", True),
    34: ("backbone.5.dw1", True),
    36: ("backbone.5.conv2", True),
    37: ("backbone.5.dw2", True),
    39: ("neck.lateral2.conv1", True),
    40: ("neck.lateral2.dw", True),
    44: ("neck.lateral1.conv1", True),
    45: ("neck.lateral1.dw", True),
    49: ("neck.lateral0.conv1", True),
    50: ("neck.lateral0.dw", True),
    52: ("head.cls.0.conv1", True),
    53: ("head.cls.0.conv2", True),
    54: ("head.cls.1.conv1", True),
    55: ("head.cls.1.conv2", True),
    56: ("head.cls.2.conv1", True),
    57: ("head.cls.2.conv2", True),
    58: ("head.bbox.0.conv1", True),
    59: ("head.bbox.0.conv2", True),
    60: ("head.bbox.1.conv1", True),
    61: ("head.bbox.1.conv2", True),
    62: ("head.bbox.2.conv1", True),
    63: ("head.bbox.2.conv2", True),
    64: ("head.obj.0.conv1", True),
    65: ("head.obj.0.conv2", True),
    66: ("head.obj.1.conv1", True),
    67: ("head.obj.1.conv2", True),
    68: ("head.obj.2.conv1", True),
    69: ("head.obj.2.conv2", True),
    70: ("head.kps.0.conv1", True),
    71: ("head.kps.0.conv2", True),
    72: ("head.kps.1.conv1", True),
    73: ("head.kps.1.conv2", True),
    74: ("head.kps.2.conv1", True),
    75: ("head.kps.2.conv2", True),
}


def _tensor_bytes(init, name: str) -> bytes:
    dtype, dims, floats, raw = init
    if dtype != 1:  # FLOAT
        raise ValueError(f"{name}: expected FLOAT, got ONNX dtype {dtype}")
    if raw is not None:
        data = raw
    else:
        data = struct.pack("<" + "f" * len(floats), *floats)
    expected = 4
    for dim in dims:
        expected *= dim
    if len(data) != expected:
        raise ValueError(f"{name}: {len(data)} bytes, expected {expected}")
    return data


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", help="path to the YuNet ONNX file")
    parser.add_argument("--url", default=DEFAULT_URL, help="download URL when --input is absent")
    parser.add_argument("--output", required=True, help="destination .safetensors path")
    args = parser.parse_args()

    if args.input:
        with open(args.input, "rb") as handle:
            model = handle.read()
    else:
        with urllib.request.urlopen(args.url) as response:
            model = response.read()

    nodes, initializers = _parse_graph(model)
    tensors: dict[str, tuple[list[int], bytes]] = {}
    for index, (op, inputs, _outputs) in enumerate(nodes):
        if op != "Conv":
            continue
        if index not in CONV_NAMES:
            raise ValueError(f"unexpected Conv node at index {index}")
        prefix, _ = CONV_NAMES[index]
        if len(inputs) != 3:
            raise ValueError(f"{prefix}: expected weight and bias inputs")
        weight_name, bias_name = inputs[1], inputs[2]
        weight = initializers[weight_name]
        bias = initializers[bias_name]
        tensors[f"{prefix}.weight"] = (weight[1], _tensor_bytes(weight, weight_name))
        tensors[f"{prefix}.bias"] = (bias[1], _tensor_bytes(bias, bias_name))

    # Stable output order keeps the file reproducible byte-for-byte.
    header = {}
    payload = bytearray()
    for name in sorted(tensors):
        dims, data = tensors[name]
        start = len(payload)
        payload.extend(data)
        header[name] = {
            "dtype": "F32",
            "shape": dims,
            "data_offsets": [start, len(payload)],
        }

    header_bytes = json.dumps(header, separators=(",", ":")).encode("utf-8")
    padding = (-len(header_bytes)) % 8
    header_bytes += b" " * padding
    with open(args.output, "wb") as handle:
        handle.write(struct.pack("<Q", len(header_bytes)))
        handle.write(header_bytes)
        handle.write(payload)

    print(f"wrote {args.output}: {len(tensors)} tensors, {len(payload)} bytes payload")
    return 0


if __name__ == "__main__":
    sys.exit(main())
