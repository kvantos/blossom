//! The read view: core's render blocks as iced widgets.

use blossom_core::render::{Block, Span, Style};
use iced::widget::text::Span as TextSpan;
use iced::widget::{checkbox, column, container, rich_text, row, rule, span, text};
use iced::{Color, Element, Fill, Font, Theme, font};

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

/// The blocks as a column. `on_link` turns a clicked link target into a message.
pub fn blocks<'a, M: Clone + 'a>(
    blocks: &'a [Block],
    ink: Ink,
    on_link: fn(String) -> M,
) -> Element<'a, M> {
    column(blocks.iter().map(|b| block(b, ink, on_link)))
        .spacing(10)
        .width(Fill)
        .into()
}

fn block<'a, M: Clone + 'a>(b: &'a Block, ink: Ink, on_link: fn(String) -> M) -> Element<'a, M> {
    match b {
        Block::Paragraph(spans) => paragraph(spans, ink, on_link),
        Block::ListItem {
            bullet,
            checkbox: mark,
            children,
            ..
        } => {
            let marker: Element<'a, M> = match mark {
                // Read-only for now; toggling comes with editing.
                Some(c) => checkbox(*c == 'X').size(14).into(),
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
                column(children.iter().map(|c| block(c, ink, on_link)))
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
            container(column(inner.iter().map(|c| block(c, ink, on_link))).spacing(8))
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
