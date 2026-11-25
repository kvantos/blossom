//! The diagram editor: an Excalidraw scene on a canvas.
//!
//! Tools: select (click to pick, drag to move), pen, line, arrow,
//! rectangle, ellipse, diamond and text; five colours; a 10-unit snap grid;
//! delete; undo and redo. The mouse wheel pans, Ctrl/Cmd+wheel zooms.
//! The canvas program keeps only the stroke in progress and the modifier
//! keys; everything else comes back as [`Edit`]s, so the scene and its undo
//! history live in [`DiagramEditor`].

use blossom_vault::Revision;
use iced::widget::canvas::{self, Cache, Frame, Geometry, Path, Stroke, Text};
use iced::widget::{
    button, canvas as canvas_widget, column, container, row, space, text, text_input,
};
use iced::{
    Color, Element as Widget, Fill, Font, Point as IcedPoint, Rectangle, Renderer, Size, Theme,
    Vector, keyboard, mouse,
};

use crate::app::Message;
use crate::excalidraw::{self, COLORS, Element, Kind, Point, Scene};
use crate::style;

/// Snap grid spacing, in scene units.
const GRID: f32 = 10.0;

/// How close (in screen pixels) a click must be to pick an element.
const PICK: f32 = 5.0;

/// Zoom limits.
const ZOOM: (f32, f32) = (0.1, 8.0);

/// The drawing tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Select and move.
    Select,
    /// Freehand.
    Pen,
    /// Straight line.
    Line,
    /// Line with an arrow head.
    Arrow,
    /// Rectangle.
    Rect,
    /// Ellipse.
    Ellipse,
    /// Diamond.
    Diamond,
    /// Text: click to place the text typed in the toolbar.
    Text,
}

impl Tool {
    const ALL: [Tool; 8] = [
        Tool::Select,
        Tool::Pen,
        Tool::Line,
        Tool::Arrow,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Diamond,
        Tool::Text,
    ];

    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Pen => "Pen",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Diamond => "Diamond",
            Tool::Text => "Text",
        }
    }
}

/// Changes to the editor.
#[derive(Clone, Debug)]
pub enum Edit {
    /// Pick a tool.
    Tool(Tool),
    /// Pick a colour (index into [`COLORS`]); recolours the selection.
    Color(usize),
    /// Turn the grid snap on or off.
    Snap,
    /// The text tool's text.
    Label(String),
    /// A finished element.
    Add(Box<Element>),
    /// Select element N, or nothing.
    Select(Option<usize>),
    /// Move element N (and its label) by (dx, dy) scene units.
    Move(usize, f32, f32),
    /// Delete the selected element (and its label).
    Delete,
    /// Undo.
    Undo,
    /// Redo.
    Redo,
    /// Pan the view by (dx, dy) screen pixels.
    Pan(f32, f32),
    /// Zoom by a factor around a screen point.
    Zoom(f32, f32, f32),
    /// Show the whole drawing at 100 %.
    Fit,
}

/// The view: `screen = scene * zoom + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Screen position of the scene origin.
    pub offset: [f32; 2],
    /// Scale.
    pub zoom: f32,
}

impl View {
    fn to_scene(self, p: Point) -> Point {
        [
            (p[0] - self.offset[0]) / self.zoom,
            (p[1] - self.offset[1]) / self.zoom,
        ]
    }

    fn fit(scene: &Scene) -> View {
        let (x0, y0) = scene.bounds().map_or((0.0, 0.0), |b| (b.0, b.1));
        View {
            offset: [40.0 - x0, 40.0 - y0],
            zoom: 1.0,
        }
    }
}

