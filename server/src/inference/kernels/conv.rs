//! Convolution kernels: group-1 direct convolution, depthwise convolution and
//! transposed convolution. The executor resolves `SAME_*` padding to explicit
//! pads before calling in.
//!
//! Group-1 kernels use a direct convolution with a vectorisable inner loop over
//! the output row: the weight is scalar, the input row is contiguous, so the
//! compiler turns `acc[x] += row[x] * w` into packed FMAs. 1x1 convolutions are a
//! plain GEMM. This avoids materialising an im2col matrix, which cost more than
//! the arithmetic for these small channel counts.

use super::gemm;

/// One convolution's geometry, with padding already resolved.
pub(crate) struct ConvSpec {
    pub(crate) kernel: [usize; 2],
    pub(crate) stride: [usize; 2],
    pub(crate) dilation: [usize; 2],
    /// top, left, bottom, right
    pub(crate) pads: [usize; 4],
    pub(crate) groups: usize,
}

pub(crate) fn conv(
    input: &[f32],
    in_dims: &[usize],
    weight: &[f32],
    bias: Option<&[f32]>,
    out: &mut [f32],
    out_dims: &[usize],
    spec: &ConvSpec,
) -> Result<(), String> {
    let (ic, ih, iw) = (in_dims[1], in_dims[2], in_dims[3]);
    let (oc, oh, ow) = (out_dims[1], out_dims[2], out_dims[3]);
    if input.len() < ic * ih * iw || out.len() < oc * oh * ow {
        return Err("conv: buffer too small".to_string());
    }
    let (kh, kw) = (spec.kernel[0], spec.kernel[1]);
    let (sh, sw) = (spec.stride[0], spec.stride[1]);
    let (dh, dw) = (spec.dilation[0], spec.dilation[1]);
    let [pt, pl, _, _] = spec.pads;
    if spec.groups == 1 {
        if weight.len() != oc * ic * kh * kw {
            return Err(format!(
                "conv: weight has {} elements, expected {}",
                weight.len(),
                oc * ic * kh * kw
            ));
        }
        let pixels = oh * ow;
        if kh == 1 && kw == 1 && sh == 1 && sw == 1 && pt == 0 && pl == 0 {
            // The 1x1 projection is a plain GEMM over all pixels.
            gemm::sgemm(oc, ic, pixels, weight, input, &mut out[..oc * pixels]);
            if let Some(bias) = bias {
                add_bias(out, oc, pixels, bias);
            }
            return Ok(());
        }
        let spec = DirectSpec {
            kernel: [kh, kw],
            stride: [sh, sw],
            dilation: [dh, dw],
            pads: [pt, pl, spec.pads[2], spec.pads[3]],
        };
        return conv_im2col(input, ic, ih, iw, weight, bias, out, oc, oh, ow, spec);
    }
    if spec.groups != ic || spec.groups != oc {
        return Err(format!(
            "conv: unsupported groups {} for {ic}->{oc} channels",
            spec.groups
        ));
    }
    if weight.len() != oc * kh * kw {
        return Err("conv: depthwise weight length mismatch".to_string());
    }
    let spec = DirectSpec {
        kernel: [kh, kw],
        stride: [sh, sw],
        dilation: [dh, dw],
        pads: [pt, pl, spec.pads[2], spec.pads[3]],
    };
    let pixels = oh * ow;
    let mut accumulators = vec![0f32; ow];
    for channel in 0..oc {
        let weight_channel = &weight[channel * kh * kw..(channel + 1) * kh * kw];
        let bias = bias.map(|values| values[channel]).unwrap_or(0.0);
        let input_channel = &input[channel * ih * iw..(channel + 1) * ih * iw];
        let out_channel = &mut out[channel * pixels..(channel + 1) * pixels];
        direct_conv(
            input_channel,
            1,
            ih,
            iw,
            weight_channel,
            out_channel,
            oh,
            ow,
            &mut accumulators,
            &spec,
            bias,
        );
    }
    Ok(())
}

struct DirectSpec {
    kernel: [usize; 2],
    stride: [usize; 2],
    dilation: [usize; 2],
    pads: [usize; 4],
}

