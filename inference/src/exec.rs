//! Executor: walks a [`ModelPlan`] once, computes every tensor's shape, and
//! dispatches to the kernels. The workspace slot buffers are reused between
//! runs; the exporter's liveness analysis decides which tensors share a slot.

use super::kernels::{self, BinOp, UnaryOp};
use super::plan::{AutoPad, ModelPlan, Op, Operand};
use super::tensor;

pub(super) struct Workspace {
    pub(super) slots: Vec<Vec<f32>>,
    pub(super) dims: Vec<Vec<usize>>,
}

impl Workspace {
    pub(super) fn new(slot_count: usize) -> Self {
        Self {
            slots: vec![Vec::new(); slot_count],
            dims: vec![Vec::new(); slot_count],
        }
    }
}

pub(super) fn run(
    plan: &ModelPlan,
    weights: &[f32],
    weight_starts: &[usize],
    workspace: &mut Workspace,
    input_dims: &[usize],
    input: &[f32],
) -> Result<(), String> {
    if workspace.slots.len() != plan.slot_count as usize
        || workspace.dims.len() != plan.slot_count as usize
    {
        return Err("inference: workspace size does not match the plan".to_string());
    }
    let input_slot = plan.input_slot as usize;
    let input_len: usize = input_dims.iter().product();
    if input.len() != input_len {
        return Err(format!(
            "inference: input has {} values for shape {input_dims:?}",
            input.len()
        ));
    }
    for dims in &mut workspace.dims {
        dims.clear();
    }
    workspace.dims[input_slot].extend_from_slice(input_dims);
    if workspace.slots[input_slot].len() < input_len {
        workspace.slots[input_slot].resize(input_len, 0.0);
    }
    workspace.slots[input_slot][..input_len].copy_from_slice(input);

    for op in plan.ops {
        execute(
            op,
            plan,
            weights,
            weight_starts,
            &mut workspace.slots,
            &mut workspace.dims,
        )?;
    }
    Ok(())
}

fn weight_slice<'a>(
    plan: &ModelPlan,
    weights: &'a [f32],
    weight_starts: &[usize],
    index: u16,
) -> &'a [f32] {
    let weight = &plan.weights[index as usize];
    let start = weight_starts[index as usize];
    let len = weight.len as usize / 4;
    &weights[start..start + len]
}

fn numel(dims: &[usize]) -> usize {
    dims.iter().product()
}

fn prepare_out(
    slots: &mut [Vec<f32>],
    dims: &mut [Vec<usize>],
    index: u16,
    out_dims: &[usize],
) -> usize {
    let index = index as usize;
    let len = numel(out_dims);
    dims[index].clear();
    dims[index].extend_from_slice(out_dims);
    if slots[index].len() < len {
        slots[index].resize(len, 0.0);
    }
    len
}

fn two_mut(slots: &mut [Vec<f32>], a: usize, b: usize) -> (&mut [f32], &mut [f32]) {
    assert_ne!(a, b, "inference plan aliases a slot with itself");
    let [first, second] = slots
        .get_disjoint_mut([a, b])
        .expect("slots are distinct and in range");
    (first.as_mut_slice(), second.as_mut_slice())
}

fn three_mut(
    slots: &mut [Vec<f32>],
    a: usize,
    b: usize,
    c: usize,
) -> (&mut [f32], &mut [f32], &mut [f32]) {
    assert!(a != b && a != c && b != c, "inference plan aliases slots");
    let [first, second, third] = slots
        .get_disjoint_mut([a, b, c])
        .expect("slots are distinct and in range");
    (
        first.as_mut_slice(),
        second.as_mut_slice(),
        third.as_mut_slice(),
    )
}

fn operand_dims(dims: &[Vec<usize>], operand: &Operand) -> Vec<usize> {
    match operand {
        Operand::Slot(index) => dims[*index as usize].clone(),
        Operand::Weight { dims, .. } => dims.to_vec(),
    }
}

