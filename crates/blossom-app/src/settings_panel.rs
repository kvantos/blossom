//! The Settings panel: a list of sections on the left (Appearance, Editor,
//! Agenda, Notes folder, Search by meaning), the chosen one on the right. A choice applies
//! at once and is written to `config.toml`; typed values (a server address)
//! apply with Enter or the Apply button, so the vault isn't reopened at every
//! key.

use std::sync::Arc;

use iced::widget::{
    Column, button, column, combo_box, container, pick_list, radio, row, rule, scrollable, text,
    text_input, toggler,
};
use iced::{Element, Fill};

use crate::agenda::prefs::{self, MAX_ZONES, WeekStart};
use crate::app::{Message, bold, dim};
use crate::fonts::{Choice, Installed};
use crate::meaning::Status;
use crate::search;
use crate::settings::{self, Settings};
use crate::style::{self, Colours, ModeChoice};
use crate::topbar::Labels;

/// A section of the panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    /// Colours, light/dark and fonts.
    #[default]
    Appearance,
    /// The note editor's keys.
    Editor,
    /// Week start, working hours, time zones.
    Agenda,
    /// Which folder of notes to open.
    Notes,
    /// The model for search by meaning.
    Search,
}

impl Section {
    /// Every section, in the order the panel lists them.
    pub const ALL: [Section; 5] = [
        Section::Appearance,
        Section::Editor,
        Section::Agenda,
        Section::Notes,
        Section::Search,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Editor => "Editor",
            Section::Agenda => "Agenda",
            Section::Notes => "Notes folder",
            Section::Search => "Search by meaning",
        }
    }
}

/// How search by meaning works: `[semantic] model`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelChoice {
    /// The built-in English model.
    English,
    /// The built-in multilingual model.
    Multilingual,
    /// An OpenAI-compatible server.
    Server,
}

impl ModelChoice {
    /// The choices this build offers: without built-in models, only a server.
    #[must_use]
    pub fn offered() -> Vec<ModelChoice> {
        if cfg!(feature = "builtin-model") {
            vec![
                ModelChoice::English,
                ModelChoice::Multilingual,
                ModelChoice::Server,
            ]
        } else {
            vec![ModelChoice::Server]
        }
    }

    /// The choice `[semantic] model` names; an unknown name is `None`.
    #[must_use]
    pub fn from_config(model: Option<&str>) -> Option<ModelChoice> {
        match model.map(|m| m.trim().to_ascii_lowercase()).as_deref() {
            None | Some("english") => Some(ModelChoice::English),
            Some("multilingual") => Some(ModelChoice::Multilingual),
            Some("server") => Some(ModelChoice::Server),
            Some(_) => None,
        }
    }

    /// Its name in the config.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ModelChoice::English => "english",
            ModelChoice::Multilingual => "multilingual",
            ModelChoice::Server => "server",
        }
    }

    /// The built-in model it is, if one.
    #[must_use]
    pub fn builtin(self) -> Option<blossom_semantic::Model> {
        match self {
            ModelChoice::English => Some(blossom_semantic::Model::English),
            ModelChoice::Multilingual => Some(blossom_semantic::Model::Multilingual),
            ModelChoice::Server => None,
        }
    }

    fn label(self) -> String {
        match self.builtin() {
            Some(m) => {
                let name = m.display_name();
                let mut chars = name.chars();
                let first = chars.next().map(|c| c.to_uppercase().to_string());
                format!(
                    "{}{}: built in, a one-time download of {} MB",
                    first.unwrap_or_default(),
                    chars.as_str(),
                    m.download_mb()
                )
            }
            None => "A server: Ollama, or any OpenAI-compatible one".to_string(),
        }
    }
}

/// What the Settings panel does.
#[derive(Clone, Debug)]
pub enum Setting {
    /// Open the panel.
    Open,
    /// Close it.
    Close,
    /// Show a section.
    Section(Section),
    /// The installed fonts were read.
    FontsListed(Arc<Installed>),
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
    /// Where the top bar's labels go.
    Labels(Labels),
    /// Emacs keys in the editor on or off.
    EmacsKeys(bool),
    /// The first day of the week.
    WeekStart(WeekStart),
    /// Working hours start.
    WorkStart(Hour),
    /// Working hours end.
    WorkEnd(Hour),
    /// Add a time zone, by name.
    AddZone(String),
    /// Remove the Nth time zone.
    RemoveZone(usize),
    /// How search by meaning works.
    Model(ModelChoice),
    /// The server address field.
    Server(String),
    /// The server's model field.
    ServerModel(String),
    /// Write the two server fields.
    ApplyServer,
    /// Download the chosen built-in model now.
    Download,
}

