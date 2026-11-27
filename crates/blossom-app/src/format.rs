//! The note editor's formatting buttons, as org markup: `*bold*`,
//! `/italic/`, `_underline_`, `+strike+`, `~code~`, `[[url][link]]`, `- `
//! lists and `- [ ] ` checkboxes, UPPERCASE and lowercase.
//!
//! Wrapping toggles: a selection already wrapped (the markers inside it or
//! just around it) loses them. With nothing selected the pair goes in with
//! the cursor between. The text stays selected afterwards (on one line), so
//! pressing again undoes it. Lists work on the lines the selection
//! touches, the cursor's line without one.

use std::sync::Arc;

use iced::widget::text_editor::{Action, Content, Cursor, Edit, Motion, Position};

/// A formatting button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `*bold*`
    Bold,
    /// `/italic/`
    Italic,
    /// `_underline_`
    Underline,
    /// `+strike+`
    Strike,
    /// `~code~`
    Code,
    /// `[[url][text]]`
    Link,
    /// `- ` before each line.
    List,
    /// `- [ ] ` before each line.
    Checkbox,
    /// UPPERCASE.
    Upper,
    /// lowercase.
    Lower,
}

impl Format {
    /// Every button, in toolbar order.
    pub const ALL: [Format; 10] = [
        Format::Bold,
        Format::Italic,
        Format::Underline,
        Format::Strike,
        Format::Code,
        Format::Link,
        Format::List,
        Format::Checkbox,
        Format::Upper,
        Format::Lower,
    ];

    /// What the button's hint says.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Format::Bold => "Bold  *text*",
            Format::Italic => "Italic  /text/",
            Format::Underline => "Underline  _text_",
            Format::Strike => "Strikethrough  +text+",
            Format::Code => "Code  ~text~",
            Format::Link => "Link  [[url][text]]",
            Format::List => "List  - item",
            Format::Checkbox => "Checkboxes  - [ ] item",
            Format::Upper => "UPPERCASE",
            Format::Lower => "lowercase",
        }
    }

    fn marker(self) -> Option<char> {
        match self {
            Format::Bold => Some('*'),
            Format::Italic => Some('/'),
            Format::Underline => Some('_'),
            Format::Strike => Some('+'),
            Format::Code => Some('~'),
            _ => None,
        }
    }
}

/// Applies `f` to `content`; whether the text changed.
pub fn apply(content: &mut Content, f: Format) -> bool {
    if let Some(m) = f.marker() {
        return wrap(content, m);
    }
    match f {
        Format::Link => link(content),
        Format::List => lines(content, false),
        Format::Checkbox => lines(content, true),
        Format::Upper => case(content, true),
        Format::Lower => case(content, false),
        _ => false,
    }
}

/// The selection's start and end, in order.
fn bounds(c: Cursor) -> Option<(Position, Position)> {
    let other = c.selection?;
    let (a, b) = (c.position, other);
    Some(if (a.line, a.column) <= (b.line, b.column) {
        (a, b)
    } else {
        (b, a)
    })
}

fn select(content: &mut Content, line: usize, from: usize, to: usize) {
    content.move_to(Cursor {
        position: Position { line, column: to },
        selection: Some(Position { line, column: from }),
    });
}

fn paste(content: &mut Content, text: &str) {
    content.perform(Action::Edit(Edit::Paste(Arc::new(text.to_string()))));
}

fn wrap(content: &mut Content, m: char) -> bool {
    let cursor = content.cursor();
    let Some((a, b)) = bounds(cursor) else {
        let mut pair = String::new();
        pair.push(m);
        pair.push(m);
        paste(content, &pair);
        content.perform(Action::Move(Motion::Left));
        return true;
    };
    let sel = content.selection().unwrap_or_default();
    if sel.is_empty() {
        return false;
    }
    let one_line = a.line == b.line;
    // Markers just inside the selection.
    if sel.len() >= 2 && sel.starts_with(m) && sel.ends_with(m) {
        let inner = &sel[m.len_utf8()..sel.len() - m.len_utf8()];
        paste(content, inner);
        if one_line {
            select(content, a.line, a.column, a.column + inner.len());
        }
        return true;
    }
    // Markers just around it.
    if one_line {
        let line = content
            .line(a.line)
            .map_or_else(String::new, |l| l.text.into_owned());
        let bytes = line.as_bytes();
        let around = a.column >= 1
            && b.column < bytes.len()
            && char::from(bytes[a.column - 1]) == m
            && char::from(bytes[b.column]) == m;
        if around {
            select(content, a.line, a.column - 1, b.column + 1);
            paste(content, &sel);
            select(content, a.line, a.column - 1, a.column - 1 + sel.len());
            return true;
        }
    }
    paste(content, &format!("{m}{sel}{m}"));
    if one_line {
        select(content, a.line, a.column + 1, a.column + 1 + sel.len());
    }
    true
}

