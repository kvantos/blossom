//! The application: state, messages, update and view.
//!
//! Three panes: the notebooks tree, the outline of the selected file, and the
//! read view of the selected heading (or the file's intro). All file access
//! happens in the [`worker`]; this module only holds what is on screen.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use blossom_core::render::{self, Block, Span};
use blossom_core::{Document, Heading, Timestamp};
use chrono::{Datelike, NaiveDate};
use iced::theme::Mode;
use iced::widget::{
    Column, button, column, container, row, rule, scrollable, space, text, text_input,
};
use iced::{Element, Fill, Font, Length, Subscription, Task, Theme, font};

use crate::outline;
use crate::read::{self, Ink};
use crate::shot;
use crate::style::{self, Chip};
use crate::tree::{self, FileInfo};
use crate::worker::{self, Event, Handle, OpenVault, Request};

/// The app.
pub struct App {
    mode: Mode,
    worker: Option<Handle>,
    /// A vault to open as soon as the worker is ready.
    pending: Option<OpenVault>,
    /// Used to open a typed-in folder.
    template: OpenVault,
    vault_input: String,
    root: Option<PathBuf>,
    files: Arc<Vec<FileInfo>>,
    collapsed_folders: HashSet<String>,
    file: Option<OpenFile>,
    status: Option<String>,
    today: NaiveDate,
    /// The screenshot developer hook, when on.
    shot: Option<shot::Plan>,
}

/// The file shown in the outline.
struct OpenFile {
    path: String,
    doc: Option<Arc<Document>>,
    /// Rendered heading titles, by heading index.
    titles: Vec<Vec<Span>>,
    folded: HashSet<usize>,
    selected: Selected,
    /// The read view of `selected`.
    blocks: Vec<Block>,
    missing: bool,
}

/// What the read view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selected {
    /// Nothing yet.
    Nothing,
    /// The file's intro.
    Intro,
    /// A heading.
    Heading(usize),
}

/// Everything that can happen.
#[derive(Clone, Debug)]
pub enum Message {
    /// From the worker.
    Worker(Event),
    /// The system switched light/dark.
    Mode(Mode),
    /// The welcome screen's folder field.
    VaultInput(String),
    /// Open the typed folder.
    OpenTyped,
    /// Fold or unfold a folder in the tree.
    ToggleFolder(String),
    /// Show a file's outline.
    SelectFile(String),
    /// Fold or unfold a heading in the outline.
    ToggleFold(usize),
    /// Show something in the read view.
    Select(Selected),
    /// A link was clicked.
    Link(String),
    /// Hide the status line.
    DismissStatus,
    /// The screenshot hook's capture.
    Shot(iced::window::Screenshot),
}

impl App {
    /// A new app; `open` is the vault to open on start, if known.
    pub fn new(
        open: Option<OpenVault>,
        template: OpenVault,
        today: NaiveDate,
        shot: Option<shot::Plan>,
    ) -> (App, Task<Message>) {
        let forced = shot.as_ref().and_then(|p| p.mode);
        let app = App {
            mode: forced.unwrap_or(Mode::None),
            worker: None,
            vault_input: open
                .as_ref()
                .map_or_else(String::new, |o| o.root.display().to_string()),
            pending: open,
            template,
            root: None,
            files: Arc::new(Vec::new()),
            collapsed_folders: HashSet::new(),
            file: None,
            status: None,
            today,
            shot,
        };
        if forced.is_some() {
            return (app, Task::none());
        }
        (app, iced::system::theme().map(Message::Mode))
    }

    /// The window title.
    pub fn title(&self) -> String {
        let vault = self
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        match (vault, &self.file) {
            (Some(v), Some(f)) => format!("{} — {v} — Blossom", f.path),
            (Some(v), None) => format!("{v} — Blossom"),
            _ => "Blossom".to_string(),
        }
    }

    /// Our light or dark theme; `None` until the system mode is known.
    pub fn theme(&self) -> Option<Theme> {
        (self.mode != Mode::None).then(|| style::theme(self.mode))
    }

    /// The worker and system theme changes.
    pub fn subscription(_: &Self) -> Subscription<Message> {
        Subscription::batch([
            worker::subscription().map(Message::Worker),
            iced::system::theme_changes().map(Message::Mode),
        ])
    }

