//! Max and average pooling with explicit padding. `SAME_*` padding is resolved
//! by the executor before the kernel runs.
//!
//! The accumulation is row-oriented: for every kernel row the inner loop runs
//! over contiguous output columns (a scalar tap, a contiguous input row), which
//! vectorises; the padded edges take the scalar path.

pub(crate) struct PoolSpec {
    pub(crate) kernel: [usize; 2],
    pub(crate) stride: [usize; 2],
    /// top, left, bottom, right
    pub(crate) pads: [usize; 4],
    pub(crate) ceil_mode: bool,
}

pub(crate) fn max_pool(
    input: &[f32],
    in_dims: &[usize],
    out: &mut [f32],
    out_dims: &[usize],
    spec: &PoolSpec,
) -> Result<(), String> {
    let (channels, ih, iw) = (in_dims[1], in_dims[2], in_dims[3]);
    let (oh, ow) = (out_dims[2], out_dims[3]);
    let (kh, kw) = (spec.kernel[0], spec.kernel[1]);
    let (sh, sw) = (spec.stride[0], spec.stride[1]);
    let [pt, pl, _, _] = spec.pads;
    let _ = spec.ceil_mode;
    let pixels = oh * ow;
    if input.len() < channels * ih * iw || out.len() < channels * pixels {
        return Err("max_pool: buffer too small".to_string());
    }
    let (interior_lo, interior_hi) = interior_range(ow, iw, pl, kw, sw);
    let mut accumulators = vec![f32::NEG_INFINITY; ow];
    for channel in 0..channels {
        let base = channel * ih * iw;
        let out_base = channel * pixels;
        for oy in 0..oh {
            accumulators.fill(f32::NEG_INFINITY);
            for ky in 0..kh {
                let input_y = oy as isize * sh as isize - pt as isize + ky as isize;
                if input_y < 0 || input_y >= ih as isize {
                    continue;
                }
                let row = base + input_y as usize * iw;
                for kx in 0..kw {
                    if sw == 1 && interior_hi > interior_lo {
                        let source = (row as isize - pl as isize
                            + kx as isize
                            + interior_lo as isize) as usize;
                        let values = &input[source..source + (interior_hi - interior_lo)];
                        for (accumulator, value) in accumulators[interior_lo..interior_hi]
                            .iter_mut()
                            .zip(values)
                        {
                            *accumulator = accumulator.max(*value);
                        }
                    }
                    if sw == 1 {
                        for (x, accumulator) in
                            accumulators.iter_mut().take(interior_lo).enumerate()
                        {
                            let input_x = x as isize - pl as isize + kx as isize;
                            if input_x >= 0 && input_x < iw as isize {
                                let value = input[row + input_x as usize];
                                if value > *accumulator {
                                    *accumulator = value;
                                }
                            }
                        }
                        for (offset, accumulator) in
                            accumulators[interior_hi..ow].iter_mut().enumerate()
                        {
                            let x = interior_hi + offset;
                            let input_x = x as isize - pl as isize + kx as isize;
                            if input_x >= 0 && input_x < iw as isize {
                                let value = input[row + input_x as usize];
                                if value > *accumulator {
                                    *accumulator = value;
                                }
                            }
                        }
                    } else {
                        for (x, accumulator) in accumulators.iter_mut().take(ow).enumerate() {
                            let input_x = x as isize * sw as isize - pl as isize + kx as isize;
                            if input_x < 0 || input_x >= iw as isize {
                                continue;
                            }
                            let value = input[row + input_x as usize];
                            if value > *accumulator {
                                *accumulator = value;
                            }
                        }
                    }
                }
            }
            for (x, accumulator) in accumulators[..ow].iter_mut().enumerate() {
                let _ = x;
                if *accumulator == f32::NEG_INFINITY {
                    *accumulator = 0.0;
                }
            }
            out[out_base + oy * ow..out_base + (oy + 1) * ow].copy_from_slice(&accumulators[..ow]);
        }
    }
    Ok(())
}

