#!/usr/bin/env python3
"""Export the embedded ONNX graphs into static Rust inference plans.

The server does not parse ONNX at run time. This tool reads the five unchanged
`server/models/*.onnx` files and writes `server/src/inference/generated/*.rs`:
a flat, topologically ordered operator list, a workspace-slot assignment, the
graph outputs, and the byte range of every f32 weight inside the ONNX file.

The runtime (`server/src/inference/`) then only copies the weights and executes
the plan. Nothing here runs during a build or at run time; it is a development
tool. The generated files are committed, and a unit test checks that the ONNX
file's SHA-256 still matches the plan, so a model update cannot silently leave a
stale plan behind.

Usage:
    python3 tools/models/export_plan.py            # rewrite the generated files
    python3 tools/models/export_plan.py --check    # fail if they are stale

The reader is the stdlib-only `onnx_wire.py` next to this file. Only the fields
these five graphs use are understood; anything else fails loudly.
"""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from onnx_wire import Model, ParseError  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SERVER = os.path.join(ROOT, "server")
MODELS = os.path.join(SERVER, "models")
OUT_DIR = os.path.join(SERVER, "src", "inference", "generated")

PLAN_MODELS = [
    ("YuNet", "yunet", "yunet.onnx"),
    ("OcrDet", "det", "ppocr_v6_tiny_det.onnx"),
    ("OcrRec", "rec", "ppocr_v6_tiny_rec.onnx"),
    ("DocOri", "doc_ori", "pp_lcnet_x1_0_doc_ori.onnx"),
    ("TextlineOri", "textline_ori", "pp_lcnet_x0_25_textline_ori.onnx"),
]

# Operators the runtime implements. Anything else in a model fails the export.
SUPPORTED_OPS = {
    "Constant", "Conv", "ConvTranspose", "BatchNormalization", "MatMul",
    "Add", "Mul", "Div", "Relu", "Sigmoid", "HardSigmoid", "Erf", "Softmax",
    "ReduceMean", "GlobalAveragePool", "MaxPool", "AveragePool", "Resize",
    "Concat", "Transpose", "Reshape", "Squeeze", "Unsqueeze", "Identity",
    "Shape", "Slice", "Cast",
}

# Structural operators folded at export time: their output is a Python value,
# never a runtime tensor. `Shape`/`Slice`/`Concat`/`Cast` only appear in the
# LCNet shape subgraph; `Add`/`Sub`/`Mul`/`Div` are there for shape arithmetic.
FOLDABLE_OPS = {
    "Shape", "Slice", "Concat", "Cast", "Identity", "Squeeze", "Unsqueeze",
    "Add", "Sub", "Mul", "Div",
}


class ExportError(Exception):
    pass


def fail(message):
    raise ExportError(message)


# ---------------------------------------------------------------------------
# small helpers
# ---------------------------------------------------------------------------

def div_ceil(a, b):
    return -(-a // b)


def as_list(value, default=None):
    if value is None:
        if default is None:
            fail("missing attribute")
        return list(default)
    if isinstance(value, (list, tuple)):
        return list(value)
    return [value]


def as_int(value, default=None):
    if value is None:
        if default is None:
            fail("missing integer attribute")
        return default
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, list) and len(value) == 1:
        return value[0]
    fail(f"expected an integer, got {value!r}")


def norm_pads(values):
    values = as_list(values)
    if len(values) == 2:
        return [values[0], values[1], values[0], values[1]]
    if len(values) != 4:
        fail(f"pads must have 2 or 4 entries, got {values!r}")
    return values


def norm_auto_pad(node):
    value = node.attr("auto_pad", "NOTSET")
    if value in (None, "", "NOTSET"):
        return "NotSet"
    if value in ("SAME_UPPER", "SAME_LOWER"):
        return "SameUpper" if value == "SAME_UPPER" else "SameLower"
    fail(f"{node.op_type} {node.name!r}: unsupported auto_pad {value!r}")


def conv_dim(inp, kernel, stride, dilation, pad_begin, pad_end, auto_pad):
    if inp is None:
        return None
    window = dilation * (kernel - 1) + 1
    if auto_pad != "NotSet":
        return div_ceil(inp, stride)
    if inp + pad_begin + pad_end < window:
        return 0
    return (inp + pad_begin + pad_end - window) // stride + 1


def pool_dim(inp, kernel, stride, pad_begin, pad_end, auto_pad, ceil_mode):
    if inp is None:
        return None
    if auto_pad != "NotSet":
        return div_ceil(inp, stride)
    numerator = inp + pad_begin + pad_end - kernel
    if numerator < 0:
        return 0
    if ceil_mode:
        return div_ceil(numerator, stride) + 1
    return numerator // stride + 1


def same_pads(inp, kernel, stride, lower):
    if inp is None:
        return (None, None)
    window = kernel
    output = div_ceil(inp, stride)
    total = max((output - 1) * stride + window - inp, 0)
    begin = total // 2
    if lower:
        return (total - begin, begin)
    return (begin, total - begin)


def broadcast_shape(a, b):
    rank = max(len(a), len(b))
    out = []
    for i in range(rank):
        da = a[i - (rank - len(a))] if i >= rank - len(a) else 1
        db = b[i - (rank - len(b))] if i >= rank - len(b) else 1
        if da is None or db is None:
            if da == 1 and db is not None:
                out.append(db)
            elif db == 1 and da is not None:
                out.append(da)
            else:
                out.append(None)
            continue
        if da != db and da != 1 and db != 1:
            fail(f"cannot broadcast {a!r} and {b!r}")
        out.append(max(da, db))
    return out


def numel(dims):
    n = 1
    for d in dims:
        if d is None:
            return None
        n *= d
    return n


# ---------------------------------------------------------------------------
# shape inference (enough for the embedded graphs, including dynamic det/rec)
# ---------------------------------------------------------------------------