/// The pads of a `SAME_*` operator, split the ONNX way, or the explicit ones.
fn resolve_pads(
    auto_pad: AutoPad,
    pads: [u16; 4],
    input: [usize; 2],
    kernel: [usize; 2],
    stride: [usize; 2],
    dilation: [usize; 2],
) -> [usize; 4] {
    match auto_pad {
        AutoPad::NotSet => [
            pads[0] as usize,
            pads[1] as usize,
            pads[2] as usize,
            pads[3] as usize,
        ],
        AutoPad::SameUpper | AutoPad::SameLower => {
            let lower = auto_pad == AutoPad::SameLower;
            let (top, bottom) =
                tensor::same_pads(input[0], kernel[0], stride[0], dilation[0], lower);
            let (left, right) =
                tensor::same_pads(input[1], kernel[1], stride[1], dilation[1], lower);
            [top, left, bottom, right]
        }
    }
}

fn execute_binary(
    kind: BinOp,
    lhs: &Operand,
    rhs: &Operand,
    output: u16,
    plan: &ModelPlan,
    weights: &[f32],
    weight_starts: &[usize],
    slots: &mut [Vec<f32>],
    dims: &mut [Vec<usize>],
) -> Result<(), String> {
    let lhs_dims = operand_dims(dims, lhs);
    let rhs_dims = operand_dims(dims, rhs);
    let out_dims = tensor::broadcast_shape(&lhs_dims, &rhs_dims)?;
    let out_len = prepare_out(slots, dims, output, &out_dims);
    let output = output as usize;
    match (lhs, rhs) {
        (Operand::Slot(a), Operand::Slot(b)) => {
            let (a, b, out) = three_mut(slots, *a as usize, *b as usize, output);
            kernels::elementwise::binary(
                kind,
                &a[..numel(&lhs_dims)],
                &lhs_dims,
                &b[..numel(&rhs_dims)],
                &rhs_dims,
                &mut out[..out_len],
                &out_dims,
            )
        }
        (Operand::Slot(a), Operand::Weight { index, dims: wdims }) => {
            let w = weight_slice(plan, weights, weight_starts, *index);
            let (a, out) = two_mut(slots, *a as usize, output);
            kernels::elementwise::binary(
                kind,
                &a[..numel(&lhs_dims)],
                &lhs_dims,
                &w[..numel(wdims)],
                &rhs_dims,
                &mut out[..out_len],
                &out_dims,
            )
        }
        (Operand::Weight { index, dims: wdims }, Operand::Slot(b)) => {
            let w = weight_slice(plan, weights, weight_starts, *index);
            let (b, out) = two_mut(slots, *b as usize, output);
            kernels::elementwise::binary(
                kind,
                &w[..numel(wdims)],
                &lhs_dims,
                &b[..numel(&rhs_dims)],
                &rhs_dims,
                &mut out[..out_len],
                &out_dims,
            )
        }
        (
            Operand::Weight {
                index: ia,
                dims: adims,
            },
            Operand::Weight {
                index: ib,
                dims: bdims,
            },
        ) => {
            let a = weight_slice(plan, weights, weight_starts, *ia);
            let b = weight_slice(plan, weights, weight_starts, *ib);
            let out = &mut slots[output][..out_len];
            kernels::elementwise::binary(
                kind,
                &a[..numel(adims)],
                &lhs_dims,
                &b[..numel(bdims)],
                &rhs_dims,
                out,
                &out_dims,
            )
        }
    }
}

fn execute_unary(
    kind: UnaryOp,
    input: u16,
    output: u16,
    dims: &mut [Vec<usize>],
    slots: &mut [Vec<f32>],
) -> Result<(), String> {
    let in_dims = dims[input as usize].clone();
    let out_len = prepare_out(slots, dims, output, &in_dims);
    let (input_slot, output_slot) = two_mut(slots, input as usize, output as usize);
    kernels::elementwise::unary(kind, &input_slot[..out_len], &mut output_slot[..out_len]);
    Ok(())
}

