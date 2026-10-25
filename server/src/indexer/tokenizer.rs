//! Dictionary-free tokenization for the content field.
//!
//! The content analyzer used to be jieba. Its embedded word list costs about
//! 2.1 MiB in the binary, a zstd C dependency only to decompress that list,
//! and ~33 MiB of RSS once loaded. Chinese full-text search does not need a
//! dictionary: cutting CJK runs into overlapping bigrams and letting
//! `QueryParser` turn a multi-token query into a `PhraseQuery` matches the same
//! substrings, while a dictionary-free analyzer cannot disagree between
//! indexing and query time.
//!
//! Latin text keeps whole-word tokens, split the same way `SimpleTokenizer`
//! splits it, so English search is unchanged; only CJK runs are cut into
//! bigrams.
//!
//! The stream is lazy: a document may hold up to 8 MiB of text, and building a
//! `Token` per bigram up front would allocate hundreds of MiB. It keeps one
//! reusable token instead.

use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

/// Cuts each run of CJK characters into overlapping two-character tokens and
/// leaves a one-character run as a single token.
///
/// Token positions are consecutive per emitted token, so the bigrams of
/// `北京大学` sit at positions 0, 1 and 2 and a query for the same text becomes
/// a `PhraseQuery` requiring adjacent bigrams, not merely their presence.
#[derive(Clone, Default)]
pub(crate) struct CjkBigramTokenizer;

impl CjkBigramTokenizer {
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl Tokenizer for CjkBigramTokenizer {
    type TokenStream<'a> = CjkBigramTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        CjkBigramTokenStream {
            text,
            i: 0,
            in_cjk_run: false,
            run_end: 0,
            position: 0,
            token: Token::default(),
        }
    }
}

/// Token stream produced by [`CjkBigramTokenizer`].
pub(crate) struct CjkBigramTokenStream<'a> {
    text: &'a str,
    /// Byte offset of the next character to inspect, or of the bigram to emit
    /// while `in_cjk_run` is set.
    i: usize,
    /// Whether the next bigram starts at `i` inside the run ending at `run_end`.
    in_cjk_run: bool,
    run_end: usize,
    position: usize,
    token: Token,
}

impl TokenStream for CjkBigramTokenStream<'_> {
    fn advance(&mut self) -> bool {
        loop {
            if self.in_cjk_run {
                let Some((_, cur_end)) = char_at(self.text, self.i) else {
                    self.in_cjk_run = false;
                    self.i = self.run_end;
                    continue;
                };
                if cur_end >= self.run_end {
                    // The last character of the run was the end of the last
                    // bigram; the run is finished and produces nothing more.
                    self.in_cjk_run = false;
                    self.i = self.run_end;
                    continue;
                }
                let Some((_, next_end)) = char_at(self.text, cur_end) else {
                    self.in_cjk_run = false;
                    self.i = self.run_end;
                    continue;
                };
                self.emit(self.i, next_end);
                self.i = cur_end;
                return true;
            }

            let Some((c, next)) = char_at(self.text, self.i) else {
                return false;
            };
            if is_cjk(c) {
                let run_end = self.cjk_run_end(self.i);
                let first_end = next;
                if first_end >= run_end {
                    // A one-character run stays a token of its own.
                    self.emit(self.i, first_end);
                    self.i = first_end;
                    return true;
                }
                // The first bigram starts at the run start; the stream then
                // walks its start forward one character at a time.
                self.emit(
                    self.i,
                    char_at(self.text, first_end).map_or(first_end, |(_, e)| e),
                );
                self.run_end = run_end;
                self.in_cjk_run = true;
                self.i = first_end;
                return true;
            }
            if c.is_alphanumeric() {
                let start = self.i;
                let mut end = next;
                self.i = next;
                while let Some((next_char, after)) = char_at(self.text, self.i) {
                    if is_cjk(next_char) || !next_char.is_alphanumeric() {
                        break;
                    }
                    end = after;
                    self.i = after;
                }
                self.emit(start, end);
                return true;
            }
            self.i = next;
        }
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}

impl CjkBigramTokenStream<'_> {
    /// Byte offset just past the last CJK character of the run starting at
    /// `start`.
    fn cjk_run_end(&self, start: usize) -> usize {
        let mut end = start;
        let mut i = start;
        while let Some((c, next)) = char_at(self.text, i) {
            if !is_cjk(c) {
                break;
            }
            end = next;
            i = next;
        }
        end
    }

    fn emit(&mut self, from: usize, to: usize) {
        self.token.offset_from = from;
        self.token.offset_to = to;
        self.token.position = self.position;
        self.token.position_length = 1;
        self.token.text.clear();
        self.token.text.push_str(&self.text[from..to]);
        self.position += 1;
    }
}

