//! The capture panel and the refile picker, shown in the main area.
//!
//! Capture: pick a template, answer its prompts, then edit the expanded
//! entry (the cursor starts where `%?` was) and save it to the template's
//! target. Changing the template or an answer re-expands the entry until
//! you type in it.
//!
//! Refile: type to filter every file and heading of the vault, then pick
//! where the heading goes.

use std::sync::Arc;

use blossom_core::capture::{self, Context};
use blossom_vault::config::Template;
use blossom_vault::ops::HeadingRef;
use chrono::NaiveDateTime;
use iced::widget::text_editor::{Content, Cursor, Position};
use iced::widget::{
    Column, button, column, container, pick_list, radio, row, scrollable, text, text_editor,
    text_input,
};
use iced::{Element, Fill, Font};

use crate::app::{Message, bold, dim};
use crate::fonts::{self, Choice, Installed};
use crate::settings::{self, Settings};
use crate::style::{self, Colours, ModeChoice};
use crate::worker::RefileTarget;

/// The capture panel's widget id, for focusing.
pub const CAPTURE_EDITOR: &str = "capture-editor";

/// The capture panel's id for the refile filter field.
pub const REFILE_INPUT: &str = "refile-filter";

/// The open capture panel.
pub struct CapturePanel {
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
    /// The capture time, fixed when the panel opened.
    pub now: NaiveDateTime,
}

impl CapturePanel {
    /// A panel for template `template`.
    #[must_use]
    pub fn new(templates: &[Template], template: usize, now: NaiveDateTime) -> CapturePanel {
        let mut p = CapturePanel {
            template,
            prompts: Vec::new(),
            answers: Vec::new(),
            content: Content::new(),
            edited: false,
            now,
        };
        p.choose(templates, template);
        p
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
    }

    /// The entry text.
    #[must_use]
    pub fn entry(&self) -> String {
        self.content.text()
    }
}

/// `t — TODO` in the template picker.
fn template_label(t: &Template) -> String {
    format!("{} — {}", t.key, t.name)
}

/// The capture panel.
pub fn capture_view<'a>(
    p: &'a CapturePanel,
    templates: &'a [Template],
    keywords: &blossom_core::Keywords,
    settings: &Settings,
) -> Element<'a, Message> {
    let font = fonts::named(settings.editor_font.as_deref(), Font::MONOSPACE);
    let labels: Vec<String> = templates.iter().map(template_label).collect();
    let current = templates.get(p.template).map(template_label);
    let target = templates.get(p.template).map_or_else(String::new, |t| {
        format!("→ {}, {}", t.file, t.target.describe())
    });
    let mut parts: Vec<Element<'a, Message>> = vec![
        row![
            text("Capture").size(18).font(bold()).width(Fill),
            pick_list(labels, current, Message::CaptureTemplate)
                .text_size(13)
                .padding([3, 8]),
        ]
        .spacing(8)
        .align_y(iced::Center)
        .into(),
        text(target).size(12).style(dim).into(),
    ];
    for (i, prompt) in p.prompts.iter().enumerate() {
        let value = p.answers.get(i).map_or("", String::as_str);
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
        text_editor(&p.content)
            .id(CAPTURE_EDITOR)
            .on_action(Message::CaptureAction)
            .key_binding(crate::editor::capture_bindings(
                Message::CaptureSave,
                Message::CaptureCancel,
            ))
            .highlight_with::<crate::editor::Org>(
                crate::editor::Settings {
                    keywords: keywords.clone(),
                    family: font.family,
                },
                crate::editor::format,
            )
            .font(font)
            .size(settings.editor_size)
            .padding(10)
            .height(220)
            .style(style::editor)
            .into(),
    );
    parts.push(
        row![
            text("Ctrl+Enter saves, Esc cancels")
                .size(12)
                .style(dim)
                .width(Fill),
            button(text("Cancel").size(13))
                .on_press(Message::CaptureCancel)
                .padding([3, 10])
                .style(button::secondary),
            button(text("Save").size(13))
                .on_press(Message::CaptureSave)
                .padding([3, 10]),
        ]
        .spacing(8)
        .align_y(iced::Center)
        .into(),
    );
    panel(Column::with_children(parts).spacing(10).into())
}

/// The open refile picker.
pub struct RefilePicker {
    /// The heading to move.
    pub heading: HeadingRef,
    /// What's typed in the filter.
    pub filter: String,
    /// Every file and heading, once the worker sent them.
    pub targets: Option<Arc<Vec<RefileTarget>>>,
}

/// How many matches the picker lists.
const SHOWN: usize = 200;

