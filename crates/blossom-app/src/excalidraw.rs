//! Excalidraw scenes: the source file of a diagram.
//!
//! A diagram is a pair of files side by side: `flow.excalidraw`, the
//! source (Excalidraw's own JSON, so excalidraw.com, Obsidian's Excalidraw
//! plugin or the VS Code extension can open it), and `flow.svg`, generated
//! from it by [`to_svg`], which notes link to so any viewer shows it.
//!
//! Every element keeps its JSON as read; only the fields Blossom
//! understands are parsed, and an element Blossom didn't change is written
//! back exactly as it came, so other tools' data survives. Element types
//! Blossom can't draw (images, frames, embeds) are kept, and shown as a
//! labelled outline. Blossom draws clean shapes rather than Excalidraw's
//! hand-drawn look, and writes new elements with `roughness: 0` so
//! Excalidraw draws them the same way.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};

use crate::diagram::Shape;

/// Space around the drawing in a generated SVG.
const MARGIN: f32 = 16.0;

/// The colours offered for new elements: Excalidraw's default stroke
/// colours.
pub const COLORS: [&str; 5] = ["#1e1e1e", "#e03131", "#1971c2", "#2f9e44", "#f08c00"];

/// A point.
pub type Point = [f32; 2];

/// What an element is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `rectangle`
    Rectangle,
    /// `ellipse`
    Ellipse,
    /// `diamond`
    Diamond,
    /// `line`
    Line,
    /// `arrow`
    Arrow,
    /// `freedraw`
    Freedraw,
    /// `text`
    Text,
    /// Anything else, by its type name: kept, drawn as an outline.
    Other(String),
}

impl Kind {
    fn parse(s: &str) -> Kind {
        match s {
            "rectangle" => Kind::Rectangle,
            "ellipse" => Kind::Ellipse,
            "diamond" => Kind::Diamond,
            "line" => Kind::Line,
            "arrow" => Kind::Arrow,
            "freedraw" => Kind::Freedraw,
            "text" => Kind::Text,
            other => Kind::Other(other.to_string()),
        }
    }

    /// The type name as written.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Kind::Rectangle => "rectangle",
            Kind::Ellipse => "ellipse",
            Kind::Diamond => "diamond",
            Kind::Line => "line",
            Kind::Arrow => "arrow",
            Kind::Freedraw => "freedraw",
            Kind::Text => "text",
            Kind::Other(s) => s,
        }
    }

    /// Whether its geometry is a list of points.
    #[must_use]
    pub fn is_linear(&self) -> bool {
        matches!(self, Kind::Line | Kind::Arrow | Kind::Freedraw)
    }
}

/// One element.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    /// The JSON as read (or as created).
    pub raw: Map<String, Value>,
    /// Blossom changed it: its fields are written back and its version bumped.
    pub changed: bool,
    /// `id`
    pub id: String,
    /// `type`
    pub kind: Kind,
    /// `x`
    pub x: f32,
    /// `y`
    pub y: f32,
    /// `width`
    pub width: f32,
    /// `height`
    pub height: f32,
    /// `angle`, radians, around the centre.
    pub angle: f32,
    /// `strokeColor`
    pub stroke: String,
    /// `backgroundColor` (`transparent` for none).
    pub background: String,
    /// `fillStyle`: `solid`, `hachure`, `cross-hatch`, `zigzag`.
    pub fill_style: String,
    /// `strokeWidth`
    pub stroke_width: f32,
    /// `strokeStyle`: `solid`, `dashed`, `dotted`.
    pub stroke_style: String,
    /// `opacity`, 0 to 100.
    pub opacity: f32,
    /// `roundness.type`, if rounded.
    pub roundness: Option<u64>,
    /// `points`, relative to `(x, y)`, for lines, arrows and freehand.
    pub points: Vec<Point>,
    /// `startArrowhead`
    pub start_arrow: Option<String>,
    /// `endArrowhead`
    pub end_arrow: Option<String>,
    /// `text`
    pub text: String,
    /// `fontSize`
    pub font_size: f32,
    /// `fontFamily`: 3 is monospace; the rest are drawn sans-serif.
    pub font_family: u64,
    /// `textAlign`: `left`, `center`, `right`.
    pub text_align: String,
    /// `lineHeight`
    pub line_height: f32,
    /// `containerId`: the shape a text is the label of.
    pub container: Option<String>,
    /// `isDeleted`: kept in the file, not shown.
    pub deleted: bool,
}