    /// Applies a message.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Worker(event) => return self.on_worker(event),
            Message::Mode(mode) => {
                if self.shot.as_ref().is_none_or(|p| p.mode.is_none()) {
                    self.mode = mode;
                }
            }
            Message::VaultInput(s) => self.vault_input = s,
            Message::OpenTyped => {
                let path = self.vault_input.trim();
                if !path.is_empty() {
                    let open = OpenVault {
                        root: blossom_vault::config::expand(path),
                        ..self.template.clone()
                    };
                    self.request_open(open);
                }
            }
            Message::ToggleFolder(path) => {
                if !self.collapsed_folders.remove(&path) {
                    self.collapsed_folders.insert(path);
                }
            }
            Message::SelectFile(path) => self.select_file(path),
            Message::ToggleFold(i) => {
                if let Some(f) = &mut self.file
                    && !f.folded.remove(&i)
                {
                    f.folded.insert(i);
                }
            }
            Message::Select(sel) => {
                if let Some(f) = &mut self.file {
                    f.selected = sel;
                    f.refresh_blocks();
                }
            }
            Message::Link(target) => self.follow_link(&target),
            Message::DismissStatus => self.status = None,
            Message::Shot(shot) => {
                if let Some(plan) = &self.shot
                    && let Err(e) =
                        shot::write_png(&plan.out, shot.size.width, shot.size.height, &shot.rgba)
                {
                    eprintln!("blossom: screenshot: {e}");
                }
                return iced::exit();
            }
        }
        Task::none()
    }

    fn request_open(&mut self, open: OpenVault) {
        match &self.worker {
            Some(w) => w.send(Request::Open(open)),
            None => self.pending = Some(open),
        }
    }

    fn select_file(&mut self, path: String) {
        if self.file.as_ref().is_some_and(|f| f.path == path) {
            return;
        }
        if let Some(w) = &self.worker {
            w.send(Request::Load(path.clone()));
        }
        self.file = Some(OpenFile {
            path,
            doc: None,
            titles: Vec::new(),
            folded: HashSet::new(),
            selected: Selected::Nothing,
            blocks: Vec::new(),
            missing: false,
        });
    }

    fn on_worker(&mut self, event: Event) -> Task<Message> {
        match event {
            Event::Ready(handle) => {
                if let Some(open) = self.pending.take() {
                    handle.send(Request::Open(open));
                }
                self.worker = Some(handle);
            }
            Event::Opened(root) => {
                self.vault_input = root.display().to_string();
                self.root = Some(root);
                self.file = None;
                self.collapsed_folders.clear();
                self.status = None;
            }
            Event::Files(files) => {
                self.files = files;
                if let Some(plan) = &self.shot
                    && self.file.is_none()
                    && let Some(path) = plan
                        .file
                        .clone()
                        .or_else(|| self.files.first().map(|f| f.path.clone()))
                {
                    self.select_file(path);
                }
            }
            Event::Document(path, doc) => {
                if let Some(f) = self.file.as_mut().filter(|f| f.path == path) {
                    f.set_document(doc);
                    if let Some(plan) = &self.shot {
                        if let Some(i) = plan.heading {
                            f.selected = Selected::Heading(i);
                            f.refresh_blocks();
                        }
                        return capture();
                    }
                }
            }
            Event::Missing(path) => {
                if let Some(f) = self.file.as_mut().filter(|f| f.path == path) {
                    f.missing = true;
                }
            }
            Event::Error(e) => self.status = Some(e),
        }
        Task::none()
    }

    fn follow_link(&mut self, target: &str) {
        let lower = target.to_lowercase();
        if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:")
        {
            if let Err(e) = open_externally(target) {
                self.status = Some(format!("Couldn't open {target}: {e}"));
            }
            return;
        }
        // `file:other.org` relative to the open file opens it here.
        if let Some(rel) = target.strip_prefix("file:")
            && let Some(f) = &self.file
        {
            let dir = f.path.rsplit_once('/').map_or("", |(d, _)| d);
            let joined = normalize(&if dir.is_empty() {
                rel.to_string()
            } else {
                format!("{dir}/{rel}")
            });
            if self.files.iter().any(|x| x.path == joined) {
                self.select_file(joined);
                return;
            }
        }
        self.status = Some(format!("Opening links like {target} is not supported yet"));
    }

    /// The whole window.
    pub fn view(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = if self.root.is_some() {
            row![
                container(self.sidebar())
                    .width(240)
                    .height(Fill)
                    .style(style::pane),
                rule::vertical(1),
                container(self.outline())
                    .width(Length::FillPortion(2))
                    .height(Fill),
                rule::vertical(1),
                container(self.reader())
                    .width(Length::FillPortion(3))
                    .height(Fill),
            ]
            .height(Fill)
            .into()
        } else {
            self.welcome()
        };
        match &self.status {
            Some(msg) => column![
                body,
                container(
                    row![
                        text(msg.as_str()).size(13).width(Fill),
                        button(text("Dismiss").size(13))
                            .on_press(Message::DismissStatus)
                            .style(button::text),
                    ]
                    .spacing(8)
                )
                .padding([4, 10])
                .style(style::pane),
            ]
            .into(),
            None => body,
        }
    }

    fn welcome(&self) -> Element<'_, Message> {
        container(
            column![
                text("Blossom").size(32),
                text("Open a folder of .org files").size(15),
                row![
                    text_input("~/org", &self.vault_input)
                        .on_input(Message::VaultInput)
                        .on_submit(Message::OpenTyped)
                        .width(360),
                    button("Open").on_press(Message::OpenTyped),
                ]
                .spacing(8),
                text("Tip: set `vault` in ~/.config/blossom/config.toml to open it on start.")
                    .size(12)
                    .style(dim),
            ]
            .spacing(14),
        )
        .center(Fill)
        .into()
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let selected = self.file.as_ref().map(|f| f.path.as_str());
        let rows = tree::rows(&self.files, &self.collapsed_folders)
            .into_iter()
            .map(|r| -> Element<'_, Message> {
                match r {
                    tree::Row::Folder {
                        path,
                        name,
                        depth,
                        expanded,
                    } => button(
                        row![
                            text(if expanded { "▾" } else { "▸" }).size(13).width(14),
                            text(name).size(14)
                        ]
                        .spacing(4),
                    )
                    .on_press(Message::ToggleFolder(path))
                    .padding(indent(depth, 14.0))
                    .width(Fill)
                    .style(style::row(false))
                    .into(),
                    tree::Row::File { info, name, depth } => {
                        let count = if info.open_tasks > 0 {
                            info.open_tasks.to_string()
                        } else {
                            String::new()
                        };
                        let mut line = row![text(name).size(14).width(Fill)].spacing(4);
                        if info.conflict {
                            line = line.push(
                                container(text("conflict").size(11))
                                    .padding([0, 4])
                                    .style(|t| style::chip(t, Chip::Overdue)),
                            );
                        }
                        line = line.push(text(count).size(12).style(dim));
                        let is_selected = selected == Some(info.path.as_str());
                        button(line)
                            .on_press(Message::SelectFile(info.path))
                            .padding(indent(depth, 14.0))
                            .width(Fill)
                            .style(style::row(is_selected))
                            .into()
                    }
                }
            });
        let name = self
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        column![
            container(text(name).size(13).font(bold()).style(dim)).padding([10, 12]),
            scrollable(Column::with_children(rows).spacing(1).padding([0, 6])).height(Fill),
        ]
        .into()
    }

    fn outline(&self) -> Element<'_, Message> {
        let Some(f) = &self.file else {
            return placeholder("Choose a file");
        };
        if f.missing {
            return placeholder("This file was deleted or moved");
        }
        let Some(doc) = &f.doc else {
            return placeholder("Loading…");
        };
        let title = doc
            .preamble()
            .keywords
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("TITLE"))
            .map_or_else(|| f.path.clone(), |(_, v)| v.clone());
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        if doc.preamble().intro.is_some() {
            items.push(
                button(text("Intro").size(14).font(Font {
                    style: font::Style::Italic,
                    ..Font::DEFAULT
                }))
                .on_press(Message::Select(Selected::Intro))
                .width(Fill)
                .padding([4, 8])
                .style(style::row(f.selected == Selected::Intro))
                .into(),
            );
        }
        let ink = self.ink();
        for r in outline::rows(doc, &f.folded) {
            let Some(h) = doc.heading(r.index) else {
                continue;
            };
            let toggle: Element<'_, Message> = if r.has_children {
                button(text(if r.folded { "▸" } else { "▾" }).size(13))
                    .on_press(Message::ToggleFold(r.index))
                    .padding([4, 2])
                    .style(button::text)
                    .width(18)
                    .into()
            } else {
                space::horizontal().width(18).into()
            };
            let title_spans = f.titles.get(r.index).map_or(&[][..], Vec::as_slice);
            let line = heading_line(h, title_spans, doc, self.today, ink);
            let selected = f.selected == Selected::Heading(r.index);
            items.push(
                row![
                    space::horizontal().width(depth_width(r.depth)),
                    toggle,
                    button(line)
                        .on_press(Message::Select(Selected::Heading(r.index)))
                        .width(Fill)
                        .padding([4, 6])
                        .style(style::row(selected)),
                ]
                .into(),
            );
        }
        if items.is_empty() {
            items.push(text("No headings").size(14).style(dim).into());
        }
        column![
            container(text(title).size(18).font(bold())).padding([10, 14]),
            scrollable(
                Column::with_children(items)
                    .spacing(1)
                    .padding(iced::Padding {
                        top: 0.0,
                        right: 8.0,
                        bottom: 12.0,
                        left: 8.0
                    })
            )
            .height(Fill),
        ]
        .into()
    }

    fn reader(&self) -> Element<'_, Message> {
        let Some(f) = self.file.as_ref().filter(|f| !f.missing) else {
            return placeholder("");
        };
        let Some(doc) = &f.doc else {
            return placeholder("");
        };
        let ink = self.ink();
        let mut parts: Vec<Element<'_, Message>> = Vec::new();
        match f.selected {
            Selected::Nothing => return placeholder("Select a heading to read it"),
            Selected::Intro => parts.push(text("Intro").size(22).font(bold()).into()),
            Selected::Heading(i) => {
                let Some(h) = doc.heading(i) else {
                    return placeholder("Select a heading to read it");
                };
                let title = f.titles.get(i).map_or(&[][..], Vec::as_slice);
                parts.push(read::sized(title, 24, ink, Message::Link));
                let mut meta = row![].spacing(6);
                let mut any = false;
                for c in heading_chips(h, doc, self.today, true) {
                    meta = meta.push(c);
                    any = true;
                }
                if any {
                    parts.push(meta.into());
                }
                if !h.properties.is_empty() {
                    let props = h.properties.iter().map(|(k, v)| {
                        row![
                            text(format!("{k}:"))
                                .size(12)
                                .font(Font::MONOSPACE)
                                .style(dim),
                            text(v.as_str()).size(12).font(Font::MONOSPACE),
                        ]
                        .spacing(8)
                        .into()
                    });
                    parts.push(Column::with_children(props).spacing(2).into());
                }
                parts.push(rule::horizontal(1).into());
            }
        }
        if f.blocks.is_empty() && matches!(f.selected, Selected::Heading(_)) {
            parts.push(
                text("No text under this heading")
                    .size(14)
                    .style(dim)
                    .into(),
            );
        } else {
            parts.push(read::blocks(&f.blocks, ink, Message::Link));
        }
        scrollable(
            Column::with_children(parts)
                .spacing(12)
                .padding([20, 28])
                .max_width(820),
        )
        .height(Fill)
        .width(Fill)
        .into()
    }

    fn ink(&self) -> Ink {
        Ink::of(&style::theme(self.mode))
    }
}