pub(crate) fn average_pool(
    input: &[f32],
    in_dims: &[usize],
    out: &mut [f32],
    out_dims: &[usize],
    spec: &PoolSpec,
    count_include_pad: bool,
) -> Result<(), String> {
    let (channels, ih, iw) = (in_dims[1], in_dims[2], in_dims[3]);
    let (oh, ow) = (out_dims[2], out_dims[3]);
    let (kh, kw) = (spec.kernel[0], spec.kernel[1]);
    let (sh, sw) = (spec.stride[0], spec.stride[1]);
    let [pt, pl, pb, pr] = spec.pads;
    let _ = spec.ceil_mode;
    let pixels = oh * ow;
    if input.len() < channels * ih * iw || out.len() < channels * pixels {
        return Err("average_pool: buffer too small".to_string());
    }
    let full_windows = pt == 0 && pl == 0 && pb == 0 && pr == 0 && !count_include_pad;
    let divisor = (kh * kw) as f32;
    let (interior_lo, interior_hi) = interior_range(ow, iw, pl, kw, sw);
    let mut accumulators = vec![0f32; ow];
    for channel in 0..channels {
        let base = channel * ih * iw;
        let out_base = channel * pixels;
        for oy in 0..oh {
            accumulators.fill(0.0);
            for ky in 0..kh {
                let input_y = oy as isize * sh as isize - pt as isize + ky as isize;
                if input_y < 0 || input_y >= ih as isize {
                    continue;
                }
                let row = base + input_y as usize * iw;
                for kx in 0..kw {
                    if sw == 1 && interior_hi > interior_lo {
                        let source = (row as isize - pl as isize
                            + kx as isize
                            + interior_lo as isize) as usize;
                        let values = &input[source..source + (interior_hi - interior_lo)];
                        for (accumulator, value) in accumulators[interior_lo..interior_hi]
                            .iter_mut()
                            .zip(values)
                        {
                            *accumulator += *value;
                        }
                    }
                    if sw == 1 {
                        for (x, accumulator) in
                            accumulators.iter_mut().take(interior_lo).enumerate()
                        {
                            let input_x = x as isize - pl as isize + kx as isize;
                            if input_x >= 0 && input_x < iw as isize {
                                *accumulator += input[row + input_x as usize];
                            }
                        }
                        for (offset, accumulator) in
                            accumulators[interior_hi..ow].iter_mut().enumerate()
                        {
                            let x = interior_hi + offset;
                            let input_x = x as isize - pl as isize + kx as isize;
                            if input_x >= 0 && input_x < iw as isize {
                                *accumulator += input[row + input_x as usize];
                            }
                        }
                    } else {
                        for (x, accumulator) in accumulators.iter_mut().take(ow).enumerate() {
                            let input_x = x as isize * sw as isize - pl as isize + kx as isize;
                            if input_x < 0 || input_x >= iw as isize {
                                continue;
                            }
                            *accumulator += input[row + input_x as usize];
                        }
                    }
                }
            }
            if full_windows {
                let scale = 1.0 / divisor;
                for accumulator in &mut accumulators[..ow] {
                    *accumulator *= scale;
                }
            } else {
                for (x, accumulator) in accumulators.iter_mut().take(ow).enumerate() {
                    let mut count = 0usize;
                    for ky in 0..kh {
                        let input_y = oy as isize * sh as isize - pt as isize + ky as isize;
                        for kx in 0..kw {
                            let input_x = x as isize * sw as isize - pl as isize + kx as isize;
                            if input_y >= -(pt as isize)
                                && input_y < (ih + pb) as isize
                                && input_x >= -(pl as isize)
                                && input_x < (iw + pr) as isize
                                && (count_include_pad
                                    || (input_y >= 0
                                        && input_y < ih as isize
                                        && input_x >= 0
                                        && input_x < iw as isize))
                            {
                                count += 1;
                            }
                        }
                    }
                    *accumulator = if count == 0 {
                        0.0
                    } else {
                        *accumulator / count as f32
                    };
                }
            }
            out[out_base + oy * ow..out_base + (oy + 1) * ow].copy_from_slice(&accumulators[..ow]);
        }
    }
    Ok(())
}

/// Output columns for which every kernel tap falls inside the input row.
fn interior_range(ow: usize, iw: usize, pl: usize, kw: usize, sw: usize) -> (usize, usize) {
    if sw != 1 {
        return (0, 0);
    }
    let lo = pl.min(ow);
    let hi = if iw + pl + 1 >= kw {
        (iw + pl + 1 - kw).min(ow)
    } else {
        lo
    };
    (lo, hi.max(lo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_pool_uses_two_by_two_windows() {
        let input = [1.0f32, 3.0, 2.0, 4.0];
        let mut out = [0f32; 1];
        let spec = PoolSpec {
            kernel: [2, 2],
            stride: [2, 2],
            pads: [0; 4],
            ceil_mode: false,
        };
        max_pool(&input, &[1, 1, 2, 2], &mut out, &[1, 1, 1, 1], &spec).unwrap();
        assert_eq!(out, [4.0]);
    }

    #[test]
    fn max_pool_pads_with_negative_infinity() {
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut out = [0f32; 4];
        let spec = PoolSpec {
            kernel: [2, 2],
            stride: [1, 1],
            pads: [0, 0, 1, 1],
            ceil_mode: false,
        };
        // 2x2 input, one row/column of bottom/right padding -> 2x2 output.
        max_pool(&input, &[1, 1, 2, 2], &mut out, &[1, 1, 2, 2], &spec).unwrap();
        assert_eq!(out, [4.0, 4.0, 4.0, 4.0]);
    }

    #[test]
    fn average_pool_ignores_padding_by_default() {
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut out = [0f32; 1];
        let spec = PoolSpec {
            kernel: [2, 2],
            stride: [1, 1],
            pads: [0; 4],
            ceil_mode: false,
        };
        average_pool(&input, &[1, 1, 2, 2], &mut out, &[1, 1, 1, 1], &spec, false).unwrap();
        assert_eq!(out, [2.5]);
    }

    #[test]
    fn average_pool_counts_the_padding_when_asked() {
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut out = [0f32; 4];
        let spec = PoolSpec {
            kernel: [2, 2],
            stride: [1, 1],
            pads: [1, 1, 1, 1],
            ceil_mode: false,
        };
        average_pool(&input, &[1, 1, 2, 2], &mut out, &[1, 1, 2, 2], &spec, false).unwrap();
        // corner window has one value, edge windows two, centre four.
        assert_eq!(out, [1.0, 1.5, 2.0, 2.5]);
        let mut padded = [0f32; 4];
        average_pool(
            &input,
            &[1, 1, 2, 2],
            &mut padded,
            &[1, 1, 2, 2],
            &spec,
            true,
        )
        .unwrap();
        assert_eq!(padded, [0.25, 0.75, 1.0, 2.5]);
    }
}
