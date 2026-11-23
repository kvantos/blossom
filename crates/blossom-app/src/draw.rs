//! The diagram editor: a white canvas with pen, line, arrow, rectangle,
//! ellipse and text tools, selecting and moving shapes, delete, undo/redo
//! and an optional 10-pixel snap grid. Shapes are [`diagram::Shape`]s.
//!
//! The canvas program keeps only the stroke in progress; finished strokes,
//! moves and selection come back as [`Edit`] messages, so the shapes and
//! their undo history live in [`DiagramEditor`].

use std::path::PathBuf;

use iced::widget::canvas::{self, Cache, Frame, Geometry, Path, Stroke, Text};
use iced::widget::{
    button, canvas as canvas_widget, column, container, row, space, text, text_input,
};
use iced::{Color, Element, Fill, Point as IcedPoint, Rectangle, Renderer, Size, Theme, mouse};

use crate::diagram::{self, COLORS, Point, Shape};
use crate::style;

/// Snap grid spacing.
const GRID: f32 = 10.0;

/// How close a click must be to pick a shape.
const PICK: f32 = 5.0;

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
    /// Text: click to place the text typed in the toolbar.
    Text,
}

impl Tool {
    const ALL: [Tool; 7] = [
        Tool::Select,
        Tool::Pen,
        Tool::Line,
        Tool::Arrow,
        Tool::Rect,
        Tool::Ellipse,
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
            Tool::Text => "Text",
        }
    }
}

/// Changes to the editor.
#[derive(Clone, Debug)]
pub enum Edit {
    /// Pick a tool.
    Tool(Tool),
    /// Pick a colour (index into [`COLORS`]).
    Color(usize),
    /// Turn the grid snap on or off.
    Snap,
    /// The text tool's text.
    Label(String),
    /// A finished shape.
    Add(Shape),
    /// Select a shape, or nothing.
    Select(Option<usize>),
    /// Move shape N by (dx, dy).
    Move(usize, f32, f32),
    /// Delete the selected shape.
    Delete,
    /// Undo.
    Undo,
    /// Redo.
    Redo,
}

/// An open diagram.
pub struct DiagramEditor {
    /// The org file it belongs to (the link goes there).
    pub file: String,
    /// The link path in that file, when editing an existing diagram.
    pub link: Option<String>,
    /// Its file, when editing an existing diagram.
    pub path: Option<PathBuf>,
    /// The shapes, bottom first.
    pub shapes: Vec<Shape>,
    /// The selected shape.
    pub selected: Option<usize>,
    /// The current tool.
    pub tool: Tool,
    /// Index into [`COLORS`].
    pub color: usize,
    /// Snap to the grid.
    pub snap: bool,
    /// The text tool's text.
    pub label: String,
    /// The shapes as loaded, to tell if anything changed.
    pub saved: Vec<Shape>,
    /// Cancel was pressed once with unsaved changes.
    pub discard_warned: bool,
    undo: Vec<Vec<Shape>>,
    redo: Vec<Vec<Shape>>,
    cache: Cache,
}

impl DiagramEditor {
    /// An editor on `shapes`; `link` and `path` name an existing diagram.
    #[must_use]
    pub fn new(
        file: String,
        link: Option<String>,
        path: Option<PathBuf>,
        shapes: Vec<Shape>,
    ) -> Self {
        DiagramEditor {
            file,
            link,
            path,
            saved: shapes.clone(),
            discard_warned: false,
            shapes,
            selected: None,
            tool: Tool::Pen,
            color: 0,
            snap: true,
            label: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            cache: Cache::new(),
        }
    }

    /// Whether there are unsaved changes.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.shapes != self.saved
    }

    fn change(&mut self, f: impl FnOnce(&mut Vec<Shape>)) {
        let before = self.shapes.clone();
        f(&mut self.shapes);
        if self.shapes != before {
            self.undo.push(before);
            self.redo.clear();
            self.cache.clear();
        }
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
                    self.change(|s| {
                        if let Some(shape) = s.get_mut(i) {
                            set_color(shape, color);
                        }
                    });
                }
            }
            Edit::Snap => self.snap = !self.snap,
            Edit::Label(l) => self.label = l,
            Edit::Add(shape) => self.change(|s| s.push(shape)),
            Edit::Select(i) => {
                self.selected = i.filter(|i| *i < self.shapes.len());
                self.cache.clear();
            }
            Edit::Move(i, dx, dy) => self.change(|s| {
                if let Some(shape) = s.get_mut(i) {
                    *shape = shape.moved(dx, dy);
                }
            }),
            Edit::Delete => {
                if let Some(i) = self.selected.take() {
                    self.change(|s| {
                        if i < s.len() {
                            s.remove(i);
                        }
                    });
                }
            }
            Edit::Undo | Edit::Redo => {
                let (from, to) = if matches!(edit, Edit::Undo) {
                    (&mut self.undo, &mut self.redo)
                } else {
                    (&mut self.redo, &mut self.undo)
                };
                if let Some(prev) = from.pop() {
                    to.push(std::mem::replace(&mut self.shapes, prev));
                    self.selected = None;
                    self.cache.clear();
                }
            }
        }
    }

    /// Whether undo / redo have a step.
    #[must_use]
    pub fn history(&self) -> (bool, bool) {
        (!self.undo.is_empty(), !self.redo.is_empty())
    }
}

