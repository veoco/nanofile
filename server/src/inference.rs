//! Shared tract loading for the embedded inference graphs.
//!
//! `tract` is the only inference runtime in the server: it parses the ONNX
//! bytes that are compiled into the binary, specialises them for one concrete
//! input shape and runs them on the pure-Rust CPU runtime. Nothing here reads a
//! file, starts a process or links a C library.
//!
//! The Paddle ONNX exports carry value-info shapes that do not describe the
//! graph, so the loader ignores them and each caller states the input fact it
//! needs. A plan is cheap to prepare relative to a model, but not free: callers
//! that run the same graph at several sizes keep one plan per size.

use tract::prelude::*;

/// One embedded ONNX graph and the runtime its plans are prepared on.
///
/// `tract`'s inference model is not `Clone` (its operator boxes are not), so each
/// concrete input size re-parses the graph. That is the same shape the upstream
/// PP-OCR example uses; callers cache the resulting plans per size.
pub(crate) struct OnnxModel {
    bytes: &'static [u8],
    runtime: tract::Runtime,
}

impl OnnxModel {
    /// Resolve the CPU runtime. The graph itself is parsed by [`Self::plan`].
    pub(crate) fn load(onnx: &'static [u8]) -> Result<Self, String> {
        let runtime = tract::runtime_for_name("cpu").map_err(error)?;
        Ok(Self {
            bytes: onnx,
            runtime,
        })
    }

    /// Prepare a runnable plan for one concrete input shape, e.g. `1,3,640,640,f32`.
    ///
    /// Every output fact is discarded: the graph's declared output shapes are
    /// as unreliable as its input ones, and tract infers them from the actual
    /// input fact.
    pub(crate) fn plan(&self, input_spec: &str) -> Result<tract::Runnable, String> {
        let mut model = tract::onnx()
            .map_err(error)?
            .load_buffer_with_options(self.bytes, OnnxOptions::new().ignore_value_info(true))
            .map_err(error)?;
        model.set_input_fact(0, input_spec).map_err(error)?;
        let outputs = model.output_count().map_err(error)?;
        for output in 0..outputs {
            model.set_output_fact(output, ()).map_err(error)?;
        }
        let typed = model.into_model().map_err(error)?;
        self.runtime.prepare(typed).map_err(error)
    }
}

/// Run one plan on a tightly packed `f32` tensor.
pub(crate) fn run(
    plan: &tract::Runnable,
    dims: &[usize],
    data: &[f32],
) -> Result<Vec<tract::Tensor>, String> {
    let input = tract::Tensor::from_slice(dims, data).map_err(error)?;
    plan.run([input]).map_err(error)
}

/// The shape of one output.
pub(crate) fn shape(tensor: &tract::Tensor) -> Result<Vec<usize>, String> {
    tensor.shape().map(|shape| shape.to_vec()).map_err(error)
}

/// The `f32` values of one host output.
pub(crate) fn values(tensor: &tract::Tensor) -> Result<&[f32], String> {
    tensor.as_slice::<f32>().map_err(error)
}

/// One error line, with tract's context chain.
fn error(error: impl std::fmt::Display) -> String {
    format!("{error:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_model_is_an_error_not_a_panic() {
        let model = OnnxModel::load(b"not an onnx graph").expect("cpu runtime");
        assert!(model.plan("1,3,1,1,f32").is_err());
    }
}