/// An open diagram.
pub struct DiagramEditor {
    /// The org file it belongs to (the link goes there).
    pub file: String,
    /// The SVG's link path in that file, when editing an existing diagram.
    pub link: Option<String>,
    /// The source file's revision when it was read (`None`: not written yet).
    pub base: Option<Revision>,
    /// The scene.
    pub scene: Scene,
    /// The selected element.
    pub selected: Option<usize>,
    /// The current tool.
    pub tool: Tool,
    /// Index into [`COLORS`].
    pub color: usize,
    /// Snap to the grid.
    pub snap: bool,
    /// The text tool's text.
    pub label: String,
    /// The view.
    pub view: View,
    /// Changed since opened.
    pub dirty: bool,
    /// Cancel was pressed once with unsaved changes.
    pub discard_warned: bool,
    undo: Vec<Vec<Element>>,
    redo: Vec<Vec<Element>>,
    cache: Cache,
}

impl DiagramEditor {
    /// An editor on `scene`; `link` and `base` name an existing diagram.
    #[must_use]
    pub fn new(file: String, link: Option<String>, base: Option<Revision>, scene: Scene) -> Self {
        DiagramEditor {
            file,
            link,
            base,
            view: View::fit(&scene),
            scene,
            selected: None,
            tool: Tool::Pen,
            color: 0,
            snap: true,
            label: String::new(),
            dirty: false,
            discard_warned: false,
            undo: Vec::new(),
            redo: Vec::new(),
            cache: Cache::new(),
        }
    }

    /// Whether there are unsaved changes.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.dirty
    }

    fn change(&mut self, f: impl FnOnce(&mut Scene)) {
        let before = self.scene.elements.clone();
        f(&mut self.scene);
        if self.scene.elements != before {
            self.undo.push(before);
            self.redo.clear();
            self.dirty = true;
            self.cache.clear();
        }
    }

    /// How near (scene units) an arrow end must be to a shape to bind.
    fn bind_tolerance(&self) -> f32 {
        2.0 * PICK / self.view.zoom
    }

    /// Applies an edit.
    pub fn update(&mut self, edit: Edit) {
        match edit {
            Edit::Tool(t) => {
                self.tool = t;
                if t != Tool::Select {
                    self.selected = None;
                    self.cache.clear();
                }
            }
            Edit::Color(c) => {
                self.color = c.min(COLORS.len() - 1);
                if let Some(i) = self.selected {
                    let color = COLORS[self.color].to_string();
                    self.change(|scene| {
                        if let Some(e) = scene.elements.get_mut(i) {
                            e.stroke = color;
                            e.changed = true;
                        }
                    });
                }
            }
            Edit::Snap => self.snap = !self.snap,
            Edit::Label(l) => self.label = l,
            Edit::Add(e) => {
                let tol = self.bind_tolerance();
                self.change(|scene| scene.add(*e, tol));
            }
            Edit::Select(i) => {
                self.selected = i.filter(|i| *i < self.scene.elements.len());
                self.cache.clear();
            }
            Edit::Move(i, dx, dy) => {
                let moving = self.scene.with_labels(i);
                let tol = self.bind_tolerance();
                self.change(|scene| scene.move_elements(&moving, dx, dy, tol));
            }
            Edit::Delete => {
                if let Some(i) = self.selected.take() {
                    let gone = self.scene.with_labels(i);
                    self.change(|scene| scene.remove(&gone));
                }
            }
            Edit::Undo | Edit::Redo => {
                let (from, to) = if matches!(edit, Edit::Undo) {
                    (&mut self.undo, &mut self.redo)
                } else {
                    (&mut self.redo, &mut self.undo)
                };
                if let Some(prev) = from.pop() {
                    to.push(std::mem::replace(&mut self.scene.elements, prev));
                    self.selected = None;
                    self.dirty = true;
                    self.cache.clear();
                }
            }
            Edit::Pan(dx, dy) => {
                self.view.offset[0] += dx;
                self.view.offset[1] += dy;
                self.cache.clear();
            }
            Edit::Zoom(factor, x, y) => {
                let at = self.view.to_scene([x, y]);
                self.view.zoom = (self.view.zoom * factor).clamp(ZOOM.0, ZOOM.1);
                self.view.offset = [x - at[0] * self.view.zoom, y - at[1] * self.view.zoom];
                self.cache.clear();
            }
            Edit::Fit => {
                self.view = View::fit(&self.scene);
                self.cache.clear();
            }
        }
    }

    /// Whether undo / redo have a step.
    #[must_use]
    pub fn history(&self) -> (bool, bool) {
        (!self.undo.is_empty(), !self.redo.is_empty())
    }
}