impl OpenFile {
    fn set_document(&mut self, doc: Arc<Document>) {
        self.titles = doc
            .headings()
            .iter()
            .map(|h| title_spans(&h.title))
            .collect();
        // Keep the selection across outside edits when it still exists.
        self.selected = match self.selected {
            Selected::Heading(i) if i < doc.headings().len() => Selected::Heading(i),
            _ if doc.preamble().intro.is_some() => Selected::Intro,
            _ if !doc.headings().is_empty() => Selected::Heading(0),
            _ => Selected::Nothing,
        };
        self.folded.retain(|i| *i < doc.headings().len());
        self.doc = Some(doc);
        self.missing = false;
        self.refresh_blocks();
    }

    fn refresh_blocks(&mut self) {
        let Some(doc) = &self.doc else { return };
        self.blocks = match self.selected {
            Selected::Nothing => Vec::new(),
            Selected::Intro => doc.intro_text().map_or_else(Vec::new, render::blocks),
            Selected::Heading(i) => render::blocks(doc.body_text(i)),
        };
    }
}

/// Waits for the window to settle, then captures it.
fn capture() -> Task<Message> {
    Task::perform(
        async { std::thread::sleep(std::time::Duration::from_millis(1500)) },
        |()| (),
    )
    .then(|()| iced::window::latest())
    .and_then(iced::window::screenshot)
    .map(Message::Shot)
}

