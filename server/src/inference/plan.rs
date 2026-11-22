//! Plan schema shared by the generated model descriptions and the executor.
//!
//! A plan is a flat, static description of one ONNX graph: the operator list in
//! topological order, the buffer slot every tensor lives in, the graph outputs,
//! and the weights (as byte ranges into the unchanged ONNX file). It carries no
//! names and no parser state, so loading a model is a copy of its weights and
//! nothing else.

/// One operand of a compute operator: either a workspace slot or a constant
/// weight tensor (a range in [`ModelPlan::weights`]).
pub(crate) enum Operand {
    Slot(u16),
    Weight { index: u16, dims: &'static [usize] },
}

/// The padding convention of a convolution or pooling node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoPad {
    /// Explicit `pads` in the node.
    NotSet,
    /// Pad at the end (ONNX `SAME_UPPER`).
    SameUpper,
    /// Pad at the beginning (ONNX `SAME_LOWER`).
    SameLower,
}

/// One node of the graph.
///
/// Slot numbers are workspace buffer indices, weight numbers index
/// [`ModelPlan::weights`]. Every tensor has a fixed slot, assigned by the
/// exporter's liveness analysis, so the executor never allocates per tensor.
pub(crate) enum Op {
    Conv {
        input: u16,
        weight: u16,
        bias: Option<u16>,
        output: u16,
        out_channels: u16,
        kernel: [u16; 2],
        stride: [u16; 2],
        dilation: [u16; 2],
        pads: [u16; 4],
        auto_pad: AutoPad,
        groups: u16,
    },
    ConvTranspose {
        input: u16,
        weight: u16,
        bias: Option<u16>,
        output: u16,
        in_channels: u16,
        out_channels: u16,
        kernel: [u16; 2],
        stride: [u16; 2],
        dilation: [u16; 2],
        pads: [u16; 4],
        output_padding: [u16; 2],
        groups: u16,
    },
    BatchNorm {
        input: u16,
        scale: u16,
        bias: u16,
        mean: u16,
        variance: u16,
        output: u16,
        epsilon: f32,
    },
    MatMul {
        lhs: Operand,
        rhs: Operand,
        output: u16,
    },
    Add {
        lhs: Operand,
        rhs: Operand,
        output: u16,
    },
    Mul {
        lhs: Operand,
        rhs: Operand,
        output: u16,
    },
    Div {
        lhs: Operand,
        rhs: Operand,
        output: u16,
    },
    Relu {
        input: u16,
        output: u16,
    },
    Sigmoid {
        input: u16,
        output: u16,
    },
    HardSigmoid {
        input: u16,
        output: u16,
        alpha: f32,
        beta: f32,
    },
    Erf {
        input: u16,
        output: u16,
    },
    Softmax {
        input: u16,
        output: u16,
        axis: i16,
    },
    ReduceMean {
        input: u16,
        output: u16,
        axes: &'static [i16],
        keepdims: bool,
    },
    GlobalAveragePool {
        input: u16,
        output: u16,
    },
    MaxPool {
        input: u16,
        output: u16,
        kernel: [u16; 2],
        stride: [u16; 2],
        pads: [u16; 4],
        auto_pad: AutoPad,
        ceil_mode: bool,
    },
    AveragePool {
        input: u16,
        output: u16,
        kernel: [u16; 2],
        stride: [u16; 2],
        pads: [u16; 4],
        auto_pad: AutoPad,
        ceil_mode: bool,
        count_include_pad: bool,
    },
    /// Resize with static scales (`out = floor(in * scale)`), nearest,
    /// asymmetric coordinates, nearest-mode `floor` -- the only combination the
    /// embedded graphs use.
    ResizeNearest {
        input: u16,
        output: u16,
        scales: [f32; 2],
    },
    Concat {
        inputs: &'static [u16],
        output: u16,
        axis: i16,
    },
    Transpose {
        input: u16,
        output: u16,
        perm: &'static [u16],
    },
    Reshape {
        input: Operand,
        output: u16,
        /// Shape with at most one `-1` and `0` meaning "copy the input dim".
        shape: &'static [i64],
    },
    Squeeze {
        input: u16,
        output: u16,
        axes: &'static [i16],
    },
    Unsqueeze {
        input: u16,
        output: u16,
        axes: &'static [i16],
    },
    Identity {
        input: u16,
        output: u16,
    },
}

/// A weight tensor: a byte range in [`ModelPlan::bytes`], f32 little-endian.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WeightRef {
    pub(crate) offset: u32,
    pub(crate) len: u32,
}

/// Everything the executor needs for one embedded model.
pub(crate) struct ModelPlan {
    pub(crate) input_rank: u8,
    pub(crate) input_channels: u8,
    /// Workspace slot the runtime copies the caller's input tensor into.
    pub(crate) input_slot: u16,
    pub(crate) ops: &'static [Op],
    pub(crate) slot_count: u16,
    pub(crate) outputs: &'static [u16],
    pub(crate) weights: &'static [WeightRef],
    pub(crate) source_sha256: &'static str,
    /// The unchanged ONNX file. Weight ranges index into it; the graph protobuf
    /// bytes themselves are never parsed at run time.
    pub(crate) bytes: &'static [u8],
}
