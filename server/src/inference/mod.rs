//! Fixed-model inference runtime for the graphs compiled into the binary.
//!
//! The models are frozen, so the runtime is not an ONNX interpreter: the build-
//! time exporter (`tools/models/export_plan.py`) turns each graph into a static
//! plan of flat operators and byte ranges of the weights, and this module only
//! copies the weights and executes the plan. There is no parser, no graph IR and
//! no dynamic operator registry in the binary.
//!
//! The plans live in `generated/` and are regenerated from the unchanged ONNX
//! files with `python3 tools/models/export_plan.py`; `tests` verifies that each
//! plan still matches its ONNX file's SHA-256.
//!
//! Execution is single-threaded and synchronous. A [`Model`] owns one workspace
//! of reusable buffers; callers reuse the same `Vec<Tensor>` for the outputs so
//! steady-state inference does not allocate.

mod exec;
mod generated;
mod kernels;
mod plan;
mod tensor;

pub(crate) use tensor::Tensor;

/// One of the models embedded in the binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelKind {
    /// Face detection (YuNet), 1x3x640x640.
    YuNet,
    /// OCR text detection (PP-OCR det), 1x3xHxW with H/W multiples of 32.
    OcrDet,
    /// OCR text recognition (PP-OCR rec), 1x3x48xW.
    OcrRec,
    /// Document orientation classifier (LCNet x1.0), 1x3x224x224.
    DocOri,
    /// Text-line orientation classifier (LCNet x0.25), 1x3x80x160.
    TextlineOri,
}

/// One loaded model: its plan, the copied weights, and a reusable workspace.
pub(crate) struct Model {
    plan: &'static plan::ModelPlan,
    /// The weight tensors concatenated in plan order.
    weights: Vec<f32>,
    /// Where each weight tensor starts in `weights`, in elements.
    weight_starts: Vec<usize>,
    workspace: exec::Workspace,
}

impl Model {
    /// Load one embedded model. The ONNX bytes are already in the binary; this
    /// validates the weight ranges and copies the weights into a contiguous
    /// `f32` buffer. It does not parse the graph.
    pub(crate) fn load(kind: ModelKind) -> Result<Self, String> {
        let plan = generated::plan(kind);
        if plan.source_sha256.len() != 64
            || !plan
                .source_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("inference: the generated plan has no valid source digest".to_string());
        }
        let mut elements = 0usize;
        for weight in plan.weights {
            let end = weight.offset as usize + weight.len as usize;
            if end > plan.bytes.len() {
                return Err(format!(
                    "inference: weight range {}..{} is outside the model bytes",
                    weight.offset, end
                ));
            }
            if weight.len % 4 != 0 {
                return Err("inference: weight range is not f32-aligned".to_string());
            }
            elements += weight.len as usize / 4;
        }
        let mut weights = Vec::with_capacity(elements);
        let mut weight_starts = Vec::with_capacity(plan.weights.len());
        for weight in plan.weights {
            weight_starts.push(weights.len());
            let start = weight.offset as usize;
            let end = start + weight.len as usize;
            let (chunks, remainder) = plan.bytes[start..end].as_chunks::<4>();
            debug_assert!(remainder.is_empty(), "weight range is f32-aligned");
            for chunk in chunks {
                weights.push(f32::from_le_bytes(*chunk));
            }
        }
        Ok(Self {
            plan,
            weights,
            weight_starts,
            workspace: exec::Workspace::new(plan.slot_count as usize),
        })
    }

    /// Run the plan on one NCHW input and fill `outputs`.
    ///
    /// `dims` must be the concrete input shape (batch 1, 3 channels); `input` is
    /// tightly packed in NCHW order. The `outputs` vector is resized to the
    /// model's output count and each tensor's buffers are reused.
    pub(crate) fn run(
        &mut self,
        dims: &[usize],
        input: &[f32],
        outputs: &mut Vec<Tensor>,
    ) -> Result<(), String> {
        if dims.len() != self.plan.input_rank as usize {
            return Err(format!(
                "inference: input rank {} does not match the plan's {}",
                dims.len(),
                self.plan.input_rank
            ));
        }
        if dims[0] != 1 || dims[1] != self.plan.input_channels as usize {
            return Err(format!(
                "inference: unsupported input shape {dims:?} (batch 1, {} channels)",
                self.plan.input_channels
            ));
        }
        if dims.contains(&0) {
            return Err(format!("inference: zero-sized input dimension in {dims:?}"));
        }
        exec::run(
            self.plan,
            &self.weights,
            &self.weight_starts,
            &mut self.workspace,
            dims,
            input,
        )?;
        let count = self.plan.outputs.len();
        while outputs.len() < count {
            outputs.push(Tensor::default());
        }
        outputs.truncate(count);
        for (tensor, slot) in outputs.iter_mut().zip(self.plan.outputs) {
            let slot = *slot as usize;
            let dims = self.workspace.dims[slot].clone();
            let len = Tensor::numel(&dims);
            tensor.resize(&dims);
            tensor.data.clear();
            tensor
                .data
                .extend_from_slice(&self.workspace.slots[slot][..len]);
        }
        Ok(())
    }

    /// The static plan, for tests and diagnostics.
    #[cfg(test)]
    pub(crate) fn plan(&self) -> &'static plan::ModelPlan {
        self.plan
    }
}

/// The shape of one output.
pub(crate) fn shape(tensor: &Tensor) -> Result<Vec<usize>, String> {
    Ok(tensor.dims.clone())
}

/// The `f32` values of one output.
pub(crate) fn values(tensor: &Tensor) -> Result<&[f32], String> {
    Ok(&tensor.data)
}

#[cfg(test)]
mod tests;
