//! The only unsafe in the inference runtime: `matrixmultiply` exposes its GEMM
//! as an `unsafe fn` over raw pointers.
//!
//! The call is sound by construction: every buffer is a borrowed slice whose
//! length the caller validated, the pointers are derived from those slices, the
//! strides stay inside the buffers, and the output buffer is a distinct slot
//! borrowed exclusively by the executor. Keeping the `unsafe` block in this one
//! file keeps the rest of the runtime under the workspace-wide `unsafe_code`
//! deny.
#![allow(unsafe_code)]

/// Row-major `C = A * B` for `A: m x k`, `B: k x n`, `C: m x n`.
pub(crate) fn sgemm(m: usize, k: usize, n: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
    debug_assert_eq!(a.len(), m * k);
    debug_assert_eq!(b.len(), k * n);
    debug_assert!(c.len() >= m * n);
    if m == 0 || k == 0 || n == 0 {
        return;
    }
    // SAFETY: see the module comment. `a` is m*k contiguous f32, `b` is k*n,
    // `c` is at least m*n; the strides describe exactly those row-major layouts.
    unsafe {
        matrixmultiply::sgemm(
            m,
            k,
            n,
            1.0,
            a.as_ptr(),
            k as isize,
            1,
            b.as_ptr(),
            n as isize,
            1,
            0.0,
            c.as_mut_ptr(),
            n as isize,
            1,
        );
    }
}
