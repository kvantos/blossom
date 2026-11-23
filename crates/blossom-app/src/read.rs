//! The read view: core's render blocks as iced widgets.
//!
//! Images are drawn from [`Picture`]s the app loads through the worker,
//! keyed by the link path as written; one not loaded yet shows its name.

use std::collections::HashMap;

use blossom_core::render::{Block, Span, Style};
use iced::widget::text::Span as TextSpan;
use iced::widget::{
    button, checkbox, column, container, image, rich_text, row, rule, span, svg, text,
};
use iced::{Color, ContentFit, Element, Fill, Font, Length, Theme, font};

use crate::style;

/// Body text size.
pub const SIZE: u32 = 15;

/// Colours spans need (they are built without the theme).
#[derive(Clone, Copy, Debug)]
pub struct Ink {
    /// Links.
    pub link: Color,
    /// Timestamps.
    pub date: Color,
}

impl Ink {
    /// The colours of `theme`.
    #[must_use]
    pub fn of(theme: &Theme) -> Ink {
        Ink {
            link: style::link(theme),
            date: theme.palette().warning,
        }
    }
}

/// What clicks in the read view produce.
pub struct Actions<M> {
    /// A link target was clicked.
    pub link: fn(String) -> M,
    /// The checkbox of the item at this byte offset (in the rendered text)
    /// was clicked.
    pub check: fn(usize) -> M,
    /// An image's button: the link path as written.
    pub image: fn(String) -> M,
}

/// An image for the read view.
#[derive(Clone, Debug)]
pub enum Picture {
    /// Being read.
    Loading,
    /// A bitmap.
    Raster(image::Handle),
    /// An SVG.
    Svg(svg::Handle),
    /// The file isn't there or can't be read.
    Missing,
}

/// Loaded images, by link path.
pub type Pictures = HashMap<String, Picture>;

/// Widest an inline image is drawn.
const MAX_IMAGE_WIDTH: f32 = 760.0;

// Function pointers are `Copy` whatever `M` is, which a derive can't see.
impl<M> Clone for Actions<M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M> Copy for Actions<M> {}

/// The blocks as a column.
pub fn blocks<'a, M: Clone + 'a>(
    blocks: &'a [Block],
    ink: Ink,
    on: Actions<M>,
    pictures: &'a Pictures,
) -> Element<'a, M> {
    column(blocks.iter().map(|b| block(b, ink, on, pictures)))
        .spacing(10)
        .width(Fill)
        .into()
}

/// Every image path in `blocks`, nested ones included.
#[must_use]
pub fn image_paths(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    for b in blocks {
        match b {
            Block::Image { path, .. } => out.push(path.clone()),
            Block::ListItem { children, .. } | Block::Quote(children) => {
                out.extend(image_paths(children));
            }
            _ => {}
        }
    }
    out
}

fn picture<'a, M: Clone + 'a>(
    path: &'a str,
    on: Actions<M>,
    pictures: &'a Pictures,
) -> Element<'a, M> {
    let shown: Option<Element<'a, M>> = match pictures.get(path) {
        Some(Picture::Raster(h)) => Some(
            image(h.clone())
                .content_fit(ContentFit::ScaleDown)
                .width(Length::Shrink)
                .into(),
        ),
        Some(Picture::Svg(h)) => Some(
            svg(h.clone())
                .content_fit(ContentFit::ScaleDown)
                .width(Length::Shrink)
                .into(),
        ),
        Some(Picture::Missing) => None,
        Some(Picture::Loading) | None => Some(text("Loading image…").size(12).into()),
    };
    let is_svg = std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    let dim = |t: &Theme| text::Style {
        color: Some(style::dim(t)),
    };
    let caption = row![
        text(if shown.is_some() {
            path.to_string()
        } else {
            format!("Missing image: {path}")
        })
        .size(11)
        .style(dim)
        .width(Fill),
        button(text(if is_svg { "Edit diagram" } else { "Open" }).size(11))
            .on_press((on.image)(path.to_string()))
            .padding([1, 6])
            .style(button::text),
    ]
    .spacing(6)
    .align_y(iced::Center);
    let mut col = column![].spacing(2).max_width(MAX_IMAGE_WIDTH);
    if let Some(s) = shown {
        col = col.push(container(s).padding(1).style(style::picture));
    }
    col.push(caption).into()
}