impl RefilePicker {
    /// Indexes of the targets matching the filter: every word in the file
    /// name or outline path, ignoring case. The heading itself is left out.
    #[must_use]
    pub fn matches(&self) -> Vec<usize> {
        let Some(all) = &self.targets else {
            return Vec::new();
        };
        let words: Vec<String> = self
            .filter
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        all.iter()
            .enumerate()
            .filter(|(_, t)| {
                t.heading
                    .as_ref()
                    .is_none_or(|h| !(h.path == self.heading.path && h.index == self.heading.index))
            })
            .filter(|(_, t)| {
                let hay = format!("{} {}", t.path, t.outline).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .take(SHOWN)
            .collect()
    }
}

/// What the Appearance panel does.
#[derive(Clone, Debug)]
pub enum Appearance {
    /// Open the panel.
    Open,
    /// Close it.
    Close,
    /// Which colours.
    Colours(Colours),
    /// Light, dark or follow the system.
    Mode(ModeChoice),
    /// The note text's font.
    ReadFont(Choice),
    /// The note text's size.
    ReadSize(u32),
    /// The editor's font.
    EditorFont(Choice),
    /// The editor's size.
    EditorSize(u32),
    /// The installed fonts were read.
    FontsListed(Arc<Installed>),
}

/// The Appearance panel: colours, light/dark and fonts. A choice applies at
/// once. `installed` is `None` while the font list is being read.
pub fn appearance_view<'a>(s: &'a Settings, installed: Option<&Installed>) -> Element<'a, Message> {
    let read = Choice(s.read_font.clone());
    let edit = Choice(s.editor_font.clone());
    let (read_list, edit_list) = installed.map_or_else(
        || (vec![read.clone()], vec![edit.clone()]),
        |i| (i.for_reading(), i.for_editing()),
    );
    let sizes: Vec<u32> = settings::SIZES.collect();
    let font_row = |label,
                    list,
                    current,
                    on_font: fn(Choice) -> Appearance,
                    size,
                    on_size: fn(u32) -> Appearance| {
        row![
            text(label).size(13).width(80),
            pick_list(list, Some(current), move |c| Message::Appearance(on_font(
                c
            )))
            .text_size(13)
            .padding([3, 8])
            .width(Fill),
            pick_list(sizes.clone(), Some(size), move |n| {
                Message::Appearance(on_size(n))
            })
            .text_size(13)
            .padding([3, 8])
            .width(64),
        ]
        .spacing(8)
        .align_y(iced::Center)
    };
    let colours = Colours::ALL.map(|c| {
        radio(c.label(), c, Some(s.colours), |c| {
            Message::Appearance(Appearance::Colours(c))
        })
        .size(16)
        .text_size(14)
        .into()
    });
    let modes = ModeChoice::ALL.map(|m| {
        radio(m.label(), m, Some(s.mode), |m| {
            Message::Appearance(Appearance::Mode(m))
        })
        .size(16)
        .text_size(14)
        .into()
    });
    panel_sized(
        column![
            text("Appearance").size(18).font(bold()),
            text("Colours").size(13).style(dim),
            Column::from_iter(colours).spacing(8),
            text("Mode").size(13).style(dim),
            Column::from_iter(modes).spacing(8),
            text(if installed.is_some() {
                "Fonts"
            } else {
                "Fonts (reading the installed fonts…)"
            })
            .size(13)
            .style(dim),
            font_row(
                "Notes",
                read_list,
                read,
                Appearance::ReadFont,
                s.read_size,
                Appearance::ReadSize
            ),
            font_row(
                "Editor",
                edit_list,
                edit,
                Appearance::EditorFont,
                s.editor_size,
                Appearance::EditorSize
            ),
            row![
                text("Esc closes").size(12).style(dim).width(Fill),
                button(text("Close").size(13))
                    .on_press(Message::Appearance(Appearance::Close))
                    .padding([3, 10]),
            ]
            .align_y(iced::Center),
        ]
        .spacing(10)
        .into(),
        440.0,
    )
}

/// The refile picker.
pub fn refile_view(p: &RefilePicker) -> Element<'_, Message> {
    let list: Element<'_, Message> = match &p.targets {
        None => text("Loading…").size(13).style(dim).into(),
        Some(all) => {
            let rows = p.matches().into_iter().filter_map(|i| {
                let t = all.get(i)?;
                let label = if t.outline.is_empty() {
                    row![text(t.path.as_str()).size(14).font(bold())]
                } else {
                    row![
                        text(t.outline.as_str()).size(14).width(Fill),
                        text(t.path.as_str()).size(11).style(dim),
                    ]
                    .spacing(8)
                };
                Some(
                    button(label.align_y(iced::Center))
                        .on_press(Message::RefilePick(i))
                        .width(Fill)
                        .padding([3, 8])
                        .style(style::row(false))
                        .into(),
                )
            });
            scrollable(Column::with_children(rows).spacing(1))
                .height(320)
                .into()
        }
    };
    panel(
        column![
            text(format!("Refile “{}”", p.heading.title))
                .size(18)
                .font(bold()),
            text("Moves it, with its children, to the end of the chosen heading or file.")
                .size(12)
                .style(dim),
            text_input("Type to filter: file or heading", &p.filter)
                .id(REFILE_INPUT)
                .on_input(Message::RefileFilter)
                .size(14)
                .padding([4, 8]),
            list,
            row![
                text("Esc cancels").size(12).style(dim).width(Fill),
                button(text("Cancel").size(13))
                    .on_press(Message::RefileCancel)
                    .padding([3, 10])
                    .style(button::secondary),
            ]
            .align_y(iced::Center),
        ]
        .spacing(10)
        .into(),
    )
}

fn panel(content: Element<'_, Message>) -> Element<'_, Message> {
    panel_sized(content, 620.0)
}

/// A raised panel `width` wide.
fn panel_sized(content: Element<'_, Message>, width: f32) -> Element<'_, Message> {
    container(content)
        .padding(18)
        .width(width)
        .style(style::modal)
        .into()
}