fn f(m: &Map<String, Value>, k: &str, default: f32) -> f32 {
    // Scene coordinates fit f32 comfortably.
    #[allow(clippy::cast_possible_truncation)]
    m.get(k)
        .and_then(Value::as_f64)
        .map_or(default, |v| v as f32)
}

fn s(m: &Map<String, Value>, k: &str, default: &str) -> String {
    m.get(k)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

/// A number as Excalidraw writes it: integral values without `.0`.
fn num(v: f32) -> Value {
    let rounded = (f64::from(v) * 10_000.0).round() / 10_000.0;
    if rounded.fract() == 0.0 && rounded.abs() < 9.0e15 {
        // Checked integral and in range just above.
        #[allow(clippy::cast_possible_truncation)]
        Value::from(rounded as i64)
    } else {
        Value::from(rounded)
    }
}

/// A fresh random-looking id and seed. Not cryptographic: Excalidraw only
/// needs ids unique within a scene and seeds to vary.
fn fresh() -> (String, u64) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in nanos
        .to_le_bytes()
        .into_iter()
        .chain(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes())
        .chain(std::process::id().to_le_bytes())
    {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut x = h;
    let id: String = (0..21)
        .map(|i| {
            x = x
                .rotate_left(7)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                .wrapping_add(i);
            char::from(alphabet[usize::try_from(x % 62).unwrap_or(0)])
        })
        .collect();
    (id, h % 2_147_483_647)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Element {
    /// Reads one element; `None` when it isn't an object with a type.
    #[must_use]
    pub fn from_json(v: &Value) -> Option<Element> {
        let m = v.as_object()?;
        let kind = Kind::parse(m.get("type")?.as_str()?);
        let points = m
            .get("points")
            .and_then(Value::as_array)
            .map(|ps| {
                ps.iter()
                    .filter_map(|p| {
                        let a = p.as_array()?;
                        // Scene coordinates fit f32.
                        #[allow(clippy::cast_possible_truncation)]
                        Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Element {
            id: s(m, "id", ""),
            x: f(m, "x", 0.0),
            y: f(m, "y", 0.0),
            width: f(m, "width", 0.0),
            height: f(m, "height", 0.0),
            angle: f(m, "angle", 0.0),
            stroke: s(m, "strokeColor", "#1e1e1e"),
            background: s(m, "backgroundColor", "transparent"),
            fill_style: s(m, "fillStyle", "solid"),
            stroke_width: f(m, "strokeWidth", 2.0),
            stroke_style: s(m, "strokeStyle", "solid"),
            opacity: f(m, "opacity", 100.0),
            roundness: m
                .get("roundness")
                .and_then(|r| r.get("type"))
                .and_then(Value::as_u64),
            points,
            start_arrow: m
                .get("startArrowhead")
                .and_then(Value::as_str)
                .map(String::from),
            end_arrow: m
                .get("endArrowhead")
                .and_then(Value::as_str)
                .map(String::from),
            text: s(m, "text", ""),
            font_size: f(m, "fontSize", 20.0),
            font_family: m.get("fontFamily").and_then(Value::as_u64).unwrap_or(1),
            text_align: s(m, "textAlign", "left"),
            line_height: f(m, "lineHeight", 1.25),
            container: m
                .get("containerId")
                .and_then(Value::as_str)
                .map(String::from),
            deleted: m.get("isDeleted").and_then(Value::as_bool).unwrap_or(false),
            kind,
            raw: m.clone(),
            changed: false,
        })
    }

    /// A new element with Excalidraw's defaults (clean, `roughness: 0`).
    #[must_use]
    pub fn new(kind: Kind, x: f32, y: f32, stroke: &str) -> Element {
        let (id, seed) = fresh();
        let (_, nonce) = fresh();
        let mut raw = json!({
            "id": id,
            "type": kind.name(),
            "x": num(x),
            "y": num(y),
            "width": 0,
            "height": 0,
            "angle": 0,
            "strokeColor": stroke,
            "backgroundColor": "transparent",
            "fillStyle": "solid",
            "strokeWidth": 2,
            "strokeStyle": "solid",
            "roughness": 0,
            "opacity": 100,
            "groupIds": [],
            "frameId": null,
            "roundness": null,
            "seed": seed,
            "version": 1,
            "versionNonce": nonce,
            "isDeleted": false,
            "boundElements": null,
            "updated": now_ms(),
            "link": null,
            "locked": false,
        });
        if let Some(m) = raw.as_object_mut() {
            match kind {
                Kind::Rectangle => {
                    m.insert("roundness".into(), json!({"type": 3}));
                }
                Kind::Line | Kind::Arrow => {
                    m.insert("points".into(), json!([]));
                    m.insert("lastCommittedPoint".into(), Value::Null);
                    m.insert("startBinding".into(), Value::Null);
                    m.insert("endBinding".into(), Value::Null);
                    m.insert("startArrowhead".into(), Value::Null);
                    let end = if kind == Kind::Arrow {
                        json!("arrow")
                    } else {
                        Value::Null
                    };
                    m.insert("endArrowhead".into(), end);
                }
                Kind::Freedraw => {
                    m.insert("points".into(), json!([]));
                    m.insert("pressures".into(), json!([]));
                    m.insert("simulatePressure".into(), json!(true));
                    m.insert("lastCommittedPoint".into(), Value::Null);
                }
                Kind::Text => {
                    m.insert("text".into(), json!(""));
                    m.insert("fontSize".into(), json!(20));
                    m.insert("fontFamily".into(), json!(2));
                    m.insert("textAlign".into(), json!("left"));
                    m.insert("verticalAlign".into(), json!("top"));
                    m.insert("containerId".into(), Value::Null);
                    m.insert("originalText".into(), json!(""));
                    m.insert("autoResize".into(), json!(true));
                    m.insert("lineHeight".into(), json!(1.25));
                }
                _ => {}
            }
        }
        let mut e = Element::from_json(&raw).unwrap_or_else(|| unreachable_element(&kind));
        e.kind = kind;
        e.changed = true;
        e
    }

    /// The element as JSON: as read when unchanged, else with Blossom's
    /// fields written back and its version bumped.
    #[must_use]
    pub fn to_json(&self) -> Value {
        if !self.changed {
            return Value::Object(self.raw.clone());
        }
        let mut m = self.raw.clone();
        let mut put = |k: &str, v: Value| {
            m.insert(k.to_string(), v);
        };
        put("x", num(self.x));
        put("y", num(self.y));
        put("width", num(self.width));
        put("height", num(self.height));
        put("strokeColor", json!(self.stroke));
        put("strokeWidth", num(self.stroke_width));
        if self.kind.is_linear() {
            put(
                "points",
                Value::Array(
                    self.points
                        .iter()
                        .map(|[x, y]| json!([num(*x), num(*y)]))
                        .collect(),
                ),
            );
        }
        if self.kind == Kind::Freedraw {
            put("pressures", json!([]));
            put("simulatePressure", json!(true));
        }
        if self.kind == Kind::Text {
            put("fontSize", num(self.font_size));
            put("text", json!(self.text));
            put("originalText", json!(self.text));
        }
        put("isDeleted", json!(self.deleted));
        let version = m.get("version").and_then(Value::as_u64).unwrap_or(0) + 1;
        m.insert("version".into(), json!(version));
        m.insert("versionNonce".into(), json!(fresh().1));
        m.insert("updated".into(), json!(now_ms()));
        Value::Object(m)
    }

    /// The box of its own coordinates, ignoring rotation.
    #[must_use]
    pub fn local_bounds(&self) -> (f32, f32, f32, f32) {
        if self.kind.is_linear() && !self.points.is_empty() {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for [px, py] in &self.points {
                x0 = x0.min(*px);
                y0 = y0.min(*py);
                x1 = x1.max(*px);
                y1 = y1.max(*py);
            }
            return (self.x + x0, self.y + y0, self.x + x1, self.y + y1);
        }
        (self.x, self.y, self.x + self.width, self.y + self.height)
    }

    /// The rotation centre.
    #[must_use]
    pub fn center(&self) -> Point {
        let (x0, y0, x1, y1) = self.local_bounds();
        [f32::midpoint(x0, x1), f32::midpoint(y0, y1)]
    }

    /// The box around it, rotation included.
    #[must_use]
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let (x0, y0, x1, y1) = self.local_bounds();
        if self.angle == 0.0 {
            return (x0, y0, x1, y1);
        }
        let c = self.center();
        let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| rotate(p, c, self.angle));
        corners.iter().fold(
            (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
            |(a, b, cc, d), [x, y]| (a.min(*x), b.min(*y), cc.max(*x), d.max(*y)),
        )
    }

    /// Whether scene point `p` is on it, within `tolerance`.
    #[must_use]
    pub fn hit(&self, p: Point, tolerance: f32) -> bool {
        let p = rotate(p, self.center(), -self.angle);
        let tol = tolerance + self.stroke_width / 2.0;
        match self.kind {
            Kind::Line | Kind::Arrow | Kind::Freedraw => {
                let pts: Vec<Point> = self
                    .points
                    .iter()
                    .map(|[x, y]| [self.x + x, self.y + y])
                    .collect();
                match pts.as_slice() {
                    [] => false,
                    [one] => dist(p, *one, *one) <= tol,
                    _ => pts.windows(2).any(|w| dist(p, w[0], w[1]) <= tol),
                }
            }
            _ => {
                let (x0, y0, x1, y1) = self.local_bounds();
                p[0] >= x0 - tol && p[0] <= x1 + tol && p[1] >= y0 - tol && p[1] <= y1 + tol
            }
        }
    }

    /// Moves it by `(dx, dy)`.
    pub fn shift(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
        self.changed = true;
    }

    /// Absolute points of a linear element.
    #[must_use]
    pub fn absolute_points(&self) -> Vec<Point> {
        self.points
            .iter()
            .map(|[x, y]| [self.x + x, self.y + y])
            .collect()
    }

    /// Sets the points of a linear element from absolute ones, keeping
    /// Excalidraw's convention: the first point is `(0, 0)` at `(x, y)`.
    pub fn set_points(&mut self, absolute: &[Point]) {
        let Some(first) = absolute.first().copied() else {
            return;
        };
        self.x = first[0];
        self.y = first[1];
        self.points = absolute
            .iter()
            .map(|[x, y]| [x - first[0], y - first[1]])
            .collect();
        let (x0, y0, x1, y1) = self.local_bounds();
        self.width = x1 - x0;
        self.height = y1 - y0;
        self.changed = true;
    }

    /// Text lines.
    #[must_use]
    pub fn lines(&self) -> Vec<&str> {
        self.text.split('\n').collect()
    }

    /// Sets a text element's text and estimated size.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        let widest = self
            .lines()
            .iter()
            .map(|l| text_width(l, self.font_size))
            .fold(0.0, f32::max);
        // A handful of lines; precision is irrelevant.
        #[allow(clippy::cast_precision_loss)]
        let n = self.lines().len() as f32;
        self.width = widest;
        self.height = n * self.font_size * self.line_height;
        self.changed = true;
    }
}

/// Never reached: [`Element::new`] always builds a valid object.
fn unreachable_element(kind: &Kind) -> Element {
    Element {
        raw: Map::new(),
        changed: true,
        id: String::new(),
        kind: kind.clone(),
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
        angle: 0.0,
        stroke: COLORS[0].into(),
        background: "transparent".into(),
        fill_style: "solid".into(),
        stroke_width: 2.0,
        stroke_style: "solid".into(),
        opacity: 100.0,
        roundness: None,
        points: Vec::new(),
        start_arrow: None,
        end_arrow: None,
        text: String::new(),
        font_size: 20.0,
        font_family: 2,
        text_align: "left".into(),
        line_height: 1.25,
        container: None,
        deleted: false,
    }
}

/// Text width estimate: there is no font measuring outside the renderer.
#[must_use]
pub fn text_width(text: &str, size: f32) -> f32 {
    // Character counts are small; precision loss doesn't matter.
    #[allow(clippy::cast_precision_loss)]
    let n = text.chars().count() as f32;
    n * size * 0.55
}

/// `p` rotated by `angle` radians around `c`.
#[must_use]
pub fn rotate(p: Point, c: Point, angle: f32) -> Point {
    if angle == 0.0 {
        return p;
    }
    let (s, co) = angle.sin_cos();
    let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
    [c[0] + dx * co - dy * s, c[1] + dx * s + dy * co]
}

fn dist(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    };
    (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
}

/// The two strokes of an arrow head at `tip`, coming from `from`.
#[must_use]
pub fn arrow_head(from: Point, tip: Point, width: f32) -> [Point; 2] {
    let angle = (tip[1] - from[1]).atan2(tip[0] - from[0]);
    let len = 12.0 + 2.0 * width;
    let at = |a: f32| [tip[0] - len * a.cos(), tip[1] - len * a.sin()];
    [at(angle - 0.45), at(angle + 0.45)]
}

/// Corner radius of a rounded rectangle, as Excalidraw computes it.
#[must_use]
pub fn corner_radius(e: &Element) -> f32 {
    let side = e.width.abs().min(e.height.abs());
    match e.roundness {
        Some(3) => (side * 0.25).min(32.0),
        Some(_) => side * 0.25,
        None => 0.0,
    }
}

/// The dash pattern of a stroke style.
#[must_use]
pub fn dashes(e: &Element) -> Option<[f32; 2]> {
    match e.stroke_style.as_str() {
        "dashed" => Some([8.0, 8.0 + e.stroke_width]),
        "dotted" => Some([1.5, 6.0 + e.stroke_width]),
        _ => None,
    }
}

/// How opaque the fill is: hatched fills are drawn as a light solid fill.
#[must_use]
pub fn fill_alpha(e: &Element) -> f32 {
    if e.fill_style == "solid" { 1.0 } else { 0.35 }
}

/// A whole `.excalidraw` file.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// The file's JSON, `elements` aside.
    pub raw: Map<String, Value>,
    /// The elements, bottom first.
    pub elements: Vec<Element>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene::new()
    }
}

impl Scene {
    /// An empty scene.
    #[must_use]
    pub fn new() -> Scene {
        let raw = json!({
            "type": "excalidraw",
            "version": 2,
            "source": "blossom",
            "elements": [],
            "appState": {"gridSize": 20, "viewBackgroundColor": "#ffffff"},
            "files": {},
        });
        Scene {
            raw: raw.as_object().cloned().unwrap_or_default(),
            elements: Vec::new(),
        }
    }

    /// Parses a `.excalidraw` file.
    ///
    /// # Errors
    ///
    /// A message when it isn't JSON or not an Excalidraw scene.
    pub fn parse(text: &str) -> Result<Scene, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut raw = v.as_object().cloned().ok_or("not an Excalidraw file")?;
        if raw.get("type").and_then(Value::as_str) != Some("excalidraw") {
            return Err("not an Excalidraw file".into());
        }
        let elements = raw
            .get("elements")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Element::from_json).collect())
            .unwrap_or_default();
        raw.insert("elements".into(), json!([]));
        Ok(Scene { raw, elements })
    }

    /// The file text, indented as Excalidraw writes it.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut raw = self.raw.clone();
        raw.insert(
            "elements".into(),
            Value::Array(self.elements.iter().map(Element::to_json).collect()),
        );
        let mut out = serde_json::to_string_pretty(&Value::Object(raw)).unwrap_or_default();
        out.push('\n');
        out
    }

    /// The elements to draw, bottom first.
    pub fn visible(&self) -> impl Iterator<Item = (usize, &Element)> {
        self.elements.iter().enumerate().filter(|(_, e)| !e.deleted)
    }

    /// The background colour.
    #[must_use]
    pub fn background(&self) -> String {
        self.raw
            .get("appState")
            .and_then(|a| a.get("viewBackgroundColor"))
            .and_then(Value::as_str)
            .filter(|c| c.starts_with('#'))
            .unwrap_or("#ffffff")
            .to_string()
    }

    /// The box around every visible element.
    #[must_use]
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        self.visible()
            .map(|(_, e)| e.bounds())
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
    }

    /// The scene from a Phase 6 SVG's embedded shapes.
    #[must_use]
    pub fn from_shapes(shapes: &[Shape]) -> Scene {
        let mut scene = Scene::new();
        for shape in shapes {
            let e = match shape {
                Shape::Pen {
                    points,
                    color,
                    width,
                } => {
                    let mut e = Element::new(Kind::Freedraw, 0.0, 0.0, color);
                    e.stroke_width = *width;
                    e.set_points(points);
                    e
                }
                Shape::Line {
                    from,
                    to,
                    arrow,
                    color,
                    width,
                } => {
                    let kind = if *arrow { Kind::Arrow } else { Kind::Line };
                    let mut e = Element::new(kind, 0.0, 0.0, color);
                    e.stroke_width = *width;
                    e.set_points(&[*from, *to]);
                    e
                }
                Shape::Rect {
                    x,
                    y,
                    w,
                    h,
                    color,
                    width,
                }
                | Shape::Ellipse {
                    x,
                    y,
                    w,
                    h,
                    color,
                    width,
                } => {
                    let kind = if matches!(shape, Shape::Rect { .. }) {
                        Kind::Rectangle
                    } else {
                        Kind::Ellipse
                    };
                    let mut e = Element::new(kind, *x, *y, color);
                    e.width = *w;
                    e.height = *h;
                    e.stroke_width = *width;
                    e
                }
                Shape::Text {
                    x,
                    y,
                    text,
                    size,
                    color,
                } => {
                    let mut e = Element::new(Kind::Text, *x, *y, color);
                    e.font_size = *size;
                    e.set_text(text);
                    e
                }
            };
            scene.elements.push(e);
        }
        scene
    }
}

