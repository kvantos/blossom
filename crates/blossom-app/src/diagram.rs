//! Diagrams saved by Blossom before the `.excalidraw` source file existed:
//! one SVG with its shapes as JSON in
//! `<metadata><blossom-diagram xmlns="urn:blossom:diagram">…</blossom-diagram></metadata>`.
//! They are read once, when edited, and turned into an Excalidraw scene
//! ([`crate::excalidraw::Scene::from_shapes`]).

use serde::Deserialize;

/// The metadata element's name.
const TAG: &str = "blossom-diagram";

/// A point.
pub type Point = [f32; 2];

/// One shape of the old format.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Shape {
    /// A freehand stroke.
    Pen {
        /// Its points.
        points: Vec<Point>,
        /// `#rrggbb`.
        color: String,
        /// Stroke width.
        width: f32,
    },
    /// A line, maybe with an arrow head at `to`.
    Line {
        /// Start.
        from: Point,
        /// End.
        to: Point,
        /// Arrow head at `to`.
        arrow: bool,
        /// `#rrggbb`.
        color: String,
        /// Stroke width.
        width: f32,
    },
    /// A rectangle.
    Rect {
        /// Left.
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
        /// Left.
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

#[derive(Deserialize)]
struct Stored {
    shapes: Vec<Shape>,
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The shapes embedded in an old Blossom SVG, or `None` when it has none.
///
/// # Errors
///
/// A message when the embedded data is damaged.
pub fn from_svg(svg: &str) -> Result<Option<Vec<Shape>>, String> {
    let Some(open) = svg.find(&format!("<{TAG}")) else {
        return Ok(None);
    };
    let Some(start) = svg[open..].find('>').map(|i| open + i + 1) else {
        return Ok(None);
    };
    let Some(end) = svg[start..].find(&format!("</{TAG}>")).map(|i| start + i) else {
        return Ok(None);
    };
    let stored: Stored = serde_json::from_str(&xml_unescape(&svg[start..end]))
        .map_err(|e| format!("the diagram data in this SVG is damaged: {e}"))?;
    Ok(Some(stored.shapes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_old_svgs() {
        let svg = "<svg><metadata><blossom-diagram xmlns=\"urn:blossom:diagram\">{\"version\":1,\"shapes\":[{\"kind\":\"text\",\"x\":1.0,\"y\":2.0,\"text\":\"a &lt; b\",\"size\":16.0,\"color\":\"#000000\"}]}</blossom-diagram></metadata></svg>";
        let shapes = from_svg(svg).unwrap().unwrap();
        assert!(matches!(&shapes[0], Shape::Text { text, .. } if text == "a < b"));
        assert_eq!(from_svg("<svg/>"), Ok(None));
        assert!(from_svg("<blossom-diagram>{nope</blossom-diagram>").is_err());
    }
}