fn set_color(shape: &mut Shape, new: String) {
    match shape {
        Shape::Pen { color, .. }
        | Shape::Line { color, .. }
        | Shape::Rect { color, .. }
        | Shape::Ellipse { color, .. }
        | Shape::Text { color, .. } => *color = new,
    }
}

/// `#rrggbb` as a colour; black when it doesn't parse.
fn rgb(hex: &str) -> Color {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    let [_, r, g, b] = v.to_be_bytes();
    Color::from_rgb8(r, g, b)
}

fn snap(p: Point, on: bool) -> Point {
    if on {
        [(p[0] / GRID).round() * GRID, (p[1] / GRID).round() * GRID]
    } else {
        p
    }
}

/// The stroke in progress, kept by the canvas.
#[derive(Default)]
pub enum Drag {
    /// Nothing.
    #[default]
    Idle,
    /// Drawing a shape from `start`; `points` for the pen.
    Drawing {
        /// Where it started.
        start: Point,
        /// Where the pointer is.
        current: Point,
        /// Pen points.
        points: Vec<Point>,
    },
    /// Moving shape `index`.
    Moving {
        /// The shape.
        index: usize,
        /// Where it started.
        start: Point,
        /// Where the pointer is.
        current: Point,
    },
}

/// The canvas program, borrowing the editor.
struct Board<'a> {
    editor: &'a DiagramEditor,
    on: fn(Edit) -> crate::app::Message,
}

impl Board<'_> {
    fn width(&self) -> f32 {
        if self.editor.tool == Tool::Pen {
            3.0
        } else {
            2.0
        }
    }

    /// The shape a drag from `start` to `end` makes with the current tool.
    fn shape(&self, start: Point, end: Point, points: &[Point]) -> Option<Shape> {
        let color = COLORS[self.editor.color].to_string();
        let width = self.width();
        let (x, y) = (start[0].min(end[0]), start[1].min(end[1]));
        let (w, h) = ((end[0] - start[0]).abs(), (end[1] - start[1]).abs());
        let moved = w > 1.0 || h > 1.0;
        match self.editor.tool {
            Tool::Pen => Some(Shape::Pen {
                points: points.to_vec(),
                color,
                width,
            }),
            Tool::Line | Tool::Arrow if moved => Some(Shape::Line {
                from: start,
                to: end,
                arrow: self.editor.tool == Tool::Arrow,
                color,
                width,
            }),
            Tool::Rect if moved => Some(Shape::Rect {
                x,
                y,
                w,
                h,
                color,
                width,
            }),
            Tool::Ellipse if moved => Some(Shape::Ellipse {
                x,
                y,
                w,
                h,
                color,
                width,
            }),
            _ => None,
        }
    }
}

