//! Org highlighting for iced's text editor, on top of
//! [`blossom_core::highlight`], plus the editor's key bindings.

use std::ops::Range;
use std::sync::Arc;

use blossom_core::Keywords;
use blossom_core::editing::{self, ListContinue};
use blossom_core::highlight::{self, Kind, LineState};
use iced::advanced::text::highlighter::{self, Format};
use iced::keyboard::{self, Key, key};
use iced::widget::text_editor::{Action, Binding, Content, Cursor, Edit, KeyPress, Motion};
use iced::{Color, Font, Theme, font};

use crate::style;

/// What the highlighter needs: the TODO keywords and the editor's font
/// family (bold and italic runs keep it).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The file's TODO keywords.
    pub keywords: Keywords,
    /// The editor's font family.
    pub family: font::Family,
}

/// A highlighted run: what it is, and the family to draw it in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    /// What the run is.
    pub kind: Kind,
    /// The editor's font family.
    pub family: font::Family,
}

/// The highlighter. It keeps the state at the start of every line it has
/// seen, so a change re-highlights from that line on.
pub struct Org {
    settings: Settings,
    /// `states[n]` is the state at the start of line `n`.
    states: Vec<LineState>,
    current: usize,
}

impl highlighter::Highlighter for Org {
    type Settings = Settings;
    type Highlight = Mark;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Mark)>;

    fn new(settings: &Settings) -> Org {
        Org {
            settings: settings.clone(),
            states: vec![LineState::default()],
            current: 0,
        }
    }

    fn update(&mut self, settings: &Settings) {
        self.settings = settings.clone();
        self.change_line(0);
    }

    fn change_line(&mut self, line: usize) {
        let line = line.min(self.states.len() - 1);
        self.states.truncate(line + 1);
        self.current = line;
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        let mut state = self.states.get(self.current).copied().unwrap_or_default();
        let runs = highlight::highlight_line(line, &mut state, &self.settings.keywords);
        self.current += 1;
        self.states.truncate(self.current);
        self.states.push(state);
        let family = self.settings.family;
        runs.into_iter()
            .map(|(range, kind)| (range, Mark { kind, family }))
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn current_line(&self) -> usize {
        self.current
    }
}

