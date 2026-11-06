//! Perspective crop of one detection quad.
//!
//! A DB box is a rotated quadrilateral, not an axis-aligned rectangle. The
//! recogniser wants an upright line, so this module computes the homography that
//! maps a destination rectangle onto the quad and samples the source through it.
//! A tall, narrow quad (vertical text) is rotated a quarter turn first.

use super::det::{Point, Quad};
use super::resample::sample_rgb;

/// An RGB crop and its size.
pub(crate) struct Crop {
    pub rgb: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Warp `quad` out of the source image.
pub(crate) fn warp(rgb: &[u8], width: usize, height: usize, quad: Quad) -> Option<Crop> {
    if width == 0 || height == 0 {
        return None;
    }
    let upright = roll_vertical(quad);
    let crop_w = ((upright.points[1].x - upright.points[0].x).powi(2)
        + (upright.points[1].y - upright.points[0].y).powi(2))
    .sqrt()
    .round()
    .max(2.0) as usize;
    let crop_h = ((upright.points[3].x - upright.points[0].x).powi(2)
        + (upright.points[3].y - upright.points[0].y).powi(2))
    .sqrt()
    .round()
    .max(2.0) as usize;

    let destination = [
        [0.0f32, 0.0],
        [crop_w as f32, 0.0],
        [crop_w as f32, crop_h as f32],
        [0.0, crop_h as f32],
    ];
    let mut equations = Vec::with_capacity(8);
    for (index, [x, y]) in destination.iter().copied().enumerate() {
        let u = upright.points[index].x;
        let v = upright.points[index].y;
        equations.push(vec![x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u]);
        equations.push(vec![0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v]);
    }
    let homography = solve(equations);

    let mut out = vec![0u8; crop_w * crop_h * 3];
    for y in 0..crop_h {
        for x in 0..crop_w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let denominator = homography[6] * px + homography[7] * py + 1.0;
            if denominator.abs() < 1e-9 {
                continue;
            }
            let source_x = (homography[0] * px + homography[1] * py + homography[2]) / denominator;
            let source_y = (homography[3] * px + homography[4] * py + homography[5]) / denominator;
            let pixel = sample_rgb(rgb, width, height, source_x, source_y);
            let offset = (y * crop_w + x) * 3;
            out[offset] = pixel[0].round().clamp(0.0, 255.0) as u8;
            out[offset + 1] = pixel[1].round().clamp(0.0, 255.0) as u8;
            out[offset + 2] = pixel[2].round().clamp(0.0, 255.0) as u8;
        }
    }
    Some(Crop {
        rgb: out,
        width: crop_w,
        height: crop_h,
    })
}

fn roll_vertical(quad: Quad) -> Quad {
    let width = ((quad.points[1].x - quad.points[0].x).powi(2)
        + (quad.points[1].y - quad.points[0].y).powi(2))
    .sqrt();
    let height = ((quad.points[3].x - quad.points[0].x).powi(2)
        + (quad.points[3].y - quad.points[0].y).powi(2))
    .sqrt();
    if height > width * 1.5 {
        Quad {
            points: [
                quad.points[1],
                quad.points[2],
                quad.points[3],
                quad.points[0],
            ],
        }
    } else {
        quad
    }
}

/// Solve the 8x8 linear system. Degenerate rows leave their unknown at zero,
/// which produces a black crop rather than a panic.
#[allow(clippy::needless_range_loop)]
fn solve(rows: Vec<Vec<f32>>) -> [f32; 8] {
    const SIZE: usize = 8;
    let mut matrix = rows;
    for column in 0..SIZE {
        let mut pivot = column;
        for row in column + 1..SIZE {
            if matrix[row][column].abs() > matrix[pivot][column].abs() {
                pivot = row;
            }
        }
        matrix.swap(column, pivot);
        let scale = matrix[column][column];
        if scale.abs() < 1e-12 {
            continue;
        }
        for index in column..=SIZE {
            matrix[column][index] /= scale;
        }
        for row in 0..SIZE {
            if row == column {
                continue;
            }
            let factor = matrix[row][column];
            for index in column..=SIZE {
                matrix[row][index] -= factor * matrix[column][index];
            }
        }
    }
    let mut homography = [0f32; 8];
    for (index, value) in homography.iter_mut().enumerate() {
        *value = matrix[index][SIZE];
    }
    homography
}

#[allow(dead_code)]
fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_axis_aligned_quad_crops_the_same_pixels() {
        let width = 4;
        let height = 4;
        let mut rgb = vec![0u8; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                let offset = (y * width + x) * 3;
                rgb[offset] = (x * 10) as u8;
                rgb[offset + 1] = (y * 10) as u8;
                rgb[offset + 2] = 7;
            }
        }
        let quad = Quad {
            points: [
                point(0.0, 0.0),
                point(4.0, 0.0),
                point(4.0, 4.0),
                point(0.0, 4.0),
            ],
        };
        let crop = warp(&rgb, width, height, quad).expect("crop");
        assert_eq!((crop.width, crop.height), (4, 4));
        let offset = (2 * crop.width + 2) * 3;
        // The destination centre samples between source pixels 2 and 3.
        assert!(
            (20..=30).contains(&crop.rgb[offset]),
            "{}",
            crop.rgb[offset]
        );
        assert!(
            (20..=30).contains(&crop.rgb[offset + 1]),
            "{}",
            crop.rgb[offset + 1]
        );
        assert_eq!(crop.rgb[offset + 2], 7);
    }

    #[test]
    fn a_rotated_quad_produces_a_positive_crop() {
        let width = 16;
        let height = 16;
        let rgb = vec![128u8; width * height * 3];
        let quad = Quad {
            points: [
                point(2.0, 1.0),
                point(12.0, 3.0),
                point(10.0, 11.0),
                point(0.0, 9.0),
            ],
        };
        let crop = warp(&rgb, width, height, quad).expect("crop");
        assert!(crop.width > 2 && crop.height > 2);
        assert_eq!(crop.rgb.len(), crop.width * crop.height * 3);
    }

    #[test]
    fn vertical_text_is_rotated_before_cropping() {
        let quad = Quad {
            points: [
                point(0.0, 0.0),
                point(4.0, 0.0),
                point(4.0, 20.0),
                point(0.0, 20.0),
            ],
        };
        let upright = roll_vertical(quad);
        assert_eq!(upright.points[0], point(4.0, 0.0));
    }
}