/// An hour of the day for the working hours, shown as `08:00`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hour(pub u32);

impl std::fmt::Display for Hour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:00", self.0)
    }
}

/// The open panel.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    /// The section shown.
    pub section: Section,
    /// The server address being typed.
    pub server: String,
    /// The server's model being typed.
    pub server_model: String,
    /// The time zone picker: every zone, filtered as you type.
    pub zones: combo_box::State<String>,
}

/// What the Search section shows besides the panel's own fields.
pub struct SearchState<'a> {
    /// The config's `[semantic] model`: a choice, or `None` for a name the
    /// app doesn't know (shown as no choice).
    pub model: Option<ModelChoice>,
    /// `server` and `server_model` as written in the config.
    pub saved_server: (&'a str, &'a str),
    /// Whether the chosen built-in model is downloaded.
    pub downloaded: bool,
    /// How search by meaning is getting on, once started.
    pub status: Option<&'a Status>,
    /// A notes folder is open (the download runs in its worker).
    pub vault_open: bool,
}

/// Everything the panel shows.
pub struct View<'a> {
    /// The open panel.
    pub panel: &'a Panel,
    /// The appearance choices.
    pub settings: &'a Settings,
    /// The installed fonts, `None` while being read.
    pub installed: Option<&'a Installed>,
    /// The notes folder: the one open, else the config's.
    pub folder: Option<String>,
    /// The config file, to point at.
    pub config_path: Option<String>,
    /// The Search section.
    pub search: SearchState<'a>,
}

fn send(s: Setting) -> Message {
    Message::Setting(s)
}

/// The Settings panel.
pub fn view(v: View<'_>) -> Element<'_, Message> {
    let nav = Column::from_iter(Section::ALL.map(|s| {
        button(text(s.label()).size(14))
            .on_press(send(Setting::Section(s)))
            .width(Fill)
            .padding([5, 10])
            .style(style::row(s == v.panel.section))
            .into()
    }))
    .spacing(2)
    .width(170);
    let body = match v.panel.section {
        Section::Appearance => appearance(v.settings, v.installed),
        Section::Editor => editor(v.settings),
        Section::Agenda => agenda(v.settings, &v.panel.zones),
        Section::Notes => notes(v.folder, v.config_path),
        Section::Search => search_section(v.panel, &v.search),
    };
    container(
        column![
            text("Settings").size(18).font(bold()),
            row![
                nav,
                rule::vertical(1),
                scrollable(container(body).padding([0, 12])).height(Fill),
            ]
            .spacing(12)
            .height(400),
            row![
                text("Esc closes").size(12).style(dim).width(Fill),
                button(text("Close").size(13))
                    .on_press(send(Setting::Close))
                    .padding([3, 10]),
            ]
            .align_y(iced::Center),
        ]
        .spacing(12),
    )
    .padding(18)
    .width(680)
    .style(style::modal)
    .into()
}

fn heading(label: &str) -> Element<'_, Message> {
    text(label).size(13).style(dim).into()
}

/// Colours, light/dark and fonts.
fn appearance<'a>(s: &'a Settings, installed: Option<&Installed>) -> Element<'a, Message> {
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
                    on_font: fn(Choice) -> Setting,
                    size,
                    on_size: fn(u32) -> Setting| {
        row![
            text(label).size(13).width(60),
            pick_list(list, Some(current), move |c| send(on_font(c)))
                .text_size(13)
                .padding([3, 8])
                .width(Fill),
            pick_list(sizes.clone(), Some(size), move |n| send(on_size(n)))
                .text_size(13)
                .padding([3, 8])
                .width(64),
        ]
        .spacing(8)
        .align_y(iced::Center)
    };
    let colours = Colours::ALL.map(|c| {
        radio(c.label(), c, Some(s.colours), |c| send(Setting::Colours(c)))
            .size(16)
            .text_size(14)
            .into()
    });
    let modes = ModeChoice::ALL.map(|m| {
        radio(m.label(), m, Some(s.mode), |m| send(Setting::Mode(m)))
            .size(16)
            .text_size(14)
            .into()
    });
    let labels = Labels::ALL.map(|l| {
        radio(l.label(), l, Some(s.top_bar_labels), |l| {
            send(Setting::Labels(l))
        })
        .size(16)
        .text_size(14)
        .into()
    });
    column![
        heading("Colours"),
        Column::from_iter(colours).spacing(8),
        heading("Mode"),
        Column::from_iter(modes).spacing(8),
        heading(if installed.is_some() {
            "Fonts"
        } else {
            "Fonts (reading the installed fonts…)"
        }),
        font_row(
            "Notes",
            read_list,
            read,
            Setting::ReadFont,
            s.read_size,
            Setting::ReadSize
        ),
        font_row(
            "Editor",
            edit_list,
            edit,
            Setting::EditorFont,
            s.editor_size,
            Setting::EditorSize
        ),
        heading("Top bar"),
        Column::from_iter(labels).spacing(8),
    ]
    .spacing(10)
    .into()
}

