//! Softmax, `ReduceMean` and global average pooling.

pub(crate) fn softmax(
    input: &[f32],
    dims: &[usize],
    out: &mut [f32],
    axis: i16,
) -> Result<(), String> {
    let rank = dims.len();
    let axis = if axis < 0 { axis + rank as i16 } else { axis };
    if axis < 0 || axis as usize >= rank {
        return Err(format!("softmax: axis {axis} out of range for rank {rank}"));
    }
    let axis = axis as usize;
    let outer: usize = dims[..axis].iter().product();
    let axis_len = dims[axis];
    let inner: usize = dims[axis + 1..].iter().product();
    for o in 0..outer {
        for i in 0..inner {
            let base = o * axis_len * inner + i;
            let mut max = f32::NEG_INFINITY;
            for a in 0..axis_len {
                let value = input[base + a * inner];
                if value > max {
                    max = value;
                }
            }
            let mut sum = 0f32;
            for a in 0..axis_len {
                let index = base + a * inner;
                let value = (input[index] - max).exp();
                out[index] = value;
                sum += value;
            }
            let inv = if sum == 0.0 { 0.0 } else { 1.0 / sum };
            for a in 0..axis_len {
                out[base + a * inner] *= inv;
            }
        }
    }
    Ok(())
}

/// Mean over `axes`, which are already normalised and sorted by the executor.
pub(crate) fn reduce_mean(
    input: &[f32],
    dims: &[usize],
    out: &mut [f32],
    axes: &[usize],
    keepdims: bool,
) -> Result<(), String> {
    let rank = dims.len();
    if axes.is_empty() {
        let sum: f32 = input.iter().sum();
        out[0] = sum / input.len() as f32;
        return Ok(());
    }
    if rank == 4 && axes == [2, 3] {
        let channels = dims[1];
        let pixels = dims[2] * dims[3];
        for (channel, destination) in out.iter_mut().take(channels).enumerate() {
            let base = channel * pixels;
            let sum: f32 = input[base..base + pixels].iter().sum();
            *destination = sum / pixels as f32;
        }
        return Ok(());
    }
    if rank == 4 && axes == [3] {
        let (channels, height, width) = (dims[1], dims[2], dims[3]);
        for channel in 0..channels {
            for y in 0..height {
                let base = (channel * height + y) * width;
                let sum: f32 = input[base..base + width].iter().sum();
                out[channel * height + y] = sum / width as f32;
            }
        }
        return Ok(());
    }
    // Generic fallback: accumulate into the reduced output by scanning the input.
    let out_len: usize = dims
        .iter()
        .enumerate()
        .map(|(i, d)| if axes.contains(&i) { 1 } else { *d })
        .product();
    if out.len() < out_len {
        return Err("reduce_mean: output buffer too small".to_string());
    }
    out[..out_len].fill(0.0);
    let mut out_strides = vec![0usize; rank];
    let mut stride = 1usize;
    for i in (0..rank).rev() {
        let size = if axes.contains(&i) { 1 } else { dims[i] };
        out_strides[i] = stride;
        stride *= size;
    }
    let mut input_strides = vec![0usize; rank];
    let mut stride = 1usize;
    for i in (0..rank).rev() {
        input_strides[i] = stride;
        stride *= dims[i];
    }
    for (index, &value) in input.iter().enumerate() {
        let mut remaining = index;
        let mut out_index = 0usize;
        for i in 0..rank {
            let coord = remaining / input_strides[i];
            remaining %= input_strides[i];
            if !axes.contains(&i) {
                out_index += coord * out_strides[i];
            }
        }
        out[out_index] += value;
    }
    let count: usize = axes.iter().map(|&axis| dims[axis]).product();
    let scale = 1.0 / count as f32;
    for value in &mut out[..out_len] {
        *value *= scale;
    }
    let _ = keepdims;
    Ok(())
}

pub(crate) fn global_average_pool(
    input: &[f32],
    dims: &[usize],
    out: &mut [f32],
) -> Result<(), String> {
    if dims.len() != 4 {
        return Err("global_average_pool: expected rank 4".to_string());
    }
    let channels = dims[1];
    let pixels = dims[2] * dims[3];
    if input.len() < channels * pixels || out.len() < channels {
        return Err("global_average_pool: buffer too small".to_string());
    }
    for (channel, destination) in out.iter_mut().take(channels).enumerate() {
        let base = channel * pixels;
        let sum: f32 = input[base..base + pixels].iter().sum();
        *destination = sum / pixels as f32;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_sums_to_one() {
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut out = [0f32; 4];
        softmax(&input, &[1, 4], &mut out, 1).unwrap();
        let sum: f32 = out.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        assert!(out[3] > out[2] && out[2] > out[1] && out[1] > out[0]);
    }

    #[test]
    fn reduce_mean_over_the_width() {
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut out = [0f32; 2];
        reduce_mean(&input, &[1, 1, 2, 2], &mut out, &[3], true).unwrap();
        assert_eq!(out, [1.5, 3.5]);
    }
}