def infer_outputs(node, dims, constants):
    """Return {output_name: dims} for one node, using known tensor dims."""

    op = node.op_type

    def input_dims(index):
        if index >= len(node.inputs) or node.inputs[index] == "":
            return None
        name = node.inputs[index]
        if name in constants:
            return [len(constants[name])]
        return dims.get(name)

    if op in ("Identity", "Relu", "Sigmoid", "HardSigmoid", "Erf", "Softmax",
              "BatchNormalization", "Cast", "Shape", "Slice"):
        shape = input_dims(0)
        if op == "Shape":
            return {node.outputs[0]: [len(shape) if shape is not None else None]}
        if op == "Slice":
            shape = list(shape or [])
            starts = as_list(node.attr("starts", []))
            axes = as_list(node.attr("axes", list(range(len(starts)))))
            out = list(shape)
            for axis, start in zip(axes, starts):
                if axis < 0:
                    axis += len(out)
                if out[axis] is not None and isinstance(start, int) and start >= 0:
                    out[axis] = max(out[axis] - start, 0)
            return {node.outputs[0]: out}
        return {node.outputs[0]: list(shape) if shape is not None else None}

    if op in ("Add", "Mul", "Div", "Sub"):
        a = input_dims(0)
        b = input_dims(1)
        if a is None or b is None:
            return {node.outputs[0]: None}
        return {node.outputs[0]: broadcast_shape(a, b)}

    if op == "Conv":
        weight = node.inputs[1]
        wshape = dims.get(weight)
        if not wshape or len(wshape) != 4:
            fail(f"Conv {node.name!r}: weight shape is not known")
        out_channels = wshape[0]
        shape = input_dims(0)
        if shape is None or len(shape) != 4:
            return {node.outputs[0]: None}
        kernel = as_list(node.attr("kernel_shape"))
        stride = as_list(node.attr("strides", [1, 1]))
        dilation = as_list(node.attr("dilations", [1, 1]))
        pads = norm_pads(node.attr("pads", [0, 0, 0, 0]))
        auto_pad = norm_auto_pad(node)
        if auto_pad == "SameUpper" or auto_pad == "SameLower":
            pads = list(same_pads(shape[2], kernel[0], stride[0], auto_pad == "SameLower")) + \
                list(same_pads(shape[3], kernel[1], stride[1], auto_pad == "SameLower"))
        h = conv_dim(shape[2], kernel[0], stride[0], dilation[0], pads[0], pads[2], auto_pad)
        w = conv_dim(shape[3], kernel[1], stride[1], dilation[1], pads[1], pads[3], auto_pad)
        return {node.outputs[0]: [shape[0], out_channels, h, w]}

    if op == "ConvTranspose":
        weight = node.inputs[1]
        wshape = dims.get(weight)
        if not wshape or len(wshape) != 4:
            fail(f"ConvTranspose {node.name!r}: weight shape is not known")
        shape = input_dims(0)
        if shape is None or len(shape) != 4:
            return {node.outputs[0]: None}
        kernel = as_list(node.attr("kernel_shape"))
        stride = as_list(node.attr("strides", [1, 1]))
        dilation = as_list(node.attr("dilations", [1, 1]))
        pads = norm_pads(node.attr("pads", [0, 0, 0, 0]))
        out_pad = as_list(node.attr("output_padding", [0, 0]))
        groups = as_int(node.attr("group", 1))
        out_channels = wshape[1] * groups
        if shape[2] is None or shape[3] is None:
            return {node.outputs[0]: [shape[0], out_channels, None, None]}
        h = (shape[2] - 1) * stride[0] - pads[0] - pads[2] + dilation[0] * (kernel[0] - 1) + 1 + out_pad[0]
        w = (shape[3] - 1) * stride[1] - pads[1] - pads[3] + dilation[1] * (kernel[1] - 1) + 1 + out_pad[1]
        return {node.outputs[0]: [shape[0], out_channels, h, w]}

    if op == "MatMul":
        a = input_dims(0)
        b = input_dims(1)
        if a is None or b is None:
            return {node.outputs[0]: None}
        if len(b) != 2:
            fail(f"MatMul {node.name!r}: only a rank-2 right operand is supported")
        out = list(a[:-1]) + [b[1]]
        return {node.outputs[0]: out}

    if op == "GlobalAveragePool":
        shape = input_dims(0)
        if shape is None or len(shape) != 4:
            return {node.outputs[0]: None}
        return {node.outputs[0]: [shape[0], shape[1], 1, 1]}

    if op in ("MaxPool", "AveragePool"):
        shape = input_dims(0)
        if shape is None or len(shape) != 4:
            return {node.outputs[0]: None}
        kernel = as_list(node.attr("kernel_shape"))
        stride = as_list(node.attr("strides", [1, 1]))
        pads = norm_pads(node.attr("pads", [0, 0, 0, 0]))
        auto_pad = norm_auto_pad(node)
        ceil_mode = bool(as_int(node.attr("ceil_mode", 0)))
        if auto_pad != "NotSet":
            pads = list(same_pads(shape[2], kernel[0], stride[0], auto_pad == "SameLower")) + \
                list(same_pads(shape[3], kernel[1], stride[1], auto_pad == "SameLower"))
        h = pool_dim(shape[2], kernel[0], stride[0], pads[0], pads[2], auto_pad, ceil_mode)
        w = pool_dim(shape[3], kernel[1], stride[1], pads[1], pads[3], auto_pad, ceil_mode)
        return {node.outputs[0]: [shape[0], shape[1], h, w]}

    if op == "ReduceMean":
        shape = input_dims(0)
        if shape is None:
            return {node.outputs[0]: None}
        axes = [a if a >= 0 else a + len(shape) for a in as_list(node.attr("axes", range(len(shape))))]
        keep = bool(as_int(node.attr("keepdims", 1)))
        out = []
        for i, d in enumerate(shape):
            if i in axes:
                if keep:
                    out.append(1)
            else:
                out.append(d)
        return {node.outputs[0]: out}

    if op == "Resize":
        shape = input_dims(0)
        if shape is None or len(shape) != 4:
            return {node.outputs[0]: None}
        out_h = shape[2]
        out_w = shape[3]
        scales, sizes = resize_constants(node, dims, constants)
        if sizes is not None:
            out_h, out_w = sizes
        elif scales is not None:
            if out_h is not None:
                out_h = int(out_h * scales[0])
            if out_w is not None:
                out_w = int(out_w * scales[1])
        return {node.outputs[0]: [shape[0], shape[1], out_h, out_w]}

    if op == "Concat":
        shapes = [input_dims(i) for i in range(len(node.inputs))]
        shapes = [s for s in shapes if s is not None]
        if not shapes:
            return {node.outputs[0]: None}
        rank = len(shapes[0])
        axis = as_int(node.attr("axis"))
        if axis < 0:
            axis += rank
        out = list(shapes[0])
        total = 0
        for s in shapes:
            if s[axis] is None:
                out[axis] = None
            else:
                total += s[axis]
        if out[axis] is not None:
            out[axis] = total
        return {node.outputs[0]: out}

    if op == "Transpose":
        shape = input_dims(0)
        if shape is None:
            return {node.outputs[0]: None}
        perm = as_list(node.attr("perm", list(reversed(range(len(shape))))))
        perm = [p + len(shape) if p < 0 else p for p in perm]
        return {node.outputs[0]: [shape[p] for p in perm]}

    if op == "Reshape":
        shape = input_dims(0)
        target = reshape_target(node, constants)
        if target is None or shape is None:
            return {node.outputs[0]: None}
        return {node.outputs[0]: resolve_reshape(shape, target)}

    if op in ("Squeeze", "Unsqueeze"):
        shape = input_dims(0)
        if shape is None:
            return {node.outputs[0]: None}
        axes = squeeze_axes(node, constants, len(shape))
        if op == "Squeeze":
            axes = [a if a >= 0 else a + len(shape) for a in axes]
            out = [d for i, d in enumerate(shape) if i not in axes]
        else:
            out = list(shape)
            for axis in sorted((a if a >= 0 else a + len(shape) + len(axes)) for a in axes):
                out.insert(axis, 1)
        return {node.outputs[0]: out}

    if op == "Constant":
        return {}

    fail(f"{node.op_type} {node.name!r}: no shape inference")


