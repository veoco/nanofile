//! Layout-only kernels: transpose, contiguous copies (reshape/squeeze) and
//! concatenation. They never change values.

pub(crate) fn transpose(
    input: &[f32],
    in_dims: &[usize],
    out: &mut [f32],
    perm: &[usize],
) -> Result<(), String> {
    let rank = in_dims.len();
    if perm.len() != rank || rank > 4 {
        return Err("transpose: unsupported rank or permutation".to_string());
    }
    let mut out_dims = vec![0usize; rank];
    for (i, &p) in perm.iter().enumerate() {
        out_dims[i] = in_dims[p];
    }
    let mut in_strides = vec![0usize; rank];
    let mut stride = 1usize;
    for i in (0..rank).rev() {
        in_strides[i] = stride;
        stride *= in_dims[i];
    }
    let mut out_strides = vec![0usize; rank];
    let mut stride = 1usize;
    for i in (0..rank).rev() {
        out_strides[i] = stride;
        stride *= out_dims[i];
    }
    // Walk the input in order and scatter it to the transposed output.
    let mut coords = vec![0usize; rank];
    let total: usize = in_dims.iter().product();
    for (index, &value) in input.iter().enumerate().take(total) {
        let mut remaining = index;
        for i in 0..rank {
            coords[i] = remaining / in_strides[i];
            remaining %= in_strides[i];
        }
        let mut out_index = 0usize;
        for i in 0..rank {
            out_index += coords[perm[i]] * out_strides[i];
        }
        out[out_index] = value;
    }
    Ok(())
}

pub(crate) fn copy_contiguous(input: &[f32], out: &mut [f32]) -> Result<(), String> {
    if input.len() != out.len() {
        return Err(format!(
            "layout copy: {} values into {} slots",
            input.len(),
            out.len()
        ));
    }
    out.copy_from_slice(input);
    Ok(())
}

/// Copy one input of a concatenation into its slice of the output.
///
/// `dst` is the whole output buffer, `src` one input. `outer` is the product of
/// the dimensions before `axis`, `inner` the product after it, `in_axis` this
/// input's size along `axis` and `offset` the number of already-written values
/// along `axis`.
pub(crate) fn concat_copy(
    dst: &mut [f32],
    src: &[f32],
    outer: usize,
    in_axis: usize,
    out_axis: usize,
    inner: usize,
    offset: usize,
) {
    for o in 0..outer {
        let dst_start = (o * out_axis + offset) * inner;
        let src_start = o * in_axis * inner;
        dst[dst_start..dst_start + in_axis * inner]
            .copy_from_slice(&src[src_start..src_start + in_axis * inner]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transpose_swaps_the_last_two_axes() {
        // (1, 2, 1, 2) -> (1, 2, 2, 1)
        let input = [1.0f32, 2.0, 3.0, 4.0];
        let mut out = [0f32; 4];
        transpose(&input, &[1, 2, 1, 2], &mut out, &[0, 1, 3, 2]).unwrap();
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn concat_copies_each_input_beside_the_others() {
        let mut out = [0f32; 6];
        concat_copy(&mut out, &[1.0, 2.0], 1, 2, 6, 1, 0);
        concat_copy(&mut out, &[3.0, 4.0, 5.0, 6.0], 1, 4, 6, 1, 2);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }
}
