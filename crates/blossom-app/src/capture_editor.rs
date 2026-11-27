//! Capture in the normal editor: pick a template, answer its prompts, then
//! write the entry in the same editor as a section (the same keys, Emacs
//! ones included, and list continuation; the cursor starts where `%?` was).
//! Saving (Ctrl/Cmd+Enter or Save) files it at the template's target;
//! Escape discards it, asking once more when something was typed. Changing
//! the template or an answer re-expands the entry until you type in it.
//! The entry isn't in a file yet, so it keeps its own undo history.

use std::time::Instant;

use blossom_core::Keywords;
use blossom_core::capture::{self, Context};
use blossom_vault::config::Template;
use chrono::NaiveDateTime;
use iced::widget::text_editor::{Content, Cursor, Position};
use iced::widget::{Column, button, pick_list, row, text, text_editor, text_input};
use iced::{Element, Fill, Font, Theme};

use crate::app::{Message, dim};
use crate::editor;
use crate::file::{Group, TYPING_GROUP, UNDO_LIMIT};
use crate::fonts;
use crate::settings::Settings;
use crate::style;

/// The editor's widget id, for focusing.
pub const EDITOR: &str = "capture-editor";

/// One undo step: the text and cursor before a change.
type Step = (String, Cursor);

/// The entry being captured.
pub struct CaptureEditor {
    /// Index into the templates.
    pub template: usize,
    /// The template's prompts.
    pub prompts: Vec<String>,
    /// One answer per prompt.
    pub answers: Vec<String>,
    /// The entry being written.
    pub content: Content,
    /// The entry was typed in, so it is no longer re-expanded.
    pub edited: bool,
    /// The capture time, fixed when capture opened.
    pub now: NaiveDateTime,
    /// Escape was pressed once on a typed-in entry; again discards it.
    pub discard_warned: bool,
    undo: Vec<Step>,
    redo: Vec<Step>,
    last_typing: Option<Instant>,
}

impl CaptureEditor {
    /// Capture with template `template`.
    #[must_use]
    pub fn new(templates: &[Template], template: usize, now: NaiveDateTime) -> CaptureEditor {
        let mut c = CaptureEditor {
            template,
            prompts: Vec::new(),
            answers: Vec::new(),
            content: Content::new(),
            edited: false,
            now,
            discard_warned: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_typing: None,
        };
        c.choose(templates, template);
        c
    }

    /// Switches to template `index`, keeping answers to prompts it shares.
    pub fn choose(&mut self, templates: &[Template], index: usize) {
        let Some(t) = templates.get(index) else {
            return;
        };
        self.template = index;
        let old: Vec<(String, String)> = self
            .prompts
            .iter()
            .cloned()
            .zip(self.answers.iter().cloned())
            .collect();
        self.prompts = capture::prompts(&t.template);
        self.answers = self
            .prompts
            .iter()
            .map(|p| {
                old.iter()
                    .find(|(q, _)| q == p)
                    .map_or_else(String::new, |(_, a)| a.clone())
            })
            .collect();
        self.edited = false;
        self.expand(templates);
    }

    /// Re-expands the template into the editor, unless it was typed in.
    pub fn expand(&mut self, templates: &[Template]) {
        if self.edited {
            return;
        }
        let Some(t) = templates.get(self.template) else {
            return;
        };
        let ctx = Context {
            now: self.now,
            answers: self
                .prompts
                .iter()
                .cloned()
                .zip(self.answers.iter().cloned())
                .collect(),
            ..Context::default()
        };
        let e = capture::expand(&t.template, &ctx);
        self.content = Content::with_text(&e.text);
        let before = &e.text[..e.cursor];
        let line = before.matches('\n').count();
        let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1);
        self.content.move_to(Cursor {
            position: Position { line, column },
            selection: None,
        });
        // A fresh entry: nothing to undo into.
        self.undo.clear();
        self.redo.clear();
        self.last_typing = None;
    }

    /// Runs `change` on the entry; when it reports a change, that becomes
    /// an undo step (typing joins the step before while keys keep coming).
    pub fn edit(&mut self, group: Group, change: impl FnOnce(&mut Content) -> bool) {
        let before = (self.content.text(), self.content.cursor());
        if !change(&mut self.content) {
            return;
        }
        let now = Instant::now();
        let joins = group == Group::Typing
            && self
                .last_typing
                .is_some_and(|t| now.duration_since(t) < TYPING_GROUP);
        if !joins {
            self.undo.push(before);
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.last_typing = (group == Group::Typing).then_some(now);
        self.redo.clear();
        self.edited = true;
        self.discard_warned = false;
    }

    /// Undoes (or with `redo`, redoes) a step; whether there was one.
    pub fn undo(&mut self, redo: bool) -> bool {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let Some((text, cursor)) = from.pop() else {
            return false;
        };
        to.push((self.content.text(), self.content.cursor()));
        self.content = Content::with_text(&text);
        self.content.move_to(cursor);
        self.last_typing = None;
        true
    }

    /// The entry text.
    #[must_use]
    pub fn entry(&self) -> String {
        self.content.text()
    }
}

