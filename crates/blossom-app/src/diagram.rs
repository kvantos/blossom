//! Diagrams: a list of shapes, saved as an SVG file that any viewer (Emacs
//! included) can show, with the shapes as JSON in its `<metadata>` so
//! Blossom can edit it again.
//!
//! ```text
//! <svg …><metadata><blossom-diagram xmlns="urn:blossom:diagram">{"version":1,"shapes":[…]}</blossom-diagram></metadata>
//!   <rect … fill="#ffffff"/> …the shapes…
//! </svg>
//! ```
//!
//! On save every shape is moved so the drawing starts a small margin from
//! the top left, and the SVG is sized to fit; the stored JSON uses the same
//! coordinates as the SVG.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// The metadata element's name.
const TAG: &str = "blossom-diagram";

/// Space around the drawing in a saved file.
const MARGIN: f32 = 16.0;

/// The smallest saved size.
const MIN_SIZE: (f32, f32) = (120.0, 60.0);

/// The colours offered, dark first. The canvas is always white, as is the
/// saved file's background, so these read in light and dark themes alike.
pub const COLORS: [&str; 5] = ["#1f1f1f", "#d62f4b", "#2f6fd6", "#2e9e57", "#e08a00"];

/// A point.
pub type Point = [f32; 2];

/// One shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Shape {
    /// A freehand stroke.
    Pen {
        /// Its points, in order.
        points: Vec<Point>,
        /// `#rrggbb`.
        color: String,
        /// Stroke width.
        width: f32,
    },
    /// A straight line, maybe with an arrow head at `to`.
    Line {
        /// Start.
        from: Point,
        /// End.
        to: Point,
        /// Draw an arrow head at `to`.
        arrow: bool,
        /// `#rrggbb`.
        color: String,
        /// Stroke width.
        width: f32,
    },
    /// A rectangle.
    Rect {
        /// Top left and size.
        x: f32,
        /// Top.
        y: f32,
        /// Width.
        w: f32,
        /// Height.
        h: f32,
        /// `#rrggbb`.
        color: String,
        /// Stroke width.
        width: f32,
    },
    /// An ellipse inside a box.
    Ellipse {
        /// Box left.
        x: f32,
        /// Box top.
        y: f32,
        /// Box width.
        w: f32,
        /// Box height.
        h: f32,
        /// `#rrggbb`.
        color: String,
        /// Stroke width.
        width: f32,
    },
    /// One line of text, `(x, y)` its top left.
    Text {
        /// Left.
        x: f32,
        /// Top.
        y: f32,
        /// The text.
        text: String,
        /// Font size.
        size: f32,
        /// `#rrggbb`.
        color: String,
    },
}

/// A box: left, top, right, bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// Left.
    pub x0: f32,
    /// Top.
    pub y0: f32,
    /// Right.
    pub x1: f32,
    /// Bottom.
    pub y1: f32,
}

impl Bounds {
    fn of(points: impl IntoIterator<Item = Point>) -> Bounds {
        let mut b = Bounds {
            x0: f32::MAX,
            y0: f32::MAX,
            x1: f32::MIN,
            y1: f32::MIN,
        };
        for [x, y] in points {
            b.x0 = b.x0.min(x);
            b.y0 = b.y0.min(y);
            b.x1 = b.x1.max(x);
            b.y1 = b.y1.max(y);
        }
        b
    }

    fn union(self, o: Bounds) -> Bounds {
        Bounds {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    fn contains(self, [x, y]: Point, tolerance: f32) -> bool {
        x >= self.x0 - tolerance
            && x <= self.x1 + tolerance
            && y >= self.y0 - tolerance
            && y <= self.y1 + tolerance
    }
}

/// Text width estimate: there is no font measuring outside the renderer.
#[must_use]
pub fn text_width(text: &str, size: f32) -> f32 {
    // Character counts are small; precision loss doesn't matter.
    #[allow(clippy::cast_precision_loss)]
    let n = text.chars().count() as f32;
    n * size * 0.56
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a[0] + t * dx, a[1] + t * dy);
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

/// The two short strokes of an arrow head at `to`, coming from `from`.
#[must_use]
pub fn arrow_head(from: Point, to: Point, width: f32) -> [Point; 2] {
    let angle = (to[1] - from[1]).atan2(to[0] - from[0]);
    let len = 10.0 + 2.0 * width;
    let spread = 0.45_f32;
    let at = |a: f32| [to[0] - len * a.cos(), to[1] - len * a.sin()];
    [at(angle - spread), at(angle + spread)]
}

impl Shape {
    /// Its colour.
    #[must_use]
    pub fn color(&self) -> &str {
        match self {
            Shape::Pen { color, .. }
            | Shape::Line { color, .. }
            | Shape::Rect { color, .. }
            | Shape::Ellipse { color, .. }
            | Shape::Text { color, .. } => color,
        }
    }

    /// Its bounding box.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        match self {
            Shape::Pen { points, width, .. } => {
                let b = Bounds::of(points.iter().copied());
                Bounds {
                    x0: b.x0 - width,
                    y0: b.y0 - width,
                    x1: b.x1 + width,
                    y1: b.y1 + width,
                }
            }
            Shape::Line {
                from,
                to,
                arrow,
                width,
                ..
            } => {
                let mut pts = vec![*from, *to];
                if *arrow {
                    pts.extend(arrow_head(*from, *to, *width));
                }
                Bounds::of(pts)
            }
            Shape::Rect { x, y, w, h, .. } | Shape::Ellipse { x, y, w, h, .. } => {
                Bounds::of([[*x, *y], [x + w, y + h]])
            }
            Shape::Text {
                x, y, text, size, ..
            } => Bounds::of([[*x, *y], [x + text_width(text, *size), y + size * 1.25]]),
        }
    }