/// Direct convolution of `ic` input channels into one output channel.
///
/// `input` is the `ic x ih x iw` input plane, `weight` this output channel's
/// `[ic, kh, kw]` block. For stride 1 the inner loop over `x` is contiguous with
/// a scalar weight, which the compiler vectorises; the padded edges and strides
/// above 1 use a scalar loop.
#[allow(clippy::too_many_arguments)]
fn direct_conv(
    input: &[f32],
    ic: usize,
    ih: usize,
    iw: usize,
    weight: &[f32],
    out: &mut [f32],
    oh: usize,
    ow: usize,
    accumulators: &mut [f32],
    spec: &DirectSpec,
    bias: f32,
) {
    let (kh, kw) = (spec.kernel[0], spec.kernel[1]);
    let (sh, sw) = (spec.stride[0], spec.stride[1]);
    let (dh, dw) = (spec.dilation[0], spec.dilation[1]);
    let [pt, pl, _, _] = spec.pads;
    let kernel_span_x = (kw - 1) * dw + 1;
    // Columns for which every kernel tap stays inside the input row.
    let interior_lo = pl.min(ow);
    let interior_hi = if iw + pl + 1 >= kernel_span_x {
        (iw + pl + 1 - kernel_span_x).min(ow)
    } else {
        interior_lo
    };
    let interior_hi = interior_hi.max(interior_lo);
    let stride_one = sw == 1;
    for oy in 0..oh {
        accumulators.fill(0.0);
        for channel in 0..ic {
            let input_channel = &input[channel * ih * iw..(channel + 1) * ih * iw];
            for ky in 0..kh {
                let input_y = oy as isize * sh as isize - pt as isize + (ky * dh) as isize;
                if input_y < 0 || input_y >= ih as isize {
                    continue;
                }
                let row = input_y as usize * iw;
                let weight_row = &weight[(channel * kh + ky) * kw..(channel * kh + ky + 1) * kw];
                if stride_one {
                    for (kx, &weight_value) in weight_row.iter().enumerate() {
                        let shift = kx * dw;
                        if interior_hi > interior_lo {
                            let source = (row as isize - pl as isize
                                + shift as isize
                                + interior_lo as isize)
                                as usize;
                            let values =
                                &input_channel[source..source + (interior_hi - interior_lo)];
                            for (accumulator, value) in accumulators[interior_lo..interior_hi]
                                .iter_mut()
                                .zip(values)
                            {
                                *accumulator += *value * weight_value;
                            }
                        }
                        for x in 0..interior_lo {
                            add_edge(
                                accumulators,
                                input_channel,
                                row,
                                x,
                                shift,
                                pl,
                                iw,
                                weight_value,
                            );
                        }
                        for x in interior_hi..ow {
                            add_edge(
                                accumulators,
                                input_channel,
                                row,
                                x,
                                shift,
                                pl,
                                iw,
                                weight_value,
                            );
                        }
                    }
                } else {
                    for (kx, &weight_value) in weight_row.iter().enumerate() {
                        let shift = kx * dw;
                        for (x, accumulator) in accumulators.iter_mut().take(ow).enumerate() {
                            let input_x = x as isize * sw as isize - pl as isize + shift as isize;
                            if input_x < 0 || input_x >= iw as isize {
                                continue;
                            }
                            *accumulator += input_channel[row + input_x as usize] * weight_value;
                        }
                    }
                }
            }
        }
        if bias != 0.0 {
            for accumulator in &mut accumulators[..ow] {
                *accumulator += bias;
            }
        }
        out[oy * ow..(oy + 1) * ow].copy_from_slice(&accumulators[..ow]);
    }
}