/// A heading title's inline markup as spans.
fn title_spans(title: &str) -> Vec<Span> {
    match render::blocks(title).into_iter().next() {
        Some(Block::Paragraph(spans)) => spans,
        _ => vec![Span {
            text: title.to_string(),
            style: render::Style::default(),
            link: None,
        }],
    }
}

/// One outline row: keyword, priority and title; tags and dates on a second
/// line, so a long title keeps the full width.
fn heading_line<'a>(
    h: &'a Heading,
    title: &'a [Span],
    doc: &Document,
    today: NaiveDate,
    ink: Ink,
) -> Element<'a, Message> {
    let done = h
        .keyword
        .as_deref()
        .is_some_and(|k| doc.keywords().is_done(k));
    let mut first = row![].spacing(6).align_y(iced::Center);
    if let Some(k) = &h.keyword {
        first = first.push(chip(k.clone(), if done { Chip::Done } else { Chip::Todo }));
    }
    if let Some(p) = &h.priority {
        first = first.push(chip(format!("#{p}"), Chip::Priority));
    }
    first = first.push(container(read::paragraph(title, ink, Message::Link)).width(Fill));
    let chips = heading_chips(h, doc, today, false);
    if chips.is_empty() {
        return first.into();
    }
    column![first, row(chips).spacing(6).wrap()]
        .spacing(4)
        .into()
}

