//! Elementwise kernels: broadcasting binary ops, activations and batch
//! normalisation.

use super::{BinOp, UnaryOp};
use crate::tensor::broadcast_strides;

pub(crate) fn binary(
    op: BinOp,
    a: &[f32],
    a_dims: &[usize],
    b: &[f32],
    b_dims: &[usize],
    out: &mut [f32],
    out_dims: &[usize],
) -> Result<(), String> {
    let len: usize = out_dims.iter().product();
    if out.len() < len {
        return Err("binary: output buffer too small".to_string());
    }
    let apply = |x: f32, y: f32| match op {
        BinOp::Add => x + y,
        BinOp::Mul => x * y,
        BinOp::Div => x / y,
    };
    if b.len() == 1 {
        let y = b[0];
        for (slot, &x) in out[..len].iter_mut().zip(&a[..len]) {
            *slot = apply(x, y);
        }
        return Ok(());
    }
    if a.len() == 1 {
        let x = a[0];
        for (slot, &y) in out[..len].iter_mut().zip(&b[..len]) {
            *slot = apply(x, y);
        }
        return Ok(());
    }
    if a_dims == out_dims && b_dims == out_dims {
        for ((slot, &x), &y) in out[..len].iter_mut().zip(&a[..len]).zip(&b[..len]) {
            *slot = apply(x, y);
        }
        return Ok(());
    }
    // A per-channel operand, the shape the LCNet bias add and the OCR
    // normalisation constants use. `a` is the large tensor in both cases below.
    if a_dims == out_dims && is_per_channel(b_dims) {
        per_channel(&mut out[..len], a, b, out_dims, apply);
        return Ok(());
    }
    if b_dims == out_dims && is_per_channel(a_dims) {
        per_channel(&mut out[..len], b, a, out_dims, |y: f32, x: f32| {
            apply(x, y)
        });
        return Ok(());
    }
    if a_dims.len() > 4 || b_dims.len() > 4 || out_dims.len() > 4 {
        return Err("binary: rank above 4 is not supported".to_string());
    }
    let ar = broadcast_strides(a_dims, out_dims);
    let br = broadcast_strides(b_dims, out_dims);
    let mut as_ = [0usize; 4];
    let mut bs = [0usize; 4];
    for (slot, stride) in as_.iter_mut().zip(&ar) {
        *slot = *stride;
    }
    for (slot, stride) in bs.iter_mut().zip(&br) {
        *slot = *stride;
    }
    let rank = out_dims.len();
    let mut index = 0usize;
    match rank {
        1 => {
            let mut a_offset = 0usize;
            let mut b_offset = 0usize;
            for _ in 0..out_dims[0] {
                out[index] = apply(a[a_offset], b[b_offset]);
                index += 1;
                a_offset += as_[0];
                b_offset += bs[0];
            }
        }
        2 => {
            let (d0, d1) = (out_dims[0], out_dims[1]);
            let mut a0 = 0usize;
            let mut b0 = 0usize;
            for _ in 0..d0 {
                let mut ai = a0;
                let mut bi = b0;
                for _ in 0..d1 {
                    out[index] = apply(a[ai], b[bi]);
                    index += 1;
                    ai += as_[1];
                    bi += bs[1];
                }
                a0 += as_[0];
                b0 += bs[0];
            }
        }
        3 => {
            let (d0, d1, d2) = (out_dims[0], out_dims[1], out_dims[2]);
            let mut a0 = 0usize;
            let mut b0 = 0usize;
            for _ in 0..d0 {
                let mut a1 = a0;
                let mut b1 = b0;
                for _ in 0..d1 {
                    let mut ai = a1;
                    let mut bi = b1;
                    for _ in 0..d2 {
                        out[index] = apply(a[ai], b[bi]);
                        index += 1;
                        ai += as_[2];
                        bi += bs[2];
                    }
                    a1 += as_[1];
                    b1 += bs[1];
                }
                a0 += as_[0];
                b0 += bs[0];
            }
        }
        _ => {
            let (d0, d1, d2, d3) = (out_dims[0], out_dims[1], out_dims[2], out_dims[3]);
            let mut a0 = 0usize;
            let mut b0 = 0usize;
            for _ in 0..d0 {
                let mut a1 = a0;
                let mut b1 = b0;
                for _ in 0..d1 {
                    let mut a2 = a1;
                    let mut b2 = b1;
                    for _ in 0..d2 {
                        let mut ai = a2;
                        let mut bi = b2;
                        for _ in 0..d3 {
                            out[index] = apply(a[ai], b[bi]);
                            index += 1;
                            ai += as_[3];
                            bi += bs[3];
                        }
                        a2 += as_[2];
                        b2 += bs[2];
                    }
                    a1 += as_[1];
                    b1 += bs[1];
                }
                a0 += as_[0];
                b0 += bs[0];
            }
        }
    }
    Ok(())
}

fn is_per_channel(dims: &[usize]) -> bool {
    dims.len() == 4 && dims[0] == 1 && dims[2] == 1 && dims[3] == 1
}

