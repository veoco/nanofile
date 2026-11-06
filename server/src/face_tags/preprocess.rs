//! Letterbox/preprocess for YuNet.
//!
//! YuNet's published ONNX graph has a fixed 640x640 input and expects BGR in
//! 0..=255 without mean subtraction. We preserve the aspect ratio and pad the
//! right/bottom with zeros, matching OpenCV's `copyMakeBorder` behaviour.

use image::DynamicImage;
use image::imageops::FilterType;

/// Input edge length the model was exported with.
pub const INPUT_SIZE: u32 = 640;

pub struct Prepared {
    /// CHW BGR, 0..=255, inside the 640x640 canvas.
    pub data: Vec<f32>,
    /// `prepared_pixels / original_pixels`; the same scale on both axes.
    pub scale: f32,
    /// Size of the resized image inside the 640x640 canvas.
    pub resized_w: u32,
    pub resized_h: u32,
}

/// Resize `image` to fit inside `INPUT_SIZE` and pad to a square BGR plane.
pub fn prepare(image: &DynamicImage) -> Result<Prepared, String> {
    let rgb = image.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    if w == 0 || h == 0 {
        return Err("image has zero dimensions".to_string());
    }
    let scale = INPUT_SIZE as f32 / w.max(h) as f32;
    let resized_w = ((w as f32 * scale).round() as u32).clamp(1, INPUT_SIZE);
    let resized_h = ((h as f32 * scale).round() as u32).clamp(1, INPUT_SIZE);
    let resized = image::imageops::resize(&rgb, resized_w, resized_h, FilterType::Triangle);

    let plane = (INPUT_SIZE * INPUT_SIZE) as usize;
    let mut data = vec![0f32; plane * 3];
    for y in 0..resized_h as usize {
        for x in 0..resized_w as usize {
            let pixel = resized.get_pixel(x as u32, y as u32);
            let idx = y * INPUT_SIZE as usize + x;
            data[idx] = pixel[2] as f32; // B
            data[plane + idx] = pixel[1] as f32; // G
            data[plane * 2 + idx] = pixel[0] as f32; // R
        }
    }

    Ok(Prepared {
        data,
        scale,
        resized_w,
        resized_h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    #[test]
    fn letterbox_keeps_aspect_and_pads_black() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(320, 160, image::Rgb([10, 20, 30])));
        let prepared = prepare(&img).expect("prepare");
        assert_eq!((prepared.resized_w, prepared.resized_h), (640, 320));
        let plane = (INPUT_SIZE * INPUT_SIZE) as usize;
        // Planes are B, G, R; the top-left pixel is (10,20,30) RGB.
        assert_eq!(prepared.data[0], 30.0);
        assert_eq!(prepared.data[plane], 20.0);
        assert_eq!(prepared.data[plane * 2], 10.0);
        // The bottom half is padding.
        assert_eq!(prepared.data[320 * INPUT_SIZE as usize], 0.0);
    }
}
