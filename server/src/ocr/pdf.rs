//! Rasterise the pages of a PDF that have no text layer and run OCR on them.
//!
//! The text layer stays with `lopdf`: this module only receives the page
//! numbers the indexer decided are scan-like, renders exactly those pages with
//! hayro, and returns one string per requested page. The OCR models are loaded
//! once for the whole request through [`Recognizer`], not once per page.

use std::time::{Duration, Instant};

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

use super::OcrOptions;
use super::engine::Recognizer;
use crate::indexer::extract::{MAX_INDEXED_CONTENT_BYTES, guard, reason};

/// Time a PDF request may spend rendering and recognising before it returns
/// what it has.
///
/// The worker's hard wall is 180 s. This is not a smaller resource budget: it
/// is the point at which the child stops starting new pages so the pages it
/// already read reach the parent instead of dying with a killed child.
pub const PDF_OCR_SOFT_DEADLINE: Duration = Duration::from_secs(150);

/// Why a PDF request produced no page text.
#[derive(Debug)]
pub enum PdfRefusal {
    /// A verdict on the bytes: parse/encryption failure, a panic, or every
    /// requested page failed to render.
    NoText(&'static str),
    /// A model or renderer environment failure; the caller retries.
    Unavailable(String),
}

/// Render and recognise the requested 1-based page numbers.
///
/// The returned vector has one entry per requested page, in the same order.
/// Pages the soft deadline, the text cap or an out-of-range number skipped are
/// empty strings; the indexer keeps those pages' text layer.
pub fn recognize(
    data: Vec<u8>,
    pages: &[u32],
    options: OcrOptions,
) -> Result<Vec<String>, PdfRefusal> {
    guard(
        "pdf-ocr",
        |_| Err(PdfRefusal::NoText(reason::PANIC)),
        move || recognize_inner(data, pages, options),
    )
}

fn recognize_inner(
    data: Vec<u8>,
    pages: &[u32],
    options: OcrOptions,
) -> Result<Vec<String>, PdfRefusal> {
    let started = Instant::now();
    let pdf = match Pdf::new(data) {
        Ok(pdf) => pdf,
        Err(LoadPdfError::Decryption(error)) => {
            tracing::debug!("pdf ocr: cannot decrypt: {error:?}");
            return Err(PdfRefusal::NoText(reason::ENCRYPTED));
        }
        Err(LoadPdfError::Invalid) => return Err(PdfRefusal::NoText(reason::PARSE)),
    };

    let page_count = pdf.pages().len();
    let mut results = vec![String::new(); pages.len()];
    let mut recognizer: Option<Recognizer> = None;
    let cache = RenderCache::new();
    let interpreter_settings = InterpreterSettings::default();
    let render_settings = RenderSettings::default();
    let mut last_page = Duration::ZERO;
    let mut text_bytes = 0usize;
    let mut rendered = 0usize;
    let mut failed = 0usize;

    for (slot, &page_number) in pages.iter().enumerate() {
        if soft_deadline_reached(started.elapsed(), last_page) {
            tracing::debug!(
                "pdf ocr: soft deadline reached after {rendered} page(s); {} left blank",
                pages.len() - slot
            );
            break;
        }
        if text_bytes >= MAX_INDEXED_CONTENT_BYTES {
            break;
        }

        let Some(index) = page_number
            .checked_sub(1)
            .map(|number| number as usize)
            .filter(|index| *index < page_count)
        else {
            continue;
        };
        let Some(page) = pdf.pages().get(index) else {
            continue;
        };
        let page_started = Instant::now();
        let page_result = render_page(
            page,
            options,
            &mut recognizer,
            &cache,
            &interpreter_settings,
            &render_settings,
        );
        last_page = page_started.elapsed();
        match page_result {
            PageOutcome::Text(mut text) => {
                rendered += 1;
                text.retain(|character| character != '\u{1e}');
                let remaining = MAX_INDEXED_CONTENT_BYTES.saturating_sub(text_bytes);
                if text.len() > remaining {
                    let cut = floor_char_boundary(&text, remaining);
                    text.truncate(cut);
                }
                text_bytes += text.len();
                results[slot] = text;
            }
            PageOutcome::RenderFailed => failed += 1,
            PageOutcome::Unavailable(reason) => return Err(PdfRefusal::Unavailable(reason)),
        }
    }

    if rendered == 0 && failed > 0 {
        return Err(PdfRefusal::NoText(reason::PARSE));
    }
    Ok(results)
}

/// What one page did.
enum PageOutcome {
    /// The page rendered and was recognised; the text may be empty.
    Text(String),
    /// This page could not be rendered; the next page may still work.
    RenderFailed,
    /// The models could not load or run; the whole request is retryable.
    Unavailable(String),
}

/// Render one page and recognise it, loading the models on first use.
fn render_page<'a>(
    page: &'a hayro::hayro_syntax::page::Page<'a>,
    options: OcrOptions,
    recognizer: &mut Option<Recognizer>,
    cache: &RenderCache<'a>,
    interpreter_settings: &InterpreterSettings,
    render_settings: &RenderSettings,
) -> PageOutcome {
    let (width, height) = page.render_dimensions();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return PageOutcome::RenderFailed;
    }
    let long_edge = width.max(height);
    let scale = (options.max_side as f32 / long_edge).clamp(0.05, 4.0);
    let (scaled_width, scaled_height) = (width * scale, height * scale);
    if scaled_width < 1.0
        || scaled_height < 1.0
        || scaled_width > f32::from(u16::MAX)
        || scaled_height > f32::from(u16::MAX)
    {
        return PageOutcome::RenderFailed;
    }

    let pixmap = render(
        page,
        cache,
        interpreter_settings,
        render_settings,
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let png = match pixmap.into_png() {
        Ok(png) => png,
        Err(error) => {
            tracing::debug!("pdf ocr: cannot encode a rendered page: {error}");
            return PageOutcome::RenderFailed;
        }
    };
    let image = match image::load_from_memory(&png) {
        Ok(image) => image,
        Err(error) => {
            tracing::debug!("pdf ocr: cannot decode a rendered page: {error}");
            return PageOutcome::RenderFailed;
        }
    };
    if recognizer.is_none() {
        match Recognizer::new() {
            Ok(new) => *recognizer = Some(new),
            Err(error) => return PageOutcome::Unavailable(error),
        }
    }
    let Some(recognizer) = recognizer.as_mut() else {
        return PageOutcome::Unavailable("the OCR recognizer is unavailable".to_string());
    };
    match recognizer.recognize(&image, options) {
        Ok(text) => PageOutcome::Text(text),
        Err(error) => PageOutcome::Unavailable(error),
    }
}

