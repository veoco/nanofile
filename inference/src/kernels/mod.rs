//! Numeric kernels for the fixed-model runtime.
//!
//! Every kernel is a plain safe function over borrowed slices; the only unsafe
//! block in the runtime is the `matrixmultiply` call in [`gemm`]. Shapes are
//! passed explicitly because the executor computes them one operator ahead.

pub(crate) mod conv;
pub(crate) mod elementwise;
pub(crate) mod gemm;
pub(crate) mod layout;
pub(crate) mod pool;
pub(crate) mod reduce;
pub(crate) mod resize;

/// Elementwise binary operator.
#[derive(Clone, Copy, Debug)]
pub(crate) enum BinOp {
    Add,
    Mul,
    Div,
}

/// Elementwise unary operator with no parameters.
#[derive(Clone, Copy, Debug)]
pub(crate) enum UnaryOp {
    Relu,
    Sigmoid,
    Erf,
}