/// Whether `s` reads as a link target rather than words.
fn looks_like_url(s: &str) -> bool {
    !s.contains(char::is_whitespace)
        && (s.contains("://") || s.starts_with("file:") || s.starts_with("mailto:"))
}

fn link(content: &mut Content) -> bool {
    let cursor = content.cursor();
    let sel = content.selection().filter(|s| !s.is_empty());
    let start = bounds(cursor).map_or(cursor.position, |(a, _)| a);
    match sel {
        // Unwrap a link: keep its text, else its target.
        Some(s) if s.starts_with("[[") && s.ends_with("]]") => {
            let inner = &s[2..s.len() - 2];
            let keep = inner.split_once("][").map_or(inner, |(_, text)| text);
            paste(content, keep);
            true
        }
        Some(s) if looks_like_url(&s) => {
            paste(content, &format!("[[{s}]]"));
            true
        }
        sel => {
            const URL: &str = "https://";
            let text = sel.unwrap_or_else(|| "link".to_string());
            let one_line = !text.contains('\n');
            paste(content, &format!("[[{URL}][{text}]]"));
            if one_line {
                // The address is selected, to type over.
                select(
                    content,
                    start.line,
                    start.column + 2,
                    start.column + 2 + URL.len(),
                );
            }
            true
        }
    }
}

/// A line split into indent, list kind and the rest.
#[derive(PartialEq, Eq)]
enum Kind {
    Plain,
    Bullet,
    Check,
}

fn split(line: &str) -> (&str, Kind, &str) {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    for b in [
        "- [ ] ", "- [X] ", "- [x] ", "- [-] ", "+ [ ] ", "+ [X] ", "+ [x] ",
    ] {
        if let Some(rest) = body.strip_prefix(b) {
            return (indent, Kind::Check, rest);
        }
    }
    for b in ["- ", "+ "] {
        if let Some(rest) = body.strip_prefix(b) {
            return (indent, Kind::Bullet, rest);
        }
    }
    (indent, Kind::Plain, body)
}

/// `lines` with list (or checkbox) markers toggled.
fn toggle_lines(lines: &[String], checkbox: bool) -> Vec<String> {
    let want = if checkbox { Kind::Check } else { Kind::Bullet };
    let filled: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let all = !filled.is_empty() && filled.iter().all(|l| split(l).1 == want);
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                return l.clone();
            }
            let (indent, kind, rest) = split(l);
            if all {
                format!("{indent}{rest}")
            } else if kind == want {
                l.clone()
            } else if checkbox {
                format!("{indent}- [ ] {rest}")
            } else {
                format!("{indent}- {rest}")
            }
        })
        .collect()
}

fn lines(content: &mut Content, checkbox: bool) -> bool {
    let cursor = content.cursor();
    let (a, b) = bounds(cursor).unwrap_or((cursor.position, cursor.position));
    // A selection ending at the start of a line leaves that line alone.
    let last = if b.line > a.line && b.column == 0 {
        b.line - 1
    } else {
        b.line
    };
    let old: Vec<String> = (a.line..=last)
        .map(|n| {
            content
                .line(n)
                .map_or_else(String::new, |l| l.text.into_owned())
        })
        .collect();
    let new = toggle_lines(&old, checkbox);
    if new == old {
        return false;
    }
    let end = old.last().map_or(0, String::len);
    content.move_to(Cursor {
        position: Position {
            line: last,
            column: end,
        },
        selection: Some(Position {
            line: a.line,
            column: 0,
        }),
    });
    paste(content, &new.join("\n"));
    true
}

fn case(content: &mut Content, upper: bool) -> bool {
    let cursor = content.cursor();
    let Some((a, b)) = bounds(cursor) else {
        return false;
    };
    let Some(sel) = content.selection().filter(|s| !s.is_empty()) else {
        return false;
    };
    let new = if upper {
        sel.to_uppercase()
    } else {
        sel.to_lowercase()
    };
    if new == sel {
        return false;
    }
    paste(content, &new);
    if a.line == b.line {
        select(content, a.line, a.column, a.column + new.len());
    }
    true
}