/// How a highlight looks in `theme`.
#[allow(clippy::trivially_copy_pass_by_ref)] // iced fixes the signature
pub fn format(mark: &Mark, theme: &Theme) -> Format<Font> {
    let kind = &mark.kind;
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
    // Keep weight and style but the editor's family.
    let font = font.map(|f| Font {
        family: mark.family,
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
    /// Esc: leave the editor (capture: discard the entry).
    Done,
    /// Ctrl/Cmd+Enter: leave the editor (capture: file the entry).
    Save,
    /// Enter, with list continuation.
    Enter,
    /// Ctrl/Cmd+V: an image on the clipboard is attached, text is pasted.
    Paste,
    /// An Emacs kill: the text goes to the kill ring.
    Kill(Kill),
    /// C-y: insert the last kill.
    Yank,
}

/// What an Emacs kill removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kill {
    /// C-k: to the end of the line, or the line break when already there.
    Line,
    /// M-d: to the end of the word.
    WordForward,
    /// M-DEL: to the start of the word.
    WordBackward,
}

/// The letter or sign a key press names for a binding: the key as typed
/// when that is plain ASCII (so Dvorak's C-f is where Dvorak has f), else
/// the key at that place on a US keyboard (so C-f works with a Russian
/// layout, and M-f with macOS's Option, which types ƒ).
fn binding_char(press: &KeyPress) -> Option<char> {
    use keyboard::key::Code;
    if let Key::Character(c) = &press.key {
        let mut chars = c.chars();
        if let (Some(ch), None) = (chars.next(), chars.next())
            && ch.is_ascii_graphic()
        {
            return Some(ch.to_ascii_lowercase());
        }
    }
    let keyboard::key::Physical::Code(code) = press.physical_key else {
        return None;
    };
    Some(match code {
        Code::KeyA => 'a',
        Code::KeyB => 'b',
        Code::KeyD => 'd',
        Code::KeyE => 'e',
        Code::KeyF => 'f',
        Code::KeyK => 'k',
        Code::KeyN => 'n',
        Code::KeyP => 'p',
        Code::KeyY => 'y',
        Code::Comma => ',',
        Code::Period => '.',
        _ => return None,
    })
}

/// The Emacs binding for `press`, if it is one: C-f/b/n/p, C-a/e, C-d,
/// C-k, C-y, M-f/b, M-d, M-DEL and M-</M->. Ctrl is Control on macOS too
/// (Cmd keeps its own keys); Meta is Alt or Option.
fn emacs<M>(press: &KeyPress, on: fn(Command) -> M) -> Option<Binding<M>> {
    let m = press.modifiers;
    let ctrl = m.control() && !m.alt() && !m.logo() && !m.shift();
    let meta = m.alt() && !m.control() && !m.logo();
    if meta && !m.shift() && matches!(press.key, Key::Named(key::Named::Backspace)) {
        return Some(Binding::Custom(on(Command::Kill(Kill::WordBackward))));
    }
    if !ctrl && !meta {
        return None;
    }
    let ch = binding_char(press)?;
    Some(match (ctrl, m.shift(), ch) {
        (true, _, 'f') => Binding::Move(Motion::Right),
        (true, _, 'b') => Binding::Move(Motion::Left),
        (true, _, 'n') => Binding::Move(Motion::Down),
        (true, _, 'p') => Binding::Move(Motion::Up),
        (true, _, 'a') => Binding::Move(Motion::Home),
        (true, _, 'e') => Binding::Move(Motion::End),
        (true, _, 'd') => Binding::Delete,
        (true, _, 'k') => Binding::Custom(on(Command::Kill(Kill::Line))),
        (true, _, 'y') => Binding::Custom(on(Command::Yank)),
        (false, false, 'f') => Binding::Move(Motion::WordRight),
        (false, false, 'b') => Binding::Move(Motion::WordLeft),
        (false, false, 'd') => Binding::Custom(on(Command::Kill(Kill::WordForward))),
        (false, true, ',' | '<') => Binding::Move(Motion::DocumentStart),
        (false, true, '.' | '>') => Binding::Move(Motion::DocumentEnd),
        _ => return None,
    })
}

/// The editor's key bindings: the app's shortcuts pass through, then
/// iced's defaults plus undo/redo, paste (an image too), Escape to leave,
/// Ctrl/Cmd+Enter to save and Enter for list continuation; with `emacs`, the Emacs keys first
/// (then C-y yanks, and redo is Ctrl+Shift+Z).
pub fn bindings<M>(
    on: fn(Command) -> M,
    emacs_keys: bool,
) -> impl Fn(KeyPress) -> Option<Binding<M>> {
    move |press: KeyPress| {
        if !matches!(
            press.status,
            iced::widget::text_editor::Status::Focused { .. }
        ) {
            return None;
        }
        // The app's own shortcuts go past the editor to the app.
        if crate::keys::app_key(&press.key, press.physical_key, press.modifiers)
            .is_some_and(crate::keys::AppKey::in_editor)
        {
            return None;
        }
        if emacs_keys && let Some(b) = emacs(&press, on) {
            return Some(b);
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
            // With Shift too: Ctrl+Shift+V is "paste image" outside the editor.
            Key::Character(c) if m.command() && !m.alt() && c.eq_ignore_ascii_case("v") => {
                return Some(Binding::Custom(on(Command::Paste)));
            }
            Key::Named(key::Named::Escape) => return Some(Binding::Custom(on(Command::Done))),
            Key::Named(key::Named::Enter) if m.command() && !m.shift() && !m.alt() => {
                return Some(Binding::Custom(on(Command::Save)));
            }
            Key::Named(key::Named::Enter) if m == keyboard::Modifiers::empty() => {
                return Some(Binding::Custom(on(Command::Enter)));
            }
            _ => {}
        }
        Binding::from_key_press(press)
    }
}

/// The kill ring: the last kill, and whether the next kill adds to it
/// (kills in a row do; anything else in the editor ends the run).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KillRing {
    /// The killed text.
    pub text: String,
    /// The next kill adds to `text`.
    pub append: bool,
}

/// An Emacs kill in `content`: the text goes to `ring` (kills in a row add
/// up, M-DEL's in front). Whether anything was killed.
pub fn kill(content: &mut Content, kind: Kill, ring: &mut KillRing) -> bool {
    // A kill ignores the selection, as in Emacs.
    let cursor = content.cursor();
    content.move_to(Cursor {
        position: cursor.position,
        selection: None,
    });
    let motion = match kind {
        Kill::Line => {
            let line_len = content
                .line(cursor.position.line)
                .map_or(0, |l| l.text.len());
            // At the end of the line, the line break goes.
            if cursor.position.column >= line_len {
                Motion::Right
            } else {
                Motion::End
            }
        }
        Kill::WordForward => Motion::WordRight,
        Kill::WordBackward => Motion::WordLeft,
    };
    content.perform(Action::Select(motion));
    // iced counts a run of spaces as a word; Emacs kills through it to the
    // word beyond.
    if motion != Motion::Right
        && content
            .selection()
            .is_some_and(|s| !s.is_empty() && s.trim().is_empty())
    {
        content.perform(Action::Select(motion));
    }
    let Some(killed) = content.selection().filter(|s| !s.is_empty()) else {
        return false;
    };
    content.perform(Action::Edit(Edit::Delete));
    if !ring.append {
        ring.text.clear();
    }
    if kind == Kill::WordBackward {
        ring.text.insert_str(0, &killed);
    } else {
        ring.text.push_str(&killed);
    }
    ring.append = true;
    true
}

/// C-y: inserts the last kill; whether there was one.
pub fn yank(content: &mut Content, ring: &KillRing) -> bool {
    if ring.text.is_empty() {
        return false;
    }
    content.perform(Action::Edit(Edit::Paste(Arc::new(ring.text.clone()))));
    true
}