/// A CSS colour as Excalidraw stores it: `#rgb`, `#rrggbb`, `#rrggbbaa`,
/// `transparent` or one of CSS's basic names; black when it doesn't parse.
fn rgb(css: &str) -> Color {
    let css = css.trim();
    let named = match css.to_ascii_lowercase().as_str() {
        "transparent" => return Color::TRANSPARENT,
        "black" => "000000",
        "white" => "ffffff",
        "red" => "ff0000",
        "green" => "008000",
        "blue" => "0000ff",
        "yellow" => "ffff00",
        "orange" => "ffa500",
        "purple" => "800080",
        "gray" | "grey" => "808080",
        "silver" => "c0c0c0",
        "maroon" => "800000",
        "olive" => "808000",
        "lime" => "00ff00",
        "aqua" | "cyan" => "00ffff",
        "teal" => "008080",
        "navy" => "000080",
        "fuchsia" | "magenta" => "ff00ff",
        _ => "",
    };
    let hex = if named.is_empty() {
        css.strip_prefix('#').filter(|h| h.is_ascii()).unwrap_or("")
    } else {
        named
    };
    let digits: String = match hex.len() {
        // `#rgb` doubles each digit.
        3 | 4 => hex.chars().take(3).flat_map(|c| [c, c]).collect(),
        6 | 8 => hex[..6].to_string(),
        _ => String::new(),
    };
    let alpha = if hex.len() == 8 {
        u8::from_str_radix(&hex[6..], 16).map_or(1.0, |a| f32::from(a) / 255.0)
    } else if hex.len() == 4 {
        u8::from_str_radix(&hex[3..].repeat(2), 16).map_or(1.0, |a| f32::from(a) / 255.0)
    } else {
        1.0
    };
    let v = u32::from_str_radix(&digits, 16).unwrap_or(0);
    let [_, r, g, b] = v.to_be_bytes();
    Color::from_rgb8(r, g, b).scale_alpha(alpha)
}

fn snap(p: Point, on: bool) -> Point {
    if on {
        [(p[0] / GRID).round() * GRID, (p[1] / GRID).round() * GRID]
    } else {
        p
    }
}

/// The canvas's own state: the stroke in progress and the modifiers.
#[derive(Default)]
pub struct Drag {
    gesture: Gesture,
    modifiers: keyboard::Modifiers,
}

/// What the pointer is doing, in scene coordinates.
#[derive(Default)]
enum Gesture {
    /// Nothing.
    #[default]
    Idle,
    /// Drawing from `start`; `points` for the pen.
    Drawing {
        start: Point,
        current: Point,
        points: Vec<Point>,
    },
    /// Moving element `index`.
    Moving {
        index: usize,
        start: Point,
        current: Point,
    },
}

/// The canvas program, borrowing the editor.
struct Board<'a> {
    editor: &'a DiagramEditor,
}

