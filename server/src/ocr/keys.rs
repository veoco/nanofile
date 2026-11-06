//! The PP-OCRv6 tiny recognition alphabet.
//!
//! The embedded file has one character per line. The model's output has two
//! extra classes: index 0 is the CTC blank and the last index is a space (the
//! export enables `use_space_char`). `ppocr_keys_v6.txt` therefore has 6,904
//! lines and the alphabet has 6,906 entries.

use std::sync::OnceLock;

/// The characters the recognition graph can emit, in model order.
pub const KEYS: &str = include_str!("../../models/ppocr_keys_v6.txt");

/// Output classes: blank + dictionary + space.
pub const CLASSES: usize = 6906;

static ALPHABET: OnceLock<Vec<&'static str>> = OnceLock::new();

/// The alphabet indexed by the model's class id.
///
/// `index 0` is the CTC blank (empty string), `1..=6904` are the dictionary
/// entries and `6905` is the trailing space.
pub fn alphabet() -> &'static [&'static str] {
    ALPHABET.get_or_init(|| {
        let mut alphabet = Vec::with_capacity(CLASSES);
        alphabet.push("");
        alphabet.extend(KEYS.lines());
        alphabet.push(" ");
        debug_assert_eq!(alphabet.len(), CLASSES);
        alphabet
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alphabet_has_a_blank_dictionary_and_space() {
        let alphabet = alphabet();
        assert_eq!(alphabet.len(), CLASSES);
        assert_eq!(alphabet[0], "");
        assert_eq!(alphabet[1], "!");
        assert_eq!(alphabet[617], "\u{3000}");
        assert_eq!(alphabet[CLASSES - 1], " ");
        assert_eq!(alphabet[CLASSES - 2], "￥");
    }
}