// --- SVG ---

fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_attr(s: &str) -> String {
    xml_text(s).replace('"', "&quot;")
}

/// A number for SVG: at most two decimals.
fn n(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// The SVG for `scene`: cropped to its elements plus a margin, on its
/// background colour.
#[must_use]
pub fn to_svg(scene: &Scene) -> String {
    let (x0, y0, x1, y1) = scene.bounds().unwrap_or((0.0, 0.0, 120.0, 60.0));
    let (vx, vy) = (x0 - MARGIN, y0 - MARGIN);
    let (w, h) = (
        (x1 - x0 + 2.0 * MARGIN).ceil(),
        (y1 - y0 + 2.0 * MARGIN).ceil(),
    );
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!-- Generated by Blossom from the .excalidraw file beside it; edit that instead. -->\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{} {} {w} {h}\">\n\
         <rect x=\"{}\" y=\"{}\" width=\"{w}\" height=\"{h}\" fill=\"{}\"/>\n",
        n(vx),
        n(vy),
        n(vx),
        n(vy),
        xml_attr(&scene.background()),
        w = n(w),
        h = n(h),
    );
    for (_, e) in scene.visible() {
        out.push_str(&element_svg(e));
    }
    out.push_str("</svg>\n");
    out
}

fn element_svg(e: &Element) -> String {
    let mut body = String::new();
    let stroke = {
        let mut a = format!(
            "stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
            xml_attr(&e.stroke),
            n(e.stroke_width)
        );
        if let Some([on, off]) = dashes(e) {
            let _ = write!(a, " stroke-dasharray=\"{} {}\"", n(on), n(off));
        }
        a
    };
    let fill = if e.background == "transparent" || e.kind.is_linear() {
        "fill=\"none\"".to_string()
    } else {
        format!(
            "fill=\"{}\" fill-opacity=\"{}\"",
            xml_attr(&e.background),
            n(fill_alpha(e))
        )
    };
    let (x0, y0, x1, y1) = e.local_bounds();
    match &e.kind {
        Kind::Rectangle => {
            let r = corner_radius(e);
            let _ = write!(
                body,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" {fill} {stroke}/>",
                n(x0),
                n(y0),
                n(x1 - x0),
                n(y1 - y0),
                n(r)
            );
        }
        Kind::Ellipse => {
            let _ = write!(
                body,
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" {fill} {stroke}/>",
                n(f32::midpoint(x0, x1)),
                n(f32::midpoint(y0, y1)),
                n((x1 - x0) / 2.0),
                n((y1 - y0) / 2.0)
            );
        }
        Kind::Diamond => {
            let (cx, cy) = (f32::midpoint(x0, x1), f32::midpoint(y0, y1));
            let _ = write!(
                body,
                "<polygon points=\"{},{} {},{} {},{} {},{}\" {fill} {stroke}/>",
                n(cx),
                n(y0),
                n(x1),
                n(cy),
                n(cx),
                n(y1),
                n(x0),
                n(cy)
            );
        }
        Kind::Line | Kind::Arrow | Kind::Freedraw => body.push_str(&linear_svg(e, &stroke)),
        Kind::Text => body.push_str(&text_svg(e)),
        Kind::Other(name) => {
            let _ = write!(
                body,
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#999999\" stroke-dasharray=\"6 4\"/>\
                 <text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"12\" fill=\"#999999\">{}</text>",
                n(x0),
                n(y0),
                n(x1 - x0),
                n(y1 - y0),
                n(x0 + 4.0),
                n(y0 + 16.0),
                xml_text(name)
            );
        }
    }
    let mut attrs = String::new();
    if e.opacity < 100.0 {
        let _ = write!(attrs, " opacity=\"{}\"", n(e.opacity / 100.0));
    }
    if e.angle != 0.0 {
        let c = e.center();
        let _ = write!(
            attrs,
            " transform=\"rotate({} {} {})\"",
            n(e.angle.to_degrees()),
            n(c[0]),
            n(c[1])
        );
    }
    format!("<g{attrs}>{body}</g>\n")
}