    /// Whether `p` is on the shape, within `tolerance`.
    #[must_use]
    pub fn hit(&self, p: Point, tolerance: f32) -> bool {
        match self {
            Shape::Pen { points, width, .. } => {
                let tol = tolerance + width / 2.0;
                match points.as_slice() {
                    [one] => distance_to_segment(p, *one, *one) <= tol,
                    _ => points
                        .windows(2)
                        .any(|w| distance_to_segment(p, w[0], w[1]) <= tol),
                }
            }
            Shape::Line {
                from, to, width, ..
            } => distance_to_segment(p, *from, *to) <= tolerance + width / 2.0,
            Shape::Rect { .. } | Shape::Ellipse { .. } | Shape::Text { .. } => {
                self.bounds().contains(p, tolerance)
            }
        }
    }

    /// The shape moved by `(dx, dy)`.
    #[must_use]
    pub fn moved(&self, dx: f32, dy: f32) -> Shape {
        let m = |[x, y]: Point| [x + dx, y + dy];
        let mut s = self.clone();
        match &mut s {
            Shape::Pen { points, .. } => {
                for p in points {
                    *p = m(*p);
                }
            }
            Shape::Line { from, to, .. } => {
                *from = m(*from);
                *to = m(*to);
            }
            Shape::Rect { x, y, .. } | Shape::Ellipse { x, y, .. } | Shape::Text { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
        }
        s
    }
}

/// The stored form.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    shapes: Vec<Shape>,
}

/// Escapes element text.
fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escapes an attribute value.
fn xml_escape(s: &str) -> String {
    xml_text(s).replace('"', "&quot;")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A number for SVG: at most two decimals, no trailing zeros.
fn num(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// Shapes moved to start at the margin, and the size that fits them.
#[must_use]
pub fn normalized(shapes: &[Shape]) -> (Vec<Shape>, f32, f32) {
    let Some(b) = shapes.iter().map(Shape::bounds).reduce(Bounds::union) else {
        return (Vec::new(), MIN_SIZE.0, MIN_SIZE.1);
    };
    let (dx, dy) = (MARGIN - b.x0, MARGIN - b.y0);
    let moved = shapes.iter().map(|s| s.moved(dx, dy)).collect();
    let w = (b.x1 - b.x0 + 2.0 * MARGIN).ceil().max(MIN_SIZE.0);
    let h = (b.y1 - b.y0 + 2.0 * MARGIN).ceil().max(MIN_SIZE.1);
    (moved, w, h)
}

/// The SVG file for `shapes`.
#[must_use]
pub fn to_svg(shapes: &[Shape]) -> String {
    let (shapes, w, h) = normalized(shapes);
    let stored = Stored { version: 1, shapes };
    let json = serde_json::to_string(&stored).unwrap_or_else(|_| "{}".into());
    let (w, h) = (num(w), num(h));
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n\
         <metadata><{TAG} xmlns=\"urn:blossom:diagram\">{}</{TAG}></metadata>\n\
         <rect width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/>\n\
         <g fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\n",
        xml_text(&json)
    );
    for s in &stored.shapes {
        let _ = writeln!(out, "  {}", shape_svg(s));
    }
    out.push_str("</g>\n</svg>\n");
    out
}

fn shape_svg(s: &Shape) -> String {
    let stroke =
        |c: &str, w: f32| format!("stroke=\"{}\" stroke-width=\"{}\"", xml_escape(c), num(w));
    match s {
        Shape::Pen {
            points,
            color,
            width,
        } => {
            let mut d = String::new();
            for (i, [x, y]) in points.iter().enumerate() {
                let _ = write!(
                    d,
                    "{}{} {} ",
                    if i == 0 { "M" } else { "L" },
                    num(*x),
                    num(*y)
                );
            }
            if points.len() == 1 {
                d.push_str("l 0 0");
            }
            format!("<path d=\"{}\" {}/>", d.trim_end(), stroke(color, *width))
        }
        Shape::Line {
            from,
            to,
            arrow,
            color,
            width,
        } => {
            let mut out = format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" {}/>",
                num(from[0]),
                num(from[1]),
                num(to[0]),
                num(to[1]),
                stroke(color, *width)
            );
            if *arrow {
                let [a, b] = arrow_head(*from, *to, *width);
                let _ = write!(
                    out,
                    "<path d=\"M{} {} L{} {} L{} {}\" {}/>",
                    num(a[0]),
                    num(a[1]),
                    num(to[0]),
                    num(to[1]),
                    num(b[0]),
                    num(b[1]),
                    stroke(color, *width)
                );
            }
            out
        }
        Shape::Rect {
            x,
            y,
            w,
            h,
            color,
            width,
        } => format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" {}/>",
            num(*x),
            num(*y),
            num(*w),
            num(*h),
            stroke(color, *width)
        ),
        Shape::Ellipse {
            x,
            y,
            w,
            h,
            color,
            width,
        } => format!(
            "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" {}/>",
            num(x + w / 2.0),
            num(y + h / 2.0),
            num(w / 2.0),
            num(h / 2.0),
            stroke(color, *width)
        ),
        Shape::Text {
            x,
            y,
            text,
            size,
            color,
        } => format!(
            "<text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"{}\" fill=\"{}\" stroke=\"none\">{}</text>",
            num(*x),
            num(y + size * 0.9),
            num(*size),
            xml_escape(color),
            xml_text(text)
        ),
    }
}