/// Whether the next page is unlikely to finish inside the soft deadline.
///
/// The first page always runs. After it, the previous page's duration is the
/// only estimate available; adding it to the elapsed time leaves room for one
/// more page of the same cost, not merely for one more start.
fn soft_deadline_reached(elapsed: Duration, last_page: Duration) -> bool {
    !last_page.is_zero() && elapsed + last_page > PDF_OCR_SOFT_DEADLINE
}

/// Largest index `<= i` that is a UTF-8 char boundary of `s`.
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_char_boundary_is_never_split() {
        let text = "a中b";
        assert_eq!(floor_char_boundary(text, 1), 1);
        assert_eq!(floor_char_boundary(text, 2), 1);
        assert_eq!(floor_char_boundary(text, 4), 4);
        assert_eq!(floor_char_boundary(text, 99), text.len());
    }

    #[test]
    fn the_soft_deadline_reserves_the_previous_page_cost() {
        assert!(!soft_deadline_reached(Duration::ZERO, Duration::ZERO));
        assert!(!soft_deadline_reached(
            Duration::from_secs(100),
            Duration::from_secs(10)
        ));
        assert!(soft_deadline_reached(
            Duration::from_secs(145),
            Duration::from_secs(10)
        ));
    }

    #[test]
    fn a_pdf_with_no_pages_returns_no_page_text() {
        let result = recognize(Vec::new(), &[1], OcrOptions::default());
        assert!(matches!(result, Err(PdfRefusal::NoText(reason::PARSE))));
    }
}
