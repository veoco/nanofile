//! DB text detection: geometry, normalisation and box extraction.
//!
//! The graph is PP-OCRv6 tiny's DB detector. Its output is a probability map
//! the size of the input; `postprocess` turns that map into text boxes with
//! the same steps as PaddleOCR's `DBPostProcess`:
//!
//! 1. threshold the map and label its 8-connected components;
//! 2. take each component's boundary points, convex hull and minimum-area
//!    rectangle;
//! 3. score the tight rectangle, drop low-confidence components and expand the
//!    survivors by the DB unclip distance.
//!
//! This is a Rust implementation of the published algorithm; the steps follow
//! PaddleOCR's `DBPostProcess` and tract's PP-OCRv6 example. PP-OCRv6 is
//! Apache-2.0.

use std::collections::HashMap;

use super::resample::{IMAGENET_MEAN, IMAGENET_STD, resize_rgb};

/// How the source image is resized before detection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
    pub ratio_w: f32,
    pub ratio_h: f32,
}

impl Geometry {
    /// Paddle's `DetResizeForTest` with `limit_type = "max"`.
    ///
    /// Both dimensions are rounded to a multiple of 32, so the network's
    /// downsampling never leaves a partial cell on the edge.
    pub fn new(width: usize, height: usize, max_side: u32) -> Self {
        let limit = max_side.clamp(320, 1600) as f32;
        let longest = width.max(height) as f32;
        let scale = if longest > limit {
            limit / longest
        } else {
            1.0
        };
        let resize = |edge: usize| -> usize {
            (((edge as f32 * scale) / 32.0).round() * 32.0).max(32.0) as usize
        };
        let resized_w = resize(width);
        let resized_h = resize(height);
        Self {
            width: resized_w,
            height: resized_h,
            ratio_w: resized_w as f32 / width.max(1) as f32,
            ratio_h: resized_h as f32 / height.max(1) as f32,
        }
    }
}

/// A point in image space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A text quad, ordered top-left, top-right, bottom-right, bottom-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub points: [Point; 4],
}

impl Quad {
    fn width(&self) -> f32 {
        distance(self.points[0], self.points[1])
    }

    fn height(&self) -> f32 {
        distance(self.points[3], self.points[0])
    }
}

/// One detected text box in source-image coordinates.
#[derive(Clone, Debug)]
pub struct TextBox {
    pub quad: Quad,
    pub score: f32,
}

/// Knobs that come from the model's own `inference.yml`.
#[derive(Clone, Copy, Debug)]
pub struct Postprocess {
    pub thresh: f32,
    pub box_thresh: f32,
    pub unclip_ratio: f32,
    pub max_boxes: usize,
}

impl Default for Postprocess {
    fn default() -> Self {
        Self {
            thresh: 0.2,
            box_thresh: 0.4,
            unclip_ratio: 1.4,
            max_boxes: 300,
        }
    }
}

/// Resize and normalise an RGB image into the detector's CHW BGR input.
pub(crate) fn tensor(rgb: &[u8], width: usize, height: usize, geometry: Geometry) -> Vec<f32> {
    let resized = resize_rgb(rgb, width, height, geometry.width, geometry.height);
    let plane = geometry.width * geometry.height;
    let mut data = vec![0f32; plane * 3];
    for i in 0..plane {
        for channel in 0..3 {
            let value = resized[i * 3 + (2 - channel)] as f32 / 255.0;
            data[channel * plane + i] = (value - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
        }
    }
    data
}

/// Turn the detector's probability map into text boxes in source coordinates.
pub(crate) fn postprocess(
    probability: &[f32],
    map_width: usize,
    map_height: usize,
    geometry: Geometry,
    options: Postprocess,
) -> Vec<TextBox> {
    if map_width == 0 || map_height == 0 || probability.len() < map_width * map_height {
        return Vec::new();
    }

    let mut bitmap = vec![false; map_width * map_height];
    for (pixel, probability) in bitmap.iter_mut().zip(probability) {
        *pixel = *probability > options.thresh;
    }

    let mut boxes = Vec::new();
    for points in components(&bitmap, map_width, map_height) {
        if points.len() < 4 {
            continue;
        }
        let Some(tight) = unclipped_quad(&points, 0.0) else {
            continue;
        };
        let score = mean_inside(probability, map_width, map_height, tight);
        if score < options.box_thresh {
            continue;
        }
        let Some(quad) = unclipped_quad(&points, options.unclip_ratio) else {
            continue;
        };
        if quad.width().min(quad.height()) < 3.0 {
            continue;
        }
        boxes.push(TextBox {
            quad: scale_quad(quad, geometry.ratio_w, geometry.ratio_h),
            score,
        });
    }

    boxes.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    boxes.truncate(options.max_boxes);
    boxes
}

fn scale_quad(quad: Quad, ratio_w: f32, ratio_h: f32) -> Quad {
    let mut scaled = quad;
    for point in &mut scaled.points {
        point.x /= ratio_w.max(f32::EPSILON);
        point.y /= ratio_h.max(f32::EPSILON);
    }
    scaled
}

/// 8-connected components of the binary map, returning each component's
/// boundary pixels.
fn components(bitmap: &[bool], width: usize, height: usize) -> Vec<Vec<Point>> {
    let mut labels = vec![0usize; width * height];
    let mut parent = vec![0usize];
    let mut next = 0usize;

    let label_at = |labels: &[usize], x: isize, y: isize| -> usize {
        if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
            0
        } else {
            labels[y as usize * width + x as usize]
        }
    };

    for y in 0..height as isize {
        for x in 0..width as isize {
            let index = y as usize * width + x as usize;
            if !bitmap[index] {
                continue;
            }
            let neighbours = [
                label_at(&labels, x - 1, y),
                label_at(&labels, x - 1, y - 1),
                label_at(&labels, x, y - 1),
                label_at(&labels, x + 1, y - 1),
            ];
            let mut assigned = 0usize;
            for neighbour in neighbours {
                if neighbour == 0 {
                    continue;
                }
                if assigned == 0 {
                    assigned = neighbour;
                } else {
                    union(&mut parent, assigned, neighbour);
                }
            }
            if assigned == 0 {
                next += 1;
                parent.push(next);
                assigned = next;
            }
            labels[index] = assigned;
        }
    }

    let bitmap_at = |x: isize, y: isize| -> bool {
        if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
            false
        } else {
            bitmap[y as usize * width + x as usize]
        }
    };

    let mut groups: HashMap<usize, Vec<Point>> = HashMap::new();
    for y in 0..height as isize {
        for x in 0..width as isize {
            let index = y as usize * width + x as usize;
            if !bitmap[index] {
                continue;
            }
            let edge = !bitmap_at(x - 1, y)
                || !bitmap_at(x + 1, y)
                || !bitmap_at(x, y - 1)
                || !bitmap_at(x, y + 1);
            if !edge {
                continue;
            }
            let root = find(&mut parent, labels[index]);
            groups.entry(root).or_default().push(Point {
                x: x as f32,
                y: y as f32,
            });
        }
    }
    groups.into_values().collect()
}

