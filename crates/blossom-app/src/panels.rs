//! The refile picker, shown in the main area: type to filter every file
//! and heading of the vault, then pick where the heading goes.

use std::sync::Arc;

use blossom_vault::ops::HeadingRef;
use iced::widget::{Column, button, column, container, row, scrollable, text, text_input};
use iced::{Element, Fill};

use crate::app::{Message, bold, dim};
use crate::style;
use crate::worker::RefileTarget;

/// The refile filter field's id, for focusing.
pub const REFILE_INPUT: &str = "refile-filter";

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
