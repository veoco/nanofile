//! Text line orientation classification: the two-class PP-LCNet head.
//!
//! Detection crops one line at a time after the page has been turned upright.
//! This classifier catches the line that is still upside down inside an
//! otherwise upright page: it returns `0_degree` or `180_degree`, and `upright`
//! flips the crop before recognition. The graph already ends in softmax, so
//! `decode` takes the largest score rather than normalising again.
//!
//! The graph is PP-LCNet_x0_25_textline_ori (Apache-2.0); its `inference.yml`
//! is the source of the 160×80 resize and the normalisation constants.

use super::crop::{self, Crop};
use super::resample::{IMAGENET_MEAN, IMAGENET_STD, resize_rgb};

/// Width of the classifier input.
pub(crate) const WIDTH: usize = 160;
/// Height of the classifier input.
pub(crate) const HEIGHT: usize = 80;
/// The model has one score per direction.
pub(crate) const CLASSES: usize = 2;

/// Whether a detected line needs a half turn before recognition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Turn {
    Upright,
    UpsideDown,
}

impl Turn {
    /// Map a class index from the model's `label_list` (`0_degree`/`180_degree`).
    pub(crate) fn from_label(label: usize) -> Option<Self> {
        match label {
            0 => Some(Self::Upright),
            1 => Some(Self::UpsideDown),
            _ => None,
        }
    }

    /// Make the line upright; `Upright` returns it unchanged.
    pub(crate) fn upright(self, crop: Crop) -> Crop {
        match self {
            Self::Upright => crop,
            Self::UpsideDown => crop::flip_180(crop),
        }
    }
}

/// Resize and normalise one line crop into the classifier's CHW BGR input.
///
/// `inference.yml` resizes the whole crop to 160×80 without keeping its aspect
/// ratio, then applies the same ImageNet normalisation as the page classifier.
pub(crate) fn tensor(rgb: &[u8], width: usize, height: usize) -> Vec<f32> {
    let resized = resize_rgb(rgb, width, height, WIDTH, HEIGHT);
    let plane = WIDTH * HEIGHT;
    let mut data = vec![0f32; plane * 3];
    for index in 0..plane {
        for channel in 0..3 {
            let value = resized[index * 3 + (2 - channel)] as f32 / 255.0;
            data[channel * plane + index] =
                (value - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
        }
    }
    data
}

/// Pick the most likely line direction. The graph already ends in softmax.
pub(crate) fn decode(probabilities: &[f32]) -> Option<(Turn, f32)> {
    if probabilities.len() != CLASSES {
        return None;
    }
    let (label, score) = probabilities
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(right.1))?;
    Some((Turn::from_label(label)?, *score))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn labels_map_to_the_direction_they_correct() {
        for (label, expected) in [(0, Turn::Upright), (1, Turn::UpsideDown)] {
            let mut probabilities = [0.01f32; CLASSES];
            probabilities[label] = 0.99;
            let (turn, score) = decode(&probabilities).expect("two scores");
            assert_eq!(turn, expected, "label {label}");
            assert!((score - 0.99).abs() < 1e-6);
        }
        assert_eq!(decode(&[0.5f32; 3]), None);
    }

    #[test]
    fn the_model_input_is_bgr_and_three_planes() {
        let red = RgbImage::from_pixel(WIDTH as u32, HEIGHT as u32, Rgb([255, 0, 0]));
        let data = tensor(red.as_raw(), WIDTH, HEIGHT);
        assert_eq!(data.len(), 3 * WIDTH * HEIGHT);
        let plane = WIDTH * HEIGHT;
        assert!((data[0] - (0.0 - 0.485) / 0.229).abs() < 1e-5);
        assert!((data[plane] - (0.0 - 0.456) / 0.224).abs() < 1e-5);
        assert!((data[2 * plane] - (1.0 - 0.406) / 0.225).abs() < 1e-5);
    }
}
