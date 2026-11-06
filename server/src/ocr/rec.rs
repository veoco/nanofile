//! Text-line recognition: normalisation, CTC decoding and line widths.
//!
//! The recognition graph is fully convolutional in the time axis: it accepts
//! `48 x W` and returns `T x 6906`. Widths are bucketed so one child prepares at
//! most a handful of plans, and each line is padded on the right with zeros the
//! same way Paddle's `resize_norm_img` pads.

use super::crop::Crop;
use super::keys;
use super::resample::resize_rgb;

/// Text-line heights the model was trained with.
pub(crate) const HEIGHT: usize = 48;
/// Widest input any one line may be resized to.
pub(crate) const MAX_WIDTH: usize = 1280;
/// Width buckets. A crop is padded up to the next bucket.
pub(crate) const BUCKETS: [usize; 5] = [160, 320, 640, 960, 1280];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Decoded {
    pub text: String,
    pub score: f32,
}

/// The width this crop would like, before bucketing.
pub(crate) fn target_width(crop: &Crop) -> usize {
    let width = (crop.width as f32 * HEIGHT as f32 / crop.height.max(1) as f32).round() as usize;
    width.clamp(8, MAX_WIDTH)
}

/// The smallest bucket that holds `width`.
pub(crate) fn bucket_for(width: usize) -> usize {
    BUCKETS
        .iter()
        .copied()
        .find(|bucket| *bucket >= width)
        .unwrap_or(MAX_WIDTH)
}

/// Normalised CHW BGR input, `48 x bucket`, padded on the right.
pub(crate) fn tensor(crop: &Crop, bucket: usize) -> Vec<f32> {
    let target = target_width(crop).min(bucket);
    let resized = resize_rgb(&crop.rgb, crop.width, crop.height, target, HEIGHT);
    let plane = HEIGHT * bucket;
    let mut data = vec![0f32; plane * 3];
    for y in 0..HEIGHT {
        for x in 0..target {
            let source = y * target + x;
            let destination = y * bucket + x;
            for channel in 0..3 {
                let value = resized[source * 3 + (2 - channel)] as f32 / 255.0;
                data[channel * plane + destination] = (value - 0.5) / 0.5;
            }
        }
    }
    data
}

/// Greedy CTC decode.
///
/// The graph ends in softmax, so the max value is already a probability; the
/// exponential fallback keeps a logits export working too.
pub(crate) fn decode(
    logits: &[f32],
    time: usize,
    classes: usize,
    min_score: f32,
) -> Option<Decoded> {
    let alphabet = keys::alphabet();
    if classes != alphabet.len() || logits.len() < time * classes {
        return None;
    }

    let mut text = String::new();
    let mut total = 0.0f32;
    let mut count = 0usize;
    let mut previous = usize::MAX;

    for step in 0..time {
        let row = &logits[step * classes..(step + 1) * classes];
        let mut best = 0usize;
        let mut max = f32::NEG_INFINITY;
        for (index, value) in row.iter().enumerate() {
            if *value > max {
                max = *value;
                best = index;
            }
        }
        if best != 0 && best != previous {
            text.push_str(alphabet[best]);
            let score = if !(0.0..=1.0).contains(&max) {
                let normalizer: f32 = row.iter().map(|value| (value - max).exp()).sum();
                1.0 / normalizer
            } else {
                max
            };
            total += score;
            count += 1;
        }
        previous = best;
    }

    let text = text.trim().to_string();
    if text.is_empty() || count == 0 {
        return None;
    }
    let score = total / count as f32;
    (score >= min_score).then_some(Decoded { text, score })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logits(rows: &[(usize, f32)]) -> Vec<f32> {
        let classes = keys::CLASSES;
        let mut data = vec![0f32; rows.len() * classes];
        for (step, (class, value)) in rows.iter().enumerate() {
            data[step * classes + class] = *value;
        }
        data
    }

    #[test]
    fn repeats_and_blanks_collapse() {
        let data = logits(&[(1, 0.9), (1, 0.8), (0, 0.99), (2, 0.7)]);
        let decoded = decode(&data, 4, keys::CLASSES, 0.5).expect("text");
        assert_eq!(decoded.text, "!\"");
        assert!((decoded.score - 0.8).abs() < 1e-6);
    }

    #[test]
    fn the_trailing_space_class_is_a_space() {
        let data = logits(&[(1, 0.9), (keys::CLASSES - 1, 0.9), (2, 0.9)]);
        let decoded = decode(&data, 3, keys::CLASSES, 0.5).expect("text");
        assert_eq!(decoded.text, "! \"");
    }

    #[test]
    fn an_all_space_line_is_dropped() {
        let data = logits(&[(keys::CLASSES - 1, 0.95)]);
        // The decoded text is trimmed, so an all-space line is not indexable.
        assert!(decode(&data, 1, keys::CLASSES, 0.5).is_none());
    }

    #[test]
    fn a_low_confidence_line_is_dropped() {
        let data = logits(&[(1, 0.2)]);
        assert!(decode(&data, 1, keys::CLASSES, 0.5).is_none());
    }

    #[test]
    fn the_wrong_class_count_is_rejected() {
        let data = vec![0f32; 4 * 10];
        assert!(decode(&data, 4, 10, 0.0).is_none());
    }

    #[test]
    fn widths_are_bucketed() {
        assert_eq!(bucket_for(8), 160);
        assert_eq!(bucket_for(160), 160);
        assert_eq!(bucket_for(161), 320);
        assert_eq!(bucket_for(1280), 1280);
        assert_eq!(bucket_for(5000), 1280);
    }
}
