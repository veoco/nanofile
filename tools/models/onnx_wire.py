"""Minimal ONNX protobuf reader used by ``export_plan.py``.

Only the Python standard library is used: the build-time exporter must run on a
machine with no ``onnx`` package, and the generated plan has to be reproducible
byte for byte. The reader is deliberately narrow -- it understands exactly the
fields the embedded graphs use -- and every accessor that does not understand a
field fails loudly instead of guessing.

The parser works on the file bytes and keeps absolute byte offsets for
length-delimited fields, so a ``TensorProto``'s ``raw_data`` can be referenced
in place: the runtime copies f32 weights straight out of the unchanged ONNX
bytes, and no second copy of the weights is ever committed.
"""

import hashlib
import struct


class ParseError(Exception):
    pass


def _varint(buf, i, end):
    result = 0
    shift = 0
    while True:
        if i >= end:
            raise ParseError("truncated varint")
        byte = buf[i]
        i += 1
        result |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return result, i
        shift += 7
        if shift > 63:
            raise ParseError("varint too long")


def _signed(value):
    if value >= 1 << 63:
        value -= 1 << 64
    return value


def fields(buf, start, end):
    """Yield ``(field, wire, value, offset, length)``.

    ``wire`` is the protobuf wire type. Varints yield an int value; fixed32/64
    yield bytes; length-delimited fields yield ``None`` as value and expose their
    absolute ``offset``/``length`` so callers can parse nested messages or slice
    bytes without copying.
    """
    i = start
    while i < end:
        key, i = _varint(buf, i, end)
        field = key >> 3
        wire = key & 7
        if wire == 0:
            value, i = _varint(buf, i, end)
            yield field, 0, value, None, None
        elif wire == 2:
            length, i = _varint(buf, i, end)
            yield field, 2, None, i, length
            i += length
        elif wire == 5:
            yield field, 5, buf[i:i + 4], i, 4
            i += 4
        elif wire == 1:
            yield field, 1, buf[i:i + 8], i, 8
            i += 8
        else:
            raise ParseError(f"unsupported wire type {wire}")


def field_map(buf, start, end):
    """``{field: [(wire, value, offset, length), ...]}`` preserving order."""
    out = {}
    for field, wire, value, offset, length in fields(buf, start, end):
        out.setdefault(field, []).append((wire, value, offset, length))
    return out


def one_int(buf, start, end, field, default=None):
    entries = field_map(buf, start, end).get(field)
    if not entries:
        if default is not None:
            return default
        raise ParseError(f"missing field {field}")
    return entries[0][1]


def ints(buf, start, end, field):
    """Repeated int64; accepts packed and unpacked encodings."""
    out = []
    for wire, value, offset, length in field_map(buf, start, end).get(field, []):
        if wire == 0:
            out.append(_signed(value))
        elif wire == 2:
            i = offset
            while i < offset + length:
                v, i = _varint(buf, i, offset + length)
                out.append(_signed(v))
        else:
            raise ParseError(f"field {field} is not an int list")
    return out


def floats(buf, start, end, field):
    """Repeated float; packed (wire 2) or repeated fixed32 (wire 5)."""
    out = []
    for wire, value, offset, length in field_map(buf, start, end).get(field, []):
        if wire == 5:
            out.append(struct.unpack("<f", value)[0])
        elif wire == 2:
            for i in range(offset, offset + length, 4):
                out.append(struct.unpack("<f", buf[i:i + 4])[0])
        else:
            raise ParseError(f"field {field} is not a float list")
    return out


def text(buf, start, end, field, default=None):
    entries = field_map(buf, start, end).get(field)
    if not entries:
        return default
    wire, _, offset, length = entries[0]
    if wire != 2:
        raise ParseError(f"field {field} is not a string")
    return buf[offset:offset + length].decode("utf-8", "strict")


def span(buf, field_entry):
    wire, _, offset, length = field_entry
    if wire != 2:
        raise ParseError("expected a length-delimited field")
    return offset, length


DTYPES = {
    1: "f32", 2: "u8", 3: "i8", 4: "u16", 5: "i16", 6: "i32", 7: "i64",
    8: "str", 9: "bool", 10: "f16", 11: "f64", 12: "u32", 13: "u64",
    14: "c64", 15: "c128", 16: "bf16",
}

ATTR_TYPES = {
    1: "FLOAT", 2: "INT", 3: "STRING", 4: "TENSOR", 5: "GRAPH",
    6: "FLOATS", 7: "INTS", 8: "STRINGS", 9: "TENSORS",
}


class Tensor:
    """A `TensorProto`: name, dims, dtype, and either a raw span or values."""

    def __init__(self, buf, start, end):
        f = field_map(buf, start, end)
        self.name = text(buf, start, end, 8, "")
        self.dims = ints(buf, start, end, 1)
        dtype = f.get(2)
        self.dtype = DTYPES.get(dtype[0][1], "?") if dtype else "?"
        self.raw = None
        if 9 in f:
            self.raw = span(buf, f[9][0])
        self.float_data = floats(buf, start, end, 4) if 4 in f else []
        self.int64_data = ints(buf, start, end, 7) if 7 in f else []
        self.int32_data = ints(buf, start, end, 5) if 5 in f else []
        self.buf = buf

    @property
    def numel(self):
        n = 1
        for d in self.dims:
            n *= d
        return n

    def f32(self):
        if self.dtype != "f32":
            raise ParseError(f"tensor {self.name!r} is {self.dtype}, expected f32")
        if self.raw is not None:
            offset, length = self.raw
            if length != self.numel * 4:
                raise ParseError(f"tensor {self.name!r}: raw {length} != {self.numel}*4")
            return list(struct.unpack_from(f"<{self.numel}f", self.buf, offset))
        if self.float_data:
            return list(self.float_data)
        raise ParseError(f"tensor {self.name!r} has no f32 data")

    def i64(self):
        if self.raw is not None:
            offset, length = self.raw
            if self.dtype != "i64":
                raise ParseError(f"tensor {self.name!r} is {self.dtype}, expected i64")
            if length != self.numel * 8:
                raise ParseError(f"tensor {self.name!r}: raw {length} != {self.numel}*8")
            return list(struct.unpack_from(f"<{self.numel}q", self.buf, offset))
        if self.int64_data:
            return list(self.int64_data)
        raise ParseError(f"tensor {self.name!r} has no i64 data")