def reshape_target(node, constants):
    if len(node.inputs) > 1 and node.inputs[1] != "":
        name = node.inputs[1]
        if name in constants:
            return constants[name]
        tensor = _TENSORS.get(name)
        if tensor is not None and tensor.dtype == "i64":
            return tensor.i64()
    shape = node.attr("shape")
    if shape is not None:
        return as_list(shape)
    return None


def resolve_reshape(shape, target):
    target = [int(v) for v in target]
    out = []
    for i, value in enumerate(target):
        if value == 0:
            out.append(shape[i] if i < len(shape) else None)
        elif value == -1:
            out.append(None)
        else:
            out.append(value)
    if out.count(None) == 1 and all(d is not None for d in shape) and all(
            d is not None for d in out if d is not None):
        known = 1
        for d in out:
            if d is not None:
                known *= d
        total = numel(shape)
        if known:
            out[out.index(None)] = total // known
    return out


def squeeze_axes(node, constants, rank):
    if len(node.inputs) > 1 and node.inputs[1] != "":
        name = node.inputs[1]
        if name in constants:
            return [int(v) for v in constants[name]]
        tensor = _TENSORS.get(name)
        if tensor is not None and tensor.dtype == "i64":
            return [int(v) for v in tensor.i64()]
    value = node.attr("axes")
    if value is not None:
        return [int(v) for v in as_list(value)]
    return []


def resize_constants(node, dims, constants):
    """Return (scale_hw, size_hw) from a Resize node's constant inputs."""
    scales = None
    sizes = None
    if len(node.inputs) > 2 and node.inputs[2] != "":
        name = node.inputs[2]
        values = constants.get(name)
        if values is None:
            tensor = _tensor_by_name(name)
            values = tensor.f32() if tensor is not None else None
        if values is not None:
            if len(values) != 4:
                fail(f"Resize {node.name!r}: scales must have 4 entries")
            scales = (float(values[2]), float(values[3]))
    if len(node.inputs) > 3 and node.inputs[3] != "":
        name = node.inputs[3]
        values = constants.get(name)
        if values is None:
            tensor = _tensor_by_name(name)
            values = tensor.i64() if tensor is not None else None
        if values is not None:
            if len(values) != 4:
                fail(f"Resize {node.name!r}: sizes must have 4 entries")
            sizes = (int(values[2]), int(values[3]))
    if scales is None and sizes is None:
        fail(f"Resize {node.name!r}: neither scales nor sizes are constant")
    return scales, sizes


# A hook filled in per model; `resize_constants` needs access to the tensors.
_TENSORS = {}
_FOLDED = {}


def _tensor_by_name(name):
    tensor = _TENSORS.get(name)
    if tensor is None:
        return None
    return tensor


# ---------------------------------------------------------------------------
# constant folding
# ---------------------------------------------------------------------------

def get_constant(node, name):
    if name == "":
        return None
    if name in _FOLDED:
        return _FOLDED[name]
    tensor = _TENSORS.get(name)
    if tensor is not None:
        if tensor.dtype == "i64" and tensor.numel <= 64:
            return tensor.i64()
        if tensor.dtype == "f32" and tensor.numel <= 64:
            return tensor.f32()
    return None