impl Board<'_> {
    /// A left press at `p`: pick a shape, place text, or start a stroke.
    fn press(&self, state: &mut Drag, p: Point) -> Option<canvas::Action<crate::app::Message>> {
        let ed = self.editor;
        let publish = |e: Edit| Some(canvas::Action::publish((self.on)(e)).and_capture());
        match ed.tool {
            Tool::Select => {
                let hit = ed.shapes.iter().rposition(|s| s.hit(p, PICK));
                if let Some(index) = hit {
                    *state = Drag::Moving {
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
                publish(Edit::Add(Shape::Text {
                    x,
                    y,
                    text: label.to_string(),
                    size: 18.0,
                    color: COLORS[ed.color].to_string(),
                }))
            }
            _ => {
                let start = if ed.tool == Tool::Pen {
                    p
                } else {
                    snap(p, ed.snap)
                };
                *state = Drag::Drawing {
                    start,
                    current: start,
                    points: vec![p],
                };
                Some(canvas::Action::request_redraw().and_capture())
            }
        }
    }
}

impl canvas::Program<crate::app::Message> for Board<'_> {
    type State = Drag;

    fn update(
        &self,
        state: &mut Drag,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<crate::app::Message>> {
        let ed = self.editor;
        let pos = cursor.position_in(bounds).map(|p| [p.x, p.y]);
        let publish = |e: Edit| Some(canvas::Action::publish((self.on)(e)).and_capture());
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                self.press(state, pos?)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let p = pos?;
                match state {
                    Drag::Drawing {
                        current, points, ..
                    } => {
                        if ed.tool == Tool::Pen {
                            let last = points.last().copied().unwrap_or(p);
                            if (p[0] - last[0]).hypot(p[1] - last[1]) >= 2.0 {
                                points.push(p);
                            }
                            *current = p;
                        } else {
                            *current = snap(p, ed.snap);
                        }
                        Some(canvas::Action::request_redraw())
                    }
                    Drag::Moving { current, .. } => {
                        *current = p;
                        Some(canvas::Action::request_redraw())
                    }
                    Drag::Idle => None,
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                match std::mem::take(state) {
                    Drag::Drawing {
                        start,
                        current,
                        points,
                    } => self
                        .shape(start, current, &points)
                        .and_then(|s| publish(Edit::Add(s))),
                    Drag::Moving {
                        index,
                        start,
                        current,
                    } => {
                        let (mut dx, mut dy) = (current[0] - start[0], current[1] - start[1]);
                        if ed.snap {
                            dx = (dx / GRID).round() * GRID;
                            dy = (dy / GRID).round() * GRID;
                        }
                        if dx == 0.0 && dy == 0.0 {
                            Some(canvas::Action::request_redraw())
                        } else {
                            publish(Edit::Move(index, dx, dy))
                        }
                    }
                    Drag::Idle => None,
                }
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
        let shapes = ed.cache.draw(renderer, bounds.size(), |frame| {
            frame.fill_rectangle(IcedPoint::ORIGIN, bounds.size(), Color::WHITE);
            if ed.snap {
                grid(frame, bounds.size());
            }
            for (i, s) in ed.shapes.iter().enumerate() {
                if !matches!(state, Drag::Moving { index, .. } if *index == i) {
                    draw_shape(frame, s);
                }
            }
            if let Some(i) = ed.selected
                && let Some(s) = ed.shapes.get(i)
            {
                outline(frame, s);
            }
        });
        let mut over = Frame::new(renderer, bounds.size());
        match state {
            Drag::Drawing {
                start,
                current,
                points,
            } => {
                if let Some(s) = self.shape(*start, *current, points) {
                    draw_shape(&mut over, &s);
                }
            }
            Drag::Moving {
                index,
                start,
                current,
            } => {
                if let Some(s) = ed.shapes.get(*index) {
                    let m = s.moved(current[0] - start[0], current[1] - start[1]);
                    draw_shape(&mut over, &m);
                    outline(&mut over, &m);
                }
            }
            Drag::Idle => {}
        }
        vec![shapes, over.into_geometry()]
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
        match (self.editor.tool, state) {
            (_, Drag::Moving { .. }) => mouse::Interaction::Grabbing,
            (Tool::Select, _) => mouse::Interaction::Pointer,
            (Tool::Text, _) => mouse::Interaction::Text,
            _ => mouse::Interaction::Crosshair,
        }
    }
}

fn grid(frame: &mut Frame, size: Size) {
    let dot = Color::from_rgb8(0xd8, 0xd8, 0xd8);
    let step = GRID * 2.0;
    let mut y = step;
    while y < size.height {
        let mut x = step;
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

/// Draws one shape.
fn draw_shape(frame: &mut Frame, s: &Shape) {
    let color = rgb(s.color());
    let stroke = |w: f32| {
        Stroke::default()
            .with_color(color)
            .with_width(w)
            .with_line_cap(canvas::LineCap::Round)
            .with_line_join(canvas::LineJoin::Round)
    };
    match s {
        Shape::Pen { points, width, .. } => {
            let path = Path::new(|b| {
                if let Some((first, rest)) = points.split_first() {
                    b.move_to(pt(*first));
                    if rest.is_empty() {
                        b.line_to(pt(*first));
                    }
                    for p in rest {
                        b.line_to(pt(*p));
                    }
                }
            });
            frame.stroke(&path, stroke(*width));
        }
        Shape::Line {
            from,
            to,
            arrow,
            width,
            ..
        } => {
            frame.stroke(&Path::line(pt(*from), pt(*to)), stroke(*width));
            if *arrow {
                let [a, b] = diagram::arrow_head(*from, *to, *width);
                let head = Path::new(|p| {
                    p.move_to(pt(a));
                    p.line_to(pt(*to));
                    p.line_to(pt(b));
                });
                frame.stroke(&head, stroke(*width));
            }
        }
        Shape::Rect {
            x, y, w, h, width, ..
        } => frame.stroke(
            &Path::rounded_rectangle(IcedPoint::new(*x, *y), Size::new(*w, *h), 3.0.into()),
            stroke(*width),
        ),
        Shape::Ellipse {
            x, y, w, h, width, ..
        } => {
            let path = Path::new(|b| {
                b.ellipse(canvas::path::arc::Elliptical {
                    center: IcedPoint::new(x + w / 2.0, y + h / 2.0),
                    radii: iced::Vector::new(w / 2.0, h / 2.0),
                    rotation: iced::Radians(0.0),
                    start_angle: iced::Radians(0.0),
                    end_angle: iced::Radians(std::f32::consts::TAU),
                });
            });
            frame.stroke(&path, stroke(*width));
        }
        Shape::Text {
            x, y, text, size, ..
        } => frame.fill_text(Text {
            content: text.clone(),
            position: IcedPoint::new(*x, *y),
            color,
            size: (*size).into(),
            ..Text::default()
        }),
    }
}

/// The dashed box around a selected shape.
fn outline(frame: &mut Frame, s: &Shape) {
    let b = s.bounds();
    let path = Path::rectangle(
        IcedPoint::new(b.x0 - 4.0, b.y0 - 4.0),
        Size::new(b.x1 - b.x0 + 8.0, b.y1 - b.y0 + 8.0),
    );
    frame.stroke(
        &path,
        Stroke {
            line_dash: canvas::LineDash {
                segments: &[4.0, 4.0],
                offset: 0,
            },
            ..Stroke::default()
                .with_color(Color::from_rgb8(0x2f, 0x6f, 0xd6))
                .with_width(1.0)
        },
    );
}

/// The editor: toolbar and canvas.
pub fn view<'a>(
    ed: &'a DiagramEditor,
    on: fn(Edit) -> crate::app::Message,
    save: crate::app::Message,
    cancel: crate::app::Message,
) -> Element<'a, crate::app::Message> {
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
    .spacing(4);
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
    let small = |label: &'a str, msg: Option<crate::app::Message>| {
        button(text(label).size(13))
            .on_press_maybe(msg)
            .padding([3, 8])
            .style(button::text)
    };
    let title = if ed.path.is_some() {
        "Edit diagram"
    } else {
        "New diagram"
    };
    let mut second = row![
        colors,
        small("Undo", undo.then(|| on(Edit::Undo))),
        small("Redo", redo.then(|| on(Edit::Redo))),
        small("Delete", ed.selected.is_some().then(|| on(Edit::Delete))),
        button(text(if ed.snap { "Snap: on" } else { "Snap: off" }).size(13))
            .on_press(on(Edit::Snap))
            .padding([3, 8])
            .style(button::text),
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
    column![
        row![
            text(title).size(18).font(crate::app::bold()).width(Fill),
            button(text("Cancel").size(13))
                .on_press(cancel)
                .padding([3, 10])
                .style(button::secondary),
            button(text("Save").size(13))
                .on_press(save)
                .padding([3, 10]),
        ]
        .spacing(8)
        .align_y(iced::Center),
        tools,
        second,
        container(
            canvas_widget(Board { editor: ed, on })
                .width(Fill)
                .height(Fill)
        )
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

    fn rect(x: f32) -> Shape {
        Shape::Rect {
            x,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            color: COLORS[0].into(),
            width: 2.0,
        }
    }

    #[test]
    fn edits_and_history() {
        let mut ed = DiagramEditor::new("a.org".into(), None, None, vec![]);
        assert!(!ed.changed());
        ed.update(Edit::Add(rect(0.0)));
        ed.update(Edit::Add(rect(20.0)));
        ed.update(Edit::Select(Some(0)));
        ed.update(Edit::Color(1));
        assert_eq!(ed.shapes[0].color(), COLORS[1]);
        ed.update(Edit::Move(1, 10.0, 0.0));
        assert_eq!(ed.shapes[1], rect(30.0));
        ed.update(Edit::Delete);
        assert_eq!(ed.shapes.len(), 1);
        assert!(ed.changed());
        ed.update(Edit::Undo);
        assert_eq!(ed.shapes.len(), 2);
        ed.update(Edit::Redo);
        assert_eq!(ed.shapes.len(), 1);
        for _ in 0..10 {
            ed.update(Edit::Undo);
        }
        assert_eq!(ed.shapes, Vec::<Shape>::new());
        assert_eq!(snap([14.0, 16.0], true), [10.0, 20.0]);
        assert_eq!(rgb("#d62f4b"), Color::from_rgb8(0xd6, 0x2f, 0x4b));
    }
}