/// Why an SVG can't be edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// It has no Blossom metadata: it was made elsewhere.
    NotOurs,
    /// The metadata is damaged.
    Damaged(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::NotOurs => {
                f.write_str("this SVG wasn't drawn in Blossom, so it can't be edited here")
            }
            ReadError::Damaged(e) => write!(f, "the diagram data in this SVG is damaged: {e}"),
        }
    }
}

/// The shapes stored in an SVG written by [`to_svg`].
///
/// # Errors
///
/// [`ReadError::NotOurs`] without Blossom metadata, [`ReadError::Damaged`]
/// when it doesn't parse.
pub fn from_svg(svg: &str) -> Result<Vec<Shape>, ReadError> {
    let open = svg.find(&format!("<{TAG}")).ok_or(ReadError::NotOurs)?;
    let start = open + svg[open..].find('>').ok_or(ReadError::NotOurs)? + 1;
    let end = start
        + svg[start..]
            .find(&format!("</{TAG}>"))
            .ok_or(ReadError::NotOurs)?;
    let stored: Stored = serde_json::from_str(&xml_unescape(&svg[start..end]))
        .map_err(|e| ReadError::Damaged(e.to_string()))?;
    Ok(stored.shapes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes() -> Vec<Shape> {
        vec![
            Shape::Rect {
                x: 100.0,
                y: 50.0,
                w: 80.0,
                h: 40.0,
                color: COLORS[0].into(),
                width: 2.0,
            },
            Shape::Line {
                from: [180.0, 70.0],
                to: [260.0, 70.0],
                arrow: true,
                color: COLORS[1].into(),
                width: 2.0,
            },
            Shape::Text {
                x: 110.0,
                y: 60.0,
                text: "a < b & \"c\"".into(),
                size: 16.0,
                color: COLORS[2].into(),
            },
            Shape::Pen {
                points: vec![[100.0, 120.0], [110.0, 125.5], [120.0, 121.0]],
                color: COLORS[3].into(),
                width: 3.0,
            },
            Shape::Ellipse {
                x: 200.0,
                y: 100.0,
                w: 60.0,
                h: 30.0,
                color: COLORS[4].into(),
                width: 2.0,
            },
        ]
    }

    #[test]
    fn round_trips_through_svg() {
        let svg = to_svg(&shapes());
        assert!(
            svg.contains("<rect x=\"19\" y=\"16\" width=\"80\" height=\"40\""),
            "{svg}"
        );
        // The pen's stroke width reaches 3 px left of the rectangle.
        assert!(svg.contains(">a &lt; b &amp; \"c\"</text>"), "{svg}");
        assert!(svg.contains("{\"version\":1,"));
        let back = from_svg(&svg).unwrap();
        let (expected, _, _) = normalized(&shapes());
        assert_eq!(back, expected);
        // Saving again changes nothing.
        assert_eq!(to_svg(&back), svg);
        assert_eq!(from_svg("<svg/>"), Err(ReadError::NotOurs));
        assert!(matches!(
            from_svg("<blossom-diagram>{nope</blossom-diagram>"),
            Err(ReadError::Damaged(_))
        ));
        assert!(to_svg(&[]).contains("width=\"120\" height=\"60\""));
    }

    #[test]
    fn hits_and_moves() {
        let s = shapes();
        assert!(s[0].hit([140.0, 70.0], 4.0));
        assert!(!s[0].hit([300.0, 70.0], 4.0));
        assert!(s[1].hit([220.0, 72.0], 4.0));
        assert!(!s[1].hit([220.0, 90.0], 4.0));
        assert!(s[3].hit([105.0, 122.0], 2.0));
        let m = s[0].moved(10.0, -5.0);
        assert_eq!(m.bounds().x0, 110.0);
        assert_eq!(m.bounds().y0, 45.0);
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.001), "0");
        assert_eq!(num(2.345), "2.35");
    }
}