def try_fold(node, dims):
    op = node.op_type
    out = node.outputs[0] if node.outputs else None
    if op == "Shape":
        shape = dims.get(node.inputs[0])
        if shape is None or any(d is None for d in shape):
            return None
        _FOLDED[out] = [int(d) for d in shape]
        return True
    if op == "Slice":
        values = get_constant(node, node.inputs[0])
        if values is None:
            return None
        starts = as_list(node.attr("starts", []))
        ends = as_list(node.attr("ends", []))
        axes = as_list(node.attr("axes", list(range(len(starts)))))
        result = list(values)
        for axis, start, end in zip(axes, starts, ends):
            rank = 1
            start = int(start)
            end = int(end)
            if start < 0:
                start += len(result)
            if end < 0:
                end += len(result)
            end = min(end, len(result))
            result = result[start:end]
        _FOLDED[out] = result
        return True
    if op == "Cast":
        values = get_constant(node, node.inputs[0])
        if values is None:
            return None
        to = as_int(node.attr("to"))
        if to in (1, 11):
            _FOLDED[out] = [float(v) for v in values]
        elif to in (6, 7):
            _FOLDED[out] = [int(v) for v in values]
        else:
            fail(f"Cast {node.name!r}: unsupported target type {to}")
        return True
    if op == "Identity":
        values = get_constant(node, node.inputs[0])
        if values is None:
            return None
        _FOLDED[out] = list(values)
        return True
    if op == "Concat":
        values = [get_constant(node, name) for name in node.inputs]
        if any(v is None for v in values):
            return None
        axis = as_int(node.attr("axis"))
        if axis != 0:
            fail(f"Concat {node.name!r}: only axis 0 shape concatenation is supported")
        merged = []
        for v in values:
            merged.extend(v)
        _FOLDED[out] = merged
        return True
    if op in ("Add", "Sub", "Mul", "Div"):
        values = [get_constant(node, name) for name in node.inputs]
        if any(v is None for v in values):
            return None
        if len(values) != 2:
            fail(f"{op} {node.name!r}: shape arithmetic with two operands expected")
        a, b = list(values[0]), list(values[1])
        if len(a) == 1:
            a = a * len(b)
        if len(b) == 1:
            b = b * len(a)
        if len(a) != len(b):
            fail(f"{op} {node.name!r}: shape arithmetic lengths differ")
        if op == "Add":
            _FOLDED[out] = [x + y for x, y in zip(a, b)]
        elif op == "Sub":
            _FOLDED[out] = [x - y for x, y in zip(a, b)]
        elif op == "Mul":
            _FOLDED[out] = [x * y for x, y in zip(a, b)]
        else:
            _FOLDED[out] = [int(x / y) for x, y in zip(a, b)]
        return True
    if op in ("Squeeze", "Unsqueeze"):
        values = get_constant(node, node.inputs[0])
        if values is None:
            return None
        _FOLDED[out] = list(values)
        return True
    return False


# ---------------------------------------------------------------------------
# emission
# ---------------------------------------------------------------------------

class Emitted:
    def __init__(self, kind, inputs, outputs, render, uses_operand=False):
        self.kind = kind
        self.inputs = inputs      # activation tensor names consumed
        self.outputs = outputs    # activation tensor names produced
        self.render = render      # callable(slot_of, weight_of) -> rust text
        self.uses_operand = uses_operand


