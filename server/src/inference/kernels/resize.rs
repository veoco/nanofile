//! Nearest-neighbour resize with ONNX `asymmetric` coordinates and
//! `nearest_mode = floor`: `source = floor(destination / scale)`.

pub(crate) fn resize_nearest(
    input: &[f32],
    in_dims: &[usize],
    out: &mut [f32],
    out_dims: &[usize],
    scale_h: f32,
    scale_w: f32,
) -> Result<(), String> {
    let (channels, ih, iw) = (in_dims[1], in_dims[2], in_dims[3]);
    let (oh, ow) = (out_dims[2], out_dims[3]);
    if input.len() < channels * ih * iw || out.len() < channels * oh * ow {
        return Err("resize: buffer too small".to_string());
    }
    if scale_h <= 0.0 || scale_w <= 0.0 {
        return Err("resize: non-positive scale".to_string());
    }
    // The coordinate maps do not depend on the channel, so build them once
    // instead of dividing per pixel.
    let mut x_map = vec![0usize; ow];
    for (ox, mapped) in x_map.iter_mut().enumerate() {
        *mapped = (((ox as f32) / scale_w).floor() as usize).min(iw - 1);
    }
    let y_map: Vec<usize> = (0..oh)
        .map(|oy| ((oy as f32) / scale_h).floor() as usize)
        .map(|y| y.min(ih - 1))
        .collect();
    // An integer upscale can replicate each source pixel in runs, which is much
    // faster than a gather per output pixel.
    let integer_w = (scale_w.fract() == 0.0 && scale_w >= 1.0).then_some(scale_w as usize);
    for channel in 0..channels {
        let in_base = channel * ih * iw;
        let out_base = channel * oh * ow;
        for oy in 0..oh {
            let source_row = &input[in_base + y_map[oy] * iw..in_base + (y_map[oy] + 1) * iw];
            let target = &mut out[out_base + oy * ow..out_base + (oy + 1) * ow];
            if let Some(scale_x) = integer_w {
                for (source, value) in source_row.iter().enumerate() {
                    let start = source * scale_x;
                    let end = ((source + 1) * scale_x).min(ow);
                    if start < ow {
                        target[start..end].fill(*value);
                    }
                }
            } else {
                for (destination, &source) in target.iter_mut().zip(&x_map) {
                    *destination = source_row[source];
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_doubles_each_pixel() {
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut out = [0f32; 16];
        resize_nearest(&input, &[1, 1, 2, 2], &mut out, &[1, 1, 4, 4], 2.0, 2.0).unwrap();
        assert_eq!(
            out,
            [
                1.0, 1.0, 2.0, 2.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 3.0, 3.0, 4.0, 4.0
            ]
        );
    }

    #[test]
    fn a_fractional_scale_floors_the_source_coordinate() {
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut out = [0f32; 9];
        resize_nearest(&input, &[1, 1, 2, 2], &mut out, &[1, 1, 3, 3], 1.5, 1.5).unwrap();
        // floor(x / 1.5): 0,0,1 for both axes.
        assert_eq!(out, [1.0, 1.0, 2.0, 1.0, 1.0, 2.0, 3.0, 3.0, 4.0]);
    }
}