fn find(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let a = find(parent, a);
    let b = find(parent, b);
    if a != b {
        parent[b] = a;
    }
}

/// Minimum-area rectangle around the boundary points, expanded by the DB
/// unclip distance.
fn unclipped_quad(points: &[Point], unclip_ratio: f32) -> Option<Quad> {
    let hull = convex_hull(points);
    if hull.len() < 3 {
        return None;
    }

    // area, u axis, v axis, and the four projections of the chosen rectangle.
    type Rectangle = (f32, f32, f32, f32, f32, f32, f32, f32, f32);
    let mut best: Option<Rectangle> = None;
    for index in 0..hull.len() {
        let start = hull[index];
        let end = hull[(index + 1) % hull.len()];
        let length = distance(start, end).max(1.0);
        let ux = (end.x - start.x) / length;
        let uy = (end.y - start.y) / length;
        let vx = -uy;
        let vy = ux;
        let (mut min_u, mut max_u, mut min_v, mut max_v) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        for point in &hull {
            let u = point.x * ux + point.y * uy;
            let v = point.x * vx + point.y * vy;
            min_u = min_u.min(u);
            max_u = max_u.max(u);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
        let area = (max_u - min_u) * (max_v - min_v);
        if best.is_none_or(|best| area < best.0) {
            best = Some((area, ux, uy, vx, vy, min_u, max_u, min_v, max_v));
        }
    }
    let (_, ux, uy, vx, vy, mut min_u, mut max_u, mut min_v, mut max_v) = best?;

    let distance = polygon_area(&hull).abs() * unclip_ratio / perimeter(&hull).max(1.0);
    min_u -= distance;
    max_u += distance;
    min_v -= distance;
    max_v += distance;

    let corner = |u: f32, v: f32| Point {
        x: u * ux + v * vx,
        y: u * uy + v * vy,
    };
    let quad = order_quad([
        corner(min_u, min_v),
        corner(max_u, min_v),
        corner(max_u, max_v),
        corner(min_u, max_v),
    ]);
    (polygon_area(&quad.points).abs() > 1e-3).then_some(quad)
}

fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
    });
    sorted.dedup_by(|a, b| a.x == b.x && a.y == b.y);
    if sorted.len() <= 2 {
        return sorted;
    }

    let build = |source: &[Point]| -> Vec<Point> {
        let mut hull: Vec<Point> = Vec::new();
        for point in source {
            while hull.len() >= 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], *point) <= 0.0
            {
                hull.pop();
            }
            hull.push(*point);
        }
        hull
    };
    let mut lower = build(&sorted);
    let reversed: Vec<Point> = sorted.iter().rev().copied().collect();
    let mut upper = build(&reversed);
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn cross(origin: Point, a: Point, b: Point) -> f32 {
    (a.x - origin.x) * (b.y - origin.y) - (a.y - origin.y) * (b.x - origin.x)
}