/// `large` and `out` have the same shape; `constant` is `[1, C, 1, 1]`.
fn per_channel<F: Fn(f32, f32) -> f32>(
    out: &mut [f32],
    large: &[f32],
    constant: &[f32],
    dims: &[usize],
    apply: F,
) {
    let channels = dims[1];
    let pixels = dims[2] * dims[3];
    for (channel, &value) in constant.iter().take(channels).enumerate() {
        let base = channel * pixels;
        for (slot, &x) in out[base..base + pixels]
            .iter_mut()
            .zip(&large[base..base + pixels])
        {
            *slot = apply(x, value);
        }
    }
}

pub(crate) fn unary(op: UnaryOp, input: &[f32], out: &mut [f32]) {
    match op {
        UnaryOp::Relu => {
            for (slot, &x) in out.iter_mut().zip(input) {
                *slot = x.max(0.0);
            }
        }
        UnaryOp::Sigmoid => {
            for (slot, &x) in out.iter_mut().zip(input) {
                *slot = 1.0 / (1.0 + (-x).exp());
            }
        }
        UnaryOp::Erf => erf(input, out),
    }
}

/// `erf` through a linear-interpolated table.
///
/// The OCR detector evaluates `erf` on millions of values per image and the
/// scalar libm call dominated its profile. The table is exact at every node and
/// the interpolation error is below 1e-7, well inside the 1e-3 parity bound.
fn erf(input: &[f32], out: &mut [f32]) {
    const SIZE: usize = 8192;
    const MAX: f32 = 4.0;
    static TABLE: std::sync::OnceLock<Box<[f32; SIZE + 1]>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut values = Box::new([0f32; SIZE + 1]);
        let step = MAX / SIZE as f32;
        for (index, value) in values.iter_mut().enumerate() {
            *value = libm::erff(index as f32 * step);
        }
        values
    });
    let scale = SIZE as f32 / MAX;
    for (slot, &x) in out.iter_mut().zip(input) {
        let absolute = x.abs();
        if absolute >= MAX {
            *slot = if x.is_sign_negative() { -1.0 } else { 1.0 };
            continue;
        }
        let position = absolute * scale;
        let index = position as usize;
        let fraction = position - index as f32;
        let value = table[index] + (table[index + 1] - table[index]) * fraction;
        *slot = if x.is_sign_negative() { -value } else { value };
    }
}

pub(crate) fn hard_sigmoid(input: &[f32], out: &mut [f32], alpha: f32, beta: f32) {
    for (slot, &x) in out.iter_mut().zip(input) {
        *slot = (alpha * x + beta).clamp(0.0, 1.0);
    }
}

/// `y = (x - mean) / sqrt(var + eps) * scale + bias`, per channel.
pub(crate) fn batch_norm(
    input: &[f32],
    dims: &[usize],
    scale: &[f32],
    bias: &[f32],
    mean: &[f32],
    variance: &[f32],
    out: &mut [f32],
    epsilon: f32,
) -> Result<(), String> {
    if dims.len() < 2 {
        return Err(format!("batch_norm: expected rank 2 or more, got {dims:?}"));
    }
    let channels = dims[1];
    let pixels: usize = dims[2..].iter().product();
    if scale.len() < channels
        || bias.len() < channels
        || mean.len() < channels
        || variance.len() < channels
    {
        return Err("batch_norm: parameter length mismatch".to_string());
    }
    for channel in 0..channels {
        let a = scale[channel] / (variance[channel] + epsilon).sqrt();
        let b = bias[channel] - mean[channel] * a;
        let base = channel * pixels;
        for index in base..base + pixels {
            out[index] = input[index] * a + b;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_broadcasts_a_scalar() {
        let mut out = [0f32; 3];
        binary(
            BinOp::Add,
            &[1.0, 2.0, 3.0],
            &[3],
            &[10.0],
            &[],
            &mut out,
            &[3],
        )
        .unwrap();
        assert_eq!(out, [11.0, 12.0, 13.0]);
    }

    #[test]
    fn binary_broadcasts_per_channel() {
        let mut out = [0f32; 8];
        binary(
            BinOp::Mul,
            &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            &[1, 2, 2, 2],
            &[10.0, 100.0],
            &[1, 2, 1, 1],
            &mut out,
            &[1, 2, 2, 2],
        )
        .unwrap();
        assert_eq!(out, [10.0, 20.0, 30.0, 40.0, 500.0, 600.0, 700.0, 800.0]);
    }

    #[test]
    fn batch_norm_matches_the_formula() {
        let mut out = [0f32; 2];
        batch_norm(
            &[1.0, 2.0],
            &[1, 1, 1, 2],
            &[2.0],
            &[1.0],
            &[1.0],
            &[1.0],
            &mut out,
            1.0,
        )
        .unwrap();
        assert!((out[0] - 1.0).abs() < 1e-6);
        assert!((out[1] - 2.4142135).abs() < 1e-6);
    }
}