impl Board<'_> {
    /// The element a drag from `start` to `end` makes with the current tool.
    fn element(&self, start: Point, end: Point, points: &[Point]) -> Option<Element> {
        let ed = self.editor;
        let color = COLORS[ed.color];
        let (left, top) = (start[0].min(end[0]), start[1].min(end[1]));
        let (width, height) = ((end[0] - start[0]).abs(), (end[1] - start[1]).abs());
        let moved = width > 1.0 || height > 1.0;
        let boxed = |kind: Kind| {
            let mut e = Element::new(kind, left, top, color);
            e.width = width;
            e.height = height;
            e
        };
        match ed.tool {
            Tool::Pen => {
                let mut e = Element::new(Kind::Freedraw, start[0], start[1], color);
                e.set_points(points);
                Some(e)
            }
            Tool::Line | Tool::Arrow if moved => {
                let kind = if ed.tool == Tool::Arrow {
                    Kind::Arrow
                } else {
                    Kind::Line
                };
                let mut e = Element::new(kind, start[0], start[1], color);
                e.set_points(&[start, end]);
                Some(e)
            }
            Tool::Rect if moved => Some(boxed(Kind::Rectangle)),
            Tool::Ellipse if moved => Some(boxed(Kind::Ellipse)),
            Tool::Diamond if moved => Some(boxed(Kind::Diamond)),
            _ => None,
        }
    }

    /// A left press at scene point `p`.
    fn press(&self, state: &mut Drag, p: Point) -> Option<canvas::Action<Message>> {
        let ed = self.editor;
        let publish = |e: Edit| Some(canvas::Action::publish(Message::Diagram(e)).and_capture());
        match ed.tool {
            Tool::Select => {
                let tol = PICK / ed.view.zoom;
                let hit = ed
                    .scene
                    .visible()
                    .filter(|(_, e)| e.hit(p, tol))
                    .map(|(i, _)| i)
                    .last();
                if let Some(index) = hit {
                    state.gesture = Gesture::Moving {
                        index,
                        start: p,
                        current: p,
                    };
                }
                publish(Edit::Select(hit))
            }
            Tool::Text => {
                let label = ed.label.trim();
                if label.is_empty() {
                    return None;
                }
                let [x, y] = snap(p, ed.snap);
                let mut e = Element::new(Kind::Text, x, y, COLORS[ed.color]);
                e.set_text(label);
                publish(Edit::Add(Box::new(e)))
            }
            _ => {
                let start = if ed.tool == Tool::Pen {
                    p
                } else {
                    snap(p, ed.snap)
                };
                state.gesture = Gesture::Drawing {
                    start,
                    current: start,
                    points: vec![p],
                };
                Some(canvas::Action::request_redraw().and_capture())
            }
        }
    }

    /// The pointer moved to scene point `p`.
    fn drag(&self, state: &mut Drag, p: Point) -> Option<canvas::Action<Message>> {
        let ed = self.editor;
        match &mut state.gesture {
            Gesture::Drawing {
                current, points, ..
            } => {
                if ed.tool == Tool::Pen {
                    let last = points.last().copied().unwrap_or(p);
                    if (p[0] - last[0]).hypot(p[1] - last[1]) >= 2.0 / ed.view.zoom {
                        points.push(p);
                    }
                    *current = p;
                } else {
                    *current = snap(p, ed.snap);
                }
                Some(canvas::Action::request_redraw())
            }
            Gesture::Moving { current, .. } => {
                *current = p;
                Some(canvas::Action::request_redraw())
            }
            Gesture::Idle => None,
        }
    }

    /// The button went up.
    fn release(&self, state: &mut Drag) -> Option<canvas::Action<Message>> {
        let publish = |e: Edit| Some(canvas::Action::publish(Message::Diagram(e)).and_capture());
        match std::mem::take(&mut state.gesture) {
            Gesture::Drawing {
                start,
                current,
                points,
            } => self
                .element(start, current, &points)
                .and_then(|e| publish(Edit::Add(Box::new(e)))),
            Gesture::Moving {
                index,
                start,
                current,
            } => {
                let (mut dx, mut dy) = (current[0] - start[0], current[1] - start[1]);
                if self.editor.snap {
                    dx = (dx / GRID).round() * GRID;
                    dy = (dy / GRID).round() * GRID;
                }
                if dx == 0.0 && dy == 0.0 {
                    Some(canvas::Action::request_redraw())
                } else {
                    publish(Edit::Move(index, dx, dy))
                }
            }
            Gesture::Idle => None,
        }
    }
}