fn polygon_area(points: &[Point]) -> f32 {
    let mut sum = 0.0;
    for index in 0..points.len() {
        let next = points[(index + 1) % points.len()];
        sum += points[index].x * next.y - next.x * points[index].y;
    }
    sum / 2.0
}

fn perimeter(points: &[Point]) -> f32 {
    let mut length = 0.0;
    for index in 0..points.len() {
        length += distance(points[index], points[(index + 1) % points.len()]);
    }
    length
}

fn distance(a: Point, b: Point) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn order_quad(points: [Point; 4]) -> Quad {
    let sum = |point: &Point| point.x + point.y;
    let diff = |point: &Point| point.y - point.x;
    let arg_min = |value: &dyn Fn(&Point) -> f32| -> usize {
        let mut best = 0;
        for index in 1..points.len() {
            if value(&points[index]) < value(&points[best]) {
                best = index;
            }
        }
        best
    };
    let arg_max = |value: &dyn Fn(&Point) -> f32| -> usize {
        let mut best = 0;
        for index in 1..points.len() {
            if value(&points[index]) > value(&points[best]) {
                best = index;
            }
        }
        best
    };
    Quad {
        points: [
            points[arg_min(&sum)],
            points[arg_min(&diff)],
            points[arg_max(&sum)],
            points[arg_max(&diff)],
        ],
    }
}

fn mean_inside(probability: &[f32], width: usize, height: usize, quad: Quad) -> f32 {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for point in quad.points {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }
    let x0 = min_x.floor().max(0.0) as usize;
    let x1 = max_x.ceil().min(width as f32 - 1.0) as usize;
    let y0 = min_y.floor().max(0.0) as usize;
    let y1 = max_y.ceil().min(height as f32 - 1.0) as usize;
    let mut total = 0.0;
    let mut count = 0usize;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if point_in_quad(x as f32 + 0.5, y as f32 + 0.5, quad) {
                total += probability[y * width + x];
                count += 1;
            }
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

fn point_in_quad(x: f32, y: f32, quad: Quad) -> bool {
    let mut inside = false;
    let points = &quad.points;
    let mut j = 3;
    for i in 0..4 {
        let (xi, yi) = (points[i].x, points[i].y);
        let (xj, yj) = (points[j].x, points[j].y);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi + 1e-9) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_rounds_to_a_multiple_of_32() {
        let geometry = Geometry::new(1920, 1080, 960);
        assert_eq!(geometry.width, 960);
        assert_eq!(geometry.height, 544);
        assert!((geometry.ratio_w - 0.5).abs() < 1e-6);
        assert!((geometry.ratio_h - 544.0 / 1080.0).abs() < 1e-6);

        let tiny = Geometry::new(8, 8, 960);
        assert_eq!((tiny.width, tiny.height), (32, 32));
    }

    #[test]
    fn a_rectangle_becomes_one_scaled_box() {
        let width = 64;
        let height = 64;
        let mut probability = vec![0f32; width * height];
        for y in 10..20 {
            for x in 10..30 {
                probability[y * width + x] = 0.9;
            }
        }
        let geometry = Geometry {
            width,
            height,
            ratio_w: 1.0,
            ratio_h: 1.0,
        };
        let boxes = postprocess(
            &probability,
            width,
            height,
            geometry,
            Postprocess::default(),
        );
        assert_eq!(boxes.len(), 1, "{boxes:?}");
        let box_ = &boxes[0];
        assert!((box_.score - 0.9).abs() < 1e-6);
        // The unclip expansion makes the box larger than the 20x10 rectangle.
        assert!(box_.quad.width() > 20.0, "{:?}", box_.quad);
        assert!(box_.quad.height() > 10.0, "{:?}", box_.quad);
    }

    #[test]
    fn low_confidence_components_are_dropped() {
        let width = 32;
        let height = 32;
        let mut probability = vec![0f32; width * height];
        for y in 5..9 {
            for x in 5..9 {
                probability[y * width + x] = 0.1;
            }
        }
        let geometry = Geometry {
            width,
            height,
            ratio_w: 1.0,
            ratio_h: 1.0,
        };
        assert!(
            postprocess(
                &probability,
                width,
                height,
                geometry,
                Postprocess::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn a_rectangle_keeps_its_area() {
        // A rotated rectangle: the minimum-area rectangle is the same shape.
        let points = [
            Point { x: 2.0, y: 1.0 },
            Point { x: 10.0, y: 5.0 },
            Point { x: 8.0, y: 9.0 },
            Point { x: 0.0, y: 5.0 },
        ];
        let quad = unclipped_quad(&points, 0.0).expect("quad");
        let area = polygon_area(&quad.points).abs();
        let expected = 40.0; // |(8,4) x (-2,4)|
        assert!((area - expected).abs() < 1.0, "{area}");
    }

    #[test]
    fn point_in_quad_accepts_the_centre_and_rejects_the_outside() {
        let quad = Quad {
            points: [
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 10.0 },
                Point { x: 0.0, y: 10.0 },
            ],
        };
        assert!(point_in_quad(5.0, 5.0, quad));
        assert!(!point_in_quad(15.0, 5.0, quad));
    }
}