/// Enter, continuing a list (`- `, `1. `, `- [ ] `) at the end of an
/// item's line, or ending it on an empty item.
pub fn enter(content: &mut Content) {
    let cursor = content.cursor();
    let line = content
        .line(cursor.position.line)
        .map_or_else(String::new, |l| l.text.into_owned());
    let at_end = cursor.selection.is_none() && cursor.position.column == line.len();
    match editing::continue_list(&line).filter(|_| at_end) {
        Some(ListContinue::Item(prefix)) => {
            content.perform(Action::Edit(Edit::Enter));
            content.perform(Action::Edit(Edit::Paste(Arc::new(prefix))));
        }
        Some(ListContinue::End) => {
            content.perform(Action::Move(Motion::Home));
            content.perform(Action::Select(Motion::End));
            content.perform(Action::Edit(Edit::Backspace));
        }
        None => content.perform(Action::Edit(Edit::Enter)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::text::Highlighter;
    use iced::widget::text_editor::{Motion, Status};
    use keyboard::key::{Code, NativeCode, Physical};

    fn press(key: Key, physical: Physical, modifiers: keyboard::Modifiers) -> KeyPress {
        KeyPress {
            key: key.clone(),
            modified_key: key,
            physical_key: physical,
            modifiers,
            text: None,
            status: Status::Focused { is_hovered: false },
        }
    }

    fn bind(p: KeyPress, emacs: bool) -> Option<Binding<Command>> {
        bindings(|c| c, emacs)(p)
    }

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    const CTRL: keyboard::Modifiers = keyboard::Modifiers::CTRL;
    const ALT: keyboard::Modifiers = keyboard::Modifiers::ALT;
    const NONE: Physical = Physical::Unidentified(NativeCode::Unidentified);

    #[test]
    fn emacs_keys() {
        let b = |k: &str, m| bind(press(ch(k), NONE, m), true);
        assert_eq!(b("f", CTRL), Some(Binding::Move(Motion::Right)));
        assert_eq!(b("a", CTRL), Some(Binding::Move(Motion::Home)));
        assert_eq!(b("e", CTRL), Some(Binding::Move(Motion::End)));
        assert_eq!(b("d", CTRL), Some(Binding::Delete));
        assert_eq!(
            b("k", CTRL),
            Some(Binding::Custom(Command::Kill(Kill::Line)))
        );
        assert_eq!(b("y", CTRL), Some(Binding::Custom(Command::Yank)));
        assert_eq!(b("f", ALT), Some(Binding::Move(Motion::WordRight)));
        assert_eq!(
            b("d", ALT),
            Some(Binding::Custom(Command::Kill(Kill::WordForward)))
        );
        assert_eq!(
            b(",", ALT | keyboard::Modifiers::SHIFT),
            Some(Binding::Move(Motion::DocumentStart))
        );
        assert_eq!(
            bind(press(Key::Named(key::Named::Backspace), NONE, ALT), true),
            Some(Binding::Custom(Command::Kill(Kill::WordBackward)))
        );
        // A Russian layout, or Option+f on macOS: the key's place counts.
        assert_eq!(
            bind(press(ch("а"), Physical::Code(Code::KeyF), CTRL), true),
            Some(Binding::Move(Motion::Right))
        );
        assert_eq!(
            bind(press(ch("ƒ"), Physical::Code(Code::KeyF), ALT), true),
            Some(Binding::Move(Motion::WordRight))
        );
        // Undo and the plain keys still work.
        // The app's shortcuts pass by; Ctrl+Shift+V pastes (an image too).
        let both = CTRL | keyboard::Modifiers::SHIFT;
        if cfg!(not(target_os = "macos")) {
            assert_eq!(bind(press(ch("A"), NONE, both), true), None);
            assert_eq!(bind(press(ch("N"), NONE, both), false), None);
            assert_eq!(
                bind(press(ch("V"), NONE, both), true),
                Some(Binding::Custom(Command::Paste))
            );
        }
        assert_eq!(b("z", CTRL), Some(Binding::Custom(Command::Undo)));
        // Off: C-y redoes and C-a selects all, as before.
        assert_eq!(
            bind(press(ch("y"), NONE, CTRL), false),
            Some(Binding::Custom(Command::Redo))
        );
        assert_eq!(
            bind(press(ch("a"), NONE, CTRL), false),
            Some(Binding::SelectAll)
        );
    }

    #[test]
    fn rehighlights_from_a_changed_line() {
        let mut h = Org::new(&Settings {
            keywords: Keywords::default(),
            family: Font::MONOSPACE.family,
        });
        let lines = [
            "#+begin_src",
            "* not a heading in a block",
            "#+end_src",
            "* TODO h",
        ];
        let all: Vec<Vec<Kind>> = lines
            .iter()
            .map(|l| h.highlight_line(l).map(|(_, m)| m.kind).collect())
            .collect();
        assert_eq!(all[1], [Kind::Block]);
        assert_eq!(all[3][1], Kind::Todo);
        // Line 1 changed: highlighting resumes there with the block state.
        h.change_line(1);
        assert_eq!(h.current_line(), 1);
        let again: Vec<Kind> = h.highlight_line(lines[1]).map(|(_, m)| m.kind).collect();
        assert_eq!(again, [Kind::Block]);
    }
}
