//! Bilinear RGB sampling shared by detection, recognition, orientation
//! classification and the crop warp.
//!
//! The pixel-centre convention matches OpenCV's `INTER_LINEAR` and the Paddle
//! pipeline: the source coordinate of destination pixel `x` is
//! `(x + 0.5) * scale - 0.5`.

/// ImageNet mean/std, applied to the BGR channels Paddle's `DecodeImage`
/// produces. Channel 0 is blue, as in the model's published pipeline.
pub(crate) const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub(crate) const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Bilinear sample at `(x, y)` in source pixel coordinates. Edges clamp.
pub(crate) fn sample_rgb(rgb: &[u8], width: usize, height: usize, x: f32, y: f32) -> [f32; 3] {
    if width == 0 || height == 0 {
        return [0.0; 3];
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let x1 = (x0 + 1.0).min(width as f32 - 1.0);
    let y1 = (y0 + 1.0).min(height as f32 - 1.0);
    let dx = x - x0;
    let dy = y - y0;

    let at = |px: f32, py: f32, channel: usize| -> f32 {
        let cx = px.clamp(0.0, width as f32 - 1.0) as usize;
        let cy = py.clamp(0.0, height as f32 - 1.0) as usize;
        rgb[(cy * width + cx) * 3 + channel] as f32
    };

    let mut out = [0.0f32; 3];
    for (channel, value) in out.iter_mut().enumerate() {
        let top = at(x0, y0, channel) * (1.0 - dx) + at(x1, y0, channel) * dx;
        let bottom = at(x0, y1, channel) * (1.0 - dx) + at(x1, y1, channel) * dx;
        *value = top * (1.0 - dy) + bottom * dy;
    }
    out
}

/// Resize an RGB buffer with bilinear sampling.
pub(crate) fn resize_rgb(
    rgb: &[u8],
    width: usize,
    height: usize,
    resize_w: usize,
    resize_h: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; resize_w * resize_h * 3];
    if width == 0 || height == 0 || resize_w == 0 || resize_h == 0 {
        return out;
    }
    let scale_x = width as f32 / resize_w as f32;
    let scale_y = height as f32 / resize_h as f32;
    for y in 0..resize_h {
        let fy = (y as f32 + 0.5) * scale_y - 0.5;
        for x in 0..resize_w {
            let fx = (x as f32 + 0.5) * scale_x - 0.5;
            let pixel = sample_rgb(rgb, width, height, fx, fy);
            let offset = (y * resize_w + x) * 3;
            out[offset] = pixel[0].round().clamp(0.0, 255.0) as u8;
            out[offset + 1] = pixel[1].round().clamp(0.0, 255.0) as u8;
            out[offset + 2] = pixel[2].round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_pixel_resize_is_that_pixel() {
        let rgb = [10u8, 20, 30];
        let out = resize_rgb(&rgb, 1, 1, 3, 2);
        assert_eq!(out.len(), 3 * 2 * 3);
        for pixel in out.as_chunks::<3>().0 {
            assert_eq!(pixel, &[10, 20, 30]);
        }
    }

    #[test]
    fn bilinear_keeps_the_corner_colours() {
        // 2x2: red, green / blue, white.
        let rgb = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
        let out = resize_rgb(&rgb, 2, 2, 4, 4);
        let at = |x: usize, y: usize| {
            let offset = (y * 4 + x) * 3;
            [out[offset], out[offset + 1], out[offset + 2]]
        };
        assert_eq!(at(0, 0), [255, 0, 0]);
        assert_eq!(at(3, 0), [0, 255, 0]);
        assert_eq!(at(0, 3), [0, 0, 255]);
        assert_eq!(at(3, 3), [255, 255, 255]);
    }

    #[test]
    fn sampling_outside_clamps_instead_of_panicking() {
        let rgb = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let sample = sample_rgb(&rgb, 2, 2, -5.0, 9.0);
        assert_eq!(sample, [7.0, 8.0, 9.0]);
    }
}