impl canvas::Program<Message> for Board<'_> {
    type State = Drag;

    fn update(
        &self,
        state: &mut Drag,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let view = self.editor.view;
        let screen = cursor.position_in(bounds);
        let scene = screen.map(|p| view.to_scene([p.x, p.y]));
        match event {
            canvas::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.modifiers = *m;
                None
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                self.press(state, scene?)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => self.drag(state, scene?),
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                self.release(state)
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let at = screen?;
                let (dx, dy) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (x * 40.0, y * 40.0),
                    mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                };
                let edit = if state.modifiers.command() {
                    Edit::Zoom(if dy > 0.0 { 1.1 } else { 1.0 / 1.1 }, at.x, at.y)
                } else if state.modifiers.shift() {
                    Edit::Pan(dy, dx)
                } else {
                    Edit::Pan(dx, dy)
                };
                Some(canvas::Action::publish(Message::Diagram(edit)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Drag,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let ed = self.editor;
        let view = ed.view;
        let hidden: Vec<usize> = match &state.gesture {
            Gesture::Moving { index, .. } => ed.scene.affected(*index),
            _ => Vec::new(),
        };
        let base = ed.cache.draw(renderer, bounds.size(), |frame| {
            frame.fill_rectangle(
                IcedPoint::ORIGIN,
                bounds.size(),
                rgb(&ed.scene.background()),
            );
            if ed.snap {
                grid(frame, bounds.size(), view);
            }
            frame.with_save(|f| {
                f.translate(Vector::new(view.offset[0], view.offset[1]));
                f.scale(view.zoom);
                for (i, e) in ed.scene.visible() {
                    if !hidden.contains(&i) {
                        draw_element(f, e);
                    }
                }
                if let Some(e) = ed.selected.and_then(|i| ed.scene.elements.get(i)) {
                    outline(f, e, view.zoom);
                }
            });
        });
        let mut over = Frame::new(renderer, bounds.size());
        over.translate(Vector::new(view.offset[0], view.offset[1]));
        over.scale(view.zoom);
        match &state.gesture {
            Gesture::Drawing {
                start,
                current,
                points,
            } => {
                if let Some(e) = self.element(*start, *current, points) {
                    // Drawn as it will be added: an arrow snaps to the shapes it binds.
                    let mut preview = ed.scene.clone();
                    preview.add(e, ed.bind_tolerance());
                    if let Some(e) = preview.elements.last() {
                        draw_element(&mut over, e);
                    }
                }
            }
            Gesture::Moving {
                index,
                start,
                current,
            } => {
                let mut preview = ed.scene.clone();
                preview.move_elements(
                    &ed.scene.with_labels(*index),
                    current[0] - start[0],
                    current[1] - start[1],
                    ed.bind_tolerance(),
                );
                for j in &hidden {
                    let e = &preview.elements[*j];
                    draw_element(&mut over, e);
                    if j == index {
                        outline(&mut over, e, view.zoom);
                    }
                }
            }
            Gesture::Idle => {}
        }
        vec![base, over.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Drag,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if !cursor.is_over(bounds) {
            return mouse::Interaction::default();
        }
        match (self.editor.tool, &state.gesture) {
            (_, Gesture::Moving { .. }) => mouse::Interaction::Grabbing,
            (Tool::Select, _) => mouse::Interaction::Pointer,
            (Tool::Text, _) => mouse::Interaction::Text,
            _ => mouse::Interaction::Crosshair,
        }
    }
}

fn grid(frame: &mut Frame, size: Size, view: View) {
    let step = GRID * 2.0 * view.zoom;
    if step < 6.0 {
        return;
    }
    let dot = Color::from_rgb8(0xd8, 0xd8, 0xd8);
    let mut y = view.offset[1].rem_euclid(step);
    while y < size.height {
        let mut x = view.offset[0].rem_euclid(step);
        while x < size.width {
            frame.fill_rectangle(IcedPoint::new(x - 0.75, y - 0.75), Size::new(1.5, 1.5), dot);
            x += step;
        }
        y += step;
    }
}

fn pt([x, y]: Point) -> IcedPoint {
    IcedPoint::new(x, y)
}

/// Draws one element in scene coordinates.
fn draw_element(frame: &mut Frame, e: &Element) {
    if e.angle == 0.0 {
        draw_unrotated(frame, e);
        return;
    }
    let c = e.center();
    frame.with_save(|f| {
        f.translate(Vector::new(c[0], c[1]));
        f.rotate(e.angle);
        f.translate(Vector::new(-c[0], -c[1]));
        draw_unrotated(f, e);
    });
}

fn draw_unrotated(frame: &mut Frame, e: &Element) {
    let alpha = (e.opacity / 100.0).clamp(0.0, 1.0);
    let color = rgb(&e.stroke).scale_alpha(alpha);
    let segments: Vec<f32> = excalidraw::dashes(e).map_or_else(Vec::new, |d| d.to_vec());
    let stroke = Stroke {
        line_dash: canvas::LineDash {
            segments: &segments,
            offset: 0,
        },
        ..Stroke::default()
            .with_color(color)
            .with_width(e.stroke_width)
            .with_line_cap(canvas::LineCap::Round)
            .with_line_join(canvas::LineJoin::Round)
    };
    let fill = (e.background != "transparent")
        .then(|| rgb(&e.background).scale_alpha(alpha * excalidraw::fill_alpha(e)));
    let (x0, y0, x1, y1) = e.local_bounds();
    let closed = |frame: &mut Frame, path: Path| {
        if let Some(c) = fill {
            frame.fill(&path, c);
        }
        frame.stroke(&path, stroke);
    };
    match &e.kind {
        Kind::Rectangle => closed(
            frame,
            Path::rounded_rectangle(
                IcedPoint::new(x0, y0),
                Size::new(x1 - x0, y1 - y0),
                excalidraw::corner_radius(e).into(),
            ),
        ),
        Kind::Ellipse => closed(
            frame,
            Path::new(|b| {
                b.ellipse(canvas::path::arc::Elliptical {
                    center: IcedPoint::new(f32::midpoint(x0, x1), f32::midpoint(y0, y1)),
                    radii: Vector::new((x1 - x0) / 2.0, (y1 - y0) / 2.0),
                    rotation: iced::Radians(0.0),
                    start_angle: iced::Radians(0.0),
                    end_angle: iced::Radians(std::f32::consts::TAU),
                });
            }),
        ),
        Kind::Diamond => {
            let (cx, cy) = (f32::midpoint(x0, x1), f32::midpoint(y0, y1));
            closed(
                frame,
                Path::new(|b| {
                    b.move_to(IcedPoint::new(cx, y0));
                    b.line_to(IcedPoint::new(x1, cy));
                    b.line_to(IcedPoint::new(cx, y1));
                    b.line_to(IcedPoint::new(x0, cy));
                    b.close();
                }),
            );
        }
        Kind::Line | Kind::Arrow | Kind::Freedraw => draw_linear(frame, e, stroke, color),
        Kind::Text => draw_text(frame, e, color),
        Kind::Other(name) => {
            let grey = Color::from_rgb8(0x99, 0x99, 0x99);
            frame.stroke(
                &Path::rectangle(IcedPoint::new(x0, y0), Size::new(x1 - x0, y1 - y0)),
                Stroke {
                    line_dash: canvas::LineDash {
                        segments: &[6.0, 4.0],
                        offset: 0,
                    },
                    ..Stroke::default().with_color(grey).with_width(1.0)
                },
            );
            frame.fill_text(Text {
                content: name.clone(),
                position: IcedPoint::new(x0 + 4.0, y0 + 4.0),
                color: grey,
                size: 12.0.into(),
                ..Text::default()
            });
        }
    }
}

fn draw_linear(frame: &mut Frame, e: &Element, stroke: Stroke<'_>, color: Color) {
    let pts = e.absolute_points();
    let path = Path::new(|b| {
        if let Some((first, rest)) = pts.split_first() {
            b.move_to(pt(*first));
            if rest.is_empty() {
                b.line_to(pt(*first));
            }
            for p in rest {
                b.line_to(pt(*p));
            }
        }
    });
    frame.stroke(&path, stroke);
    let solid = Stroke {
        line_dash: canvas::LineDash::default(),
        ..stroke
    };
    for (kind, tip, from) in excalidraw::arrow_ends(e, &pts) {
        let [a, b] = excalidraw::arrow_head(from, tip, e.stroke_width);
        let head = Path::new(|p| {
            p.move_to(pt(a));
            p.line_to(pt(tip));
            p.line_to(pt(b));
            if kind == "triangle" {
                p.close();
            }
        });
        if kind == "triangle" {
            frame.fill(&head, color);
        }
        frame.stroke(&head, solid);
    }
}

fn draw_text(frame: &mut Frame, e: &Element, color: Color) {
    let (x0, y0, x1, _) = e.local_bounds();
    let step = e.font_size * e.line_height;
    let (align, x) = match e.text_align.as_str() {
        "center" => (iced::widget::text::Alignment::Center, f32::midpoint(x0, x1)),
        "right" => (iced::widget::text::Alignment::Right, x1),
        _ => (iced::widget::text::Alignment::Left, x0),
    };
    for (i, line) in e.lines().iter().enumerate() {
        // A handful of lines.
        #[allow(clippy::cast_precision_loss)]
        let top = y0 + step * i as f32;
        frame.fill_text(Text {
            content: (*line).to_string(),
            position: IcedPoint::new(x, top + step / 2.0),
            color,
            size: e.font_size.into(),
            font: if e.font_family == 3 {
                Font::MONOSPACE
            } else {
                Font::DEFAULT
            },
            align_x: align,
            align_y: iced::alignment::Vertical::Center,
            ..Text::default()
        });
    }
}

/// The dashed box around a selected element.
fn outline(frame: &mut Frame, e: &Element, zoom: f32) {
    let (x0, y0, x1, y1) = e.bounds();
    let pad = 4.0 / zoom;
    let dash = [4.0 / zoom, 4.0 / zoom];
    frame.stroke(
        &Path::rectangle(
            IcedPoint::new(x0 - pad, y0 - pad),
            Size::new(x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad),
        ),
        Stroke {
            line_dash: canvas::LineDash {
                segments: &dash,
                offset: 0,
            },
            ..Stroke::default()
                .with_color(Color::from_rgb8(0x19, 0x71, 0xc2))
                .with_width(1.0 / zoom)
        },
    );
}

/// The editor: toolbar and canvas.
pub fn view(ed: &DiagramEditor) -> Widget<'_, Message> {
    let on = Message::Diagram;
    let tools = row(Tool::ALL.iter().map(|t| {
        button(text(t.label()).size(13))
            .on_press(on(Edit::Tool(*t)))
            .padding([3, 8])
            .style(if ed.tool == *t {
                button::primary
            } else {
                button::secondary
            })
            .into()
    }))
    .spacing(4)
    .wrap();
    let colors = row(COLORS.iter().enumerate().map(|(i, c)| {
        let color = rgb(c);
        let chosen = ed.color == i;
        button(space::horizontal().width(16).height(16))
            .on_press(on(Edit::Color(i)))
            .padding(0)
            .style(move |_t: &Theme, _s| button::Style {
                background: Some(color.into()),
                border: iced::Border {
                    radius: 9.0.into(),
                    width: if chosen { 3.0 } else { 1.0 },
                    color: if chosen {
                        Color::from_rgb8(0x88, 0x88, 0x88)
                    } else {
                        Color::WHITE
                    },
                },
                ..button::Style::default()
            })
            .into()
    }))
    .spacing(6)
    .align_y(iced::Center);
    let (undo, redo) = ed.history();
    let small = |label: &'static str, msg: Option<Message>| {
        button(text(label).size(13))
            .on_press_maybe(msg)
            .padding([3, 8])
            .style(button::text)
    };
    let mut second = row![
        colors,
        small("Undo", undo.then(|| on(Edit::Undo))),
        small("Redo", redo.then(|| on(Edit::Redo))),
        small("Delete", ed.selected.is_some().then(|| on(Edit::Delete))),
        small(
            if ed.snap { "Snap: on" } else { "Snap: off" },
            Some(on(Edit::Snap))
        ),
        small("Fit", Some(on(Edit::Fit))),
        text(format!("{:.0} %", ed.view.zoom * 100.0)).size(12),
    ]
    .spacing(8)
    .align_y(iced::Center);
    if ed.tool == Tool::Text {
        second = second.push(
            text_input("Text to place, then click the canvas", &ed.label)
                .on_input(move |s| on(Edit::Label(s)))
                .size(13)
                .padding([3, 6])
                .width(280),
        );
    }
    let title = match &ed.link {
        Some(l) => format!("Edit {l}"),
        None => "New diagram".to_string(),
    };
    column![
        row![
            text(title).size(18).font(crate::app::bold()).width(Fill),
            text("Wheel pans, Ctrl+wheel zooms")
                .size(12)
                .style(crate::app::dim),
            button(text("Cancel").size(13))
                .on_press(Message::DiagramCancel)
                .padding([3, 10])
                .style(button::secondary),
            button(text("Save").size(13))
                .on_press(Message::DiagramSave)
                .padding([3, 10]),
        ]
        .spacing(8)
        .align_y(iced::Center),
        tools,
        second,
        container(canvas_widget(Board { editor: ed }).width(Fill).height(Fill))
            .style(style::canvas_frame)
            .padding(1),
    ]
    .spacing(8)
    .padding([16, 20])
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32) -> Element {
        let mut e = Element::new(Kind::Rectangle, x, 0.0, COLORS[0]);
        e.width = 10.0;
        e.height = 10.0;
        e
    }

    #[test]
    fn edits_and_history() {
        let mut ed = DiagramEditor::new("a.org".into(), None, None, Scene::new());
        assert!(!ed.changed());
        ed.update(Edit::Add(Box::new(rect(0.0))));
        let mut box2 = rect(20.0);
        box2.id = "box".into();
        ed.update(Edit::Add(Box::new(box2)));
        let mut label = Element::new(Kind::Text, 22.0, 2.0, COLORS[0]);
        label.container = Some("box".into());
        ed.update(Edit::Add(Box::new(label)));
        ed.update(Edit::Select(Some(0)));
        ed.update(Edit::Color(1));
        assert_eq!(ed.scene.elements[0].stroke, COLORS[1]);
        // Moving a shape moves its label.
        ed.update(Edit::Move(1, 10.0, 0.0));
        assert_eq!(
            (ed.scene.elements[1].x, ed.scene.elements[2].x),
            (30.0, 32.0)
        );
        ed.update(Edit::Select(Some(1)));
        ed.update(Edit::Delete);
        assert_eq!(ed.scene.elements.len(), 1);
        assert!(ed.changed());
        ed.update(Edit::Undo);
        assert_eq!(ed.scene.elements.len(), 3);
        ed.update(Edit::Redo);
        assert_eq!(ed.scene.elements.len(), 1);
        // Zoom keeps the point under the pointer still.
        let before = ed.view.to_scene([100.0, 50.0]);
        ed.update(Edit::Zoom(2.0, 100.0, 50.0));
        let after = ed.view.to_scene([100.0, 50.0]);
        assert!((before[0] - after[0]).abs() < 1e-4 && (before[1] - after[1]).abs() < 1e-4);
        assert_eq!(snap([14.0, 16.0], true), [10.0, 20.0]);
        assert_eq!(rgb("#e03131"), Color::from_rgb8(0xe0, 0x31, 0x31));
        assert_eq!(rgb("#e31"), Color::from_rgb8(0xee, 0x33, 0x11));
        assert_eq!(rgb("Red"), Color::from_rgb8(0xff, 0, 0));
        assert_eq!(rgb("#00000080").a, 128.0 / 255.0);
        assert_eq!(rgb("transparent"), Color::TRANSPARENT);
        assert_eq!(rgb("#a日本b"), Color::from_rgb8(0, 0, 0));
        assert_eq!(rgb("nonsense"), Color::from_rgb8(0, 0, 0));
    }
}
