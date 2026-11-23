//! Dense `f32` tensors in NCHW layout and the shape arithmetic the executor
//! needs.

/// A dense f32 tensor with its dimensions. Rank is at most 4 in these graphs.
#[derive(Clone, Debug, Default)]
pub struct Tensor {
    pub(crate) dims: Vec<usize>,
    pub(crate) data: Vec<f32>,
}

impl Tensor {
    pub(crate) fn numel(dims: &[usize]) -> usize {
        dims.iter().product()
    }

    /// Resize to `dims`, reusing the allocation. Only grows.
    pub(crate) fn resize(&mut self, dims: &[usize]) {
        self.dims.clear();
        self.dims.extend_from_slice(dims);
        let len = Self::numel(dims);
        if self.data.len() < len {
            self.data.resize(len, 0.0);
        }
    }
}

/// ONNX NCHW convolution output size.
pub(crate) fn conv_output(
    input: usize,
    kernel: usize,
    stride: usize,
    dilation: usize,
    pad_begin: usize,
    pad_end: usize,
) -> usize {
    let window = dilation * (kernel - 1) + 1;
    if input + pad_begin + pad_end < window {
        return 0;
    }
    (input + pad_begin + pad_end - window) / stride + 1
}

/// Total padding of a `SAME_*` convolution/pooling axis, split the ONNX way.
pub(crate) fn same_pads(
    input: usize,
    kernel: usize,
    stride: usize,
    dilation: usize,
    lower: bool,
) -> (usize, usize) {
    let window = dilation * (kernel - 1) + 1;
    let output = input.div_ceil(stride);
    let total = ((output - 1) * stride + window).saturating_sub(input);
    if lower {
        (total - total / 2, total / 2)
    } else {
        (total / 2, total - total / 2)
    }
}

/// Right-aligned NumPy broadcasting.
pub(crate) fn broadcast_shape(a: &[usize], b: &[usize]) -> Result<Vec<usize>, String> {
    let rank = a.len().max(b.len());
    let mut out = vec![1usize; rank];
    for i in 0..rank {
        let da = if i < rank - a.len() {
            1
        } else {
            a[i - (rank - a.len())]
        };
        let db = if i < rank - b.len() {
            1
        } else {
            b[i - (rank - b.len())]
        };
        if da != db && da != 1 && db != 1 {
            return Err(format!("cannot broadcast {a:?} and {b:?}"));
        }
        out[i] = da.max(db);
    }
    Ok(out)
}

/// Strides for iterating a tensor of `dims` over the shape `over` (row-major),
/// with 0-stride for broadcast dimensions.
pub(crate) fn broadcast_strides(dims: &[usize], over: &[usize]) -> Vec<usize> {
    let rank = over.len();
    let mut strides = vec![0usize; rank];
    let mut stride = 1usize;
    for i in (0..dims.len()).rev() {
        let target = rank - (dims.len() - i);
        if dims[i] == over[target] {
            strides[target] = stride;
        }
        stride *= dims[i];
    }
    strides
}
