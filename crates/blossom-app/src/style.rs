//! Blossom's themes (Blossom, Solarized, Gray; each light and dark) and the
//! small styles built on them.

// Colours are written as 0xRRGGBB, which reads better than digit groups.
#![allow(clippy::unreadable_literal)]

use iced::theme::{Mode, Palette};
use iced::widget::{button, container};
use iced::{Background, Border, Color, Theme, color};
use serde::{Deserialize, Serialize};

/// A set of colours, each with a light and a dark version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Colours {
    /// Blossom's own pink.
    #[default]
    Blossom,
    /// Ethan Schoonover's Solarized.
    Solarized,
    /// Calm neutral greys with a slate-blue accent.
    Gray,
}

impl Colours {
    /// Every set, in the order the Appearance panel lists them.
    pub const ALL: [Colours; 3] = [Colours::Blossom, Colours::Solarized, Colours::Gray];

    /// The name shown in the Appearance panel.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Colours::Blossom => "Blossom (pink)",
            Colours::Solarized => "Solarized",
            Colours::Gray => "Gray",
        }
    }
}

/// Light, dark, or whatever the system uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeChoice {
    /// Follow the system's light/dark setting.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl ModeChoice {
    /// Every choice, in the order the Appearance panel lists them.
    pub const ALL: [ModeChoice; 3] = [ModeChoice::System, ModeChoice::Light, ModeChoice::Dark];

    /// The name shown in the Appearance panel.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ModeChoice::System => "Follow system",
            ModeChoice::Light => "Light",
            ModeChoice::Dark => "Dark",
        }
    }

    /// The mode to use when the system's is `system`.
    #[must_use]
    pub fn resolve(self, system: Mode) -> Mode {
        match self {
            ModeChoice::System => system,
            ModeChoice::Light => Mode::Light,
            ModeChoice::Dark => Mode::Dark,
        }
    }
}

/// The theme for `colours` in `mode` (`Mode::None` counts as light).
#[must_use]
pub fn theme(colours: Colours, mode: Mode) -> Theme {
    let dark = mode == Mode::Dark;
    let (name, palette) = match (colours, dark) {
        (Colours::Blossom, true) => (
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
        (Colours::Blossom, false) => (
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
        // Solarized's own accents. Light text is base01 rather than base00,
        // which is too faint for long reading (4.1:1).
        (Colours::Solarized, true) => (
            "Solarized Dark",
            Palette {
                background: color!(0x002b36), // base03
                text: color!(0x93a1a1),       // base1
                primary: color!(0x268bd2),    // blue
                success: color!(0x859900),    // green
                warning: color!(0xb58900),    // yellow
                danger: color!(0xdc322f),     // red
            },
        ),
        (Colours::Solarized, false) => (
            "Solarized Light",
            Palette {
                background: color!(0xfdf6e3), // base3
                text: color!(0x586e75),       // base01
                primary: color!(0x268bd2),    // blue
                success: color!(0x859900),    // green
                warning: color!(0xb58900),    // yellow
                danger: color!(0xdc322f),     // red
            },
        ),
        (Colours::Gray, true) => (
            "Gray Dark",
            Palette {
                background: color!(0x1f1f1f),
                text: color!(0xe3e3e3),
                primary: color!(0x9fb0c4),
                success: color!(0x8fbf9b),
                warning: color!(0xd4b36a),
                danger: color!(0xe08c8c),
            },
        ),
        (Colours::Gray, false) => (
            "Gray Light",
            Palette {
                background: color!(0xf6f6f6),
                text: color!(0x262626),
                primary: color!(0x4f5d6e),
                success: color!(0x3f6f4f),
                warning: color!(0x86651a),
                danger: color!(0xa23b3b),
            },
        ),
    };
    Theme::custom(name, palette)
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

/// The border around the diagram canvas.
pub fn canvas_frame(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            radius: 4.0.into(),
            width: 1.0,
            color: theme.extended_palette().background.strong.color,
        },
        ..container::Style::default()
    }
}

/// The frame around an inline image.
pub fn picture(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            radius: 4.0.into(),
            width: 1.0,
            color: theme.extended_palette().background.weak.color,
        },
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn luminance(c: Color) -> f32 {
        let f = |v: f32| {
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn every_theme_has_readable_text() {
        for colours in Colours::ALL {
            for mode in [Mode::Light, Mode::Dark] {
                let p = theme(colours, mode).palette();
                let c = contrast(p.text, p.background);
                assert!(c >= 4.5, "{colours:?} {mode:?}: text contrast {c}");
                let c = contrast(p.primary, p.background);
                assert!(c >= 2.0, "{colours:?} {mode:?}: accent contrast {c}");
                let dark = luminance(p.background) < 0.2;
                assert_eq!(dark, mode == Mode::Dark, "{colours:?} {mode:?}");
            }
        }
    }

    #[test]
    fn mode_choice_resolves() {
        assert_eq!(ModeChoice::System.resolve(Mode::Dark), Mode::Dark);
        assert_eq!(ModeChoice::System.resolve(Mode::None), Mode::None);
        assert_eq!(ModeChoice::Light.resolve(Mode::Dark), Mode::Light);
        assert_eq!(ModeChoice::Dark.resolve(Mode::Light), Mode::Dark);
    }
}