fn block<'a, M: Clone + 'a>(
    b: &'a Block,
    ink: Ink,
    on: Actions<M>,
    pictures: &'a Pictures,
) -> Element<'a, M> {
    let on_link = on.link;
    match b {
        Block::Image { path, .. } => picture(path, on, pictures),
        Block::Paragraph(spans) => paragraph(spans, ink, on_link),
        Block::ListItem {
            bullet,
            checkbox: mark,
            offset,
            children,
        } => {
            let offset = *offset;
            let marker: Element<'a, M> = match mark {
                Some(c) => checkbox(*c == 'X')
                    .size(14)
                    .on_toggle(move |_| (on.check)(offset))
                    .into(),
                None => text(if bullet.ends_with(['.', ')']) {
                    bullet.as_str()
                } else {
                    "•"
                })
                .size(SIZE)
                .into(),
            };
            row![
                container(marker).width(22),
                column(children.iter().map(|c| block(c, ink, on, pictures)))
                    .spacing(4)
                    .width(Fill)
            ]
            .spacing(4)
            .into()
        }
        Block::Code {
            language,
            text: code,
        } => {
            let body = text(code.as_str()).font(Font::MONOSPACE).size(SIZE - 1);
            let content: Element<'a, M> = match language {
                Some(lang) if !lang.is_empty() => column![
                    text(lang.as_str()).size(11).style(|t: &Theme| text::Style {
                        color: Some(style::dim(t))
                    }),
                    body
                ]
                .spacing(4)
                .into(),
                _ => body.into(),
            };
            container(content)
                .padding(10)
                .width(Fill)
                .style(style::code_block)
                .into()
        }
        Block::Quote(inner) => {
            container(column(inner.iter().map(|c| block(c, ink, on, pictures))).spacing(8))
                .padding([8, 14])
                .width(Fill)
                .style(style::quote)
                .into()
        }
        Block::Table(rows) => column(rows.iter().map(|cells| {
            row(cells.iter().map(|c| {
                container(paragraph(c, ink, on_link))
                    .width(Fill)
                    .padding([2, 6])
                    .into()
            }))
            .into()
        }))
        .into(),
        Block::Rule => rule::horizontal(1).into(),
        Block::Drawer { name, text: body } => container(
            text(format!(":{name}:\n{body}"))
                .font(Font::MONOSPACE)
                .size(SIZE - 2)
                .style(|t: &Theme| text::Style {
                    color: Some(style::dim(t)),
                }),
        )
        .into(),
    }
}

/// Styled spans as rich text at body size.
pub fn paragraph<'a, M: Clone + 'a>(
    spans: &'a [Span],
    ink: Ink,
    on_link: fn(String) -> M,
) -> Element<'a, M> {
    sized(spans, SIZE, ink, on_link)
}

/// Styled spans as rich text at `size`.
pub fn sized<'a, M: Clone + 'a>(
    spans: &'a [Span],
    size: u32,
    ink: Ink,
    on_link: fn(String) -> M,
) -> Element<'a, M> {
    rich_text(spans.iter().map(|s| styled(s, ink)).collect::<Vec<_>>())
        .size(size)
        .on_link_click(on_link)
        .into()
}

fn styled(s: &Span, ink: Ink) -> TextSpan<'_, String, Font> {
    let Style {
        bold,
        italic,
        underline,
        strike,
        code,
        date,
    } = s.style;
    let mut font = if code { Font::MONOSPACE } else { Font::DEFAULT };
    if bold {
        font.weight = font::Weight::Bold;
    }
    if italic {
        font.style = font::Style::Italic;
    }
    let mut out = span(s.text.as_str())
        .font(font)
        .underline(underline || s.link.is_some())
        .strikethrough(strike);
    if date {
        out = out.color(ink.date);
    }
    if let Some(link) = &s.link {
        out = out.link(link.clone()).color(ink.link);
    }
    out
}