/// Tag and date chips. `full` adds the keyword, priority, inherited tags and CLOSED.
fn heading_chips<'a>(
    h: &Heading,
    doc: &Document,
    today: NaiveDate,
    full: bool,
) -> Vec<Element<'a, Message>> {
    let done = h
        .keyword
        .as_deref()
        .is_some_and(|k| doc.keywords().is_done(k));
    let mut out = Vec::new();
    if full {
        if let Some(k) = &h.keyword {
            out.push(chip(k.clone(), if done { Chip::Done } else { Chip::Todo }));
        }
        if let Some(p) = &h.priority {
            out.push(chip(format!("#{p}"), Chip::Priority));
        }
    }
    let tags = if full {
        doc.inherited_tags(h.index)
    } else {
        h.tags.clone()
    };
    for t in tags {
        out.push(chip(t, Chip::Tag));
    }
    let dated = |label: &str, ts: &Timestamp, can_be_late: bool| {
        let late = can_be_late && !done && ts.date < today;
        chip(
            format!("{label} {}", short_date(ts, today)),
            if late { Chip::Overdue } else { Chip::Date },
        )
    };
    if let Some(s) = &h.planning.scheduled {
        out.push(dated("Scheduled", s, true));
    }
    if let Some(d) = &h.planning.deadline {
        out.push(dated("Due", d, true));
    }
    // Events: the dedicated timestamp line, else the first stamp in the body.
    let events = if h.dedicated_timestamps.is_empty() {
        h.active_timestamps.iter().take(1).collect::<Vec<_>>()
    } else {
        h.dedicated_timestamps.iter().collect()
    };
    for e in events {
        out.push(chip(short_date(e, today), Chip::Date));
    }
    if full && let Some(c) = &h.planning.closed {
        out.push(dated("Closed", c, false));
    }
    out
}

/// `Nov 23`, `Nov 23 09:00`, `Nov 23, 2027`.
fn short_date(ts: &Timestamp, today: NaiveDate) -> String {
    let mut s = if ts.date.year() == today.year() {
        ts.date.format("%b %-d").to_string()
    } else {
        ts.date.format("%b %-d, %Y").to_string()
    };
    if let Some(t) = ts.time {
        s.push(' ');
        s.push_str(&t.format("%H:%M").to_string());
    }
    if ts.repeats() {
        s.push_str(" ↻");
    }
    s
}

fn chip<'a>(label: String, kind: Chip) -> Element<'a, Message> {
    container(text(label).size(11).font(bold()))
        .padding([1, 6])
        .style(move |t| style::chip(t, kind))
        .into()
}

fn placeholder(msg: &str) -> Element<'_, Message> {
    container(text(msg).size(14).style(dim)).center(Fill).into()
}

fn dim(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(style::dim(theme)),
    }
}

fn bold() -> Font {
    Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    }
}

fn depth_width(depth: usize) -> f32 {
    // Precision loss is irrelevant for an indent of a few levels.
    #[allow(clippy::cast_precision_loss)]
    let d = depth as f32;
    d * 16.0
}

