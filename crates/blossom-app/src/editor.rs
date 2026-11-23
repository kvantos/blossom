//! Org highlighting for iced's text editor, on top of
//! [`blossom_core::highlight`], plus the editor's key bindings.

use std::ops::Range;

use blossom_core::Keywords;
use blossom_core::highlight::{self, Kind, LineState};
use iced::advanced::text::highlighter::{self, Format};
use iced::keyboard::{self, Key, key};
use iced::widget::text_editor::{Binding, KeyPress};
use iced::{Color, Font, Theme, font};

use crate::style;

/// The highlighter. It keeps the state at the start of every line it has
/// seen, so a change re-highlights from that line on.
pub struct Org {
    keywords: Keywords,
    /// `states[n]` is the state at the start of line `n`.
    states: Vec<LineState>,
    current: usize,
}

impl highlighter::Highlighter for Org {
    type Settings = Keywords;
    type Highlight = Kind;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Kind)>;

    fn new(settings: &Keywords) -> Org {
        Org {
            keywords: settings.clone(),
            states: vec![LineState::default()],
            current: 0,
        }
    }

    fn update(&mut self, settings: &Keywords) {
        self.keywords = settings.clone();
        self.change_line(0);
    }

    fn change_line(&mut self, line: usize) {
        let line = line.min(self.states.len() - 1);
        self.states.truncate(line + 1);
        self.current = line;
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let mut state = self.states.get(self.current).copied().unwrap_or_default();
        let runs = highlight::highlight_line(line, &mut state, &self.keywords);
        self.current += 1;
        self.states.truncate(self.current);
        self.states.push(state);
        runs.into_iter()
    }

    fn current_line(&self) -> usize {
        self.current
    }
}

/// How a highlight looks in `theme`.
#[allow(clippy::trivially_copy_pass_by_ref)] // iced fixes the signature
pub fn format(kind: &Kind, theme: &Theme) -> Format<Font> {
    let p = theme.palette();
    let dim = style::dim(theme);
    let bold = Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    };
    let (color, font): (Option<Color>, Option<Font>) = match kind {
        Kind::Heading(level) => (Some(heading_color(theme, *level)), Some(bold)),
        Kind::Todo => (Some(p.primary), Some(bold)),
        Kind::Done => (Some(p.success), Some(bold)),
        Kind::Priority => (Some(p.warning), Some(bold)),
        Kind::Tags | Kind::Drawer | Kind::Property | Kind::Meta | Kind::Comment => {
            (Some(dim), None)
        }
        Kind::Planning => (Some(dim), Some(bold)),
        Kind::Timestamp => (Some(p.warning), None),
        Kind::Bullet | Kind::Checkbox => (Some(p.primary), None),
        Kind::Bold => (None, Some(bold)),
        Kind::Italic => (
            None,
            Some(Font {
                style: font::Style::Italic,
                ..Font::MONOSPACE
            }),
        ),
        Kind::Code | Kind::Block => (Some(style::code_ink(theme)), None),
        Kind::Strike | Kind::Underline => (Some(dim), None),
        Kind::Link => (Some(style::link(theme)), None),
    };
    // The editor is monospace; keep weight and style but not the family.
    let font = font.map(|f| Font {
        family: Font::MONOSPACE.family,
        ..f
    });
    Format { color, font }
}

fn heading_color(theme: &Theme, level: usize) -> Color {
    let p = theme.palette();
    match level {
        1 => p.text,
        2 => p.primary,
        3 => style::mix(p.primary, p.warning, 0.5),
        _ => style::mix(p.text, p.primary, 0.5),
    }
}

/// What the editor's own keys do, as messages for the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Undo the last change.
    Undo,
    /// Redo.
    Redo,
    /// Leave the editor.
    Done,
    /// Enter, with list continuation.
    Enter,
    /// Ctrl/Cmd+V: an image on the clipboard is attached, text is pasted.
    Paste,
}

/// The editor's key bindings: iced's defaults plus undo/redo, Escape to
/// leave and Enter for list continuation.
pub fn bindings<M>(on: fn(Command) -> M) -> impl Fn(KeyPress) -> Option<Binding<M>> {
    move |press: KeyPress| {
        if !matches!(
            press.status,
            iced::widget::text_editor::Status::Focused { .. }
        ) {
            return None;
        }
        let m = press.modifiers;
        match press.key.as_ref() {
            Key::Character(c) if m.command() && c.eq_ignore_ascii_case("z") => {
                return Some(Binding::Custom(on(if m.shift() {
                    Command::Redo
                } else {
                    Command::Undo
                })));
            }
            Key::Character(c) if m.command() && c.eq_ignore_ascii_case("y") => {
                return Some(Binding::Custom(on(Command::Redo)));
            }
            Key::Character(c) if m.command() && !m.shift() && c.eq_ignore_ascii_case("v") => {
                return Some(Binding::Custom(on(Command::Paste)));
            }
            Key::Named(key::Named::Escape) => return Some(Binding::Custom(on(Command::Done))),
            Key::Named(key::Named::Enter) if m == keyboard::Modifiers::empty() => {
                return Some(Binding::Custom(on(Command::Enter)));
            }
            _ => {}
        }
        Binding::from_key_press(press)
    }
}

/// The capture editor's bindings: iced's defaults, Ctrl/Cmd+Enter to save
/// and Escape to cancel.
pub fn capture_bindings<M: Clone>(save: M, cancel: M) -> impl Fn(KeyPress) -> Option<Binding<M>> {
    move |press: KeyPress| {
        if !matches!(
            press.status,
            iced::widget::text_editor::Status::Focused { .. }
        ) {
            return None;
        }
        match press.key.as_ref() {
            Key::Named(key::Named::Enter) if press.modifiers.command() => {
                Some(Binding::Custom(save.clone()))
            }
            Key::Named(key::Named::Escape) => Some(Binding::Custom(cancel.clone())),
            _ => Binding::from_key_press(press),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::text::Highlighter;

    #[test]
    fn rehighlights_from_a_changed_line() {
        let mut h = Org::new(&Keywords::default());
        let lines = [
            "#+begin_src",
            "* not a heading in a block",
            "#+end_src",
            "* TODO h",
        ];
        let all: Vec<Vec<Kind>> = lines
            .iter()
            .map(|l| h.highlight_line(l).map(|(_, k)| k).collect())
            .collect();
        assert_eq!(all[1], [Kind::Block]);
        assert_eq!(all[3][1], Kind::Todo);
        // Line 1 changed: highlighting resumes there with the block state.
        h.change_line(1);
        assert_eq!(h.current_line(), 1);
        let again: Vec<Kind> = h.highlight_line(lines[1]).map(|(_, k)| k).collect();
        assert_eq!(again, [Kind::Block]);
    }
}
