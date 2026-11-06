//! Put recognised lines into a stable reading order.
//!
//! This is deliberately simpler than a page-layout analyser: lines are grouped
//! into rows by their vertical centre and each row is read left to right. Search
//! does not depend on the order, and a two-column page may be interleaved; the
//! limitation is documented for operators.

use super::det::Quad;

/// One recognised line and the box it came from.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub text: String,
    pub quad: Quad,
}

#[derive(Clone, Copy, Debug)]
struct Bounds {
    x0: f32,
    cy: f32,
    h: f32,
}

impl Bounds {
    fn of(quad: Quad) -> Self {
        let mut x0 = f32::INFINITY;
        let mut y0 = f32::INFINITY;
        let mut y1 = f32::NEG_INFINITY;
        for point in quad.points {
            x0 = x0.min(point.x);
            y0 = y0.min(point.y);
            y1 = y1.max(point.y);
        }
        Self {
            x0,
            cy: (y0 + y1) / 2.0,
            h: y1 - y0,
        }
    }
}

/// Join lines into the text stored in the index.
pub(crate) fn join(lines: Vec<Line>) -> String {
    let mut lines: Vec<(Line, Bounds)> = lines
        .into_iter()
        .filter(|line| !line.text.trim().is_empty())
        .map(|line| {
            let bounds = Bounds::of(line.quad);
            (line, bounds)
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    lines.sort_by(|a, b| {
        a.1.cy
            .partial_cmp(&b.1.cy)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.1.x0
                    .partial_cmp(&b.1.x0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut rows: Vec<Vec<(Line, Bounds)>> = Vec::new();
    let mut row_cy = 0.0f32;
    let mut row_h = 0.0f32;
    for (line, bounds) in lines {
        if let Some(row) = rows.last_mut() {
            let tolerance = 0.6 * row_h.max(bounds.h);
            if (bounds.cy - row_cy).abs() <= tolerance {
                row_h = row_h.max(bounds.h);
                row.push((line, bounds));
                continue;
            }
        }
        row_cy = bounds.cy;
        row_h = bounds.h;
        rows.push(vec![(line, bounds)]);
    }

    let mut out = Vec::with_capacity(rows.len());
    for mut row in rows {
        row.sort_by(|a, b| {
            a.1.x0
                .partial_cmp(&b.1.x0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let text = row
            .into_iter()
            .map(|(line, _)| line.text.trim().to_string())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            out.push(text);
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::det::Point;

    fn line(text: &str, x: f32, y: f32, width: f32, height: f32) -> Line {
        Line {
            text: text.to_string(),
            quad: Quad {
                points: [
                    Point { x, y },
                    Point { x: x + width, y },
                    Point {
                        x: x + width,
                        y: y + height,
                    },
                    Point { x, y: y + height },
                ],
            },
        }
    }

    #[test]
    fn boxes_on_one_row_are_joined_with_a_space() {
        let text = join(vec![
            line("world", 60.0, 10.0, 40.0, 10.0),
            line("hello", 10.0, 11.0, 40.0, 10.0),
        ]);
        assert_eq!(text, "hello world");
    }

    #[test]
    fn lower_rows_follow_upper_rows() {
        let text = join(vec![
            line("second", 10.0, 40.0, 40.0, 10.0),
            line("first", 10.0, 10.0, 40.0, 10.0),
        ]);
        assert_eq!(text, "first\nsecond");
    }

    #[test]
    fn empty_lines_disappear() {
        let text = join(vec![
            line("  ", 10.0, 10.0, 40.0, 10.0),
            line(" kept ", 10.0, 40.0, 40.0, 10.0),
        ]);
        assert_eq!(text, "kept");
    }
}
