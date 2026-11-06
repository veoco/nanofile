//! Page orientation classification: the four-class PP-LCNet head.
//!
//! The classifier looks at the whole picture before detection. It returns the
//! counter-clockwise turn (`0`, `90`, `180` or `270`) that makes the page
//! upright. `tensor` samples the classifier's 224×224 centre crop directly from
//! the source, so a picture with an extreme aspect ratio never allocates an
//! intermediate resize buffer; `upright` applies the turn with the image crate's
//! lossless rotations.
//!
//! The graph is PP-LCNet_x1_0_doc_ori (Apache-2.0); its `inference.yml` is the
//! source of the preprocessing constants. The graph already ends in softmax, so
//! `decode` takes the largest score rather than normalising again.

use image::{RgbImage, imageops};

use super::resample::{IMAGENET_MEAN, IMAGENET_STD, sample_rgb};

/// Side of the square the classifier consumes.
pub(crate) const SIZE: usize = 224;
/// The shorter edge is resized to this before the centre crop.
const SHORT_SIDE: f32 = 256.0;
/// The model has one score per quarter turn.
pub(crate) const CLASSES: usize = 4;

/// The counter-clockwise turn that makes a page upright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rotation {
    Upright,
    Ccw90,
    Ccw180,
    Ccw270,
}

impl Rotation {
    /// Map a class index from the model's `label_list` (`0/90/180/270`).
    pub(crate) fn from_label(label: usize) -> Option<Self> {
        match label {
            0 => Some(Self::Upright),
            1 => Some(Self::Ccw90),
            2 => Some(Self::Ccw180),
            3 => Some(Self::Ccw270),
            _ => None,
        }
    }

    /// Turn a decoded RGB picture upright. `Upright` returns it unchanged.
    pub(crate) fn upright(self, image: RgbImage) -> RgbImage {
        match self {
            Self::Upright => image,
            // `rotate270` is 270° clockwise, which is 90° counter-clockwise.
            Self::Ccw90 => imageops::rotate270(&image),
            Self::Ccw180 => imageops::rotate180(&image),
            Self::Ccw270 => imageops::rotate90(&image),
        }
    }
}

/// Normalise the 224×224 centre crop into the classifier's CHW BGR input.
///
/// The source coordinate of a crop pixel follows from the rounded resize
/// dimensions, matching OpenCV's resize mapping followed by the centre crop.
pub(crate) fn tensor(rgb: &[u8], width: usize, height: usize) -> Vec<f32> {
    let plane = SIZE * SIZE;
    let mut data = vec![0f32; plane * 3];
    if width == 0 || height == 0 || rgb.len() < width * height * 3 {
        return data;
    }

    let short = width.min(height) as f32;
    let resized_w = ((width as f32 * SHORT_SIDE / short).round() as usize).max(1);
    let resized_h = ((height as f32 * SHORT_SIDE / short).round() as usize).max(1);
    let left = resized_w.saturating_sub(SIZE) / 2;
    let top = resized_h.saturating_sub(SIZE) / 2;
    let ratio_x = width as f32 / resized_w as f32;
    let ratio_y = height as f32 / resized_h as f32;

    for y in 0..SIZE {
        let source_y = ((y + top) as f32 + 0.5) * ratio_y - 0.5;
        for x in 0..SIZE {
            let source_x = ((x + left) as f32 + 0.5) * ratio_x - 0.5;
            let pixel = sample_rgb(rgb, width, height, source_x, source_y);
            let index = y * SIZE + x;
            for channel in 0..3 {
                let value = pixel[2 - channel] / 255.0;
                data[channel * plane + index] =
                    (value - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
            }
        }
    }
    data
}

/// Pick the most likely quarter turn. The graph already ends in softmax.
pub(crate) fn decode(probabilities: &[f32]) -> Option<(Rotation, f32)> {
    if probabilities.len() != CLASSES {
        return None;
    }
    let (label, score) = probabilities
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(right.1))?;
    Some((Rotation::from_label(label)?, *score))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn labels_map_to_the_quarter_turn_they_correct() {
        for (label, expected) in [
            (0, Rotation::Upright),
            (1, Rotation::Ccw90),
            (2, Rotation::Ccw180),
            (3, Rotation::Ccw270),
        ] {
            let mut probabilities = [0.025f32; CLASSES];
            probabilities[label] = 0.925;
            let (rotation, score) = decode(&probabilities).expect("four scores");
            assert_eq!(rotation, expected, "label {label}");
            assert!((score - 0.925).abs() < 1e-6);
        }
        assert_eq!(decode(&[0.5f32; 3]), None);
    }

    #[test]
    fn the_model_input_is_bgr_and_three_planes() {
        let red = RgbImage::from_pixel(256, 256, Rgb([255, 0, 0]));
        let data = tensor(red.as_raw(), 256, 256);
        assert_eq!(data.len(), 3 * SIZE * SIZE);
        let plane = SIZE * SIZE;
        // Channel 0 is blue, channel 2 is red, as in the published pipeline.
        assert!((data[0] - (0.0 - 0.485) / 0.229).abs() < 1e-5);
        assert!((data[plane] - (0.0 - 0.456) / 0.224).abs() < 1e-5);
        assert!((data[2 * plane] - (1.0 - 0.406) / 0.225).abs() < 1e-5);
    }

    #[test]
    fn turning_the_picture_moves_pixels_as_labelled() {
        // Two columns, three rows: A B / C D / E F.
        let image = RgbImage::from_fn(2, 3, |x, y| Rgb([(y * 2 + x) as u8 * 10, 0, 0]));
        let at = |image: &RgbImage, x: u32, y: u32| image.get_pixel(x, y).0[0];

        let turned = Rotation::Ccw90.upright(image.clone());
        assert_eq!(turned.dimensions(), (3, 2));
        assert_eq!(
            [
                at(&turned, 0, 0),
                at(&turned, 2, 0),
                at(&turned, 0, 1),
                at(&turned, 2, 1)
            ],
            [10, 50, 0, 40],
            "90° counter-clockwise"
        );

        let turned = Rotation::Ccw180.upright(image.clone());
        assert_eq!(turned.dimensions(), (2, 3));
        assert_eq!(
            [
                at(&turned, 0, 0),
                at(&turned, 1, 0),
                at(&turned, 0, 2),
                at(&turned, 1, 2)
            ],
            [50, 40, 10, 0],
            "180°"
        );

        let turned = Rotation::Ccw270.upright(image.clone());
        assert_eq!(turned.dimensions(), (3, 2));
        assert_eq!(
            [
                at(&turned, 0, 0),
                at(&turned, 2, 0),
                at(&turned, 0, 1),
                at(&turned, 2, 1)
            ],
            [40, 0, 50, 10],
            "270° counter-clockwise"
        );

        assert_eq!(Rotation::Upright.upright(image.clone()), image);
        let round_trip = Rotation::Ccw90.upright(imageops::rotate90(&image));
        assert_eq!(round_trip, image, "90° clockwise undone by 90° CCW");
    }

    #[test]
    fn an_extreme_aspect_ratio_still_yields_one_tensor() {
        let rgb = vec![0u8; 3 * 8192];
        let data = tensor(&rgb, 1, 8192);
        assert_eq!(data.len(), 3 * SIZE * SIZE);
        assert!(data.iter().all(|value| value.is_finite()));
    }
}