/// The Emacs keys switch, and what the keys do.
fn editor(s: &Settings) -> Element<'_, Message> {
    let keys = [
        ("C-f  C-b", "forward, back a character"),
        ("C-n  C-p", "next, previous line"),
        ("C-a  C-e", "start, end of the line"),
        ("M-f  M-b", "forward, back a word"),
        ("M-<  M->", "start, end of the text"),
        ("C-d", "delete the next character"),
        ("M-d  M-DEL", "kill the next, previous word"),
        (
            "C-k",
            "kill to the end of the line (again: add the line break)",
        ),
        ("C-y", "yank: insert what was killed last"),
    ];
    let rows = keys.map(|(k, what)| {
        row![
            text(k).size(13).font(iced::Font::MONOSPACE).width(110),
            text(what).size(13),
        ]
        .into()
    });
    let mut parts: Vec<Element<'_, Message>> = vec![
        toggler(s.emacs_keys)
            .label("Emacs keys")
            .on_toggle(|on| send(Setting::EmacsKeys(on)))
            .text_size(14)
            .size(18)
            .into(),
        text(
            "C is Ctrl (Control on macOS) and M is Alt (Option on macOS). Kills in a row \
             add up, and C-y inserts them.",
        )
        .size(12)
        .style(dim)
        .into(),
    ];
    if s.emacs_keys {
        parts.push(Column::from_iter(rows).spacing(4).into());
        parts.push(
            text(format!(
                "Redo is {} while these are on, since C-y yanks.",
                crate::keys::AppKey::Redo.hint()
            ))
            .size(12)
            .style(dim)
            .into(),
        );
    }
    Column::with_children(parts).spacing(10).into()
}

/// Week start, working hours and extra time zones.
fn agenda<'a>(s: &'a Settings, zones: &'a combo_box::State<String>) -> Element<'a, Message> {
    let a = &s.agenda;
    let starts: Vec<Hour> = (0..24).map(Hour).collect();
    let ends: Vec<Hour> = (1..=24).map(Hour).collect();
    let mut parts: Vec<Element<'a, Message>> = vec![
        heading("Week starts on"),
        pick_list(WeekStart::ALL, Some(a.week_start), |w| {
            send(Setting::WeekStart(w))
        })
        .text_size(13)
        .into(),
        heading("Working hours"),
        row![
            pick_list(starts, Some(Hour(a.work.0)), |h| send(Setting::WorkStart(
                h
            )))
            .text_size(13),
            text("to").size(13),
            pick_list(ends, Some(Hour(a.work.1)), |h| send(Setting::WorkEnd(h))).text_size(13),
        ]
        .spacing(8)
        .align_y(iced::Center)
        .into(),
        text("The week view opens on them and shades the other hours.")
            .size(12)
            .style(dim)
            .into(),
        heading("Other time zones"),
    ];
    for (i, z) in a.zones.iter().enumerate() {
        parts.push(
            row![
                text(format!("{} ({})", prefs::city(*z), z.name()))
                    .size(13)
                    .width(Fill),
                button(text("Remove").size(12))
                    .on_press(send(Setting::RemoveZone(i)))
                    .padding([2, 8])
                    .style(button::secondary),
            ]
            .align_y(iced::Center)
            .into(),
        );
    }
    if a.zones.len() < MAX_ZONES {
        parts.push(
            combo_box(zones, "Add a time zone, e.g. Tokyo", None, |z| {
                send(Setting::AddZone(z))
            })
            .size(13)
            .into(),
        );
    }
    parts.push(
        text("Each shows as an hour column beside yours in the week view, and an item's card gives its time there too. Up to two.")
            .size(12)
            .style(dim)
            .into(),
    );
    Column::with_children(parts).spacing(8).into()
}