/// Row-tiled im2col + GEMM for a group-1 convolution with a kernel larger
/// than 1x1.
///
/// The column matrix for one tile of output rows is built with contiguous row
/// copies (zero-padded at the borders) and multiplied with the contiguous
/// `[oc, ic*kh*kw]` weight block in one GEMM call. This keeps the GEMM the
/// dominant cost, which is what the measured 1x1 path shows is fast.
#[allow(clippy::too_many_arguments)]
fn conv_im2col(
    input: &[f32],
    ic: usize,
    ih: usize,
    iw: usize,
    weight: &[f32],
    bias: Option<&[f32]>,
    out: &mut [f32],
    oc: usize,
    oh: usize,
    ow: usize,
    spec: DirectSpec,
) -> Result<(), String> {
    let (kh, kw) = (spec.kernel[0], spec.kernel[1]);
    let (sh, sw) = (spec.stride[0], spec.stride[1]);
    let (dh, dw) = (spec.dilation[0], spec.dilation[1]);
    let [pt, pl, _, _] = spec.pads;
    let k = ic * kh * kw;
    let pixels = oh * ow;
    // Keep the column and output tiles around 8 MB together.
    let per_row = (k * ow).max(oc * ow).max(1);
    let mut rows_per_tile = ((2usize << 20) / per_row).max(1);
    rows_per_tile = rows_per_tile.min(oh).max(1);
    let n_max = rows_per_tile * ow;
    let mut columns = vec![0f32; k * n_max];
    let mut tile = vec![0f32; oc * n_max];
    let mut row = 0;
    while row < oh {
        let rows = rows_per_tile.min(oh - row);
        let n = rows * ow;
        for channel in 0..ic {
            let plane = &input[channel * ih * iw..(channel + 1) * ih * iw];
            for ky in 0..kh {
                for kx in 0..kw {
                    let dest_row = &mut columns[((channel * kh + ky) * kw + kx) * n..][..n];
                    for j in 0..rows {
                        let oy = row + j;
                        let input_y = oy as isize * sh as isize - pt as isize + (ky * dh) as isize;
                        let target = &mut dest_row[j * ow..(j + 1) * ow];
                        if input_y < 0 || input_y >= ih as isize {
                            target.fill(0.0);
                            continue;
                        }
                        let source = &plane[input_y as usize * iw..(input_y as usize + 1) * iw];
                        let shift = kx * dw;
                        if sw == 1 {
                            let offset = shift as isize - pl as isize;
                            let low = (-offset).clamp(0, ow as isize) as usize;
                            let high = ((iw as isize - offset).clamp(0, ow as isize)) as usize;
                            let high = high.max(low);
                            target[..low].fill(0.0);
                            if high > low {
                                let start = (offset + low as isize) as usize;
                                target[low..high]
                                    .copy_from_slice(&source[start..start + (high - low)]);
                            }
                            target[high..].fill(0.0);
                        } else {
                            for (x, value) in target.iter_mut().enumerate() {
                                let input_x =
                                    x as isize * sw as isize - pl as isize + shift as isize;
                                if input_x < 0 || input_x >= iw as isize {
                                    *value = 0.0;
                                } else {
                                    *value = source[input_x as usize];
                                }
                            }
                        }
                    }
                }
            }
        }
        gemm::sgemm(oc, k, n, weight, &columns[..k * n], &mut tile[..oc * n]);
        for out_channel in 0..oc {
            let source = &tile[out_channel * n..(out_channel + 1) * n];
            let base = out_channel * pixels + row * ow;
            match bias {
                Some(bias) => {
                    let value = bias[out_channel];
                    for (destination, &source) in out[base..base + n].iter_mut().zip(source) {
                        *destination = source + value;
                    }
                }
                None => out[base..base + n].copy_from_slice(source),
            }
        }
        row += rows;
    }
    Ok(())
}

fn add_edge(
    accumulators: &mut [f32],
    input_channel: &[f32],
    row: usize,
    x: usize,
    shift: usize,
    pl: usize,
    iw: usize,
    weight_value: f32,
) {
    let input_x = x as isize - pl as isize + shift as isize;
    if input_x < 0 || input_x >= iw as isize {
        return;
    }
    accumulators[x] += input_channel[row + input_x as usize] * weight_value;
}

fn add_bias(out: &mut [f32], channels: usize, pixels: usize, bias: &[f32]) {
    for (channel, &value) in bias.iter().take(channels).enumerate() {
        let base = channel * pixels;
        for pixel in &mut out[base..base + pixels] {
            *pixel += value;
        }
    }
}