/// The toolbar above the editor; each button sends `Message::Format`.
pub fn toolbar<'a>() -> iced::Element<'a, crate::app::Message> {
    use iced::widget::{button, container, row, text, tooltip};
    use iced::{Font, font};

    use crate::app::Message;
    use crate::icons;

    let letter = |s: &'static str, font: Font| -> iced::Element<'a, Message> {
        text(s).size(14).font(font).width(18).center().into()
    };
    let bold = Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    };
    let italic = Font {
        style: font::Style::Italic,
        ..Font::DEFAULT
    };
    let buttons = Format::ALL.map(|f| {
        let face: iced::Element<'a, Message> = match f {
            Format::Bold => letter("B", bold),
            Format::Italic => letter("I", italic),
            Format::Underline => letter("U", Font::DEFAULT),
            Format::Strike => letter("S", Font::DEFAULT),
            Format::Code => icons::view(&icons::CODE, 16.0, false),
            Format::Link => icons::view(&icons::LINK, 16.0, false),
            Format::List => icons::view(&icons::LIST, 16.0, false),
            Format::Checkbox => icons::view(&icons::CHECKBOX, 16.0, false),
            Format::Upper => letter("AB", Font::DEFAULT),
            Format::Lower => letter("ab", Font::DEFAULT),
        };
        tooltip(
            button(container(face).center_y(18))
                .on_press(Message::Format(f))
                .padding([2, 6])
                .style(button::text),
            container(text(f.name()).size(12))
                .padding([3, 8])
                .style(crate::style::modal),
            tooltip::Position::Bottom,
        )
        .delay(std::time::Duration::from_millis(500))
        .into()
    });
    row(buttons).spacing(2).align_y(iced::Center).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with `from..to` (byte columns on line 0) selected.
    fn with(text: &str, from: usize, to: usize) -> Content {
        let mut c = Content::with_text(text);
        select(&mut c, 0, from, to);
        c
    }

    fn run(text: &str, from: usize, to: usize, f: Format) -> (String, Option<String>) {
        let mut c = with(text, from, to);
        apply(&mut c, f);
        (c.text(), c.selection())
    }

    #[test]
    fn wrapping_toggles() {
        // "make it bold": select "it".
        let (t, sel) = run("make it bold", 5, 7, Format::Bold);
        assert_eq!(t, "make *it* bold");
        assert_eq!(sel.as_deref(), Some("it"));
        // Again: the markers around it go.
        let mut c = with("make *it* bold", 6, 8);
        apply(&mut c, Format::Bold);
        assert_eq!(c.text(), "make it bold");
        assert_eq!(c.selection().as_deref(), Some("it"));
        // Markers inside the selection go too.
        assert_eq!(run("a /b/ c", 2, 5, Format::Italic).0, "a b c");
        assert_eq!(run("a b c", 2, 3, Format::Underline).0, "a _b_ c");
        assert_eq!(run("a b c", 2, 3, Format::Strike).0, "a +b+ c");
        assert_eq!(run("a b c", 2, 3, Format::Code).0, "a ~b~ c");
        // Nothing selected: the pair, the cursor between.
        let mut c = Content::with_text("ab");
        c.move_to(Cursor {
            position: Position { line: 0, column: 1 },
            selection: None,
        });
        apply(&mut c, Format::Bold);
        c.perform(Action::Edit(Edit::Insert('x')));
        assert_eq!(c.text(), "a*x*b");
    }

    #[test]
    fn links() {
        let (t, sel) = run("see docs here", 4, 8, Format::Link);
        assert_eq!(t, "see [[https://][docs]] here");
        assert_eq!(sel.as_deref(), Some("https://"));
        assert_eq!(
            run("go https://x.org now", 3, 16, Format::Link).0,
            "go [[https://x.org]] now"
        );
        assert_eq!(run("[[u][docs]]", 0, 11, Format::Link).0, "docs");
        assert_eq!(run("[[u]]", 0, 5, Format::Link).0, "u");
    }

    #[test]
    fn lists_and_checkboxes_toggle() {
        let s = |v: &[&str]| v.iter().map(|l| (*l).to_string()).collect::<Vec<_>>();
        assert_eq!(
            toggle_lines(&s(&["milk", "", "  eggs"]), false),
            s(&["- milk", "", "  - eggs"])
        );
        assert_eq!(
            toggle_lines(&s(&["- milk", "- eggs"]), false),
            s(&["milk", "eggs"])
        );
        assert_eq!(
            toggle_lines(&s(&["- milk", "eggs"]), true),
            s(&["- [ ] milk", "- [ ] eggs"])
        );
        assert_eq!(
            toggle_lines(&s(&["- [X] milk", "- [ ] eggs"]), true),
            s(&["milk", "eggs"])
        );
        // In the editor: two lines selected.
        let mut c = Content::with_text("milk\neggs\nbread");
        c.move_to(Cursor {
            position: Position { line: 1, column: 2 },
            selection: Some(Position { line: 0, column: 1 }),
        });
        apply(&mut c, Format::Checkbox);
        assert_eq!(c.text(), "- [ ] milk\n- [ ] eggs\nbread");
        // No selection: the cursor's line.
        let mut c = Content::with_text("a\nb");
        c.move_to(Cursor {
            position: Position { line: 1, column: 0 },
            selection: None,
        });
        apply(&mut c, Format::List);
        assert_eq!(c.text(), "a\n- b");
    }

    #[test]
    fn case() {
        let (t, sel) = run("say hello", 4, 9, Format::Upper);
        assert_eq!((t.as_str(), sel.as_deref()), ("say HELLO", Some("HELLO")));
        assert_eq!(run("SAY Привет", 0, 16, Format::Lower).0, "say привет");
        // Nothing selected: nothing happens.
        let mut c = Content::with_text("x");
        assert!(!apply(&mut c, Format::Upper));
    }
}