/// `t — TODO` in the template picker.
pub fn template_label(t: &Template) -> String {
    format!("{} — {}", t.key, t.name)
}

/// Capture, in the main area.
pub fn view<'a>(
    c: &'a CaptureEditor,
    templates: &'a [Template],
    keywords: &Keywords,
    settings: &Settings,
) -> Element<'a, Message> {
    let font = fonts::named(settings.editor_font.as_deref(), Font::MONOSPACE);
    let labels: Vec<String> = templates.iter().map(template_label).collect();
    let current = templates.get(c.template).map(template_label);
    let target = templates.get(c.template).map_or_else(String::new, |t| {
        format!("New entry → {}, {}", t.file, t.target.describe())
    });
    let save = if cfg!(target_os = "macos") {
        "⌘Enter saves"
    } else {
        "Ctrl+Enter saves"
    };
    let mut parts: Vec<Element<'a, Message>> = vec![
        row![
            text(target).size(13).style(dim).width(Fill),
            pick_list(labels, current, Message::CaptureTemplate)
                .text_size(13)
                .padding([3, 8]),
            text(format!("{save}, Esc discards")).size(12).style(dim),
            button(text("Discard").size(13))
                .on_press(Message::CaptureCancel)
                .padding([3, 10])
                .style(button::secondary),
            button(text("Save").size(13))
                .on_press(Message::CaptureSave)
                .padding([3, 10]),
        ]
        .spacing(10)
        .align_y(iced::Center)
        .into(),
    ];
    if c.discard_warned {
        parts.push(
            text("Press Esc again to discard this entry, or keep typing.")
                .size(13)
                .style(|theme: &Theme| text::Style {
                    color: Some(theme.extended_palette().danger.base.color),
                })
                .into(),
        );
    }
    for (i, prompt) in c.prompts.iter().enumerate() {
        let value = c.answers.get(i).map_or("", String::as_str);
        parts.push(
            row![
                text(prompt.as_str()).size(13).width(110),
                text_input(prompt, value)
                    .on_input(move |v| Message::CaptureAnswer(i, v))
                    .size(13)
                    .padding([3, 6]),
            ]
            .spacing(8)
            .align_y(iced::Center)
            .into(),
        );
    }
    parts.push(
        text_editor(&c.content)
            .id(EDITOR)
            .on_action(Message::EditorAction)
            .key_binding(editor::bindings(
                Message::EditorCommand,
                settings.emacs_keys,
            ))
            .highlight_with::<editor::Org>(
                editor::Settings {
                    keywords: keywords.clone(),
                    family: font.family,
                },
                editor::format,
            )
            .font(font)
            .size(settings.editor_size)
            .padding(12)
            .height(Fill)
            .style(style::editor)
            .into(),
    );
    Column::with_children(parts)
        .spacing(10)
        .padding([16, 20])
        .height(Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text_editor::{Action, Edit};

    fn templates() -> Vec<Template> {
        blossom_vault::config::Config::default()
            .templates()
            .unwrap()
    }

    #[test]
    fn starts_at_the_cursor_mark_and_undoes_its_own_edits() {
        let t = templates();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 27)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap();
        let mut c = CaptureEditor::new(&t, 0, now);
        let start = c.entry();
        c.edit(Group::Single, |ct| {
            ct.perform(Action::Edit(Edit::Insert('x')));
            true
        });
        assert!(c.edited);
        assert_ne!(c.entry(), start);
        assert!(c.undo(false));
        assert_eq!(c.entry(), start);
        assert!(!c.undo(false));
        assert!(c.undo(true));
        assert_ne!(c.entry(), start);
        // No change, no step.
        c.edit(Group::Single, |_| false);
        assert!(c.undo(false));
        assert!(!c.undo(false));
    }
}
