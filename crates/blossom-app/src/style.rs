//! Blossom's light and dark themes and the small styles built on them.

// Colours are written as 0xRRGGBB, which reads better than digit groups.
#![allow(clippy::unreadable_literal)]

use iced::theme::{Mode, Palette};
use iced::widget::{button, container};
use iced::{Background, Border, Color, Theme, color};

/// The theme for the system's light/dark mode.
#[must_use]
pub fn theme(mode: Mode) -> Theme {
    match mode {
        Mode::Dark => Theme::custom(
            "Blossom Dark",
            Palette {
                background: color!(0x1e1b1d),
                text: color!(0xe9e3e6),
                primary: color!(0xe58fb1),
                success: color!(0x8fcf9c),
                warning: color!(0xe7c27d),
                danger: color!(0xf08080),
            },
        ),
        Mode::Light | Mode::None => Theme::custom(
            "Blossom Light",
            Palette {
                background: color!(0xfdfbfc),
                text: color!(0x2b2427),
                primary: color!(0xc2457a),
                success: color!(0x2e7d4f),
                warning: color!(0xa86a00),
                danger: color!(0xc0392b),
            },
        ),
    }
}

/// What a chip shows, which decides its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    /// A not-done keyword.
    Todo,
    /// A done keyword.
    Done,
    /// A priority cookie.
    Priority,
    /// A tag.
    Tag,
    /// A date.
    Date,
    /// A date before today on an open item.
    Overdue,
}

/// `a` moved toward `b` by `t` (0 to 1).
#[must_use]
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

/// A rounded, tinted label.
pub fn chip(theme: &Theme, kind: Chip) -> container::Style {
    let p = theme.palette();
    let ink = match kind {
        Chip::Todo => p.primary,
        Chip::Done => p.success,
        Chip::Priority => p.warning,
        Chip::Tag => mix(p.text, p.background, 0.35),
        Chip::Date => mix(p.text, p.background, 0.2),
        Chip::Overdue => p.danger,
    };
    container::Style {
        text_color: Some(ink),
        background: Some(Background::Color(mix(p.background, ink, 0.14))),
        border: Border {
            radius: 4.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// A pane with a slightly different background from the window.
pub fn pane(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.weak.color)),
        ..container::Style::default()
    }
}

/// Code and fixed-width blocks.
pub fn code_block(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.weak.color)),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Quote blocks: a tinted box.
pub fn quote(theme: &Theme) -> container::Style {
    let p = theme.palette();
    container::Style {
        background: Some(Background::Color(mix(p.background, p.primary, 0.06))),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Dimmed text colour (drawers, counts, placeholders).
#[must_use]
pub fn dim(theme: &Theme) -> Color {
    let p = theme.palette();
    mix(p.text, p.background, 0.45)
}

/// Link colour.
#[must_use]
pub fn link(theme: &Theme) -> Color {
    theme.palette().primary
}

/// A list row: flat, highlighted when hovered or selected.
pub fn row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let p = theme.extended_palette();
        let background = match (selected, status) {
            (true, _) => Some(Background::Color(p.primary.weak.color.scale_alpha(0.35))),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(Background::Color(
                p.background.strong.color.scale_alpha(0.5),
            )),
            _ => None,
        };
        button::Style {
            background,
            text_color: p.background.base.text,
            border: Border {
                radius: 4.0.into(),
                ..Border::default()
            },
            ..button::Style::default()
        }
    }
}

/// Code text colour in the editor.
#[must_use]
pub fn code_ink(theme: &Theme) -> Color {
    let p = theme.palette();
    mix(p.text, p.success, 0.55)
}

/// The section editor: the read pane's background, a thin accent border
/// when focused.
pub fn editor(
    theme: &Theme,
    status: iced::widget::text_editor::Status,
) -> iced::widget::text_editor::Style {
    let p = theme.palette();
    let mut s = iced::widget::text_editor::default(theme, status);
    s.background = Background::Color(p.background);
    s.border = Border {
        radius: 6.0.into(),
        width: 1.0,
        color: match status {
            iced::widget::text_editor::Status::Focused { .. } => p.primary,
            _ => mix(p.text, p.background, 0.8),
        },
    };
    s
}

/// The banner for a save conflict.
pub fn banner(theme: &Theme) -> container::Style {
    let p = theme.palette();
    container::Style {
        background: Some(Background::Color(mix(p.background, p.danger, 0.12))),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// A raised panel (capture, refile).
pub fn modal(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.base.color)),
        text_color: Some(p.background.base.text),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: p.background.strong.color,
        },
        shadow: iced::Shadow {
            color: Color::BLACK.scale_alpha(0.3),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 18.0,
        },
        ..container::Style::default()
    }
}

/// The bar that reports an operation, with its Undo.
pub fn notice(theme: &Theme) -> container::Style {
    let p = theme.palette();
    container::Style {
        background: Some(Background::Color(mix(p.background, p.primary, 0.15))),
        ..container::Style::default()
    }
}