/// Transposed convolution with `groups == 1`. `weight` is ONNX layout
/// `[in_channels, out_channels, kh, kw]`.
///
/// Implemented as one GEMM per kernel tap (`out_channels x in_channels` times
/// the input plane) followed by a scatter-add, which is far faster than the
/// naive per-output-element loop for the 2x2 upsampling these graphs use.
#[allow(clippy::too_many_arguments)]
pub(crate) fn conv_transpose(
    input: &[f32],
    in_dims: &[usize],
    weight: &[f32],
    bias: Option<&[f32]>,
    out: &mut [f32],
    out_dims: &[usize],
    spec: &ConvSpec,
) -> Result<(), String> {
    let (ic, ih, iw) = (in_dims[1], in_dims[2], in_dims[3]);
    let (oc, oh, ow) = (out_dims[1], out_dims[2], out_dims[3]);
    if spec.groups != 1 {
        return Err("conv_transpose: grouped transpose is not supported".to_string());
    }
    let (kh, kw) = (spec.kernel[0], spec.kernel[1]);
    let (sh, sw) = (spec.stride[0], spec.stride[1]);
    let (dh, dw) = (spec.dilation[0], spec.dilation[1]);
    let [pt, pl, _, _] = spec.pads;
    let pixels = oh * ow;
    // With stride == kernel and no padding every output position is written by
    // exactly one input pixel, so the scatter can assign instead of accumulate.
    let exact_cover = sh == kh
        && sw == kw
        && pt == 0
        && pl == 0
        && spec.pads[2] == 0
        && spec.pads[3] == 0
        && dh == 1
        && dw == 1
        && oh == ih * sh
        && ow == iw * sw;
    if !exact_cover {
        out[..oc * pixels].fill(0.0);
    }
    if weight.len() != ic * oc * kh * kw {
        return Err("conv_transpose: weight length mismatch".to_string());
    }
    let plane = ih * iw;
    let mut matrix = vec![0f32; oc * ic];
    let mut tile = vec![0f32; oc * plane];
    for ky in 0..kh {
        for kx in 0..kw {
            for in_channel in 0..ic {
                let weight_channel =
                    &weight[in_channel * oc * kh * kw..(in_channel + 1) * oc * kh * kw];
                for out_channel in 0..oc {
                    matrix[out_channel * ic + in_channel] =
                        weight_channel[out_channel * kh * kw + ky * kw + kx];
                }
            }
            let input_plane = &input[..ic * plane];
            gemm::sgemm(oc, ic, plane, &matrix, input_plane, &mut tile[..oc * plane]);
            for out_channel in 0..oc {
                let out_plane = &mut out[out_channel * pixels..(out_channel + 1) * pixels];
                let tile_plane = &tile[out_channel * plane..(out_channel + 1) * plane];
                for y in 0..ih {
                    let out_y = (y * sh + ky * dh) as isize - pt as isize;
                    if out_y < 0 || out_y >= oh as isize {
                        continue;
                    }
                    let out_row = out_y as usize * ow;
                    let tile_row = y * iw;
                    if sw == 1 && dw == 1 {
                        let out_x = (kx as isize) - pl as isize;
                        let start = out_x.max(0) as usize;
                        let end = (iw as isize + out_x).min(ow as isize);
                        if end > start as isize {
                            let target = out_row + start;
                            let source = tile_row as isize + start as isize - out_x;
                            if exact_cover {
                                for x in 0..(end as usize - start) {
                                    out_plane[target + x] = tile_plane[source as usize + x];
                                }
                            } else {
                                for x in 0..(end as usize - start) {
                                    out_plane[target + x] += tile_plane[source as usize + x];
                                }
                            }
                        }
                    } else {
                        for x in 0..iw {
                            let out_x = (x * sw + kx * dw) as isize - pl as isize;
                            if out_x < 0 || out_x >= ow as isize {
                                continue;
                            }
                            let destination = out_row + out_x as usize;
                            if exact_cover {
                                out_plane[destination] = tile_plane[tile_row + x];
                            } else {
                                out_plane[destination] += tile_plane[tile_row + x];
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(bias) = bias {
        add_bias(out, oc, pixels, bias);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "microbenchmark; run in release"]
    fn bench_conv_shapes() {
        use std::time::Instant;
        for (ic, oc, size) in [
            (64usize, 64usize, 120usize),
            (32, 32, 240),
            (16, 16, 480),
            (64, 32, 120),
        ] {
            let input = vec![0.5f32; ic * size * size];
            let weight = vec![0.01f32; oc * ic * 9];
            let mut out = vec![0f32; oc * size * size];
            let spec = ConvSpec {
                kernel: [3, 3],
                stride: [1, 1],
                dilation: [1, 1],
                pads: [1, 1, 1, 1],
                groups: 1,
            };
            conv(
                &input,
                &[1, ic, size, size],
                &weight,
                None,
                &mut out,
                &[1, oc, size, size],
                &spec,
            )
            .unwrap();
            let start = Instant::now();
            let rounds = 5;
            for _ in 0..rounds {
                conv(
                    &input,
                    &[1, ic, size, size],
                    &weight,
                    None,
                    &mut out,
                    &[1, oc, size, size],
                    &spec,
                )
                .unwrap();
            }
            let seconds = start.elapsed().as_secs_f64() / rounds as f64;
            let mac = (oc * ic * 9 * size * size) as f64;
            println!(
                "conv3x3 {ic}->{oc} {size}x{size}: {:.2} ms, {:.1} GMAC/s",
                seconds * 1e3,
                mac / seconds / 1e9
            );
        }
        for (ic, oc, pixels) in [
            (64usize, 64usize, 14400usize),
            (256, 128, 784),
            (128, 128, 3136),
        ] {
            let input = vec![0.5f32; ic * pixels];
            let weight = vec![0.01f32; oc * ic];
            let mut out = vec![0f32; oc * pixels];
            let spec = ConvSpec {
                kernel: [1, 1],
                stride: [1, 1],
                dilation: [1, 1],
                pads: [0; 4],
                groups: 1,
            };
            conv(
                &input,
                &[1, ic, 1, pixels],
                &weight,
                None,
                &mut out,
                &[1, oc, 1, pixels],
                &spec,
            )
            .unwrap();
            let start = Instant::now();
            let rounds = 5;
            for _ in 0..rounds {
                conv(
                    &input,
                    &[1, ic, 1, pixels],
                    &weight,
                    None,
                    &mut out,
                    &[1, oc, 1, pixels],
                    &spec,
                )
                .unwrap();
            }
            let seconds = start.elapsed().as_secs_f64() / rounds as f64;
            let mac = (oc * ic * pixels) as f64;
            println!(
                "conv1x1 {ic}->{oc} n={pixels}: {:.2} ms, {:.1} GMAC/s",
                seconds * 1e3,
                mac / seconds / 1e9
            );
        }
    }

    #[test]
    fn one_by_one_convolution_matches_a_manual_sum() {
        // 1x1, 1 channel in, 2 channels out, 2x2 input.
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let weight = [1.0f32, 10.0];
        let spec = ConvSpec {
            kernel: [1, 1],
            stride: [1, 1],
            dilation: [1, 1],
            pads: [0; 4],
            groups: 1,
        };
        let mut out = [0f32; 8];
        conv(
            &input,
            &[1, 1, 2, 2],
            &weight,
            None,
            &mut out,
            &[1, 2, 2, 2],
            &spec,
        )
        .unwrap();
        assert_eq!(&out[..4], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(&out[4..], &[10.0, 20.0, 30.0, 40.0]);
    }

    #[test]
    fn depthwise_convolution_is_per_channel() {
        let input = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let weight = [1.0f32, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0, -1.0];
        let spec = ConvSpec {
            kernel: [2, 2],
            stride: [1, 1],
            dilation: [1, 1],
            pads: [0; 4],
            groups: 2,
        };
        let mut out = [0f32; 2];
        conv(
            &input,
            &[1, 2, 2, 2],
            &weight,
            None,
            &mut out,
            &[1, 2, 1, 1],
            &spec,
        )
        .unwrap();
        assert_eq!(out, [5.0, -13.0]);
    }

    #[test]
    fn a_padded_three_by_three_convolution_matches_the_hand_computation() {
        // 1 channel, 3x3 input, one 3x3 kernel with SAME padding (one tap each).
        let input = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let weight = [0.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let spec = ConvSpec {
            kernel: [3, 3],
            stride: [1, 1],
            dilation: [1, 1],
            pads: [1, 1, 1, 1],
            groups: 1,
        };
        let mut out = [0f32; 9];
        conv(
            &input,
            &[1, 1, 3, 3],
            &weight,
            None,
            &mut out,
            &[1, 1, 3, 3],
            &spec,
        )
        .unwrap();
        // Centred tap: each output equals the input pixel.
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
    }

    #[test]
    fn strided_three_by_three_convolution_picks_every_other_pixel() {
        let input: Vec<f32> = (0..25).map(|v| v as f32).collect();
        let mut weight = vec![0f32; 9];
        weight[4] = 1.0;
        let spec = ConvSpec {
            kernel: [3, 3],
            stride: [2, 2],
            dilation: [1, 1],
            pads: [1, 1, 1, 1],
            groups: 1,
        };
        let mut out = [0f32; 9];
        conv(
            &input,
            &[1, 1, 5, 5],
            &weight,
            None,
            &mut out,
            &[1, 1, 3, 3],
            &spec,
        )
        .unwrap();
        assert_eq!(out, [0.0, 2.0, 4.0, 10.0, 12.0, 14.0, 20.0, 22.0, 24.0]);
    }
}