fn execute(
    op: &Op,
    plan: &ModelPlan,
    weights: &[f32],
    weight_starts: &[usize],
    slots: &mut [Vec<f32>],
    dims: &mut [Vec<usize>],
) -> Result<(), String> {
    match op {
        Op::Add { lhs, rhs, output } => execute_binary(
            BinOp::Add,
            lhs,
            rhs,
            *output,
            plan,
            weights,
            weight_starts,
            slots,
            dims,
        ),
        Op::Mul { lhs, rhs, output } => execute_binary(
            BinOp::Mul,
            lhs,
            rhs,
            *output,
            plan,
            weights,
            weight_starts,
            slots,
            dims,
        ),
        Op::Div { lhs, rhs, output } => execute_binary(
            BinOp::Div,
            lhs,
            rhs,
            *output,
            plan,
            weights,
            weight_starts,
            slots,
            dims,
        ),
        Op::Relu { input, output } => execute_unary(UnaryOp::Relu, *input, *output, dims, slots),
        Op::Sigmoid { input, output } => {
            execute_unary(UnaryOp::Sigmoid, *input, *output, dims, slots)
        }
        Op::Erf { input, output } => execute_unary(UnaryOp::Erf, *input, *output, dims, slots),
        Op::HardSigmoid {
            input,
            output,
            alpha,
            beta,
        } => {
            let in_dims = dims[*input as usize].clone();
            let out_len = prepare_out(slots, dims, *output, &in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::elementwise::hard_sigmoid(
                &input_slot[..out_len],
                &mut output_slot[..out_len],
                *alpha,
                *beta,
            );
            Ok(())
        }
        Op::Conv {
            input,
            weight,
            bias,
            output,
            out_channels,
            kernel,
            stride,
            dilation,
            pads,
            auto_pad,
            groups,
        } => {
            let in_dims = dims[*input as usize].clone();
            if in_dims.len() != 4 {
                return Err("conv: expected rank-4 input".to_string());
            }
            let kernel = [kernel[0] as usize, kernel[1] as usize];
            let stride = [stride[0] as usize, stride[1] as usize];
            let dilation = [dilation[0] as usize, dilation[1] as usize];
            let pads = resolve_pads(
                *auto_pad,
                *pads,
                [in_dims[2], in_dims[3]],
                kernel,
                stride,
                dilation,
            );
            let oh = tensor::conv_output(
                in_dims[2],
                kernel[0],
                stride[0],
                dilation[0],
                pads[0],
                pads[2],
            );
            let ow = tensor::conv_output(
                in_dims[3],
                kernel[1],
                stride[1],
                dilation[1],
                pads[1],
                pads[3],
            );
            let out_dims = vec![1, *out_channels as usize, oh, ow];
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let weight = weight_slice(plan, weights, weight_starts, *weight);
            let bias = bias.map(|index| weight_slice(plan, weights, weight_starts, index));
            let spec = kernels::conv::ConvSpec {
                kernel,
                stride,
                dilation,
                pads,
                groups: *groups as usize,
            };
            let in_len = numel(&in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::conv::conv(
                &input_slot[..in_len],
                &in_dims,
                weight,
                bias,
                &mut output_slot[..out_len],
                &out_dims,
                &spec,
            )
        }
        Op::ConvTranspose {
            input,
            weight,
            bias,
            output,
            in_channels,
            out_channels,
            kernel,
            stride,
            dilation,
            pads,
            output_padding,
            groups,
        } => {
            let in_dims = dims[*input as usize].clone();
            if in_dims.len() != 4 {
                return Err("conv_transpose: expected rank-4 input".to_string());
            }
            let kernel = [kernel[0] as usize, kernel[1] as usize];
            let stride = [stride[0] as usize, stride[1] as usize];
            let dilation = [dilation[0] as usize, dilation[1] as usize];
            let pads = [
                pads[0] as usize,
                pads[1] as usize,
                pads[2] as usize,
                pads[3] as usize,
            ];
            let output_padding = [output_padding[0] as usize, output_padding[1] as usize];
            let oh = (in_dims[2] - 1) * stride[0] - pads[0] - pads[2]
                + dilation[0] * (kernel[0] - 1)
                + 1
                + output_padding[0];
            let ow = (in_dims[3] - 1) * stride[1] - pads[1] - pads[3]
                + dilation[1] * (kernel[1] - 1)
                + 1
                + output_padding[1];
            let out_dims = vec![1, *out_channels as usize, oh, ow];
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let weight = weight_slice(plan, weights, weight_starts, *weight);
            if weight.len()
                != *in_channels as usize * *out_channels as usize * kernel[0] * kernel[1]
            {
                return Err("conv_transpose: weight length mismatch".to_string());
            }
            let bias = bias.map(|index| weight_slice(plan, weights, weight_starts, index));
            let spec = kernels::conv::ConvSpec {
                kernel,
                stride,
                dilation,
                pads,
                groups: *groups as usize,
            };
            let in_len = numel(&in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::conv::conv_transpose(
                &input_slot[..in_len],
                &in_dims,
                weight,
                bias,
                &mut output_slot[..out_len],
                &out_dims,
                &spec,
            )
        }
        Op::BatchNorm {
            input,
            scale,
            bias,
            mean,
            variance,
            output,
            epsilon,
        } => {
            let in_dims = dims[*input as usize].clone();
            let out_len = prepare_out(slots, dims, *output, &in_dims);
            let scale = weight_slice(plan, weights, weight_starts, *scale);
            let bias = weight_slice(plan, weights, weight_starts, *bias);
            let mean = weight_slice(plan, weights, weight_starts, *mean);
            let variance = weight_slice(plan, weights, weight_starts, *variance);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::elementwise::batch_norm(
                &input_slot[..out_len],
                &in_dims,
                scale,
                bias,
                mean,
                variance,
                &mut output_slot[..out_len],
                *epsilon,
            )
        }
        Op::MatMul { lhs, rhs, output } => {
            let lhs_dims = operand_dims(dims, lhs);
            let rhs_dims = operand_dims(dims, rhs);
            if rhs_dims.len() != 2 || lhs_dims.is_empty() {
                return Err("matmul: unsupported operand ranks".to_string());
            }
            let mut out_dims = lhs_dims[..lhs_dims.len() - 1].to_vec();
            out_dims.push(rhs_dims[1]);
            if lhs_dims[lhs_dims.len() - 1] != rhs_dims[0] {
                return Err(format!(
                    "matmul: {lhs_dims:?} x {rhs_dims:?} inner dimensions differ"
                ));
            }
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let batches: usize = lhs_dims[..lhs_dims.len() - 2].iter().product();
            let m = lhs_dims[lhs_dims.len() - 2];
            let k = rhs_dims[0];
            let n = rhs_dims[1];
            let lhs_len = batches * m * k;
            let rhs_len = k * n;
            let output = *output as usize;
            match (lhs, rhs) {
                (Operand::Slot(a), Operand::Slot(b)) => {
                    let (a, b, out) = three_mut(slots, *a as usize, *b as usize, output);
                    matmul_slices(
                        &a[..lhs_len],
                        &b[..rhs_len],
                        &mut out[..out_len],
                        batches,
                        m,
                        k,
                        n,
                    )
                }
                (Operand::Slot(a), Operand::Weight { index, .. }) => {
                    let w = weight_slice(plan, weights, weight_starts, *index);
                    let (a, out) = two_mut(slots, *a as usize, output);
                    matmul_slices(
                        &a[..lhs_len],
                        &w[..rhs_len],
                        &mut out[..out_len],
                        batches,
                        m,
                        k,
                        n,
                    )
                }
                (Operand::Weight { index, .. }, Operand::Slot(b)) => {
                    let w = weight_slice(plan, weights, weight_starts, *index);
                    let (b, out) = two_mut(slots, *b as usize, output);
                    matmul_slices(
                        &w[..lhs_len],
                        &b[..rhs_len],
                        &mut out[..out_len],
                        batches,
                        m,
                        k,
                        n,
                    )
                }
                (Operand::Weight { index: ia, .. }, Operand::Weight { index: ib, .. }) => {
                    let a = weight_slice(plan, weights, weight_starts, *ia);
                    let b = weight_slice(plan, weights, weight_starts, *ib);
                    let out = &mut slots[output][..out_len];
                    matmul_slices(&a[..lhs_len], &b[..rhs_len], out, batches, m, k, n)
                }
            }
        }
        Op::Softmax {
            input,
            output,
            axis,
        } => {
            let in_dims = dims[*input as usize].clone();
            let out_len = prepare_out(slots, dims, *output, &in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::reduce::softmax(
                &input_slot[..out_len],
                &in_dims,
                &mut output_slot[..out_len],
                *axis,
            )
        }
        Op::ReduceMean {
            input,
            output,
            axes,
            keepdims,
        } => {
            let in_dims = dims[*input as usize].clone();
            let rank = in_dims.len();
            let axes: Vec<usize> = axes
                .iter()
                .map(|&axis| {
                    let axis = if axis < 0 { axis + rank as i16 } else { axis };
                    if axis < 0 || axis as usize >= rank {
                        return Err(format!("reduce_mean: axis {axis} out of range"));
                    }
                    Ok(axis as usize)
                })
                .collect::<Result<_, String>>()?;
            let mut out_dims = Vec::new();
            for (index, &size) in in_dims.iter().enumerate() {
                if axes.contains(&index) {
                    if *keepdims {
                        out_dims.push(1);
                    }
                } else {
                    out_dims.push(size);
                }
            }
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let in_len = numel(&in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::reduce::reduce_mean(
                &input_slot[..in_len],
                &in_dims,
                &mut output_slot[..out_len],
                &axes,
                *keepdims,
            )
        }
        Op::GlobalAveragePool { input, output } => {
            let in_dims = dims[*input as usize].clone();
            if in_dims.len() != 4 {
                return Err("global_average_pool: expected rank 4".to_string());
            }
            let out_dims = vec![in_dims[0], in_dims[1], 1, 1];
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::reduce::global_average_pool(
                &input_slot[..numel(&in_dims)],
                &in_dims,
                &mut output_slot[..out_len],
            )
        }
        Op::MaxPool {
            input,
            output,
            kernel,
            stride,
            pads,
            auto_pad,
            ceil_mode,
        } => {
            let in_dims = dims[*input as usize].clone();
            let kernel = [kernel[0] as usize, kernel[1] as usize];
            let stride = [stride[0] as usize, stride[1] as usize];
            let pads = resolve_pads(
                *auto_pad,
                *pads,
                [in_dims[2], in_dims[3]],
                kernel,
                stride,
                [1, 1],
            );
            let oh = pool_output(
                in_dims[2], kernel[0], stride[0], pads[0], pads[2], *ceil_mode,
            );
            let ow = pool_output(
                in_dims[3], kernel[1], stride[1], pads[1], pads[3], *ceil_mode,
            );
            let out_dims = vec![in_dims[0], in_dims[1], oh, ow];
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let spec = kernels::pool::PoolSpec {
                kernel,
                stride,
                pads,
                ceil_mode: *ceil_mode,
            };
            let in_len = numel(&in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::pool::max_pool(
                &input_slot[..in_len],
                &in_dims,
                &mut output_slot[..out_len],
                &out_dims,
                &spec,
            )
        }
        Op::AveragePool {
            input,
            output,
            kernel,
            stride,
            pads,
            auto_pad,
            ceil_mode,
            count_include_pad,
        } => {
            let in_dims = dims[*input as usize].clone();
            let kernel = [kernel[0] as usize, kernel[1] as usize];
            let stride = [stride[0] as usize, stride[1] as usize];
            let pads = resolve_pads(
                *auto_pad,
                *pads,
                [in_dims[2], in_dims[3]],
                kernel,
                stride,
                [1, 1],
            );
            let oh = pool_output(
                in_dims[2], kernel[0], stride[0], pads[0], pads[2], *ceil_mode,
            );
            let ow = pool_output(
                in_dims[3], kernel[1], stride[1], pads[1], pads[3], *ceil_mode,
            );
            let out_dims = vec![in_dims[0], in_dims[1], oh, ow];
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let spec = kernels::pool::PoolSpec {
                kernel,
                stride,
                pads,
                ceil_mode: *ceil_mode,
            };
            let in_len = numel(&in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::pool::average_pool(
                &input_slot[..in_len],
                &in_dims,
                &mut output_slot[..out_len],
                &out_dims,
                &spec,
                *count_include_pad,
            )
        }
        Op::ResizeNearest {
            input,
            output,
            scales,
        } => {
            let in_dims = dims[*input as usize].clone();
            let oh = ((in_dims[2] as f32 * scales[0]).floor() as usize).max(1);
            let ow = ((in_dims[3] as f32 * scales[1]).floor() as usize).max(1);
            let out_dims = vec![in_dims[0], in_dims[1], oh, ow];
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let in_len = numel(&in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::resize::resize_nearest(
                &input_slot[..in_len],
                &in_dims,
                &mut output_slot[..out_len],
                &out_dims,
                scales[0],
                scales[1],
            )
        }
        Op::Concat {
            inputs,
            output,
            axis,
        } => {
            let first_dims = dims[inputs[0] as usize].clone();
            let rank = first_dims.len();
            let axis = if *axis < 0 {
                *axis + rank as i16
            } else {
                *axis
            };
            if axis < 0 || axis as usize >= rank {
                return Err("concat: axis out of range".to_string());
            }
            let axis = axis as usize;
            let outer: usize = first_dims[..axis].iter().product();
            let inner: usize = first_dims[axis + 1..].iter().product();
            let out_axis: usize = inputs.iter().map(|index| dims[*index as usize][axis]).sum();
            let mut out_dims = first_dims.clone();
            out_dims[axis] = out_axis;
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let output_slot = *output as usize;
            let mut offset = 0usize;
            for index in inputs.iter() {
                let source_dims = dims[*index as usize].clone();
                let source_len = numel(&source_dims);
                let source_axis = source_dims[axis];
                let (source_slot, out) = two_mut(slots, *index as usize, output_slot);
                kernels::layout::concat_copy(
                    &mut out[..out_len],
                    &source_slot[..source_len],
                    outer,
                    source_axis,
                    out_axis,
                    inner,
                    offset,
                );
                offset += source_axis;
            }
            Ok(())
        }
        Op::Transpose {
            input,
            output,
            perm,
        } => {
            let in_dims = dims[*input as usize].clone();
            let perm: Vec<usize> = perm.iter().map(|&p| p as usize).collect();
            let mut out_dims = vec![0usize; in_dims.len()];
            for (index, &p) in perm.iter().enumerate() {
                out_dims[index] = in_dims[p];
            }
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let in_len = numel(&in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::layout::transpose(
                &input_slot[..in_len],
                &in_dims,
                &mut output_slot[..out_len],
                &perm,
            )
        }
        Op::Reshape {
            input,
            output,
            shape,
        } => {
            let in_dims = operand_dims(dims, input);
            let out_dims = reshape_dims(shape, &in_dims)?;
            if numel(&in_dims) != numel(&out_dims) {
                return Err(format!(
                    "reshape: {in_dims:?} has {} values, target {out_dims:?} has {}",
                    numel(&in_dims),
                    numel(&out_dims)
                ));
            }
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let in_len = numel(&in_dims);
            let output_slot = *output as usize;
            match input {
                Operand::Slot(index) => {
                    let (source, out) = two_mut(slots, *index as usize, output_slot);
                    kernels::layout::copy_contiguous(&source[..in_len], &mut out[..out_len])
                }
                Operand::Weight { index, .. } => {
                    let source = weight_slice(plan, weights, weight_starts, *index);
                    let out = &mut slots[output_slot][..out_len];
                    kernels::layout::copy_contiguous(&source[..in_len], out)
                }
            }
        }
        Op::Squeeze {
            input,
            output,
            axes,
        } => {
            let in_dims = dims[*input as usize].clone();
            let axes = normalise_axes(axes, in_dims.len())?;
            let out_dims: Vec<usize> = in_dims
                .iter()
                .enumerate()
                .filter(|(index, _)| !axes.contains(index))
                .map(|(_, &size)| size)
                .collect();
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::layout::copy_contiguous(&input_slot[..out_len], &mut output_slot[..out_len])
        }
        Op::Unsqueeze {
            input,
            output,
            axes,
        } => {
            let in_dims = dims[*input as usize].clone();
            let axes = normalise_axes(axes, in_dims.len())?;
            let mut out_dims = in_dims.clone();
            for axis in axes {
                out_dims.insert(axis, 1);
            }
            let out_len = prepare_out(slots, dims, *output, &out_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::layout::copy_contiguous(&input_slot[..out_len], &mut output_slot[..out_len])
        }
        Op::Identity { input, output } => {
            let in_dims = dims[*input as usize].clone();
            let out_len = prepare_out(slots, dims, *output, &in_dims);
            let (input_slot, output_slot) = two_mut(slots, *input as usize, *output as usize);
            kernels::layout::copy_contiguous(&input_slot[..out_len], &mut output_slot[..out_len])
        }
    }
}

fn matmul_slices(
    lhs: &[f32],
    rhs: &[f32],
    out: &mut [f32],
    batches: usize,
    m: usize,
    k: usize,
    n: usize,
) -> Result<(), String> {
    if lhs.len() < batches * m * k || rhs.len() < k * n || out.len() < batches * m * n {
        return Err("matmul: buffer too small".to_string());
    }
    for batch in 0..batches {
        let lhs = &lhs[batch * m * k..(batch + 1) * m * k];
        let out = &mut out[batch * m * n..(batch + 1) * m * n];
        kernels::gemm::sgemm(m, k, n, lhs, rhs, out);
    }
    Ok(())
}

fn normalise_axes(axes: &[i16], rank: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::with_capacity(axes.len());
    for &axis in axes {
        let axis = if axis < 0 { axis + rank as i16 } else { axis };
        if axis < 0 || axis as usize >= rank {
            return Err(format!("axis {axis} out of range for rank {rank}"));
        }
        out.push(axis as usize);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn reshape_dims(shape: &[i64], in_dims: &[usize]) -> Result<Vec<usize>, String> {
    let mut out = Vec::with_capacity(shape.len());
    let mut inferred = None;
    for (index, &value) in shape.iter().enumerate() {
        if value == 0 {
            let size = *in_dims
                .get(index)
                .ok_or_else(|| "reshape: 0 refers past the input rank".to_string())?;
            out.push(size);
        } else if value == -1 {
            if inferred.is_some() {
                return Err("reshape: more than one -1".to_string());
            }
            inferred = Some(out.len());
            out.push(1);
        } else if value < 0 {
            return Err("reshape: negative dimension".to_string());
        } else {
            out.push(value as usize);
        }
    }
    if let Some(index) = inferred {
        let mut known = 1usize;
        for (position, &size) in out.iter().enumerate() {
            if position != index {
                known *= size;
            }
        }
        let total = numel(in_dims);
        if known != 0 && total.is_multiple_of(known) {
            out[index] = total / known;
        } else {
            return Err("reshape: cannot infer -1".to_string());
        }
    }
    Ok(out)
}

fn pool_output(
    input: usize,
    kernel: usize,
    stride: usize,
    pad_begin: usize,
    pad_end: usize,
    ceil: bool,
) -> usize {
    let numerator = (input + pad_begin + pad_end).saturating_sub(kernel);
    if ceil {
        numerator.div_ceil(stride) + 1
    } else {
        numerator / stride + 1
    }
}