class Attribute:
    def __init__(self, buf, start, end):
        f = field_map(buf, start, end)
        self.name = text(buf, start, end, 1, "")
        declared = f.get(20)
        self.type = ATTR_TYPES.get(declared[0][1], "?") if declared else "?"
        self.value = None
        if self.type == "FLOAT":
            self.value = struct.unpack("<f", f[2][0][1])[0] if 2 in f else None
        elif self.type == "INT":
            self.value = _signed(f[3][0][1]) if 3 in f else None
        elif self.type == "STRING":
            offset, length = span(buf, f[4][0])
            self.value = buf[offset:offset + length].decode("utf-8", "strict")
        elif self.type == "TENSOR" and 5 in f:
            offset, length = span(buf, f[5][0])
            self.value = Tensor(buf, offset, offset + length)
        elif self.type == "FLOATS":
            self.value = floats(buf, start, end, 7)
        elif self.type == "INTS":
            self.value = ints(buf, start, end, 8)
        elif self.type == "STRINGS":
            self.value = [
                buf[off:off + ln].decode("utf-8", "strict")
                for _, _, off, ln in f.get(9, [])
            ]
        elif self.type == "TENSORS":
            self.value = []
            for _, _, off, ln in f.get(10, []):
                self.value.append(Tensor(buf, off, off + ln))
        else:
            raise ParseError(f"attribute {self.name!r} has unsupported type {self.type}")


class Node:
    def __init__(self, buf, start, end):
        f = field_map(buf, start, end)
        self.op_type = text(buf, start, end, 4, "")
        self.domain = text(buf, start, end, 7, "")
        self.name = text(buf, start, end, 3, "")
        self.inputs = [
            buf[off:off + ln].decode("utf-8", "strict")
            for _, _, off, ln in f.get(1, [])
        ]
        self.outputs = [
            buf[off:off + ln].decode("utf-8", "strict")
            for _, _, off, ln in f.get(2, [])
        ]
        self.attributes = {}
        for _, _, off, ln in f.get(5, []):
            attribute = Attribute(buf, off, off + ln)
            self.attributes[attribute.name] = attribute

    def attr(self, name, default=None):
        attribute = self.attributes.get(name)
        return default if attribute is None else attribute.value

    def ints(self, name, default=()):
        value = self.attr(name)
        if value is None:
            return list(default)
        if isinstance(value, int):
            return [value]
        return list(value)

    def int(self, name, default=None):
        return self.attr(name, default)


class ValueInfo:
    def __init__(self, buf, start, end):
        self.name = text(buf, start, end, 1, "")
        self.elem_type = None
        self.dims = []
        for field, wire, _, offset, length in fields(buf, start, end):
            if field == 2 and wire == 2:
                self._type(buf, offset, offset + length)

    def _type(self, buf, start, end):
        for field, wire, _, offset, length in fields(buf, start, end):
            if field == 1 and wire == 2:
                self._tensor_type(buf, offset, offset + length)

    def _tensor_type(self, buf, start, end):
        for field, wire, value, offset, length in fields(buf, start, end):
            if field == 1 and wire == 0:
                self.elem_type = DTYPES.get(value, "?")
            elif field == 2 and wire == 2:
                self._shape(buf, offset, offset + length)

    def _shape(self, buf, start, end):
        self.dims = []
        for field, wire, _, offset, length in fields(buf, start, end):
            if field != 1 or wire != 2:
                continue
            dim_value = None
            for dfield, dwire, dvalue, doffset, dlength in fields(buf, offset, offset + length):
                if dfield == 1 and dwire == 0:
                    dim_value = dvalue
                elif dfield == 2 and dwire == 2:
                    dim_value = buf[doffset:doffset + dlength].decode("utf-8", "strict")
            self.dims.append(dim_value)


class Graph:
    def __init__(self, buf, start, end):
        f = field_map(buf, start, end)
        self.nodes = [Node(buf, off, off + ln) for _, _, off, ln in f.get(1, [])]
        self.name = text(buf, start, end, 2, "")
        self.initializers = {}
        for _, _, off, ln in f.get(5, []):
            tensor = Tensor(buf, off, off + ln)
            self.initializers[tensor.name] = tensor
        self.inputs = [ValueInfo(buf, off, off + ln) for _, _, off, ln in f.get(11, [])]
        self.outputs = [ValueInfo(buf, off, off + ln) for _, _, off, ln in f.get(12, [])]


class Model:
    def __init__(self, path):
        self.path = path
        self.buf = open(path, "rb").read()
        self.sha256 = hashlib.sha256(self.buf).hexdigest()
        self.graph = None
        for field, wire, _, offset, length in fields(self.buf, 0, len(self.buf)):
            if field == 7 and wire == 2:
                self.graph = Graph(self.buf, offset, offset + length)
        if self.graph is None:
            raise ParseError(f"{path}: no graph")