class ModelExporter:
    def __init__(self, kind, module, filename):
        self.kind = kind
        self.module = module
        self.filename = filename
        self.model = Model(os.path.join(MODELS, filename))
        global _TENSORS, _FOLDED
        _TENSORS = {}
        _FOLDED = {}
        self.dims = {}
        self.constants = _FOLDED
        self.weights = []          # [(offset, len)]
        self.weight_index = {}     # tensor name -> index
        self.emitted = []
        self.slot_of = {}
        self.input_name = None

    # -- model preparation -------------------------------------------------
    def prepare(self):
        graph = self.model.graph
        self._read_tensors(graph)
        self._read_inputs(graph)
        self._infer_and_fold(graph)
        self._emit(graph)
        self._assign_slots(graph)
        self._check_outputs(graph)

    def _read_tensors(self, graph):
        for tensor in graph.initializers.values():
            _TENSORS[tensor.name] = tensor
            self.dims[tensor.name] = list(tensor.dims)
        for node in graph.nodes:
            if node.op_type != "Constant":
                continue
            if len(node.outputs) != 1:
                fail(f"Constant {node.name!r}: expected one output")
            from onnx_wire import Tensor
            tensor = node.attr("value")
            if not isinstance(tensor, Tensor):
                fail(f"Constant {node.name!r}: only tensor values are supported")
            tensor.name = node.outputs[0]
            _TENSORS[tensor.name] = tensor
            self.dims[tensor.name] = list(tensor.dims)
            if tensor.numel <= 64 and tensor.dtype == "i64":
                self.constants[tensor.name] = tensor.i64()

    def _read_inputs(self, graph):
        inputs = [vi for vi in graph.inputs if vi.name not in _TENSORS]
        if len(inputs) != 1:
            fail(f"{self.filename}: expected one data input, found {[i.name for i in inputs]}")
        vi = inputs[0]
        if vi.elem_type != "f32":
            fail(f"{self.filename}: input {vi.name!r} is {vi.elem_type}, expected f32")
        if len(vi.dims) != 4:
            fail(f"{self.filename}: input {vi.name!r} rank {len(vi.dims)}, expected 4")
        channels = vi.dims[1]
        if channels != 3:
            fail(f"{self.filename}: input {vi.name!r} has {channels} channels, expected 3")
        self.input_name = vi.name
        dims = [d if isinstance(d, int) else None for d in vi.dims]
        # Every call site runs batch 1; a symbolic batch is folded as 1 so the
        # LCNet shape subgraph (Shape -> Slice -> Concat -> Reshape) is constant.
        if dims[0] is None:
            dims[0] = 1
        self.dims[vi.name] = dims

    def _infer_and_fold(self, graph):
        for node in graph.nodes:
            if node.op_type not in SUPPORTED_OPS:
                fail(f"{self.filename}: unsupported op {node.op_type!r} ({node.name!r})")
            if node.domain not in ("", "ai.onnx"):
                fail(f"{self.filename}: unsupported op domain {node.domain!r}")
            if node.op_type == "Constant":
                continue
            try:
                outputs = infer_outputs(node, self.dims, self.constants)
            except ParseError as error:
                fail(f"{self.filename}: {error}")
            for name, shape in outputs.items():
                self.dims[name] = shape
            if node.op_type in FOLDABLE_OPS:
                if try_fold(node, self.dims):
                    continue

    def _emit(self, graph):
        for node in graph.nodes:
            op = node.op_type
            if op == "Constant":
                continue
            if node.outputs and node.outputs[0] in self.constants:
                continue  # folded shape arithmetic / constant tensor
            self.emitted.append(self._emit_node(node))
        if not self.emitted:
            fail(f"{self.filename}: no operators emitted")

    def _emit_node(self, node):
        op = node.op_type
        handlers = {
            "Conv": self._emit_conv,
            "ConvTranspose": self._emit_conv_transpose,
            "BatchNormalization": self._emit_batch_norm,
            "MatMul": self._emit_matmul,
            "Add": self._emit_binary,
            "Mul": self._emit_binary,
            "Div": self._emit_binary,
            "Relu": self._emit_unary,
            "Sigmoid": self._emit_unary,
            "HardSigmoid": self._emit_hard_sigmoid,
            "Erf": self._emit_unary,
            "Softmax": self._emit_softmax,
            "ReduceMean": self._emit_reduce_mean,
            "GlobalAveragePool": self._emit_global_average,
            "MaxPool": self._emit_pool,
            "AveragePool": self._emit_pool,
            "Resize": self._emit_resize,
            "Concat": self._emit_concat,
            "Transpose": self._emit_transpose,
            "Reshape": self._emit_reshape,
            "Squeeze": self._emit_squeeze,
            "Unsqueeze": self._emit_unsqueeze,
            "Identity": self._emit_identity,
        }
        handler = handlers.get(op)
        if handler is None:
            fail(f"{self.filename}: {op} {node.name!r} reached the emitter unsupported")
        return handler(node)

    # -- operand helpers ---------------------------------------------------
    def _is_activation(self, name):
        return name != "" and name not in _TENSORS and name not in self.constants

    def _activation(self, node, index, what):
        if index >= len(node.inputs):
            fail(f"{node.op_type} {node.name!r}: missing {what}")
        name = node.inputs[index]
        if not self._is_activation(name):
            fail(f"{node.op_type} {node.name!r}: {what} {name!r} is not an activation")
        return name

    def _weight(self, node, index, what, expected_rank=None):
        if index >= len(node.inputs) or node.inputs[index] == "":
            fail(f"{node.op_type} {node.name!r}: missing {what}")
        name = node.inputs[index]
        tensor = _TENSORS.get(name)
        if tensor is None:
            fail(f"{node.op_type} {node.name!r}: {what} {name!r} is not a weight tensor")
        if tensor.dtype != "f32":
            fail(f"{node.op_type} {node.name!r}: {what} {name!r} is {tensor.dtype}, expected f32")
        if tensor.raw is None:
            fail(f"{node.op_type} {node.name!r}: {what} {name!r} has no raw_data")
        offset, length = tensor.raw
        if length != tensor.numel * 4:
            fail(f"{node.op_type} {node.name!r}: {what} {name!r} byte length mismatch")
        if expected_rank is not None and len(tensor.dims) != expected_rank:
            fail(f"{node.op_type} {node.name!r}: {what} {name!r} rank {len(tensor.dims)}")
        index_value = self.weight_index.get(name)
        if index_value is None:
            index_value = len(self.weights)
            self.weight_index[name] = index_value
            self.weights.append((offset, length))
        return index_value, list(tensor.dims)

    def _optional_weight(self, node, index, what, expected_rank=None):
        if index >= len(node.inputs) or node.inputs[index] == "":
            return None
        return self._weight(node, index, what, expected_rank)

    def _operand(self, node, index):
        if index >= len(node.inputs) or node.inputs[index] == "":
            fail(f"{node.op_type} {node.name!r}: missing operand")
        name = node.inputs[index]
        if self._is_activation(name):
            return ("slot", name)
        index_value, dims = self._weight(node, index, "operand", None)
        return ("weight", (index_value, dims))

    @staticmethod
    def _render_operand(operand, slot_of):
        kind, value = operand
        if kind == "slot":
            return f"Operand::Slot({slot_of[value]})"
        index, dims = value
        rendered = ", ".join(str(int(d)) for d in dims)
        return f"Operand::Weight {{ index: {index}, dims: &[{rendered}] }}"

    # -- node emitters -----------------------------------------------------
    def _emit_conv(self, node):
        name = self._activation(node, 0, "input")
        weight_index, wdims = self._weight(node, 1, "weight", 4)
        bias = self._optional_weight(node, 2, "bias", 1)
        kernel = as_list(node.attr("kernel_shape"))
        stride = as_list(node.attr("strides", [1, 1]))
        dilation = as_list(node.attr("dilations", [1, 1]))
        pads = norm_pads(node.attr("pads", [0, 0, 0, 0]))
        auto_pad = norm_auto_pad(node)
        groups = as_int(node.attr("group", 1))
        if len(kernel) != 2 or len(stride) != 2 or len(dilation) != 2:
            fail(f"Conv {node.name!r}: malformed kernel/stride/dilation")
        in_channels = self.dims.get(name)
        in_channels = in_channels[1] if in_channels is not None else None
        if groups == 1:
            if in_channels is not None and wdims[1] != in_channels:
                fail(f"Conv {node.name!r}: weight in-channels {wdims[1]} != input {in_channels}")
        else:
            if in_channels is None:
                fail(f"Conv {node.name!r}: grouped conv with unknown input channels")
            if wdims[1] != 1 or groups != in_channels or wdims[0] != groups:
                fail(
                    f"Conv {node.name!r}: only depthwise grouping is supported "
                    f"(group={groups}, weight={wdims}, input={in_channels})"
                )
        if bias is not None and bias[1][0] != wdims[0]:
            fail(f"Conv {node.name!r}: bias length {bias[1]} != channels {wdims[0]}")
        bias_text = f"Some({bias[0]})" if bias is not None else "None"
        output = node.outputs[0]

        def render(slot_of, _weight_of):
            return (
                "Op::Conv { "
                f"input: {slot_of[name]}, weight: {weight_index}, bias: {bias_text}, "
                f"output: {slot_of[output]}, out_channels: {wdims[0]}, "
                f"kernel: [{kernel[0]}, {kernel[1]}], "
                f"stride: [{stride[0]}, {stride[1]}], dilation: [{dilation[0]}, {dilation[1]}], "
                f"pads: [{pads[0]}, {pads[1]}, {pads[2]}, {pads[3]}], "
                f"auto_pad: AutoPad::{auto_pad}, groups: {groups} }}"
            )

        return Emitted("Conv", [name], [output], render)

    def _emit_conv_transpose(self, node):
        name = self._activation(node, 0, "input")
        weight_index, wdims = self._weight(node, 1, "weight", 4)
        bias = self._optional_weight(node, 2, "bias", 1)
        kernel = as_list(node.attr("kernel_shape"))
        stride = as_list(node.attr("strides", [1, 1]))
        dilation = as_list(node.attr("dilations", [1, 1]))
        pads = norm_pads(node.attr("pads", [0, 0, 0, 0]))
        out_pad = as_list(node.attr("output_padding", [0, 0]))
        groups = as_int(node.attr("group", 1))
        auto_pad = norm_auto_pad(node)
        if auto_pad != "NotSet":
            fail(f"ConvTranspose {node.name!r}: auto_pad is not supported")
        if groups != 1:
            fail(f"ConvTranspose {node.name!r}: grouped transpose is not supported")
        if bias is not None and bias[1][0] != wdims[1]:
            fail(f"ConvTranspose {node.name!r}: bias length mismatch")
        bias_text = f"Some({bias[0]})" if bias is not None else "None"
        output = node.outputs[0]

        def render(slot_of, _weight_of):
            return (
                "Op::ConvTranspose { "
                f"input: {slot_of[name]}, weight: {weight_index}, bias: {bias_text}, "
                f"output: {slot_of[output]}, in_channels: {wdims[0]}, "
                f"out_channels: {wdims[1] * groups}, "
                f"kernel: [{kernel[0]}, {kernel[1]}], "
                f"stride: [{stride[0]}, {stride[1]}], dilation: [{dilation[0]}, {dilation[1]}], "
                f"pads: [{pads[0]}, {pads[1]}, {pads[2]}, {pads[3]}], "
                f"output_padding: [{out_pad[0]}, {out_pad[1]}], groups: {groups} }}"
            )

        return Emitted("ConvTranspose", [name], [output], render)

    def _emit_batch_norm(self, node):
        name = self._activation(node, 0, "input")
        scale = self._weight(node, 1, "scale", 1)
        bias = self._weight(node, 2, "bias", 1)
        mean = self._weight(node, 3, "mean", 1)
        variance = self._weight(node, 4, "variance", 1)
        channels = self.dims.get(name)
        channels = channels[1] if channels is not None else None
        if channels is not None:
            for label, (_, dims) in (("scale", scale), ("bias", bias), ("mean", mean), ("variance", variance)):
                if dims[0] != channels:
                    fail(f"BatchNormalization {node.name!r}: {label} length {dims[0]} != {channels}")
        epsilon = float(node.attr("epsilon", 1e-5))
        output = node.outputs[0]

        def render(slot_of, _weight_of):
            return (
                "Op::BatchNorm { "
                f"input: {slot_of[name]}, scale: {scale[0]}, bias: {bias[0]}, "
                f"mean: {mean[0]}, variance: {variance[0]}, "
                f"output: {slot_of[output]}, epsilon: {fl(epsilon)} }}"
            )

        return Emitted("BatchNorm", [name], [output], render)

    @staticmethod
    def _operand_inputs(*operands):
        return [value for kind, value in operands if kind == "slot"]

    def _emit_matmul(self, node):
        lhs = self._operand(node, 0)
        rhs = self._operand(node, 1)
        output = node.outputs[0]
        inputs = self._operand_inputs(lhs, rhs)

        def render(slot_of, _weight_of):
            return (
                "Op::MatMul { "
                f"lhs: {self._render_operand(lhs, slot_of)}, "
                f"rhs: {self._render_operand(rhs, slot_of)}, "
                f"output: {slot_of[output]} }}"
            )

        return Emitted("MatMul", inputs, [output], render, True)

    def _emit_binary(self, node):
        lhs = self._operand(node, 0)
        rhs = self._operand(node, 1)
        output = node.outputs[0]
        op = node.op_type
        inputs = self._operand_inputs(lhs, rhs)

        def render(slot_of, _weight_of):
            return (
                f"Op::{op} {{ "
                f"lhs: {self._render_operand(lhs, slot_of)}, "
                f"rhs: {self._render_operand(rhs, slot_of)}, "
                f"output: {slot_of[output]} }}"
            )

        return Emitted(op, inputs, [output], render, True)

    def _emit_unary(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]
        op = node.op_type

        def render(slot_of, _weight_of):
            return f"Op::{op} {{ input: {slot_of[name]}, output: {slot_of[output]} }}"

        return Emitted(op, [name], [output], render)

    def _emit_hard_sigmoid(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]
        alpha = float(node.attr("alpha", 0.2))
        beta = float(node.attr("beta", 0.5))

        def render(slot_of, _weight_of):
            return (
                "Op::HardSigmoid { "
                f"input: {slot_of[name]}, output: {slot_of[output]}, "
                f"alpha: {fl(alpha)}, beta: {fl(beta)} }}"
            )

        return Emitted("HardSigmoid", [name], [output], render)

    def _emit_softmax(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]
        axis = as_int(node.attr("axis", -1))

        def render(slot_of, _weight_of):
            return (
                f"Op::Softmax {{ input: {slot_of[name]}, output: {slot_of[output]}, axis: {axis} }}"
            )

        return Emitted("Softmax", [name], [output], render)

    def _emit_reduce_mean(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]
        axes = as_list(node.attr("axes", []))
        keep = bool(as_int(node.attr("keepdims", 1)))
        axes_text = ", ".join(str(int(a)) for a in axes)

        def render(slot_of, _weight_of):
            return (
                "Op::ReduceMean { "
                f"input: {slot_of[name]}, output: {slot_of[output]}, "
                f"axes: &[{axes_text}], keepdims: {str(keep).lower()} }}"
            )

        return Emitted("ReduceMean", [name], [output], render)

    def _emit_global_average(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]

        def render(slot_of, _weight_of):
            return (
                f"Op::GlobalAveragePool {{ input: {slot_of[name]}, output: {slot_of[output]} }}"
            )

        return Emitted("GlobalAveragePool", [name], [output], render)

    def _emit_pool(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]
        kernel = as_list(node.attr("kernel_shape"))
        stride = as_list(node.attr("strides", [1, 1]))
        pads = norm_pads(node.attr("pads", [0, 0, 0, 0]))
        auto_pad = norm_auto_pad(node)
        ceil_mode = bool(as_int(node.attr("ceil_mode", 0)))
        count_include_pad = bool(as_int(node.attr("count_include_pad", 0)))
        op = node.op_type
        if len(kernel) != 2 or len(stride) != 2:
            fail(f"{op} {node.name!r}: malformed kernel/stride")
        ceil_text = str(ceil_mode).lower()
        count_text = str(count_include_pad).lower()

        def render(slot_of, _weight_of):
            if op == "MaxPool":
                return (
                    "Op::MaxPool { "
                    f"input: {slot_of[name]}, output: {slot_of[output]}, "
                    f"kernel: [{kernel[0]}, {kernel[1]}], stride: [{stride[0]}, {stride[1]}], "
                    f"pads: [{pads[0]}, {pads[1]}, {pads[2]}, {pads[3]}], "
                    f"auto_pad: AutoPad::{auto_pad}, ceil_mode: {ceil_text} }}"
                )
            return (
                "Op::AveragePool { "
                f"input: {slot_of[name]}, output: {slot_of[output]}, "
                f"kernel: [{kernel[0]}, {kernel[1]}], stride: [{stride[0]}, {stride[1]}], "
                f"pads: [{pads[0]}, {pads[1]}, {pads[2]}, {pads[3]}], "
                f"auto_pad: AutoPad::{auto_pad}, ceil_mode: {ceil_text}, "
                f"count_include_pad: {count_text} }}"
            )

        return Emitted(op, [name], [output], render)

    def _emit_resize(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]
        mode = node.attr("mode", "nearest")
        coordinate = node.attr("coordinate_transformation_mode", "half_pixel")
        nearest = node.attr("nearest_mode", "round_prefer_floor")
        if mode != "nearest" or coordinate != "asymmetric" or nearest != "floor":
            fail(
                f"Resize {node.name!r}: unsupported mode "
                f"{mode!r}/{coordinate!r}/{nearest!r}"
            )
        scales, sizes = resize_constants(node, self.dims, self.constants)
        if sizes is not None:
            fail(f"Resize {node.name!r}: the sizes form is not implemented")
        # Resize takes X, roi, scales, sizes; roi only appears as an empty constant.
        if len(node.inputs) > 1 and node.inputs[1] != "":
            tensor = _TENSORS.get(node.inputs[1])
            if tensor is None or tensor.dtype != "f32" or tensor.numel > 8:
                fail(f"Resize {node.name!r}: unsupported roi input")

        def render(slot_of, _weight_of):
            return (
                "Op::ResizeNearest { "
                f"input: {slot_of[name]}, output: {slot_of[output]}, "
                f"scales: [{fl(scales[0])}, {fl(scales[1])}] }}"
            )

        return Emitted("Resize", [name], [output], render)

    def _emit_concat(self, node):
        names = [self._activation(node, i, "input") for i in range(len(node.inputs))]
        output = node.outputs[0]
        axis = as_int(node.attr("axis"))

        def render(slot_of, _weight_of):
            slots = ", ".join(str(slot_of[n]) for n in names)
            return (
                f"Op::Concat {{ inputs: &[{slots}], output: {slot_of[output]}, axis: {axis} }}"
            )

        return Emitted("Concat", names, [output], render)

    def _emit_transpose(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]
        shape = self.dims.get(name)
        rank = len(shape) if shape is not None else max(len(node.inputs), 0)
        perm = as_list(node.attr("perm", list(reversed(range(rank)))))
        perm = [int(p) + rank if int(p) < 0 else int(p) for p in perm]
        perm_text = ", ".join(str(p) for p in perm)

        def render(slot_of, _weight_of):
            return (
                f"Op::Transpose {{ input: {slot_of[name]}, output: {slot_of[output]}, "
                f"perm: &[{perm_text}] }}"
            )

        return Emitted("Transpose", [name], [output], render)

    def _emit_reshape(self, node):
        operand = self._operand(node, 0)
        output = node.outputs[0]
        target = reshape_target(node, self.constants)
        if target is None:
            fail(f"Reshape {node.name!r}: shape input is not a constant")
        shape_text = ", ".join(str(int(v)) for v in target)
        inputs = self._operand_inputs(operand)

        def render(slot_of, _weight_of):
            return (
                f"Op::Reshape {{ input: {self._render_operand(operand, slot_of)}, "
                f"output: {slot_of[output]}, shape: &[{shape_text}] }}"
            )

        return Emitted("Reshape", inputs, [output], render, True)

    def _emit_squeeze(self, node):
        return self._emit_axis_op(node)

    def _emit_unsqueeze(self, node):
        return self._emit_axis_op(node)

    def _emit_axis_op(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]
        shape = self.dims.get(name)
        rank = len(shape) if shape is not None else 4
        axes = squeeze_axes(node, self.constants, rank)
        axes_text = ", ".join(str(int(a)) for a in axes)
        op = node.op_type

        def render(slot_of, _weight_of):
            return (
                f"Op::{op} {{ input: {slot_of[name]}, output: {slot_of[output]}, "
                f"axes: &[{axes_text}] }}"
            )

        return Emitted(op, [name], [output], render)

    def _emit_identity(self, node):
        name = self._activation(node, 0, "input")
        output = node.outputs[0]

        def render(slot_of, _weight_of):
            return f"Op::Identity {{ input: {slot_of[name]}, output: {slot_of[output]} }}"

        return Emitted("Identity", [name], [output], render)

    # -- slot assignment ---------------------------------------------------
    def _assign_slots(self, graph):
        inputs = [vi.name for vi in graph.inputs if vi.name not in _TENSORS]
        if len(inputs) != 1:
            fail(f"{self.filename}: expected one data input")
        slot_of = {inputs[0]: 0}
        next_slot = 1
        free = []
        uses = {}
        for index, op in enumerate(self.emitted):
            for name in op.inputs:
                uses.setdefault(name, []).append(index)
        last_use = {}
        for name, indexes in uses.items():
            last_use[name] = max(indexes)
        for name in graph.outputs:
            if name.name in slot_of or any(op.outputs and name.name in op.outputs for op in self.emitted):
                last_use[name.name] = len(self.emitted)
        for index, op in enumerate(self.emitted):
            for output in op.outputs:
                if free:
                    slot_of[output] = free.pop()
                else:
                    slot_of[output] = next_slot
                    next_slot += 1
            for name in op.inputs:
                if last_use.get(name) == index:
                    free.append(slot_of[name])
        for name, _ in ((vi.name, None) for vi in graph.outputs):
            if name not in slot_of:
                fail(f"{self.filename}: graph output {name!r} has no slot")
        self.slot_of = slot_of
        self.slot_count = next_slot
        self.last_use = last_use

    def _check_outputs(self, graph):
        if not graph.outputs:
            fail(f"{self.filename}: no graph outputs")
        self.outputs = [vi.name for vi in graph.outputs]

    # -- rendering ---------------------------------------------------------
    def render(self):
        lines = []
        lines.append(f"//! Generated from `server/models/{self.filename}` by")
        lines.append("//! `tools/models/export_plan.py`; do not edit by hand.")
        lines.append("")
        lines.append("use crate::inference::plan::{AutoPad, ModelPlan, Op, Operand, WeightRef};")
        lines.append("")
        lines.append("#[rustfmt::skip]")
        lines.append("pub(super) static PLAN: ModelPlan = ModelPlan {")
        lines.append(f"    input_rank: {len(self.dims[self.input_name])},")
        lines.append(f"    input_channels: {self.dims[self.input_name][1]},")
        lines.append(f"    input_slot: {self.slot_of[self.input_name]},")
        lines.append("    ops: OPS,")
        lines.append(f"    slot_count: {self.slot_count},")
        lines.append("    outputs: OUTPUTS,")
        lines.append("    weights: WEIGHTS,")
        lines.append(f"    source_sha256: \"{self.model.sha256}\",")
        lines.append(
            "    bytes: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), "
            f"\"/models/{self.filename}\")),"
        )
        lines.append("};")
        lines.append("")
        outputs = ", ".join(str(self.slot_of[name]) for name in self.outputs)
        lines.append("#[rustfmt::skip]")
        lines.append(f"static OUTPUTS: &[u16] = &[{outputs}];")
        lines.append("")
        lines.append("#[rustfmt::skip]")
        lines.append("static WEIGHTS: &[WeightRef] = &[")
        for offset, length in self.weights:
            lines.append(f"    WeightRef {{ offset: {offset}, len: {length} }},")
        lines.append("];")
        lines.append("")
        lines.append("#[rustfmt::skip]")
        lines.append("static OPS: &[Op] = &[")
        for op in self.emitted:
            lines.append("    " + op.render(self.slot_of, self.weight_index) + ",")
        lines.append("];")
        lines.append("")
        return "\n".join(lines)


