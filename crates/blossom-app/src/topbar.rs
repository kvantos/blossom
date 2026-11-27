//! The top bar: show/hide the sidebar, the search field, then Agenda,
//! Capture and Settings. Icons alone show a hint after a second's hover
//! (what it does and its shortcut); the "Top bar labels" setting puts the
//! names under or beside the icons instead, and then there are no hints.

use std::sync::LazyLock;
use std::time::Duration;

use iced::widget::{Space, button, column, container, row, svg, text, text_input, tooltip};
use iced::{Element, Fill, Theme};
use serde::{Deserialize, Serialize};

use crate::app::Message;
use crate::keys::AppKey;
use crate::search;
use crate::settings_panel::Setting;
use crate::style;

/// Where the top bar's labels go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Labels {
    /// No labels; a hint on hover.
    #[default]
    None,
    /// Under each icon.
    Below,
    /// Beside each icon.
    Beside,
}

impl Labels {
    /// Every choice, in the order the Settings panel lists them.
    pub const ALL: [Labels; 3] = [Labels::None, Labels::Below, Labels::Beside];

    /// The name shown in the Settings panel.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Labels::None => "Icons only (a hint on hover)",
            Labels::Below => "Names under the icons",
            Labels::Beside => "Names beside the icons",
        }
    }
}

/// How long the pointer rests on an icon before its hint shows.
const HINT_DELAY: Duration = Duration::from_millis(900);

/// The icons: 24×24 outlines, tinted with the theme's text colour.
mod icon {
    pub const SIDEBAR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4.5" width="18" height="15" rx="2.5"/><path d="M9.5 4.5v15M5.5 8.5h1.5M5.5 11.5h1.5"/></svg>"##;
    pub const AGENDA: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="5" width="17" height="15.5" rx="2.5"/><path d="M3.5 10h17M8 3v4M16 3v4M8 14h2M14 14h2M8 17h2"/></svg>"##;
    pub const CAPTURE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M13 3.5H6.5a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V10"/><path d="M18 2.5v6M15 5.5h6"/><path d="M8 13h8M8 16.5h5"/></svg>"##;
    pub const SETTINGS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6.5h5M13 6.5h7M4 12h11M19 12h1M4 17.5h3M11 17.5h9"/><circle cx="11" cy="6.5" r="2"/><circle cx="17" cy="12" r="2"/><circle cx="9" cy="17.5" r="2"/></svg>"##;
}

fn handle(svg: &'static str) -> svg::Handle {
    svg::Handle::from_memory(svg.as_bytes())
}

static SIDEBAR: LazyLock<svg::Handle> = LazyLock::new(|| handle(icon::SIDEBAR));
static AGENDA: LazyLock<svg::Handle> = LazyLock::new(|| handle(icon::AGENDA));
static CAPTURE: LazyLock<svg::Handle> = LazyLock::new(|| handle(icon::CAPTURE));
static SETTINGS: LazyLock<svg::Handle> = LazyLock::new(|| handle(icon::SETTINGS));

/// What the top bar shows.
pub struct Bar<'a> {
    /// Where the labels go.
    pub labels: Labels,
    /// The sidebar is shown.
    pub sidebar: bool,
    /// The search field's text.
    pub search: &'a str,
    /// The search screen is showing, by meaning.
    pub meaning: bool,
    /// The agenda is showing.
    pub agenda: bool,
}

/// One icon button with its label or hint.
fn item<'a>(
    labels: Labels,
    icon: &svg::Handle,
    name: &'a str,
    hint: String,
    on_press: Message,
    active: bool,
) -> Element<'a, Message> {
    let picture = svg(icon.clone())
        .width(20)
        .height(20)
        .style(|theme: &Theme, _| svg::Style {
            color: Some(theme.palette().text),
        });
    let content: Element<'a, Message> = match labels {
        Labels::None => picture.into(),
        Labels::Below => column![picture, text(name).size(11)]
            .spacing(2)
            .align_x(iced::Center)
            .into(),
        Labels::Beside => row![picture, text(name).size(13)]
            .spacing(6)
            .align_y(iced::Center)
            .into(),
    };
    let b = button(content)
        .on_press(on_press)
        .padding(if labels == Labels::Below {
            [3, 8]
        } else {
            [5, 8]
        })
        .style(style::row(active));
    if labels != Labels::None {
        return b.into();
    }
    tooltip(
        b,
        container(text(hint).size(12))
            .padding([3, 8])
            .style(style::modal),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .delay(HINT_DELAY)
    .into()
}

/// The top bar.
pub fn view(bar: &Bar<'_>) -> Element<'static, Message> {
    let labels = bar.labels;
    let field = text_input(
        if bar.meaning {
            "Describe what you're looking for…"
        } else {
            "Search your notes…"
        },
        bar.search,
    )
    .id(search::INPUT)
    .on_input(Message::SearchInput)
    .on_submit(Message::ShowSearch)
    .size(14)
    .padding([5, 10])
    .width(Fill);
    let field: Element<'static, Message> = if labels == Labels::None {
        tooltip(
            field,
            container(text(format!("Search ({})", AppKey::Search.hint())).size(12))
                .padding([3, 8])
                .style(style::modal),
            tooltip::Position::Bottom,
        )
        .gap(4)
        .delay(HINT_DELAY)
        .into()
    } else {
        field.into()
    };
    let sidebar_hint = format!(
        "{} the sidebar ({})",
        if bar.sidebar { "Hide" } else { "Show" },
        AppKey::Sidebar.hint()
    );
    container(
        row![
            item(
                labels,
                &SIDEBAR,
                "Sidebar",
                sidebar_hint,
                Message::ToggleSidebar,
                false,
            ),
            container(field).max_width(480).width(Fill),
            Space::new().width(Fill),
            item(
                labels,
                &AGENDA,
                "Agenda",
                format!("Agenda ({})", AppKey::Agenda.hint()),
                Message::ShowAgenda,
                bar.agenda,
            ),
            item(
                labels,
                &CAPTURE,
                "Capture",
                format!("Capture a note ({})", AppKey::Capture.hint()),
                Message::OpenCapture,
                false,
            ),
            item(
                labels,
                &SETTINGS,
                "Settings",
                format!("Settings ({})", AppKey::Settings.hint()),
                Message::Setting(Setting::Open),
                false,
            ),
        ]
        .spacing(6)
        .align_y(iced::Center),
    )
    .padding([4, 8])
    .width(Fill)
    .style(style::pane)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_parse() {
        for svg in [icon::SIDEBAR, icon::AGENDA, icon::CAPTURE, icon::SETTINGS] {
            assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
            assert_eq!(svg.matches('<').count(), svg.matches('>').count());
        }
    }
}