/// The character at byte offset `i`, and the byte offset just past it.
fn char_at(text: &str, i: usize) -> Option<(char, usize)> {
    let c = text.get(i..)?.chars().next()?;
    Some((c, i + c.len_utf8()))
}

/// Whether `c` is in one of the CJK Unified Ideographs blocks.
///
/// These are the ranges kana and Hangul are deliberately not part of: they are
/// left to the alphanumeric path, and a dictionary would not segment them
/// better than a run token does.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{2A6DF}'
        | '\u{2A700}'..='\u{2B73F}'
        | '\u{2B740}'..='\u{2B81F}'
        | '\u{2B820}'..='\u{2CEAF}'
        | '\u{2CEB0}'..='\u{2EBEF}'
        | '\u{2EBF0}'..='\u{2EE5F}'
        | '\u{2F800}'..='\u{2FA1F}'
        | '\u{30000}'..='\u{3134F}'
        | '\u{31350}'..='\u{323AF}'
    )
}

#[cfg(test)]
mod tests {
    use super::CjkBigramTokenizer;
    use tantivy::tokenizer::{LowerCaser, TextAnalyzer, TokenStream, Tokenizer};

    /// `(text, offset_from, offset_to, position)` for every emitted token.
    fn collect(text: &str) -> Vec<(String, usize, usize, usize)> {
        let mut tokenizer = CjkBigramTokenizer::new();
        let mut stream = tokenizer.token_stream(text);
        let mut tokens = Vec::new();
        while let Some(token) = stream.next() {
            tokens.push((
                token.text.clone(),
                token.offset_from,
                token.offset_to,
                token.position,
            ));
        }
        tokens
    }

    #[test]
    fn splits_cjk_runs_into_overlapping_bigrams() {
        assert_eq!(
            collect("北京大学"),
            vec![
                ("北京".to_string(), 0, 6, 0),
                ("京大".to_string(), 3, 9, 1),
                ("大学".to_string(), 6, 12, 2),
            ]
        );
    }

    #[test]
    fn one_character_cjk_run_is_a_single_token() {
        assert_eq!(collect("北"), vec![("北".to_string(), 0, 3, 0)]);
    }

    #[test]
    fn latin_words_stay_whole_and_positions_ignore_separators() {
        assert_eq!(
            collect("Hello 北京"),
            vec![
                ("Hello".to_string(), 0, 5, 0),
                ("北京".to_string(), 6, 12, 1),
            ]
        );
    }

    #[test]
    fn punctuation_separates_runs() {
        assert_eq!(
            collect("北京-大学"),
            vec![
                ("北京".to_string(), 0, 6, 0),
                ("大学".to_string(), 7, 13, 1),
            ]
        );
        assert!(collect("，。！").is_empty());
    }

    #[test]
    fn digits_and_cjk_are_separate_runs() {
        assert_eq!(
            collect("4S店"),
            vec![("4S".to_string(), 0, 2, 0), ("店".to_string(), 2, 5, 1)]
        );
    }

    #[test]
    fn mixed_scripts_keep_their_own_boundaries() {
        assert_eq!(
            collect("a北"),
            vec![("a".to_string(), 0, 1, 0), ("北".to_string(), 1, 4, 1)]
        );
    }

    #[test]
    fn cjk_extension_blocks_are_recognized() {
        // U+20000 is four bytes and belongs to CJK Unified Ideographs Extension B.
        assert_eq!(
            collect("\u{20000}好"),
            vec![("\u{20000}好".to_string(), 0, 7, 0)]
        );
    }

    #[test]
    fn empty_and_non_text_input_produce_no_tokens() {
        assert!(collect("").is_empty());
        assert!(collect("😀").is_empty());
        assert_eq!(collect("😀好"), vec![("好".to_string(), 4, 7, 0)]);
    }

    #[test]
    fn lower_caser_still_applies_to_latin_tokens() {
        let mut analyzer = TextAnalyzer::builder(CjkBigramTokenizer::new())
            .filter(LowerCaser)
            .build();
        let mut stream = analyzer.token_stream("Hello北京");
        let mut texts = Vec::new();
        while let Some(token) = stream.next() {
            texts.push(token.text.clone());
        }
        assert_eq!(texts, vec!["hello".to_string(), "北京".to_string()]);
    }
}