/// A line, arrow or freehand stroke.
fn linear_svg(e: &Element, stroke: &str) -> String {
    let mut body = String::new();
    let pts = e.absolute_points();
    let mut d = String::new();
    for (i, [x, y]) in pts.iter().enumerate() {
        let _ = write!(d, "{}{} {} ", if i == 0 { "M" } else { "L" }, n(*x), n(*y));
    }
    if pts.len() == 1 {
        d.push_str("l 0 0");
    }
    let _ = write!(
        body,
        "<path d=\"{}\" fill=\"none\" {stroke}/>",
        d.trim_end()
    );
    for (head, tip, from) in arrow_ends(e, &pts) {
        let [a, b] = arrow_head(from, tip, e.stroke_width);
        if head == "triangle" {
            let _ = write!(
                body,
                "<polygon points=\"{},{} {},{} {},{}\" fill=\"{}\" {stroke}/>",
                n(a[0]),
                n(a[1]),
                n(tip[0]),
                n(tip[1]),
                n(b[0]),
                n(b[1]),
                xml_attr(&e.stroke)
            );
        } else {
            let _ = write!(
                body,
                "<path d=\"M{} {} L{} {} L{} {}\" fill=\"none\" {stroke}/>",
                n(a[0]),
                n(a[1]),
                n(tip[0]),
                n(tip[1]),
                n(b[0]),
                n(b[1])
            );
        }
    }
    body
}