/// The notes folder, and where the settings live.
fn notes<'a>(folder: Option<String>, config_path: Option<String>) -> Element<'a, Message> {
    let mut parts: Vec<Element<'a, Message>> = vec![
        text(folder.unwrap_or_else(|| "None chosen yet".to_string()))
            .size(14)
            .into(),
        button(text("Choose another folder…").size(13))
            .on_press(Message::PickFolder)
            .padding([4, 12])
            .style(button::secondary)
            .into(),
        text("Blossom opens the folder you choose here every time it starts.")
            .size(12)
            .style(dim)
            .into(),
    ];
    if let Some(path) = config_path {
        parts.push(heading("Settings file"));
        parts.push(text(path).size(13).into());
        parts.push(
            text(
                "Every setting, these and more, with a line on what each does. \
                 Changes made there apply at once.",
            )
            .size(12)
            .style(dim)
            .into(),
        );
    }
    Column::with_children(parts).spacing(10).into()
}

/// The model for search by meaning, and its download or server.
fn search_section<'a>(p: &'a Panel, s: &SearchState<'a>) -> Element<'a, Message> {
    let choices = ModelChoice::offered().into_iter().map(|m| {
        radio(m.label(), m, s.model, |m| send(Setting::Model(m)))
            .size(16)
            .text_size(14)
            .into()
    });
    let mut parts: Vec<Element<'a, Message>> = vec![
        text(
            "Search by meaning finds notes about the same thing even when they use \
             other words. Turn it on with \"By meaning\" on the search screen.",
        )
        .size(13)
        .into(),
        heading("Model"),
        Column::from_iter(choices).spacing(8).into(),
    ];
    if s.model.is_none() {
        parts.push(
            text("The settings file names a model this build doesn't offer; choose one above.")
                .size(12)
                .style(dim)
                .into(),
        );
    }
    match s.model {
        Some(ModelChoice::Server) => parts.push(server_fields(p, s.saved_server)),
        Some(m) => parts.push(model_state(m, s)),
        None => {}
    }
    Column::with_children(parts).spacing(10).into()
}

/// Whether the built-in model is there, with its download and progress.
fn model_state<'a>(m: ModelChoice, s: &SearchState<'a>) -> Element<'a, Message> {
    let note = |t: String| -> Element<'a, Message> { text(t).size(13).style(dim).into() };
    // Downloading, loading, reading the notes or failed: the search
    // screen's own status says it best.
    if let Some(status) = s.status
        && !matches!(status, Status::Ready | Status::NeedsDownload { .. })
        && let Some(e) = search::status_view(status)
    {
        return e;
    }
    let Some(model) = m.builtin() else {
        return column![].into();
    };
    if s.downloaded {
        return note(format!(
            "The {} model is downloaded; everything runs on this computer.",
            model.display_name()
        ));
    }
    let mut download = button(text(format!("Download now ({} MB)", model.download_mb())).size(13))
        .padding([4, 12])
        .style(button::primary);
    if s.vault_open {
        download = download.on_press(send(Setting::Download));
    }
    column![
        note(if s.vault_open {
            "Not downloaded yet. It downloads by itself the first time you search by meaning, or now:"
                .to_string()
        } else {
            "Not downloaded yet. Open a notes folder first to download it.".to_string()
        }),
        download,
    ]
    .spacing(8)
    .into()
}

/// The server's address and model, applied together.
fn server_fields<'a>(p: &'a Panel, saved: (&str, &str)) -> Element<'a, Message> {
    let field = |label, placeholder, value: &'a str, on: fn(String) -> Setting| {
        row![
            text(label).size(13).width(70),
            text_input(placeholder, value)
                .on_input(move |s| send(on(s)))
                .on_submit(send(Setting::ApplyServer))
                .size(13)
                .padding([4, 8]),
        ]
        .spacing(8)
        .align_y(iced::Center)
    };
    let changed = (p.server.trim(), p.server_model.trim()) != (saved.0.trim(), saved.1.trim());
    let mut apply = button(text("Apply").size(13))
        .padding([4, 12])
        .style(button::primary);
    if changed {
        apply = apply.on_press(send(Setting::ApplyServer));
    }
    column![
        field(
            "Address",
            blossom_semantic::DEFAULT_URL,
            &p.server,
            Setting::Server
        ),
        field(
            "Model",
            blossom_semantic::DEFAULT_MODEL,
            &p.server_model,
            Setting::ServerModel
        ),
        row![
            text("Empty fields use the defaults shown (Ollama on this computer).")
                .size(12)
                .style(dim)
                .width(Fill),
            apply,
        ]
        .spacing(8)
        .align_y(iced::Center),
        text("A server that needs an API key: set api_key_env in the settings file.")
            .size(12)
            .style(dim),
    ]
    .spacing(8)
    .into()
}