fn indent(depth: usize, step: f32) -> iced::Padding {
    #[allow(clippy::cast_precision_loss)]
    let d = depth as f32;
    iced::Padding {
        top: 4.0,
        bottom: 4.0,
        left: 6.0 + d * step,
        right: 6.0,
    }
}

/// `a/./b/../c.org` → `a/c.org`.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    parts.join("/")
}

/// Opens a web or mail link with the system handler.
fn open_externally(target: &str) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blossom_core::Keywords;

    fn app() -> App {
        let template = OpenVault {
            root: PathBuf::new(),
            keywords: Keywords::default(),
            ignore: vec![],
            index_dir: None,
        };
        App::new(
            None,
            template,
            NaiveDate::from_ymd_opt(2026, 11, 23).unwrap(),
            None,
        )
        .0
    }

    fn doc(s: &str) -> Arc<Document> {
        Arc::new(Document::parse(s, Keywords::default()))
    }

    #[test]
    fn opening_a_file_selects_intro_then_follows_clicks() {
        let mut a = app();
        let _ = a.update(Message::Worker(Event::Opened(PathBuf::from("/v"))));
        assert_eq!(a.title(), "v — Blossom");
        let _ = a.update(Message::SelectFile("a.org".into()));
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("Intro *text*\n* TODO one\nbody of one\n* two\n"),
        )));
        let f = a.file.as_ref().unwrap();
        assert_eq!(f.selected, Selected::Intro);
        assert!(matches!(&f.blocks[..], [Block::Paragraph(_)]));
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let f = a.file.as_ref().unwrap();
        assert_eq!(f.blocks.len(), 1);
        assert_eq!(a.title(), "a.org — v — Blossom");
        // A document for another file is ignored.
        let _ = a.update(Message::Worker(Event::Document(
            "b.org".into(),
            doc("* x\n"),
        )));
        assert_eq!(a.file.as_ref().unwrap().titles.len(), 2);
    }

    #[test]
    fn outside_edit_keeps_selection_when_possible() {
        let mut a = app();
        let _ = a.update(Message::SelectFile("a.org".into()));
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* one\n* two\nold\n"),
        )));
        let _ = a.update(Message::Select(Selected::Heading(1)));
        let _ = a.update(Message::ToggleFold(1));
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* one\n* two\nnew body\n"),
        )));
        let f = a.file.as_ref().unwrap();
        assert_eq!(f.selected, Selected::Heading(1));
        assert!(matches!(&f.blocks[..], [Block::Paragraph(s)] if s[0].text == "new body"));
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* only\n"),
        )));
        assert_eq!(a.file.as_ref().unwrap().selected, Selected::Heading(0));
        assert!(a.file.as_ref().unwrap().folded.is_empty());
        let _ = a.update(Message::Worker(Event::Missing("a.org".into())));
        assert!(a.file.as_ref().unwrap().missing);
    }

    #[test]
    fn file_links_open_files_in_the_vault() {
        let mut a = app();
        a.files = Arc::new(vec![
            FileInfo {
                path: "notes/a.org".into(),
                headings: 1,
                open_tasks: 0,
                conflict: false,
            },
            FileInfo {
                path: "b.org".into(),
                headings: 1,
                open_tasks: 0,
                conflict: false,
            },
        ]);
        let _ = a.update(Message::SelectFile("notes/a.org".into()));
        let _ = a.update(Message::Link("file:../b.org".into()));
        assert_eq!(a.file.as_ref().unwrap().path, "b.org");
        let _ = a.update(Message::Link("id:123".into()));
        assert!(a.status.as_deref().unwrap().contains("not supported"));
        assert_eq!(normalize("a/./b/../c.org"), "a/c.org");
    }

    #[test]
    fn dates_and_folders() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        let ts = |s: &str| Timestamp::parse(s).unwrap();
        assert_eq!(
            short_date(&ts("<2026-11-23 Mon 09:00>"), today),
            "Nov 23 09:00"
        );
        assert_eq!(
            short_date(&ts("<2027-01-02 Sat +1w>"), today),
            "Jan 2, 2027 ↻"
        );
        let mut a = app();
        let _ = a.update(Message::ToggleFolder("x".into()));
        assert!(a.collapsed_folders.contains("x"));
        let _ = a.update(Message::ToggleFolder("x".into()));
        assert!(a.collapsed_folders.is_empty());
    }
}