/// The lines of a text element.
fn text_svg(e: &Element) -> String {
    let mut body = String::new();
    let (x0, y0, x1, _) = e.local_bounds();
    let (anchor, tx) = match e.text_align.as_str() {
        "center" => ("middle", f32::midpoint(x0, x1)),
        "right" => ("end", x1),
        _ => ("start", x0),
    };
    let family = if e.font_family == 3 {
        "monospace"
    } else {
        "Helvetica, Arial, sans-serif"
    };
    let step = e.font_size * e.line_height;
    for (i, line) in e.lines().iter().enumerate() {
        // A handful of lines.
        #[allow(clippy::cast_precision_loss)]
        let top = y0 + step * i as f32;
        let _ = write!(
            body,
            "<text x=\"{}\" y=\"{}\" font-family=\"{family}\" font-size=\"{}\" text-anchor=\"{anchor}\" fill=\"{}\" xml:space=\"preserve\">{}</text>",
            n(tx),
            n(top + step / 2.0 + e.font_size * 0.35),
            n(e.font_size),
            xml_attr(&e.stroke),
            xml_text(line)
        );
    }
    body
}

/// Arrow heads of a linear element: (kind, tip, the point before it).
#[must_use]
pub fn arrow_ends<'a>(e: &'a Element, pts: &[Point]) -> Vec<(&'a str, Point, Point)> {
    let mut out = Vec::new();
    if pts.len() < 2 || e.kind == Kind::Freedraw {
        return out;
    }
    if let Some(h) = e.start_arrow.as_deref() {
        out.push((h, pts[0], pts[1]));
    }
    if let Some(h) = e.end_arrow.as_deref() {
        out.push((h, pts[pts.len() - 1], pts[pts.len() - 2]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"{
  "type": "excalidraw",
  "version": 2,
  "source": "https://excalidraw.com",
  "elements": [
    {"id": "r1", "type": "rectangle", "x": -40, "y": 10, "width": 100, "height": 50, "angle": 0,
     "strokeColor": "#1e1e1e", "backgroundColor": "#ffec99", "fillStyle": "hachure", "strokeWidth": 2,
     "strokeStyle": "dashed", "roughness": 1, "opacity": 100, "roundness": {"type": 3},
     "seed": 1, "version": 3, "versionNonce": 9, "isDeleted": false, "customData": {"keep": true}},
    {"id": "a1", "type": "arrow", "x": 60, "y": 35, "width": 80, "height": 0, "angle": 0,
     "strokeColor": "#e03131", "backgroundColor": "transparent", "strokeWidth": 2, "strokeStyle": "solid",
     "points": [[0, 0], [80, 0]], "startArrowhead": null, "endArrowhead": "arrow", "version": 1},
    {"id": "t1", "type": "text", "x": -30, "y": 20, "width": 60, "height": 25, "angle": 0.5,
     "strokeColor": "#1971c2", "text": "a<b\nline", "fontSize": 20, "fontFamily": 1,
     "textAlign": "center", "lineHeight": 1.25, "containerId": "r1", "version": 1},
    {"id": "i1", "type": "image", "x": 0, "y": 100, "width": 30, "height": 30, "fileId": "f"},
    {"id": "gone", "type": "ellipse", "x": 500, "y": 500, "width": 10, "height": 10, "isDeleted": true}
  ],
  "appState": {"viewBackgroundColor": "#fafafa", "gridSize": null},
  "files": {"f": {"mimeType": "image/png"}}
}"##;

    #[test]
    fn reads_and_keeps_what_it_does_not_change() {
        let scene = Scene::parse(SAMPLE).unwrap();
        assert_eq!(scene.elements.len(), 5);
        let r = &scene.elements[0];
        assert_eq!((r.x, r.width, r.roundness), (-40.0, 100.0, Some(3)));
        assert_eq!(scene.elements[1].end_arrow.as_deref(), Some("arrow"));
        assert_eq!(scene.elements[2].lines(), ["a<b", "line"]);
        assert_eq!(scene.elements[3].kind, Kind::Other("image".into()));
        assert_eq!(scene.visible().count(), 4);
        assert_eq!(scene.background(), "#fafafa");
        // Unchanged: every element comes back exactly, files and appState too.
        let back: Value = serde_json::from_str(&scene.to_json()).unwrap();
        let orig: Value = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(back, orig);
        // Changed: fields written back, version bumped, unknown fields kept.
        let mut scene = scene;
        scene.elements[0].shift(10.0, 0.5);
        let back: Value = serde_json::from_str(&scene.to_json()).unwrap();
        let r = &back["elements"][0];
        assert_eq!(r["x"], json!(-30));
        assert_eq!(r["y"], json!(10.5));
        assert_eq!(r["version"], json!(4));
        assert_eq!(r["customData"], json!({"keep": true}));
        assert_eq!(back["elements"][1], orig["elements"][1]);
        assert!(Scene::parse("{}").is_err());
        assert!(Scene::parse("nope").is_err());
    }

    #[test]
    fn renders_svg() {
        let scene = Scene::parse(SAMPLE).unwrap();
        let svg = to_svg(&scene);
        assert!(svg.contains("viewBox=\"-56 "), "{svg}");
        assert!(svg.contains("fill=\"#fafafa\""));
        assert!(svg.contains("rx=\"12.5\""));
        assert!(svg.contains("stroke-dasharray=\"8 10\""));
        assert!(svg.contains("fill-opacity=\"0.35\""));
        assert!(svg.contains(">a&lt;b</text>"));
        assert!(svg.contains("text-anchor=\"middle\""));
        assert!(svg.contains("transform=\"rotate(28.65 "));
        assert!(svg.contains(">image</text>"));
        assert!(!svg.contains("500"));
    }

    #[test]
    fn new_elements_and_legacy_shapes() {
        let mut a = Element::new(Kind::Arrow, 0.0, 0.0, COLORS[1]);
        a.set_points(&[[10.0, 20.0], [50.0, 5.0]]);
        let v = a.to_json();
        assert_eq!(v["type"], "arrow");
        assert_eq!(v["roughness"], 0);
        assert_eq!(v["points"], json!([[0, 0], [40, -15]]));
        assert_eq!(
            (v["x"].clone(), v["width"].clone(), v["height"].clone()),
            (json!(10), json!(40), json!(15))
        );
        assert_eq!(v["endArrowhead"], "arrow");
        assert_eq!(v["id"].as_str().unwrap().len(), 21);
        assert_ne!(
            Element::new(Kind::Text, 0.0, 0.0, "#000").id,
            Element::new(Kind::Text, 0.0, 0.0, "#000").id
        );
        assert!(a.hit([30.0, 12.5], 2.0));
        assert!(!a.hit([30.0, 40.0], 2.0));
        let legacy = vec![
            Shape::Rect {
                x: 1.0,
                y: 2.0,
                w: 30.0,
                h: 40.0,
                color: "#d62f4b".into(),
                width: 3.0,
            },
            Shape::Text {
                x: 5.0,
                y: 6.0,
                text: "hi".into(),
                size: 18.0,
                color: "#000000".into(),
            },
        ];
        let s = Scene::from_shapes(&legacy);
        let v: Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(v["elements"][0]["strokeWidth"], 3);
        assert_eq!(v["elements"][0]["strokeColor"], "#d62f4b");
        assert_eq!(v["elements"][1]["text"], "hi");
        assert_eq!(v["elements"][1]["fontSize"], 18);
        assert_eq!(Scene::parse(&s.to_json()).unwrap().elements.len(), 2);
    }
}