def fl(value):
    """A Rust f32 literal in the shortest form that round-trips through f32."""
    import struct

    value = struct.unpack("<f", struct.pack("<f", float(value)))[0]
    if value == 0.0:
        return "0.0f32"
    for digits in range(1, 10):
        text = format(value, f".{digits}g")
        if struct.unpack("<f", struct.pack("<f", float(text)))[0] == value:
            break
    lowered = text.lower()
    if "e" in lowered:
        mantissa, exponent = lowered.split("e")
        if "." not in mantissa:
            mantissa += ".0"
        text = f"{mantissa}e{int(exponent)}"
    elif "." not in text:
        text += ".0"
    return text + "f32"


def export_model(kind, module, filename):
    exporter = ModelExporter(kind, module, filename)
    exporter.prepare()
    text = exporter.render()
    return exporter, text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true",
                        help="fail instead of writing when a generated file is stale")
    parser.add_argument("--model", action="append", default=None,
                        help="only export this module name (repeatable)")
    args = parser.parse_args()

    selected = [m for m in PLAN_MODELS if args.model is None or m[1] in args.model]
    if not selected:
        raise SystemExit("no matching models")

    os.makedirs(OUT_DIR, exist_ok=True)
    module_entries = []
    failures = 0
    for kind, module, filename in selected:
        exporter, text = export_model(kind, module, filename)
        path = os.path.join(OUT_DIR, module + ".rs")
        old = ""
        if os.path.exists(path):
            old = open(path, encoding="utf-8").read()
        changed = old != text
        if args.check:
            if changed:
                print(f"stale: {os.path.relpath(path, ROOT)}")
                failures += 1
            else:
                print(f"ok:    {os.path.relpath(path, ROOT)}")
        else:
            if changed:
                open(path, "w", encoding="utf-8", newline="\n").write(text)
                print(f"wrote: {os.path.relpath(path, ROOT)}")
            else:
                print(f"same:  {os.path.relpath(path, ROOT)}")
        module_entries.append((kind, module, exporter))

    # The module registry is generated too, so a new model cannot be forgotten.
    registry = render_registry([(kind, module) for kind, module, _ in PLAN_MODELS])
    registry_path = os.path.join(OUT_DIR, "mod.rs")
    old = open(registry_path, encoding="utf-8").read() if os.path.exists(registry_path) else ""
    if args.check:
        if old != registry:
            print("stale: " + os.path.relpath(registry_path, ROOT))
            failures += 1
    elif old != registry:
        open(registry_path, "w", encoding="utf-8", newline="\n").write(registry)
        print("wrote: " + os.path.relpath(registry_path, ROOT))

    if args.check and failures:
        raise SystemExit(f"{failures} generated file(s) are stale; rerun without --check")


def render_registry(entries):
    lines = [
        "//! Generated model plans.",
        "//!",
        "//! Regenerate with `python3 tools/models/export_plan.py`.",
        "",
    ]
    for module in sorted(module for _, module in entries):
        lines.append(f"mod {module};")
    lines.append("")
    lines.append("use super::ModelKind;")
    lines.append("use super::plan::ModelPlan;")
    lines.append("")
    lines.append("pub(super) fn plan(kind: ModelKind) -> &'static ModelPlan {")
    lines.append("    match kind {")
    for kind, module in entries:
        lines.append(f"        ModelKind::{kind} => &{module}::PLAN,")
    lines.append("    }")
    lines.append("}")
    lines.append("")
    return "\n".join(lines)


if __name__ == "__main__":
    try:
        main()
    except ExportError as error:
        print(f"export failed: {error}", file=sys.stderr)
        raise SystemExit(1)
    except ParseError as error:
        print(f"onnx parse failed: {error}", file=sys.stderr)
        raise SystemExit(1)
