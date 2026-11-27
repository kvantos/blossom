//! The application: state, messages, update and view.
//!
//! Three panes: the notebooks tree, the outline of the selected file, and the
//! read view of the selected heading (or the file's top). The read view turns
//! into a section editor on demand. All file access happens in the
//! [`worker`]; this module holds what is on screen and the unsaved text.
//!
//! Every local change goes through [`OpenFile::change`] (undo) and
//! [`App::save_open`] (autosave). Disk versions arrive as worker events and
//! go through [`App::on_disk`], which adopts, merges or reports a conflict.
//!
//! The agenda replaces the outline and read view when chosen in the sidebar.
//! Operations that may touch other files (checking off agenda entries,
//! capture, refile, archive) run in the worker as an [`Op`]; the notice bar
//! then offers to undo them, which restores the files only if nothing
//! changed them since.
//!
//! Images in the read view are read by the worker and cached per open file.
//! A file dropped on the window, or an image pasted with Ctrl/Cmd+V, is
//! stored as an attachment and linked at the cursor (when editing) or at
//! the end of the selected heading. Diagrams open in their own editor in
//! the main area.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blossom_core::editing::{self, ListContinue};
use blossom_core::render::Span;
use blossom_core::{Document, EditError, Heading, PlanningKind, Timestamp};
use blossom_index::agenda::Agenda;
use blossom_vault::Revision;
use blossom_vault::ops::{Change, HeadingRef};
use chrono::{Datelike, NaiveDate, NaiveTime};
use iced::keyboard::{self, Key};
use iced::theme::Mode;
use iced::widget::text_editor::{Action, Edit, Motion};
use iced::widget::{
    Column, button, column, container, pick_list, row, rule, scrollable, space, text, text_editor,
    text_input,
};
use iced::{Element, Fill, Font, Length, Subscription, Task, Theme, font, window};

use crate::agenda;
use crate::draw::{self, DiagramEditor};
use crate::editor::{self, Command};
use crate::file::{DiskCopy, DiskState, Group, OpenFile, SectionEditor, Selected};
use crate::fonts;
use crate::meaning;
use crate::newfile::{self, NewFile, NewFileMsg};
use crate::outline;
use crate::panels::{self, CapturePanel, RefilePicker};
use crate::read::{self, Actions, Ink, Picture};
use crate::search;
use crate::settings::Settings;
use crate::settings_panel::{self, Setting};
use crate::shot;
use crate::state::{self, State};
use crate::style::Colours;
use crate::style::{self, Chip};
use crate::topbar;
use crate::tree::{self, FileInfo};
use crate::worker::{self, Event, Handle, Op, OpenVault, Request, SearchResults};
use blossom_vault::config::{self, Config, edit};

/// How many times a stopped worker is restarted before edits are written
/// directly for the rest of the session.
const WORKER_RESTARTS: u32 = 2;

/// The section editor's widget id, for focusing it.
const EDITOR_ID: &str = "section-editor";

/// Shown in the keyword and priority pickers for "none".
const NONE: &str = "—";

/// The welcome screen's suggested folder, created on Open if missing.
const DEFAULT_FOLDER: &str = "~/Notes";

/// The app.
#[allow(clippy::struct_excessive_bools)] // independent switches
pub struct App {
    /// The system's light/dark mode; `Mode::None` until known.
    system: Mode,
    /// The appearance choices, from the config's `[appearance]`.
    settings: Settings,
    /// The config as last read, to tell what a hand edit changed.
    config: Config,
    /// `config.toml`, which the app watches and writes choices to; `None`
    /// in tests.
    config_path: Option<PathBuf>,
    /// The Settings panel, when open.
    settings_panel: Option<settings_panel::Panel>,
    /// The sidebar is shown.
    sidebar: bool,
    /// The sidebar's "New file" form, when open.
    new_file: Option<NewFile>,
    /// The text of the last kill (C-k, M-d, M-DEL), for C-y.
    kill_ring: String,
    /// The last editor command was a kill, so the next one adds to it.
    kill_append: bool,
    /// The file last open in each folder.
    state: State,
    /// Where `state` is saved; `None` in tests.
    state_path: Option<PathBuf>,
    /// A folder just opened: open its start file once the list arrives.
    choose_start: bool,
    /// The installed fonts, once read for the Settings panel.
    fonts: Option<Arc<fonts::Installed>>,
    worker: Option<Handle>,
    /// A vault to open as soon as the worker is ready.
    pending: Option<OpenVault>,
    /// Used to open a typed-in or picked folder.
    template: OpenVault,
    vault_input: String,
    /// Whether `vault_input` names a folder that already exists.
    vault_exists: bool,
    root: Option<PathBuf>,
    files: Arc<Vec<FileInfo>>,
    collapsed_folders: HashSet<String>,
    file: Option<OpenFile>,
    /// Disk state of files with unsaved or conflicting edits, and of the open file.
    disk: HashMap<String, DiskState>,
    status: Option<String>,
    /// Closing: waiting for the worker to write pending saves.
    closing: bool,
    /// A vault to open once the flushes of the current one are done.
    opening: Option<OpenVault>,
    /// `Flush` requests not yet answered with `Flushed`.
    flushes: u32,
    /// A close was refused once because of an unresolved conflict.
    close_warned: bool,
    today: NaiveDate,
    /// The screenshot developer hook, when on.
    shot: Option<shot::Plan>,
    /// What the main area shows.
    view: View,
    /// The latest agenda from the worker.
    agenda: Option<Arc<Agenda>>,
    /// The search screen's query.
    search_input: String,
    /// Results for `search_input`, once they arrive.
    search: Option<Arc<SearchResults>>,
    /// The search screen's "By meaning" switch.
    search_meaning: bool,
    /// How search by meaning is getting on, once started.
    meaning: Option<meaning::Status>,
    /// What to select once its file has loaded: a heading or the intro.
    pending_heading: Option<(String, Selected)>,
    /// Workers started so far, less one: the worker subscription's id.
    worker_runs: u32,
    /// The worker kept stopping: edits are written directly.
    worker_failed: bool,
    /// After a worker restart: the file to show again and the message.
    restarted: Option<(Option<(String, Selected)>, String)>,
    /// The last operation's message, and its changes while it can be undone.
    notice: Option<(String, Option<Vec<Change>>)>,
    /// The capture panel, while open.
    capture: Option<CapturePanel>,
    /// A capture or diagram save was sent and its result is awaited.
    awaiting: Awaiting,
    /// The refile picker, while open.
    refile: Option<RefilePicker>,
    /// The diagram editor, while open.
    diagram: Option<DiagramEditor>,
}

/// A save whose result closes its panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Awaiting {
    /// Nothing.
    Nothing,
    /// A capture.
    Capture,
    /// A new file.
    NewFile,
    /// A diagram.
    Diagram,
}

/// What the main area shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The outline and read view of a file.
    Files,
    /// The agenda.
    Agenda,
    /// The search screen.
    Search,
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
    /// Open the typed folder, creating it if it doesn't exist.
    OpenTyped,
    /// Show the system folder picker.
    PickFolder,
    /// The folder picker closed.
    Picked(Option<PathBuf>),
    /// `config.toml` changed on disk.
    ConfigChanged,
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
    /// A checkbox in the read view, at this offset of the rendered text.
    ToggleCheckbox(usize),
    /// Open the section editor on the selection.
    StartEditing,
    /// Close the section editor.
    StopEditing,
    /// The editor's cursor, selection or text.
    EditorAction(Action),
    /// One of the editor's own keys.
    EditorCommand(Command),
    /// Undo the last change.
    Undo,
    /// Redo.
    Redo,
    /// Metadata panel: TODO keyword.
    SetKeyword(String),
    /// Metadata panel: priority.
    SetPriority(String),
    /// Metadata panel: a text field changed.
    Input(Field, String),
    /// Metadata panel: apply a text field.
    Apply(Field),
    /// Metadata panel: clear a date.
    ClearDate(PlanningKind),
    /// Conflict: overwrite the disk version with the unsaved edits.
    KeepMine,
    /// Conflict: drop the unsaved edits (they stay in undo).
    UseDisk,
    /// A key nothing else handled.
    Key(keyboard::Event),
    /// The window's close button.
    CloseRequested,
    /// Hide the status line.
    DismissStatus,
    /// The screenshot hook's capture.
    Shot(iced::window::Screenshot),
    /// Show the agenda.
    ShowAgenda,
    /// Show the search screen.
    ShowSearch,
    /// The search field changed.
    SearchInput(String),
    /// The "By meaning" switch changed.
    SearchMeaning(bool),
    /// Download the model for search by meaning.
    DownloadModel,
    /// Stop that download.
    CancelDownload,
    /// Try search by meaning again.
    RetryMeaning,
    /// Open a file at a heading (from the agenda or search).
    OpenHeading(String, usize),
    /// Open a file at its intro, the text before the first heading (from search).
    OpenIntro(String),
    /// Run a file operation in the worker.
    Run(Op),
    /// Undo the last operation.
    UndoOp,
    /// Hide the notice bar.
    DismissNotice,
    /// Open the capture panel.
    OpenCapture,
    /// The Settings panel.
    Setting(Setting),
    /// Show or hide the sidebar.
    ToggleSidebar,
    /// The "New file" form.
    NewFile(NewFileMsg),
    /// Capture: choose a template, by its label.
    CaptureTemplate(String),
    /// Capture: answer prompt N.
    CaptureAnswer(usize, String),
    /// Capture: the editor.
    CaptureAction(Action),
    /// Capture: save.
    CaptureSave,
    /// Capture: close without saving.
    CaptureCancel,
    /// Open the refile picker for the selected heading.
    StartRefile,
    /// Refile: the filter text.
    RefileFilter(String),
    /// Refile: move to target N.
    RefilePick(usize),
    /// Refile: close.
    RefileCancel,
    /// Archive the selected heading.
    Archive,
    /// An image's button in the read view: its link path.
    ImageAction(String),
    /// The clipboard's image as PNG, if it had one.
    Pasted(Option<Arc<Vec<u8>>>),
    /// The clipboard's text, for the editor.
    PasteText(Option<String>),
    /// A file was dropped on the window.
    Dropped(PathBuf),
    /// Start a new diagram for the selected heading.
    NewDiagram,
    /// A change in the diagram editor.
    Diagram(draw::Edit),
    /// Save the diagram.
    DiagramSave,
    /// Close the diagram editor without saving.
    DiagramCancel,
}

/// A metadata panel text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// Tags.
    Tags,
    /// SCHEDULED.
    Scheduled,
    /// DEADLINE.
    Deadline,
}

impl App {
    /// A new app; `open` is the vault to open on start, if known.
    pub fn new(
        open: Option<OpenVault>,
        template: OpenVault,
        today: NaiveDate,
        shot: Option<shot::Plan>,
        (config, config_path): (Config, Option<PathBuf>),
    ) -> (App, Task<Message>) {
        let (settings, problems) = Settings::from_all(&config);
        let forced = shot.as_ref().and_then(|p| p.mode);
        let search_input = shot
            .as_ref()
            .and_then(|p| p.search.clone())
            .unwrap_or_default();
        let search_meaning = shot.as_ref().is_some_and(|p| p.meaning);
        let app = App {
            system: Mode::None,
            settings,
            config,
            config_path,
            settings_panel: None,
            sidebar: true,
            new_file: None,
            kill_ring: String::new(),
            kill_append: false,
            state: State::default(),
            state_path: None,
            choose_start: false,
            fonts: None,
            worker: None,
            vault_exists: open.is_some() || typed_folder_exists(DEFAULT_FOLDER),
            vault_input: open
                .as_ref()
                .map_or_else(|| DEFAULT_FOLDER.into(), |o| o.root.display().to_string()),
            pending: open,
            template,
            root: None,
            files: Arc::new(Vec::new()),
            collapsed_folders: HashSet::new(),
            file: None,
            disk: HashMap::new(),
            status: (!problems.is_empty()).then(|| config_problem(&problems.join("; "))),
            closing: false,
            opening: None,
            flushes: 0,
            close_warned: false,
            today,
            view: if shot.as_ref().is_some_and(|p| p.search.is_some()) {
                View::Search
            } else if shot.as_ref().is_some_and(|p| p.agenda) {
                View::Agenda
            } else {
                View::Files
            },
            shot,
            agenda: None,
            search_input,
            search: None,
            search_meaning,
            meaning: None,
            pending_heading: None,
            worker_runs: 0,
            worker_failed: false,
            restarted: None,
            notice: None,
            capture: None,
            awaiting: Awaiting::Nothing,
            refile: None,
            diagram: None,
        };
        if forced.is_some() {
            return (app, Task::none());
        }
        (app, iced::system::theme().map(Message::Mode))
    }

    /// The app with the state saved at `path` (the last open files).
    #[must_use]
    pub fn with_state(mut self, path: Option<PathBuf>) -> App {
        self.state = path.as_deref().map_or_else(State::default, State::load);
        self.state_path = path;
        self
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

    /// The chosen theme; `None` while following a system mode not yet known.
    pub fn theme(&self) -> Option<Theme> {
        let mode = self.mode();
        (mode != Mode::None).then(|| style::theme(self.colours(), mode))
    }

    /// The light/dark mode in use: the screenshot hook's, the chosen one, or
    /// the system's.
    fn mode(&self) -> Mode {
        self.shot
            .as_ref()
            .and_then(|p| p.mode)
            .unwrap_or_else(|| self.settings.mode.resolve(self.system))
    }

    /// The colours in use: the screenshot hook's or the chosen ones.
    fn colours(&self) -> Colours {
        self.shot
            .as_ref()
            .and_then(|p| p.colours)
            .unwrap_or(self.settings.colours)
    }

    /// The Settings panel's messages.
    fn on_setting(&mut self, message: Setting) -> Task<Message> {
        let mut new = self.settings.clone();
        match message {
            Setting::Open => {
                let semantic = &self.config.semantic;
                self.settings_panel = Some(settings_panel::Panel {
                    section: settings_panel::Section::default(),
                    server: semantic.server.clone().unwrap_or_default(),
                    server_model: semantic.server_model.clone().unwrap_or_default(),
                });
                if self.fonts.is_none() {
                    return Task::perform(async { fonts::installed() }, |i| {
                        Message::Setting(Setting::FontsListed(Arc::new(i)))
                    });
                }
                return Task::none();
            }
            Setting::Close => {
                self.settings_panel = None;
                return Task::none();
            }
            Setting::Section(section) => {
                if let Some(p) = &mut self.settings_panel {
                    p.section = section;
                }
                return Task::none();
            }
            Setting::FontsListed(installed) => {
                self.fonts = Some(installed);
                return Task::none();
            }
            Setting::Model(m) => {
                self.write_config(&[edit::Change::set(Some("semantic"), "model", m.name())]);
                return Task::none();
            }
            Setting::Server(text) => {
                if let Some(p) = &mut self.settings_panel {
                    p.server = text;
                }
                return Task::none();
            }
            Setting::ServerModel(text) => {
                if let Some(p) = &mut self.settings_panel {
                    p.server_model = text;
                }
                return Task::none();
            }
            Setting::ApplyServer => {
                if let Some(p) = &self.settings_panel {
                    let field = |key, value: &str| match value.trim() {
                        "" => edit::Change::unset(Some("semantic"), key),
                        v => edit::Change::set(Some("semantic"), key, v),
                    };
                    let changes = [
                        field("server", &p.server),
                        field("server_model", &p.server_model),
                    ];
                    self.write_config(&changes);
                }
                return Task::none();
            }
            Setting::Download => {
                self.send(Request::DownloadModel);
                return Task::none();
            }
            Setting::Colours(c) => new.colours = c,
            Setting::Mode(m) => new.mode = m,
            Setting::ReadFont(f) => new.read_font = f.0,
            Setting::ReadSize(n) => new.read_size = n,
            Setting::EditorFont(f) => new.editor_font = f.0,
            Setting::EditorSize(n) => new.editor_size = n,
            Setting::Labels(l) => new.top_bar_labels = l,
            Setting::EmacsKeys(on) => new.emacs_keys = on,
        }
        self.set_settings(new);
        Task::none()
    }

    /// Writes `changes` to the config and applies it at once (the watcher
    /// then finds nothing new).
    fn write_config(&mut self, changes: &[edit::Change]) {
        let Some(path) = &self.config_path else {
            return;
        };
        match edit::apply(path, changes) {
            Ok(()) => self.reload_config(),
            Err(e) => self.status = Some(format!("Couldn't save the setting: {e}")),
        }
    }

    /// The Settings panel, when open.
    fn settings_view(&self) -> Option<Element<'_, Message>> {
        use settings_panel::{ModelChoice, SearchState, View};
        let panel = self.settings_panel.as_ref()?;
        let semantic = &self.config.semantic;
        let model = ModelChoice::from_config(semantic.model.as_deref())
            .filter(|m| ModelChoice::offered().contains(m));
        let data_dir = self.template.data_dir.clone().or_else(config::data_dir);
        let downloaded = model
            .and_then(ModelChoice::builtin)
            .zip(data_dir)
            .is_some_and(|(m, dir)| m.is_downloaded(&dir));
        Some(settings_panel::view(View {
            panel,
            settings: &self.settings,
            installed: self.fonts.as_deref(),
            folder: self
                .root
                .as_deref()
                .map(config::contract)
                .or_else(|| self.config.vault.clone()),
            config_path: self.config_path.as_deref().map(config::contract),
            search: SearchState {
                model,
                saved_server: (
                    semantic.server.as_deref().unwrap_or(""),
                    semantic.server_model.as_deref().unwrap_or(""),
                ),
                downloaded,
                status: self.meaning.as_ref(),
                vault_open: self.root.is_some(),
            },
        }))
    }

    /// Uses new Appearance choices and writes them to the config.
    fn set_settings(&mut self, settings: Settings) {
        let changes = settings.changes_from(&self.settings);
        self.settings = settings;
        if let Some(path) = &self.config_path
            && let Err(e) = edit::apply(path, &changes)
        {
            self.status = Some(format!("Couldn't save the appearance settings: {e}"));
        }
    }

    /// The "New file" form's messages.
    fn on_new_file(&mut self, message: NewFileMsg) -> Task<Message> {
        match message {
            NewFileMsg::Open => {
                self.sidebar = true;
                self.new_file = Some(NewFile::default());
                return iced::widget::operation::focus(newfile::NAME);
            }
            NewFileMsg::Name(name) => {
                if let Some(f) = &mut self.new_file {
                    f.name = name;
                }
            }
            NewFileMsg::Title(title) => {
                if let Some(f) = &mut self.new_file {
                    f.title = title;
                }
            }
            NewFileMsg::Create => {
                let Some(f) = &self.new_file else {
                    return Task::none();
                };
                match f.target(&self.files) {
                    Ok(file) => {
                        let text = blossom_vault::ops::new_file_text(&f.title);
                        self.awaiting = Awaiting::NewFile;
                        self.run(Op::CreateFile { file, text });
                    }
                    Err(why) => self.status = Some(why),
                }
            }
            NewFileMsg::Cancel => self.new_file = None,
        }
        Task::none()
    }

    /// The top bar's search field changed: show the search screen for it.
    fn search_typed(&mut self, input: String) {
        self.view = View::Search;
        self.search_input = input;
        self.send_search();
    }

    /// Opens a folder chosen in the app, and remembers it for next time.
    fn open_chosen(&mut self, root: PathBuf) {
        self.remember_vault(&root);
        self.open_folder(root);
    }

    /// Writes a folder opened from the GUI to the config as `vault`, so the
    /// next launch opens it.
    fn remember_vault(&mut self, root: &Path) {
        let Some(path) = &self.config_path else {
            return;
        };
        let value = config::contract(root);
        if self.config.vault.as_deref() == Some(value.as_str()) {
            return;
        }
        match edit::apply(path, &[edit::Change::set(None, "vault", value.as_str())]) {
            // Known now, so the watcher's reload doesn't open it a second time.
            Ok(()) => self.config.vault = Some(value),
            Err(e) => self.status = Some(format!("Couldn't remember the folder: {e}")),
        }
    }

    /// Applies `config.toml` after it changed on disk (a hand edit, or the
    /// app's own write, which changes nothing here). Appearance applies at
    /// once; a new `vault` is opened; other changes reopen the vault with
    /// them, keeping the open file. A file that doesn't parse is reported and
    /// the settings in use stay.
    fn reload_config(&mut self) {
        let Some(path) = &self.config_path else {
            return;
        };
        let config = match Config::load(path) {
            Ok(c) => c,
            Err(e) => {
                self.status = Some(config_problem(&e));
                return;
            }
        };
        if config == self.config {
            return;
        }
        let (settings, mut problems) = Settings::from_all(&config);
        self.settings = settings;
        if let Err(e) = config.templates() {
            problems.push(format!("{e}; using the built-in capture templates"));
        }
        self.status = (!problems.is_empty()).then(|| config_problem(&problems.join("; ")));
        let template = OpenVault {
            index_dir: self.template.index_dir.clone(),
            data_dir: self.template.data_dir.clone(),
            ..OpenVault::new(PathBuf::new(), &config)
        };
        let new_vault = config.vault != self.config.vault;
        self.config = config;
        let changed = template != self.template;
        self.template = template;
        let root = match (&self.config.vault, &self.root) {
            (Some(v), _) if new_vault => Some(config::expand(v)),
            (_, Some(root)) if changed => Some(root.clone()),
            _ => None,
        };
        if let Some(root) = root {
            if self.root.as_ref() == Some(&root) {
                let file = self.file.as_ref().map(|f| (f.path.clone(), f.selected));
                let message = self
                    .status
                    .clone()
                    .unwrap_or_else(|| "Applied the changes to config.toml.".to_string());
                self.restarted = Some((file, message));
            }
            self.open_folder(root);
        }
    }

    /// The worker, system theme changes, keys and close requests.
    pub fn subscription(app: &Self) -> Subscription<Message> {
        Subscription::batch([
            worker::subscription(app.worker_runs).map(Message::Worker),
            iced::system::theme_changes().map(Message::Mode),
            match &app.config_path {
                Some(path) => watch_config(path.clone()),
                None => Subscription::none(),
            },
            keyboard::listen().map(Message::Key),
            window::close_requests().map(|_| Message::CloseRequested),
            iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Window(window::Event::FileDropped(path)) => {
                    Some(Message::Dropped(path))
                }
                _ => None,
            }),
        ])
    }

    /// Applies a message, then asks the worker for any image the read view
    /// shows that isn't loaded yet.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Kills in a row add up; anything else in the editor ends the run.
        if matches!(
            message,
            Message::EditorAction(_) | Message::EditorCommand(_)
        ) && !matches!(message, Message::EditorCommand(Command::Kill(_)))
        {
            self.kill_append = false;
        }
        let task = self.update_inner(message);
        self.request_pictures();
        task
    }

    fn request_pictures(&mut self) {
        let (Some(f), Some(w)) = (&mut self.file, &self.worker) else {
            return;
        };
        for path in read::image_paths(&f.blocks) {
            if !f.pictures.contains_key(&path) {
                f.pictures.insert(path.clone(), Picture::Loading);
                w.send(Request::LoadImage {
                    file: f.path.clone(),
                    path,
                });
            }
        }
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Worker(event) => return self.on_worker(event),
            Message::Mode(mode) => self.system = mode,
            Message::Setting(m) => return self.on_setting(m),
            Message::ToggleSidebar => self.sidebar = !self.sidebar,
            Message::NewFile(m) => return self.on_new_file(m),
            Message::VaultInput(s) => {
                self.vault_input = s;
                self.vault_exists = typed_folder_exists(&self.vault_input);
            }
            Message::ConfigChanged => self.reload_config(),
            Message::OpenTyped => match create_folder(&self.vault_input) {
                Ok(Some(root)) => self.open_chosen(root),
                Ok(None) => {}
                Err(e) => self.status = Some(e),
            },
            Message::PickFolder => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Open a folder of .org files")
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::Picked,
                );
            }
            Message::Picked(Some(root)) => self.open_chosen(root),
            Message::Picked(None) => {}
            Message::ToggleFolder(path) => {
                if !self.collapsed_folders.remove(&path) {
                    self.collapsed_folders.insert(path);
                }
            }
            Message::SelectFile(path) => {
                self.view = View::Files;
                self.select_file(path);
            }
            Message::ToggleFold(i) => {
                if let Some(f) = &mut self.file
                    && !f.folded.remove(&i)
                {
                    f.folded.insert(i);
                }
            }
            Message::Select(sel) => {
                if let Some(f) = &mut self.file {
                    f.editor = None;
                    f.selected = sel;
                    f.refresh_blocks();
                    f.fill_inputs();
                }
            }
            Message::Link(target) => self.follow_link(&target),
            Message::ToggleCheckbox(offset) => {
                let base = self.file.as_ref().and_then(OpenFile::blocks_base);
                if let Some(base) = base {
                    self.edit_doc(|d| d.toggle_checkbox(base + offset));
                }
            }
            Message::StartEditing => return self.start_editing(),
            Message::StopEditing | Message::EditorCommand(Command::Done) => self.stop_editing(),
            Message::EditorAction(action) => self.editor_action(action),
            Message::EditorCommand(Command::Enter) => self.enter(),
            Message::EditorCommand(c @ (Command::Kill(_) | Command::Yank)) => self.kill_or_yank(c),
            Message::EditorCommand(Command::Paste) => return paste(),
            Message::EditorCommand(Command::Undo) | Message::Undo => self.undo(false),
            Message::EditorCommand(Command::Redo) | Message::Redo => self.undo(true),
            Message::SetKeyword(k) => {
                let now = chrono::Local::now().naive_local();
                let k = (k != NONE).then_some(k);
                self.edit_heading(|d, i| d.change_keyword(i, k.as_deref(), now));
            }
            Message::SetPriority(p) => {
                let p = (p != NONE).then_some(p);
                self.edit_heading(|d, i| d.set_priority(i, p.as_deref()));
            }
            Message::Input(field, value) => {
                if let Some(f) = &mut self.file {
                    *match field {
                        Field::Tags => &mut f.inputs.tags,
                        Field::Scheduled => &mut f.inputs.scheduled,
                        Field::Deadline => &mut f.inputs.deadline,
                    } = value;
                }
            }
            Message::Apply(field) => self.apply_field(field),
            Message::ClearDate(kind) => self.set_date(kind, None),
            Message::KeepMine => self.keep_mine(),
            Message::UseDisk => self.use_disk(),
            Message::Key(event) => return self.on_key(&event),
            Message::CloseRequested => return self.close(),
            Message::DismissStatus => self.status = None,
            other => return self.update_ops(other),
        }
        Task::none()
    }

    /// The agenda, capture, refile and archive messages, and the screenshot
    /// ([`App::update`] handles the rest).
    fn update_ops(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ShowAgenda => {
                self.view = View::Agenda;
                if let Some(w) = &self.worker {
                    w.send(Request::Agenda(agenda::DAYS));
                }
            }
            Message::ShowSearch => {
                self.view = View::Search;
                self.send_search();
                return iced::widget::operation::focus(search::INPUT);
            }
            Message::SearchInput(s) => self.search_typed(s),
            Message::SearchMeaning(on) => {
                self.search_meaning = on;
                self.send_search();
                return iced::widget::operation::focus(search::INPUT);
            }
            Message::DownloadModel => self.send(Request::DownloadModel),
            Message::CancelDownload => self.send(Request::CancelDownload),
            Message::RetryMeaning => self.send(Request::RetryMeaning),
            Message::OpenHeading(path, index) => self.open_at(path, Selected::Heading(index)),
            Message::OpenIntro(path) => self.open_at(path, Selected::Intro),
            Message::Run(op) => self.run(op),
            Message::UndoOp => {
                if let Some((_, Some(changes))) = self.notice.take()
                    && let Some(w) = &self.worker
                {
                    w.send(Request::Restore(changes));
                }
            }
            Message::DismissNotice => self.notice = None,
            Message::OpenCapture => return self.open_capture(),
            Message::CaptureTemplate(label) => {
                let templates = &self.template.templates;
                if let Some(p) = &mut self.capture
                    && let Some(i) = templates
                        .iter()
                        .position(|t| format!("{} — {}", t.key, t.name) == label)
                {
                    p.choose(templates, i);
                }
            }
            Message::CaptureAnswer(i, v) => {
                if let Some(p) = &mut self.capture {
                    if let Some(a) = p.answers.get_mut(i) {
                        *a = v;
                    }
                    p.expand(&self.template.templates);
                }
            }
            Message::CaptureAction(action) => {
                if let Some(p) = &mut self.capture {
                    p.edited |= action.is_edit();
                    p.content.perform(action);
                }
            }
            Message::CaptureSave => self.save_capture(),
            Message::CaptureCancel => {
                self.capture = None;
                self.awaiting = Awaiting::Nothing;
            }
            Message::StartRefile => return self.start_refile(),
            Message::RefileFilter(f) => {
                if let Some(r) = &mut self.refile {
                    r.filter = f;
                }
            }
            Message::RefilePick(i) => {
                if let Some(r) = self.refile.take()
                    && let Some(t) = r.targets.as_ref().and_then(|all| all.get(i))
                {
                    self.run(Op::Refile {
                        heading: r.heading,
                        file: t.path.clone(),
                        under: t.heading.clone(),
                    });
                }
            }
            Message::RefileCancel => self.refile = None,
            Message::Archive => {
                if let Some(h) = self.selected_ref() {
                    self.run(Op::Archive(h));
                }
            }
            Message::Shot(shot) => {
                if let Some(plan) = &self.shot
                    && let Err(e) =
                        shot::write_png(&plan.out, shot.size.width, shot.size.height, &shot.rgba)
                {
                    eprintln!("blossom: screenshot: {e}");
                }
                // Through the normal close, so the worker saves first.
                return self.close();
            }
            other => return self.update_files(other),
        }
        Task::none()
    }

    /// Images, paste, drop and diagram messages.
    fn update_files(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ImageAction(path) => self.image_action(path),
            Message::Pasted(Some(bytes)) => {
                if let Some(f) = self.file.as_ref().filter(|f| f.doc.is_some() && !f.missing)
                    && let Some(w) = &self.worker
                {
                    let name = chrono::Local::now()
                        .format("pasted-%Y%m%d-%H%M%S.png")
                        .to_string();
                    w.send(Request::Attach {
                        file: f.path.clone(),
                        name,
                        bytes,
                    });
                }
            }
            Message::Pasted(None) => {
                if self.file.as_ref().is_some_and(|f| f.editor.is_some()) {
                    return iced::clipboard::read().map(Message::PasteText);
                }
            }
            Message::PasteText(Some(t)) => {
                self.editor_action(Action::Edit(Edit::Paste(Arc::new(t))));
            }
            Message::Dropped(source) => {
                match self.file.as_ref().filter(|f| f.doc.is_some() && !f.missing) {
                    Some(f) => {
                        if let Some(w) = &self.worker {
                            w.send(Request::AttachFile {
                                file: f.path.clone(),
                                source,
                            });
                        }
                    }
                    None => self.status = Some("Open a file to attach to first".into()),
                }
            }
            Message::NewDiagram => {
                if let Some(f) = self.file.as_ref().filter(|f| f.doc.is_some() && !f.missing) {
                    self.diagram = Some(DiagramEditor::new(
                        f.path.clone(),
                        None,
                        None,
                        crate::excalidraw::Scene::new(),
                    ));
                    self.awaiting = Awaiting::Nothing;
                }
            }
            Message::Diagram(edit) => {
                if let Some(d) = &mut self.diagram {
                    d.update(edit);
                }
            }
            Message::DiagramSave => {
                if let Some(d) = &self.diagram
                    && let Some(w) = &self.worker
                {
                    w.send(Request::SaveDiagram {
                        file: d.file.clone(),
                        path: d.link.clone(),
                        base: d.base,
                        source: Arc::new(d.scene.to_json()),
                        svg: Arc::new(crate::excalidraw::to_svg(&d.scene)),
                    });
                    self.awaiting = Awaiting::Diagram;
                }
            }
            Message::DiagramCancel => {
                if let Some(d) = &mut self.diagram
                    && d.changed()
                    && !d.discard_warned
                {
                    d.discard_warned = true;
                    self.status = Some(
                        "The diagram has unsaved changes: Cancel again to discard them".into(),
                    );
                } else {
                    self.diagram = None;
                    self.awaiting = Awaiting::Nothing;
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn open_folder(&mut self, root: PathBuf) {
        self.vault_input = root.display().to_string();
        let open = OpenVault {
            root,
            ..self.template.clone()
        };
        if self.worker.is_none() {
            self.pending = Some(open);
        } else if self.disk.values().any(DiskState::dirty) {
            // Unsaved edits are written (and merged, if another program
            // changed the file) before the old vault goes.
            self.opening = Some(open);
            self.flush();
        } else {
            self.send(Request::Open(open));
        }
    }

    /// Asks the worker to write every waiting save.
    fn flush(&mut self) {
        if let Some(w) = &self.worker {
            self.flushes += 1;
            w.send(Request::Flush);
        }
    }

    /// Sends a save; while closing or switching vaults, flushes it at once,
    /// so it is written before the app exits or the vault closes.
    fn send_save(&mut self, request: Request) {
        if self.worker_failed {
            if let Request::Save { path, .. } = &request
                && let Some(problem) = self.write_direct(path)
            {
                self.status = Some(problem);
            }
            return;
        }
        self.send(request);
        if self.closing || self.opening.is_some() {
            self.flush();
        }
    }

    /// Writes `path`'s unsaved text without the worker, with the stale-write
    /// guard. When the file changed on disk (or conflicts), the text goes to
    /// `path.unsaved-YYYYMMDD-HHMMSS` beside it instead. Says what went wrong.
    fn write_direct(&mut self, path: &str) -> Option<String> {
        let root = self.root.clone()?;
        let ds = self.disk.get_mut(path).filter(|d| d.dirty())?;
        let vault = match blossom_vault::Vault::open(
            &root,
            self.template.keywords.clone(),
            &self.template.ignore,
        ) {
            Ok(v) => v,
            Err(e) => return Some(format!("Couldn't save {path}: {e}")),
        };
        if ds.conflict.is_none()
            && let Ok(rev) = vault.write(path, &ds.latest, ds.base)
        {
            ds.base = Some(rev);
            ds.base_text = Arc::clone(&ds.latest);
            return None;
        }
        let copy = format!(
            "{path}.unsaved-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
        Some(match vault.write(&copy, &ds.latest, None) {
            Ok(_) => format!("{path} changed on disk; your unsaved text is in {copy}"),
            Err(e) => format!("Couldn't save {path}: {e}"),
        })
    }

    /// The worker stopped on an internal error: writes the unsaved edits
    /// directly, then starts a new worker (at most [`WORKER_RESTARTS`]
    /// times; after that edits keep being written directly).
    fn on_worker_stopped(&mut self, why: &str) -> Task<Message> {
        self.worker = None;
        self.flushes = 0;
        let paths: Vec<String> = self.disk.keys().cloned().collect();
        let problems: Vec<String> = paths.iter().filter_map(|p| self.write_direct(p)).collect();
        let saved = if problems.is_empty() {
            "unsaved edits were written".to_string()
        } else {
            problems.join("; ")
        };
        if self.closing {
            return iced::exit();
        }
        if self.worker_runs < WORKER_RESTARTS {
            self.worker_runs += 1;
            let root = self
                .opening
                .take()
                .map(|o| o.root)
                .or_else(|| self.root.clone());
            if let Some(root) = root {
                self.pending = Some(OpenVault {
                    root,
                    ..self.template.clone()
                });
            }
            let file = self.file.as_ref().map(|f| (f.path.clone(), f.selected));
            let message =
                format!("Blossom's background worker stopped ({why}) and was restarted; {saved}");
            self.status = Some(message.clone());
            self.restarted = Some((file, message));
        } else {
            self.worker_failed = true;
            self.opening = None;
            self.status = Some(format!(
                "Blossom's background worker keeps stopping ({why}); {saved}. Edits are now saved directly; please restart Blossom"
            ));
        }
        Task::none()
    }

    fn select_file(&mut self, path: String) {
        if self.file.as_ref().is_some_and(|f| f.path == path) {
            return;
        }
        self.choose_start = false;
        if let Some(root) = &self.root
            && self.state.set_last_file(root, &path)
            && let Some(state_path) = &self.state_path
            && let Err(e) = self.state.save(state_path)
        {
            eprintln!("blossom: {e}");
        }
        if let Some(w) = &self.worker {
            w.send(Request::Load(path.clone()));
        }
        self.file = Some(OpenFile::loading(path));
    }

    /// A vault is open: everything shown for the old one goes; after a
    /// worker restart, the file that was open comes back.
    fn on_opened(&mut self, root: PathBuf) {
        if self.view == View::Agenda
            && let Some(w) = &self.worker
        {
            w.send(Request::Agenda(agenda::DAYS));
        }
        self.agenda = None;
        self.search = None;
        self.meaning = None;
        if self.view == View::Search {
            self.send_search();
        }
        self.notice = None;
        self.vault_input = root.display().to_string();
        self.root = Some(root);
        self.file = None;
        self.disk.clear();
        self.collapsed_folders.clear();
        self.status = None;
        if let Some((file, message)) = self.restarted.take() {
            self.status = Some(message);
            if let Some((path, sel)) = file {
                self.pending_heading = Some((path.clone(), sel));
                self.select_file(path);
            }
        }
        // Not after a restart, which brought its own file back.
        self.choose_start = self.file.is_none();
    }

    fn on_worker(&mut self, event: Event) -> Task<Message> {
        match event {
            Event::Ready(handle) => {
                if let Some(open) = self.pending.take() {
                    handle.send(Request::Open(open));
                }
                self.worker = Some(handle);
            }
            Event::Opened(root) => self.on_opened(root),
            Event::Files(files) => {
                self.files = files;
                if std::mem::take(&mut self.choose_start)
                    && self.shot.is_none()
                    && self.file.is_none()
                    && let Some(root) = &self.root
                    && let Some(path) = state::start_file(
                        self.state.last_file(root),
                        &self.config.inbox(),
                        &self.files,
                    )
                {
                    self.select_file(path);
                }
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
            Event::Document(path, doc, rev) => {
                let was_loading = self
                    .file
                    .as_ref()
                    .is_some_and(|f| f.path == path && f.doc.is_none());
                self.on_disk(path.clone(), Some((doc, rev)));
                self.select_pending(&path);
                if was_loading
                    && self.view == View::Files
                    && let Some(plan) = self.shot.clone()
                {
                    return self.shoot(&plan);
                }
            }
            Event::Missing(path) => self.on_disk(path, None),
            Event::Saved {
                path,
                text,
                revision,
            } => {
                if let Some(ds) = self.disk.get_mut(&path) {
                    ds.base = Some(revision);
                    ds.base_text = text;
                    if !ds.dirty() && self.file.as_ref().is_none_or(|f| f.path != path) {
                        self.disk.remove(&path);
                    }
                }
            }
            Event::Stale { path, disk } => self.on_disk(path, disk),
            Event::Flushed => {
                self.flushes = self.flushes.saturating_sub(1);
                if self.flushes == 0 {
                    if let Some(open) = self.opening.take() {
                        self.send(Request::Open(open));
                    }
                    if self.closing {
                        return iced::exit();
                    }
                }
            }
            Event::Agenda(_)
            | Event::Search(_)
            | Event::Meaning(_)
            | Event::Targets(_)
            | Event::Done(_)
            | Event::Restored(_) => {
                return self.on_op_event(event);
            }
            Event::Image { .. }
            | Event::Attached { .. }
            | Event::Diagram { .. }
            | Event::DiagramSaved { .. } => return self.on_file_event(event),
            Event::Error(e) => {
                self.awaiting = Awaiting::Nothing;
                self.status = Some(e);
            }
            Event::Stopped(why) => return self.on_worker_stopped(&why),
        }
        Task::none()
    }

    /// Worker events about the agenda and operations.
    fn on_op_event(&mut self, event: Event) -> Task<Message> {
        match event {
            Event::Agenda(a) => {
                let first = self.agenda.is_none();
                self.agenda = Some(a);
                if first
                    && self.view == View::Agenda
                    && let Some(plan) = self.shot.clone()
                {
                    return self.shoot(&plan);
                }
            }
            Event::Meaning(status) => {
                let first = self.meaning.is_none();
                let waiting = matches!(
                    status,
                    meaning::Status::NeedsDownload { .. } | meaning::Status::Failed(_)
                );
                self.meaning = Some(status);
                if first
                    && waiting
                    && self.view == View::Search
                    && let Some(plan) = self.shot.clone()
                {
                    return self.shoot(&plan);
                }
            }
            Event::Search(r) => {
                if r.query == self.search_input.trim() && r.meaning == self.search_meaning {
                    // By meaning, early answers may be empty while it reads.
                    let first = self.search.is_none()
                        || (r.meaning
                            && self.search.as_ref().is_some_and(|s| s.notes.is_empty())
                            && !r.notes.is_empty());
                    self.search = Some(r);
                    if first
                        && self.view == View::Search
                        && let Some(plan) = self.shot.clone()
                    {
                        return self.shoot(&plan);
                    }
                }
            }
            Event::Targets(t) => {
                if let Some(r) = &mut self.refile {
                    r.targets = Some(t);
                }
            }
            Event::Done(out) => {
                if self.awaiting == Awaiting::Capture {
                    self.awaiting = Awaiting::Nothing;
                    self.capture = None;
                }
                if self.awaiting == Awaiting::NewFile {
                    self.awaiting = Awaiting::Nothing;
                    self.new_file = None;
                    self.view = View::Files;
                    self.select_file(out.path.clone());
                }
                self.notice = Some((out.message.clone(), Some(out.changes.clone())));
            }
            Event::Restored(msg) => self.notice = Some((msg, None)),
            _ => {}
        }
        Task::none()
    }

    /// Worker events about images, attachments and diagrams.
    fn on_file_event(&mut self, event: Event) -> Task<Message> {
        let mut shot_task = None;
        match event {
            Event::Image { file, path, data } => {
                if let Some(f) = self.file.as_mut().filter(|f| f.path == file) {
                    let picture = match data {
                        None => Picture::Missing,
                        Some(bytes) if is_svg(&path) => {
                            Picture::Svg(iced::widget::svg::Handle::from_memory((*bytes).clone()))
                        }
                        Some(bytes) => Picture::Raster(iced::widget::image::Handle::from_bytes(
                            (*bytes).clone(),
                        )),
                    };
                    f.pictures.insert(path, picture);
                }
            }
            Event::Attached { file, link } => {
                if self.awaiting == Awaiting::Diagram {
                    self.awaiting = Awaiting::Nothing;
                    self.diagram = None;
                }
                self.insert_link(&file, &link);
            }
            Event::Diagram {
                file,
                path,
                scene,
                base,
            } => {
                if self.shot.as_ref().is_some_and(|p| p.diagram.is_some()) {
                    shot_task = Some(capture());
                }
                self.diagram = Some(DiagramEditor::new(file, Some(path), base, *scene));
                self.awaiting = Awaiting::Nothing;
            }
            Event::DiagramSaved { file, path } => {
                self.diagram = None;
                self.awaiting = Awaiting::Nothing;
                if let Some(f) = self.file.as_mut().filter(|f| f.path == file) {
                    f.pictures.remove(&path);
                }
                self.notice = Some((format!("Saved {path}"), None));
            }
            _ => {}
        }
        shot_task.unwrap_or_else(Task::none)
    }

    /// Links an attachment of `file`: at the editor's cursor, else at the
    /// end of the selected heading (or the top of the file).
    fn insert_link(&mut self, file: &str, link: &str) {
        let text = format!("[[file:{link}]]");
        let Some(f) = self.file.as_ref().filter(|f| f.path == file) else {
            self.status = Some(format!("Attached {link} to {file}; it isn't linked yet"));
            return;
        };
        if f.editor.is_some() {
            self.editor_action(Action::Edit(Edit::Paste(Arc::new(text))));
            return;
        }
        let at = match f.selected {
            Selected::Heading(i) => Some(i),
            _ => None,
        };
        self.edit_doc(|d| d.append_line(at, &text).map(drop));
        self.notice = Some((format!("Attached {link}"), None));
    }

    /// An image's button: edit a diagram, open anything else.
    fn image_action(&mut self, path: String) {
        let Some(f) = &self.file else { return };
        if is_svg(&path) {
            if let Some(w) = &self.worker {
                w.send(Request::OpenDiagram {
                    file: f.path.clone(),
                    path,
                });
            }
            return;
        }
        let Some(root) = &self.root else { return };
        let full = blossom_vault::attach::resolve(root, &f.path, &path);
        if let Err(e) = open_externally(&full.to_string_lossy()) {
            self.status = Some(format!("Couldn't open {path}: {e}"));
        }
    }

    /// Sends `request` to the worker, if it is running.
    fn send(&self, request: Request) {
        if let Some(w) = &self.worker {
            w.send(request);
        }
    }

    /// Ask the worker for results for the search field (and to keep them
    /// fresh); results for older input are ignored when they arrive.
    fn send_search(&mut self) {
        let query = self.search_input.trim();
        let meaning = self.search_meaning;
        if self
            .search
            .as_ref()
            .is_some_and(|r| r.query != query || r.meaning != meaning)
        {
            self.search = None;
        }
        if let Some(w) = &self.worker {
            w.send(Request::Search {
                query: query.to_string(),
                meaning,
            });
        }
    }

    /// Selects what [`App::open_at`] asked for, once `path` has loaded.
    fn select_pending(&mut self, path: &str) {
        let Some((p, sel)) = self.pending_heading.take() else {
            return;
        };
        if p != path {
            self.pending_heading = Some((p, sel));
        } else if let Some(f) = self.file.as_mut().filter(|f| f.path == path)
            && f.doc.as_ref().is_some_and(|d| match sel {
                Selected::Heading(i) => i < d.headings().len(),
                _ => true,
            })
        {
            f.selected = sel;
            f.refresh_blocks();
            f.fill_inputs();
        }
    }

    fn open_at(&mut self, path: String, sel: Selected) {
        self.view = View::Files;
        if let Some(f) = &mut self.file
            && f.path == path
            && f.doc.is_some()
        {
            f.editor = None;
            f.selected = sel;
            f.refresh_blocks();
            f.fill_inputs();
            return;
        }
        self.pending_heading = Some((path.clone(), sel));
        self.select_file(path);
    }

    /// Sends `op` to the worker, unless a file it touches has a save conflict.
    fn run(&mut self, op: Op) {
        let paths: Vec<&str> = match &op {
            Op::Complete(h, _) | Op::Archive(h) => vec![h.path.as_str()],
            Op::Refile { heading, file, .. } => vec![heading.path.as_str(), file.as_str()],
            Op::Capture { file, .. } | Op::CreateFile { file, .. } => vec![file.as_str()],
        };
        if let Some(p) = paths
            .iter()
            .find(|p| self.disk.get(**p).is_some_and(|d| d.conflict.is_some()))
        {
            self.status = Some(format!(
                "{p} has edits that conflict with the disk version; choose a version first"
            ));
            return;
        }
        if let Some(w) = &self.worker {
            w.send(Request::Run(op));
        }
    }

    /// The selected heading of the open file, for refile and archive.
    fn selected_ref(&self) -> Option<HeadingRef> {
        let f = self.file.as_ref()?;
        let Selected::Heading(i) = f.selected else {
            return None;
        };
        let h = f.doc.as_ref()?.heading(i)?;
        Some(HeadingRef {
            path: f.path.clone(),
            index: i,
            title: h.title.clone(),
        })
    }

    fn open_capture(&mut self) -> Task<Message> {
        if self.template.templates.is_empty() {
            self.status = Some("No capture templates are configured".into());
            return Task::none();
        }
        let now = chrono::Local::now().naive_local();
        self.capture = Some(CapturePanel::new(&self.template.templates, 0, now));
        self.awaiting = Awaiting::Nothing;
        iced::widget::operation::focus(panels::CAPTURE_EDITOR)
    }

    fn save_capture(&mut self) {
        let Some(p) = &self.capture else { return };
        let Some(t) = self.template.templates.get(p.template) else {
            return;
        };
        let entry = p.entry();
        if entry.trim().is_empty() {
            self.status = Some("Nothing to capture".into());
            return;
        }
        let op = Op::Capture {
            file: t.file.clone(),
            target: t.target.clone(),
            entry,
        };
        self.awaiting = Awaiting::Capture;
        self.run(op);
    }

    fn start_refile(&mut self) -> Task<Message> {
        let Some(heading) = self.selected_ref() else {
            return Task::none();
        };
        if let Some(w) = &self.worker {
            w.send(Request::Targets);
        }
        self.refile = Some(RefilePicker {
            heading,
            filter: String::new(),
            targets: None,
        });
        iced::widget::operation::focus(panels::REFILE_INPUT)
    }

    /// A disk version of `path` (or its absence) arrived: from loading,
    /// an outside change, or a refused save.
    fn on_disk(&mut self, path: String, disk: Option<(Arc<Document>, Revision)>) {
        let open = self.file.as_mut().filter(|f| f.path == path);
        let state = self.disk.get(&path).cloned();
        let dirty = state.as_ref().is_some_and(DiskState::dirty);
        let Some((doc, rev)) = disk else {
            if let Some(mut ds) = state.filter(|_| dirty) {
                ds.conflict = Some(DiskCopy::Deleted);
                self.disk.insert(path.clone(), ds);
                if open.is_none() {
                    self.status = Some(format!(
                        "{path} was deleted while it had unsaved edits; open it to choose"
                    ));
                }
            } else {
                self.disk.remove(&path);
                if let Some(f) = open {
                    f.missing = true;
                    f.editor = None;
                }
            }
            return;
        };
        let Some(mut ds) = state.filter(|_| dirty) else {
            if open.is_some() {
                self.disk
                    .insert(path.clone(), DiskState::clean(Some(rev), doc.text()));
            }
            if let Some(f) = open {
                f.adopt(doc);
            }
            return;
        };
        if ds.base == Some(rev) {
            // Nothing new on disk: show the unsaved text if it isn't yet.
            if let Some(f) = open.filter(|f| f.doc.is_none()) {
                f.show(Arc::new(Document::parse(
                    ds.latest.as_str(),
                    doc.keywords().clone(),
                )));
            }
            return;
        }
        if let Some(m) = editing::merge3(&ds.base_text, &ds.latest, doc.text()) {
            ds.base = Some(rev);
            ds.base_text = Arc::new(doc.text().to_string());
            ds.latest = Arc::new(m.text.clone());
            ds.conflict = None;
            let save = Request::Save {
                path: path.clone(),
                text: Arc::clone(&ds.latest),
                base: ds.base,
            };
            if let Some(f) = open {
                let merged = Document::parse(m.text.clone(), doc.keywords().clone());
                if f.doc.is_none() {
                    f.show(Arc::new(merged));
                } else {
                    f.merged(merged, &m);
                }
            }
            self.disk.insert(path, ds);
            self.send_save(save);
        } else {
            if let Some(f) = open.filter(|f| f.doc.is_none()) {
                f.show(Arc::new(Document::parse(
                    ds.latest.as_str(),
                    doc.keywords().clone(),
                )));
            } else if self.file.as_ref().is_none_or(|f| f.path != path) {
                self.status = Some(format!(
                    "{path} changed on disk and conflicts with unsaved edits; open it to choose"
                ));
            }
            ds.conflict = Some(DiskCopy::Changed(doc, rev));
            // A conflict cancels closing (and opening another vault): the
            // user must choose first.
            self.closing = false;
            if self.opening.take().is_some() {
                self.status = Some(format!(
                    "{path} changed on disk and conflicts with unsaved edits; choose a version, then open the folder again"
                ));
            }
            self.disk.insert(path, ds);
        }
    }

    /// Sends the open file's working text to be saved.
    fn save_open(&mut self) {
        let Some(f) = &self.file else { return };
        let Some(doc) = &f.doc else { return };
        let ds = self
            .disk
            .entry(f.path.clone())
            .or_insert_with(|| DiskState::clean(None, ""));
        if ds.latest.as_str() == doc.text() {
            return;
        }
        ds.latest = Arc::new(doc.text().to_string());
        if ds.conflict.is_none() {
            let save = Request::Save {
                path: f.path.clone(),
                text: Arc::clone(&ds.latest),
                base: ds.base,
            };
            self.send_save(save);
        }
    }

    /// Applies `edit` to the open document as one undo step.
    fn edit_doc(&mut self, edit: impl FnOnce(&mut Document) -> Result<(), EditError>) {
        let Some(f) = &mut self.file else { return };
        let Some(doc) = &f.doc else { return };
        let mut d = (**doc).clone();
        if let Err(e) = edit(&mut d) {
            self.status = Some(e.to_string());
            return;
        }
        if d.text() == doc.text() {
            return;
        }
        let before = f.snapshot();
        f.change(before, d, Group::Single);
        self.save_open();
    }

    /// Applies `edit` to the selected heading, keeping its editor in step.
    fn edit_heading(&mut self, edit: impl FnOnce(&mut Document, usize) -> Result<(), EditError>) {
        let Some(f) = &mut self.file else { return };
        let (Selected::Heading(i), Some(doc)) = (f.selected, &f.doc) else {
            return;
        };
        let mut d = (**doc).clone();
        if let Err(e) = edit(&mut d, i) {
            self.status = Some(e.to_string());
            return;
        }
        if d.text() == doc.text() {
            return;
        }
        let before = f.snapshot();
        if let (Some(ed), Some(range)) = (f.editor.as_mut(), d.section_range(i)) {
            let cursor = ed.content.cursor();
            *ed = SectionEditor::new(&d, range);
            ed.content.move_to(cursor);
        }
        f.change(before, d, Group::Single);
        f.fill_inputs();
        self.save_open();
    }

    fn apply_field(&mut self, field: Field) {
        let Some(f) = &self.file else { return };
        match field {
            Field::Tags => {
                let tags: Vec<String> = f
                    .inputs
                    .tags
                    .split(|c: char| c.is_whitespace() || c == ':' || c == ',')
                    .filter(|t| !t.is_empty())
                    .map(String::from)
                    .collect();
                self.edit_heading(|d, i| d.set_tags(i, &tags));
            }
            Field::Scheduled | Field::Deadline => {
                let (kind, input) = if field == Field::Scheduled {
                    (PlanningKind::Scheduled, &f.inputs.scheduled)
                } else {
                    (PlanningKind::Deadline, &f.inputs.deadline)
                };
                match parse_when(input, self.today) {
                    Ok(ts) => self.set_date(kind, ts.as_ref()),
                    Err(()) => {
                        self.status = Some(format!(
                            "Couldn't read “{}” as a date: try 2026-11-24, 2026-11-24 09:30, today, tomorrow or +3d",
                            input.trim()
                        ));
                    }
                }
            }
        }
    }

    fn set_date(&mut self, kind: PlanningKind, ts: Option<&Timestamp>) {
        self.edit_heading(|d, i| match kind {
            PlanningKind::Deadline => d.set_deadline(i, ts),
            _ => d.set_scheduled(i, ts),
        });
    }

    fn start_editing(&mut self) -> Task<Message> {
        let Some(f) = &mut self.file else {
            return Task::none();
        };
        if f.missing {
            return Task::none();
        }
        let (Some(doc), Some(range)) = (f.doc.clone(), f.edit_range()) else {
            return Task::none();
        };
        if f.selected == Selected::Nothing {
            f.selected = Selected::Intro;
        }
        f.editor = Some(SectionEditor::new(&doc, range));
        iced::widget::operation::focus(EDITOR_ID)
    }

    fn stop_editing(&mut self) {
        if let Some(f) = &mut self.file {
            f.editor = None;
            f.refresh_blocks();
            f.fill_inputs();
        }
    }

    fn editor_action(&mut self, action: Action) {
        let Some(f) = &mut self.file else { return };
        let is_edit = action.is_edit();
        let before = if is_edit { f.snapshot() } else { None };
        let Some(ed) = &mut f.editor else { return };
        ed.content.perform(action);
        if is_edit {
            self.sync_editor(before, Group::Typing);
        }
    }

    /// An Emacs kill (the text goes to the kill ring; kills in a row add
    /// up, M-DEL's in front) or C-y (inserts the kill ring's text).
    fn kill_or_yank(&mut self, command: Command) {
        let Some(f) = &mut self.file else { return };
        let before = f.snapshot();
        let Some(ed) = &mut f.editor else { return };
        let kind = match command {
            Command::Kill(kind) => kind,
            Command::Yank => {
                if !self.kill_ring.is_empty() {
                    ed.content
                        .perform(Action::Edit(Edit::Paste(Arc::new(self.kill_ring.clone()))));
                    self.sync_editor(before, Group::Single);
                }
                return;
            }
            _ => return,
        };
        // A kill ignores the selection, as in Emacs.
        let cursor = ed.content.cursor();
        ed.content.move_to(iced::widget::text_editor::Cursor {
            position: cursor.position,
            selection: None,
        });
        let motion = match kind {
            editor::Kill::Line => {
                let line_len = ed
                    .content
                    .line(cursor.position.line)
                    .map_or(0, |l| l.text.len());
                // At the end of the line, the line break goes.
                if cursor.position.column >= line_len {
                    Motion::Right
                } else {
                    Motion::End
                }
            }
            editor::Kill::WordForward => Motion::WordRight,
            editor::Kill::WordBackward => Motion::WordLeft,
        };
        ed.content.perform(Action::Select(motion));
        // iced counts a run of spaces as a word; Emacs kills through it to
        // the word beyond.
        if motion != Motion::Right
            && ed
                .content
                .selection()
                .is_some_and(|s| !s.is_empty() && s.trim().is_empty())
        {
            ed.content.perform(Action::Select(motion));
        }
        let Some(killed) = ed.content.selection().filter(|s| !s.is_empty()) else {
            return;
        };
        ed.content.perform(Action::Edit(Edit::Delete));
        if !self.kill_append {
            self.kill_ring.clear();
        }
        if kind == editor::Kill::WordBackward {
            self.kill_ring.insert_str(0, &killed);
        } else {
            self.kill_ring.push_str(&killed);
        }
        self.kill_append = true;
        self.sync_editor(before, Group::Single);
    }

    /// Enter in the editor, continuing or ending a list.
    fn enter(&mut self) {
        let Some(f) = &mut self.file else { return };
        let before = f.snapshot();
        let Some(ed) = &mut f.editor else { return };
        let cursor = ed.content.cursor();
        let line = ed
            .content
            .line(cursor.position.line)
            .map_or_else(String::new, |l| l.text.into_owned());
        let at_end = cursor.selection.is_none() && cursor.position.column == line.len();
        match editing::continue_list(&line).filter(|_| at_end) {
            Some(ListContinue::Item(prefix)) => {
                ed.content.perform(Action::Edit(Edit::Enter));
                ed.content
                    .perform(Action::Edit(Edit::Paste(Arc::new(prefix))));
            }
            Some(ListContinue::End) => {
                ed.content.perform(Action::Move(Motion::Home));
                ed.content.perform(Action::Select(Motion::End));
                ed.content.perform(Action::Edit(Edit::Backspace));
            }
            None => ed.content.perform(Action::Edit(Edit::Enter)),
        }
        self.sync_editor(before, Group::Single);
    }

    /// Splices the editor's text into the document.
    fn sync_editor(&mut self, before: Option<crate::file::Snapshot>, group: Group) {
        let Some(f) = &mut self.file else { return };
        let (Some(doc), Some(ed)) = (&f.doc, &mut f.editor) else {
            return;
        };
        let mut new = ed.content.text();
        let range = ed.range.clone();
        // The section must keep ending in a line break when text follows,
        // or the next heading would join its last line.
        if range.end < doc.text().len() && !new.ends_with('\n') {
            new.push_str(if doc.text().contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            });
        }
        if new == doc.text()[range.clone()] {
            return;
        }
        let mut d = (**doc).clone();
        d.replace_range(range.clone(), &new);
        ed.range = range.start..range.start + new.len();
        f.change(before, d, group);
        self.save_open();
    }

    fn undo(&mut self, redo: bool) {
        if let Some(f) = &mut self.file
            && f.undo(redo)
        {
            f.fill_inputs();
            self.save_open();
        }
    }

    /// Conflict: the unsaved edits win.
    fn keep_mine(&mut self) {
        let Some(f) = &self.file else { return };
        let Some(ds) = self.disk.get_mut(&f.path) else {
            return;
        };
        match ds.conflict.take() {
            Some(DiskCopy::Changed(doc, rev)) => {
                ds.base = Some(rev);
                ds.base_text = Arc::new(doc.text().to_string());
            }
            Some(DiskCopy::Deleted) => {
                ds.base = None;
                ds.base_text = Arc::new(String::new());
            }
            None => return,
        }
        let save = Request::Save {
            path: f.path.clone(),
            text: Arc::clone(&ds.latest),
            base: ds.base,
        };
        self.send_save(save);
    }

    /// Conflict: the disk version wins; the unsaved text stays one undo away.
    fn use_disk(&mut self) {
        let Some(f) = &mut self.file else { return };
        let Some(ds) = self.disk.get_mut(&f.path) else {
            return;
        };
        match ds.conflict.take() {
            Some(DiskCopy::Changed(doc, rev)) => {
                *ds = DiskState::clean(Some(rev), doc.text());
                let mine = f.snapshot();
                f.adopt(Arc::clone(&doc));
                let disk_text = Document::parse(doc.text(), doc.keywords().clone());
                // Put the dropped text on the undo stack.
                f.change(mine, disk_text, Group::Single);
                self.status = Some("Kept the disk version; Undo brings your edits back".into());
            }
            Some(DiskCopy::Deleted) => {
                self.disk.remove(&f.path);
                f.missing = true;
                f.editor = None;
            }
            None => {}
        }
    }

    fn on_key(&mut self, event: &keyboard::Event) -> Task<Message> {
        let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
            return Task::none();
        };
        if self.diagram.is_some() {
            let edit = match key.as_ref() {
                Key::Named(keyboard::key::Named::Delete | keyboard::key::Named::Backspace) => {
                    Some(draw::Edit::Delete)
                }
                Key::Named(keyboard::key::Named::Escape) => Some(draw::Edit::Select(None)),
                Key::Character(c) if modifiers.command() && c.eq_ignore_ascii_case("z") => {
                    Some(if modifiers.shift() {
                        draw::Edit::Redo
                    } else {
                        draw::Edit::Undo
                    })
                }
                Key::Character(c) if modifiers.command() && c.eq_ignore_ascii_case("y") => {
                    Some(draw::Edit::Redo)
                }
                _ => None,
            };
            if let (Some(e), Some(d)) = (edit, &mut self.diagram) {
                d.update(e);
            }
            return Task::none();
        }
        if matches!(key, Key::Named(keyboard::key::Named::Escape)) {
            if self.settings_panel.take().is_some() || self.new_file.take().is_some() {
                return Task::none();
            }
            if self.refile.take().is_some() {
                return Task::none();
            }
            if self.capture.is_some() && self.awaiting != Awaiting::Capture {
                self.capture = None;
                return Task::none();
            }
        }
        let editing = self.file.as_ref().is_some_and(|f| f.editor.is_some());
        if editing || self.capture.is_some() || self.refile.is_some() || !modifiers.command() {
            return Task::none();
        }
        match key.as_ref() {
            Key::Character(c) if c.eq_ignore_ascii_case("z") => self.undo(modifiers.shift()),
            Key::Character(c) if c.eq_ignore_ascii_case("y") => self.undo(true),
            Key::Character(c) if c.eq_ignore_ascii_case("e") => return self.start_editing(),
            Key::Character(c) if c.eq_ignore_ascii_case("o") => {
                return self.update(Message::PickFolder);
            }
            Key::Character(c) if c.eq_ignore_ascii_case("n") => return self.open_capture(),
            Key::Character(",") => return self.on_setting(Setting::Open),
            Key::Character("\\") => self.sidebar = !self.sidebar,
            Key::Character(c) if c.eq_ignore_ascii_case("v") && !modifiers.shift() => {
                return paste();
            }
            Key::Character(c) if c.eq_ignore_ascii_case("a") && modifiers.shift() => {
                return self.update(Message::ShowAgenda);
            }
            Key::Character(c) if c.eq_ignore_ascii_case("f") && modifiers.shift() => {
                return self.update(Message::ShowSearch);
            }
            _ => {}
        }
        Task::none()
    }

    /// Writes pending saves before closing; an unresolved conflict asks first.
    fn close(&mut self) -> Task<Message> {
        let conflicts = self.disk.values().any(|d| d.conflict.is_some());
        if conflicts && !self.close_warned {
            self.close_warned = true;
            self.status = Some(
                "Some edits conflict with changes on disk and aren't saved. Choose a version, or close again to discard them."
                    .into(),
            );
            return Task::none();
        }
        match &self.worker {
            Some(_) => {
                self.closing = true;
                self.flush();
                Task::none()
            }
            None => iced::exit(),
        }
    }

    fn shoot(&mut self, plan: &shot::Plan) -> Task<Message> {
        if let Some(f) = &mut self.file
            && let Some(i) = plan.heading
        {
            f.selected = Selected::Heading(i);
            f.refresh_blocks();
            f.fill_inputs();
        }
        if let Some(path) = &plan.diagram {
            // Captured once the diagram arrives.
            self.image_action(path.clone());
            return Task::none();
        }
        if let Some(name) = &plan.new_file {
            let _ = self.on_new_file(NewFileMsg::Open);
            let _ = self.on_new_file(NewFileMsg::Name(name.clone()));
        }
        if plan.appearance {
            let _ = self.on_setting(Setting::Open);
            if let Some(section) = plan.section {
                let _ = self.on_setting(Setting::Section(section));
            }
        }
        let focus = if plan.capture {
            self.open_capture()
        } else if plan.edit {
            self.start_editing()
        } else {
            Task::none()
        };
        focus.chain(capture())
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
        self.body()
    }

    /// The capture panel or refile picker, when one is open. It takes the
    /// main area rather than floating over it: iced's layered `stack` drops
    /// a text editor's text.
    fn panel(&self) -> Option<Element<'_, Message>> {
        if let Some(view) = self.settings_view() {
            return Some(view);
        }
        if let Some(r) = &self.refile {
            return Some(panels::refile_view(r));
        }
        let p = self.capture.as_ref()?;
        let keywords = self
            .file
            .as_ref()
            .and_then(|f| f.doc.as_ref())
            .map_or_else(|| self.template.keywords.clone(), |d| d.keywords().clone());
        Some(panels::capture_view(
            p,
            &self.template.templates,
            &keywords,
            &self.settings,
        ))
    }

    fn body(&self) -> Element<'_, Message> {
        let main: Element<'_, Message> = if self.root.is_none()
            && let Some(view) = self.settings_view()
        {
            container(view).center(Fill).into()
        } else if self.root.is_none() {
            self.welcome()
        } else if let Some(d) = &self.diagram {
            self.beside_sidebar(container(draw::view(d)).width(Fill).height(Fill))
        } else if let Some(panel) = self.panel() {
            self.beside_sidebar(container(panel).center(Fill))
        } else if self.view == View::Search {
            self.beside_sidebar(
                container(search::view(
                    &self.search_input,
                    self.search_meaning,
                    self.meaning.as_ref(),
                    self.search.as_deref(),
                ))
                .width(Fill)
                .height(Fill),
            )
        } else if self.view == View::Agenda {
            self.beside_sidebar(
                container(agenda::view(self.agenda.as_deref(), self.today))
                    .width(Fill)
                    .height(Fill),
            )
        } else {
            self.beside_sidebar(
                row![
                    container(self.outline())
                        .width(Length::FillPortion(2))
                        .height(Fill),
                    rule::vertical(1),
                    container(self.reader())
                        .width(Length::FillPortion(3))
                        .height(Fill),
                ]
                .height(Fill),
            )
        };
        self.framed(main)
    }

    /// `main` under the top bar (once a folder is open), with the notice
    /// and status lines below.
    fn framed<'a>(&'a self, main: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
        let main = main.into();
        let main: Element<'_, Message> = if self.root.is_some() {
            column![
                topbar::view(&topbar::Bar {
                    labels: self.settings.top_bar_labels,
                    sidebar: self.sidebar,
                    search: &self.search_input,
                    meaning: self.view == View::Search && self.search_meaning,
                    agenda: self.view == View::Agenda,
                }),
                rule::horizontal(1),
                main,
            ]
            .into()
        } else {
            main
        };
        let mut body = column![main];
        if let Some((msg, undo)) = &self.notice {
            let mut bar = row![text(msg.as_str()).size(13).width(Fill)]
                .spacing(8)
                .align_y(iced::Center);
            if undo.as_ref().is_some_and(|c| !c.is_empty()) {
                bar = bar.push(
                    button(text("Undo").size(13))
                        .on_press(Message::UndoOp)
                        .padding([2, 10]),
                );
            }
            bar = bar.push(
                button(text("Dismiss").size(13))
                    .on_press(Message::DismissNotice)
                    .style(button::text),
            );
            body = body.push(container(bar).padding([4, 10]).style(style::notice));
        }
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
            None => body.into(),
        }
    }

    fn welcome(&self) -> Element<'_, Message> {
        container(
            column![
                text("Blossom").size(32),
                text("Open a folder of .org files").size(15),
                button("Choose folder…").on_press(Message::PickFolder),
                text("or type its path:").size(13).style(dim),
                row![
                    text_input(DEFAULT_FOLDER, &self.vault_input)
                        .on_input(Message::VaultInput)
                        .on_submit(Message::OpenTyped)
                        .width(360),
                    button(if self.vault_exists {
                        "Open"
                    } else {
                        "Create and open"
                    })
                    .on_press(Message::OpenTyped)
                    .style(button::secondary),
                ]
                .spacing(8),
                text(if self.vault_exists {
                    ""
                } else {
                    "This folder doesn't exist yet; it will be created."
                })
                .size(12)
                .style(dim),
                text("Blossom opens this folder again next time.")
                    .size(12)
                    .style(dim),
                button(text("Settings…").size(12))
                    .on_press(Message::Setting(Setting::Open))
                    .padding([2, 0])
                    .style(button::text),
            ]
            .spacing(14),
        )
        .center(Fill)
        .into()
    }

    /// The sidebar, a divider, then `pane`.
    fn beside_sidebar<'a>(&'a self, pane: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
        if !self.sidebar {
            return pane.into();
        }
        row![
            container(self.sidebar())
                .width(240)
                .height(Fill)
                .style(style::pane),
            rule::vertical(1),
            pane.into(),
        ]
        .height(Fill)
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
            row![
                text(name).size(13).font(bold()).style(dim).width(Fill),
                button(text("Open…").size(12))
                    .on_press(Message::PickFolder)
                    .padding([2, 6])
                    .style(button::text),
            ]
            .align_y(iced::Center)
            .padding(iced::Padding {
                top: 8.0,
                right: 6.0,
                bottom: 8.0,
                left: 12.0
            }),
            rule::horizontal(1),
            scrollable(Column::with_children(rows).spacing(1).padding([6, 6])).height(Fill),
            rule::horizontal(1),
            newfile::view(self.new_file.as_ref(), &self.files),
        ]
        .into()
    }

    fn outline(&self) -> Element<'_, Message> {
        let Some(f) = &self.file else {
            return placeholder("Choose a file");
        };
        let Some(doc) = &f.doc else {
            return placeholder(if f.missing {
                "This file was deleted or moved"
            } else {
                "Loading…"
            });
        };
        let title = doc
            .preamble()
            .keywords
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("TITLE"))
            .map_or_else(|| f.path.clone(), |(_, v)| v.clone());
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        if !doc.top_range().is_empty() || doc.headings().is_empty() {
            let label = if doc.preamble().intro.is_some() {
                "Intro"
            } else {
                "Top of file"
            };
            items.push(
                button(text(label).size(14).font(Font {
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
        // The outline keeps the app's font; the chosen fonts are the note's.
        let ink = self.ink().ui();
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
        column![
            row![
                text(title).size(18).font(bold()).width(Fill),
                text(self.save_state(f)).size(12).style(dim),
            ]
            .align_y(iced::Center)
            .padding([10, 14]),
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

    /// "Saving…", "Not saved" or nothing.
    fn save_state(&self, f: &OpenFile) -> &'static str {
        match self.disk.get(&f.path) {
            Some(ds) if ds.conflict.is_some() => "Not saved",
            Some(ds) if ds.dirty() => "Saving…",
            _ => "",
        }
    }

    fn reader(&self) -> Element<'_, Message> {
        let Some(f) = self.file.as_ref().filter(|f| !f.missing) else {
            return placeholder("");
        };
        let Some(doc) = &f.doc else {
            return placeholder("");
        };
        let mut parts: Vec<Element<'_, Message>> = Vec::new();
        if let Some(banner) = self.conflict_banner(f) {
            parts.push(banner);
        }
        let heading = match f.selected {
            Selected::Heading(i) => doc.heading(i),
            _ => None,
        };
        if let Some(ed) = &f.editor {
            return self.editing(parts, f, ed, doc, heading);
        }
        let ink = self.ink();
        let actions = Actions {
            link: Message::Link,
            check: Message::ToggleCheckbox,
            image: Message::ImageAction,
        };
        let edit_button = button(text("Edit").size(13))
            .on_press(Message::StartEditing)
            .padding([3, 10])
            .style(button::secondary);
        match (f.selected, heading) {
            (Selected::Heading(i), Some(h)) => {
                let title = f.titles.get(i).map_or(&[][..], Vec::as_slice);
                parts.push(heading_toolbar(f, edit_button));
                parts.push(read::sized(title, ink.size * 8 / 5, ink, Message::Link));
                parts.push(self.metadata(h, doc));
                let chips = reader_chips(h, doc, self.today);
                if !chips.is_empty() {
                    parts.push(row(chips).spacing(6).wrap().into());
                }
                if !h.properties.is_empty() {
                    parts.push(properties(h));
                }
                parts.push(rule::horizontal(1).into());
            }
            (Selected::Heading(_), None) | (Selected::Nothing, _) => {
                return placeholder("Select a heading to read it");
            }
            (Selected::Intro, _) => parts.push(
                row![
                    text(if doc.preamble().intro.is_some() {
                        "Intro"
                    } else {
                        "Top of file"
                    })
                    .size(22)
                    .font(bold())
                    .width(Fill),
                    history_buttons(f),
                    edit_button
                ]
                .spacing(6)
                .into(),
            ),
        }
        if f.blocks.is_empty() {
            parts.push(
                text(match f.selected {
                    Selected::Heading(_) => "No text under this heading",
                    _ => "Nothing here yet. Choose Edit to write.",
                })
                .size(14)
                .style(dim)
                .into(),
            );
        } else {
            parts.push(read::blocks(&f.blocks, ink, actions, &f.pictures));
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

    /// The read pane while editing: header, metadata panel and editor.
    fn editing<'a>(
        &'a self,
        mut parts: Vec<Element<'a, Message>>,
        f: &'a OpenFile,
        ed: &'a SectionEditor,
        doc: &'a Document,
        heading: Option<&'a Heading>,
    ) -> Element<'a, Message> {
        parts.push(
            row![
                text(if heading.is_some() {
                    "Editing heading"
                } else {
                    "Editing top of file"
                })
                .size(13)
                .style(dim)
                .width(Fill),
                text("Esc to finish").size(12).style(dim),
                history_buttons(f),
                button(text("Done").size(13))
                    .on_press(Message::StopEditing)
                    .padding([3, 10]),
            ]
            .spacing(10)
            .align_y(iced::Center)
            .into(),
        );
        if let Some(h) = heading {
            parts.push(self.metadata(h, doc));
        }
        let ink = self.ink();
        parts.push(
            text_editor(&ed.content)
                .id(EDITOR_ID)
                .on_action(Message::EditorAction)
                .highlight_with::<editor::Org>(
                    editor::Settings {
                        keywords: doc.keywords().clone(),
                        family: ink.code.family,
                    },
                    editor::format,
                )
                .key_binding(editor::bindings(
                    Message::EditorCommand,
                    self.settings.emacs_keys,
                ))
                .font(ink.code)
                .size(self.settings.editor_size)
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

    fn conflict_banner(&self, f: &OpenFile) -> Option<Element<'_, Message>> {
        let copy = self.disk.get(&f.path)?.conflict.as_ref()?;
        let msg = match copy {
            DiskCopy::Changed(..) => {
                "Another program changed this file where you were editing. Your edits aren't saved yet."
            }
            DiskCopy::Deleted => "This file was deleted. Your edits aren't saved.",
        };
        let disk_label = match copy {
            DiskCopy::Changed(..) => "Use the disk version",
            DiskCopy::Deleted => "Discard my edits",
        };
        Some(
            container(
                column![
                    text(msg).size(13),
                    row![
                        button(text("Keep mine").size(13)).on_press(Message::KeepMine),
                        button(text(disk_label).size(13))
                            .on_press(Message::UseDisk)
                            .style(button::secondary),
                    ]
                    .spacing(8),
                ]
                .spacing(8),
            )
            .padding(10)
            .width(Fill)
            .style(style::banner)
            .into(),
        )
    }

    /// The metadata panel: keyword, priority, tags, scheduled, deadline.
    fn metadata<'a>(&'a self, h: &Heading, doc: &Document) -> Element<'a, Message> {
        let Some(f) = &self.file else {
            return space::horizontal().into();
        };
        let mut keywords: Vec<String> = vec![NONE.to_string()];
        keywords.extend(doc.keywords().all().map(String::from));
        let mut priorities: Vec<String> = [NONE, "A", "B", "C"].map(String::from).to_vec();
        if let Some(p) = h.priority.as_ref().filter(|p| !priorities.contains(p)) {
            priorities.push(p.clone());
        }
        let label = |s: &'a str| text(s).size(12).style(dim);
        let small = |s: &'a str, value: &'a str, field: Field| {
            text_input(s, value)
                .on_input(move |v| Message::Input(field, v))
                .on_submit(Message::Apply(field))
                .size(13)
                .padding([3, 6])
        };
        let date = |name: &'a str, value: &'a str, field: Field, kind: PlanningKind| {
            row![
                label(name).width(70),
                small("2026-11-24, today, +3d", value, field).width(Fill),
                button(text("Set").size(12))
                    .on_press(Message::Apply(field))
                    .padding([3, 8])
                    .style(button::secondary),
                button(text("×").size(13))
                    .on_press(Message::ClearDate(kind))
                    .padding([2, 8])
                    .style(button::text),
            ]
            .spacing(6)
            .align_y(iced::Center)
        };
        column![
            row![
                pick_list(
                    keywords,
                    Some(h.keyword.clone().unwrap_or_else(|| NONE.to_string())),
                    Message::SetKeyword
                )
                .text_size(13)
                .padding([3, 8]),
                pick_list(
                    priorities,
                    Some(h.priority.clone().unwrap_or_else(|| NONE.to_string())),
                    Message::SetPriority
                )
                .text_size(13)
                .padding([3, 8]),
                label("Tags"),
                small("work home", &f.inputs.tags, Field::Tags).width(Fill),
            ]
            .spacing(8)
            .align_y(iced::Center),
            date(
                "Scheduled",
                &f.inputs.scheduled,
                Field::Scheduled,
                PlanningKind::Scheduled
            ),
            date(
                "Deadline",
                &f.inputs.deadline,
                Field::Deadline,
                PlanningKind::Deadline
            ),
        ]
        .spacing(6)
        .into()
    }

    fn ink(&self) -> Ink {
        Ink::of(&style::theme(self.colours(), self.mode()), &self.settings)
    }
}

/// The read view's buttons for a heading.
fn heading_toolbar<'a>(
    f: &OpenFile,
    edit: iced::widget::Button<'a, Message>,
) -> Element<'a, Message> {
    let small = |label: &'static str, msg: Message| {
        button(text(label).size(13))
            .on_press(msg)
            .padding([3, 8])
            .style(button::text)
    };
    row![
        history_buttons(f),
        space::horizontal().width(Fill),
        small("Diagram…", Message::NewDiagram),
        small("Refile…", Message::StartRefile),
        small("Archive", Message::Archive),
        edit
    ]
    .spacing(6)
    .align_y(iced::Center)
    .into()
}

/// Undo and Redo, enabled when there is a step.
fn history_buttons(f: &OpenFile) -> Element<'static, Message> {
    let (undo, redo) = f.history();
    let b = |label: &'static str, on: bool, msg: Message| {
        button(text(label).size(13))
            .on_press_maybe(on.then_some(msg))
            .padding([3, 8])
            .style(button::text)
    };
    row![
        b("Undo", undo, Message::Undo),
        b("Redo", redo, Message::Redo)
    ]
    .spacing(2)
    .into()
}

/// A heading's properties, one `KEY: value` per line.
fn properties(h: &Heading) -> Element<'_, Message> {
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
    Column::with_children(props).spacing(2).into()
}

/// Reads a date typed into the metadata panel: an org timestamp,
/// `YYYY-MM-DD` with an optional `HH:MM`, `today`, `tomorrow` or `+N` days
/// (`+Nw` weeks). Empty clears the date (`Ok(None)`).
fn parse_when(input: &str, today: NaiveDate) -> Result<Option<Timestamp>, ()> {
    let s = input.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.starts_with(['<', '[']) {
        let mut ts = Timestamp::parse(s).ok_or(())?;
        ts.active = true;
        return Ok(Some(ts));
    }
    let lower = s.to_lowercase();
    let mut words = lower.split_whitespace();
    let day = words.next().ok_or(())?;
    let date = match day {
        "today" => today,
        "tomorrow" => today.succ_opt().ok_or(())?,
        d if d.starts_with('+') => {
            let (n, unit) = d[1..].split_at(
                d[1..]
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(d.len() - 1),
            );
            let n: u64 = n.parse().map_err(|_| ())?;
            let days = match unit {
                "" | "d" => n,
                "w" => n * 7,
                _ => return Err(()),
            };
            today.checked_add_days(chrono::Days::new(days)).ok_or(())?
        }
        d => NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| ())?,
    };
    let mut ts = Timestamp::new(date, true);
    if let Some(t) = words.next() {
        ts.time = Some(NaiveTime::parse_from_str(t, "%H:%M").map_err(|_| ())?);
    }
    if words.next().is_some() {
        return Err(());
    }
    Ok(Some(ts))
}

/// A status line for a problem in `config.toml`.
fn config_problem(message: &str) -> String {
    format!("Settings: {message}")
}

/// `Message::ConfigChanged` whenever `config.toml` at `path` changes on disk.
fn watch_config(path: PathBuf) -> Subscription<Message> {
    Subscription::run_with(path, |path| {
        let path = path.clone();
        iced::stream::channel(
            4,
            async move |mut output: iced::futures::channel::mpsc::Sender<Message>| {
                use iced::futures::{SinkExt as _, StreamExt as _};
                let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
                    return;
                };
                let name = name.to_string_lossy().into_owned();
                let Ok(watcher) =
                    blossom_vault::VaultWatcher::start(dir, std::time::Duration::from_millis(150))
                else {
                    return;
                };
                let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
                std::thread::spawn(move || {
                    while let Ok(batch) = watcher.changes().recv() {
                        // Empty: the platform lost track, so look anyway.
                        if (batch.is_empty() || batch.contains(&name))
                            && tx.unbounded_send(()).is_err()
                        {
                            break;
                        }
                    }
                });
                while rx.next().await.is_some() {
                    if output.send(Message::ConfigChanged).await.is_err() {
                        break;
                    }
                }
            },
        )
    })
}

/// Whether the welcome screen's typed path is an existing folder. An empty
/// field counts as existing, so no "will be created" hint shows.
fn typed_folder_exists(input: &str) -> bool {
    let input = input.trim();
    input.is_empty() || blossom_vault::config::expand(input).is_dir()
}

/// The folder typed on the welcome screen, made (with any missing parents)
/// if it doesn't exist. `None` for an empty field.
fn create_folder(input: &str) -> Result<Option<PathBuf>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    let root = blossom_vault::config::expand(input);
    std::fs::create_dir_all(&root)
        .map(|()| Some(root.clone()))
        .map_err(|e| format!("Couldn't create {}: {e}", root.display()))
}

/// Whether a link path is an SVG.
fn is_svg(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"))
}

/// Reads an image from the clipboard (off the UI thread) as PNG bytes.
fn paste() -> Task<Message> {
    Task::perform(async { clipboard_png() }, |png| {
        Message::Pasted(png.map(Arc::new))
    })
}

/// The clipboard's image encoded as PNG, or `None` (no image, or no
/// clipboard).
fn clipboard_png() -> Option<Vec<u8>> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let img = clipboard.get_image().ok()?;
    let width = u32::try_from(img.width).ok()?;
    let height = u32::try_from(img.height).ok()?;
    let rgba = image::RgbaImage::from_raw(width, height, img.bytes.into_owned())?;
    let mut out = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
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

/// One outline row: keyword, priority and title; tags and dates on a second
/// line, so a long title keeps the full width.
fn heading_line<'a>(
    h: &'a Heading,
    title: &'a [Span],
    doc: &Document,
    today: NaiveDate,
    ink: Ink,
) -> Element<'a, Message> {
    let done = is_done(h, doc);
    let mut first = row![].spacing(6).align_y(iced::Center);
    if let Some(k) = &h.keyword {
        first = first.push(chip(k.clone(), if done { Chip::Done } else { Chip::Todo }));
    }
    if let Some(p) = &h.priority {
        first = first.push(chip(format!("#{p}"), Chip::Priority));
    }
    first = first.push(container(read::paragraph(title, ink, Message::Link)).width(Fill));
    let mut chips: Vec<Element<'a, Message>> =
        h.tags.iter().map(|t| chip(t.clone(), Chip::Tag)).collect();
    chips.extend(date_chips(h, doc, today));
    chips.extend(event_chips(h, today));
    if chips.is_empty() {
        return first.into();
    }
    column![first, row(chips).spacing(6).wrap()]
        .spacing(4)
        .into()
}

fn is_done(h: &Heading, doc: &Document) -> bool {
    h.keyword
        .as_deref()
        .is_some_and(|k| doc.keywords().is_done(k))
}

/// What the read view shows besides the metadata panel: inherited tags,
/// event dates and CLOSED.
fn reader_chips<'a>(h: &Heading, doc: &Document, today: NaiveDate) -> Vec<Element<'a, Message>> {
    let mut out: Vec<Element<'a, Message>> = doc
        .inherited_tags(h.index)
        .into_iter()
        .filter(|t| !h.tags.contains(t))
        .map(|t| chip(format!("{t} (inherited)"), Chip::Tag))
        .collect();
    out.extend(event_chips(h, today));
    if let Some(c) = &h.planning.closed {
        out.push(chip(format!("Closed {}", short_date(c, today)), Chip::Date));
    }
    out
}

/// SCHEDULED and DEADLINE chips; red when past on an open item.
fn date_chips<'a>(h: &Heading, doc: &Document, today: NaiveDate) -> Vec<Element<'a, Message>> {
    let done = is_done(h, doc);
    let dated = |label: &str, ts: &Timestamp| {
        let late = !done && ts.date < today;
        chip(
            format!("{label} {}", short_date(ts, today)),
            if late { Chip::Overdue } else { Chip::Date },
        )
    };
    let mut out = Vec::new();
    if let Some(s) = &h.planning.scheduled {
        out.push(dated("Scheduled", s));
    }
    if let Some(d) = &h.planning.deadline {
        out.push(dated("Due", d));
    }
    out
}

/// Events: the dedicated timestamp line, else the first stamp in the body.
fn event_chips<'a>(h: &Heading, today: NaiveDate) -> Vec<Element<'a, Message>> {
    let events = if h.dedicated_timestamps.is_empty() {
        h.active_timestamps.iter().take(1).collect::<Vec<_>>()
    } else {
        h.dedicated_timestamps.iter().collect()
    };
    events
        .into_iter()
        .map(|e| chip(short_date(e, today), Chip::Date))
        .collect()
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

pub(crate) fn chip<'a>(label: String, kind: Chip) -> Element<'a, Message> {
    container(text(label).size(11).font(bold()))
        .padding([1, 6])
        .style(move |t| style::chip(t, kind))
        .into()
}

pub(crate) fn placeholder(msg: &str) -> Element<'_, Message> {
    container(text(msg).size(14).style(dim)).center(Fill).into()
}

pub(crate) fn dim(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(style::dim(theme)),
    }
}

pub(crate) fn bold() -> Font {
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

/// Opens a web or mail link with the system handler. The opener is waited
/// for on a thread of its own, so it doesn't linger as a zombie.
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
        .map(|mut child| {
            std::thread::spawn(move || drop(child.wait()));
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use blossom_core::Keywords;
    use blossom_core::render::Block;

    fn app() -> App {
        let template = OpenVault::new(PathBuf::new(), &blossom_vault::config::Config::default());
        App::new(
            None,
            template,
            NaiveDate::from_ymd_opt(2026, 11, 23).unwrap(),
            None,
            (Config::default(), None),
        )
        .0
    }

    /// The appearance in the config at `path`.
    fn saved(path: &Path) -> Settings {
        Settings::from_all(&Config::load(path).unwrap()).0
    }

    #[test]
    fn hand_edits_to_the_config_apply_while_running() {
        use crate::style::ModeChoice;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        edit::create(&path).unwrap();
        let mut a = app();
        a.config_path = Some(path.clone());
        a.root = Some(PathBuf::from("/v"));
        let _ = a.update(Message::Setting(Setting::Mode(ModeChoice::Dark)));
        // The app's own write changes nothing more when it's read back.
        let _ = a.update(Message::ConfigChanged);
        assert_eq!(a.settings.mode, ModeChoice::Dark);
        assert!(a.pending.is_none() && a.status.is_none());
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replace("\nmode = \"dark\"", "\nmode = \"light\"")
                .replace("# colours = \"blossom\"", "colours = \"gray\"")
                .replace("# inbox = \"inbox.org\"", "inbox = \"in.org\""),
        )
        .unwrap();
        let _ = a.update(Message::ConfigChanged);
        assert_eq!(
            (a.settings.mode, a.settings.colours),
            (ModeChoice::Light, Colours::Gray)
        );
        // The inbox changed: the vault is reopened with it.
        assert_eq!(a.template.templates[0].file, "in.org");
        assert_eq!(
            a.pending.as_ref().map(|o| o.root.clone()),
            Some(PathBuf::from("/v"))
        );
        // A file that doesn't parse leaves everything as it was.
        std::fs::write(&path, "colours = \n").unwrap();
        let _ = a.update(Message::ConfigChanged);
        assert_eq!(a.settings.colours, Colours::Gray);
        assert!(a.status.as_deref().unwrap().starts_with("Settings: "));
        // A bad value gives the default and says so.
        std::fs::write(&path, "[appearance]\ncolours = \"teal\"\n").unwrap();
        let _ = a.update(Message::ConfigChanged);
        assert_eq!(a.settings.colours, Colours::Blossom);
        assert!(a.status.as_deref().unwrap().contains("teal"));
    }

    #[test]
    fn the_settings_panel_writes_search_choices() {
        use crate::settings_panel::{ModelChoice, Section};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        edit::create(&path).unwrap();
        let mut a = app();
        a.config_path = Some(path.clone());
        a.root = Some(PathBuf::from("/v"));
        let _ = a.update(Message::Setting(Setting::Open));
        let _ = a.update(Message::Setting(Setting::Section(Section::Search)));
        assert_eq!(a.settings_panel.as_ref().unwrap().section, Section::Search);
        let _ = a.update(Message::Setting(Setting::Model(ModelChoice::Server)));
        // Written, applied, and the vault reopened with it.
        assert_eq!(
            Config::load(&path).unwrap().semantic.model.as_deref(),
            Some("server")
        );
        assert_eq!(a.template.semantic.model.as_deref(), Some("server"));
        assert!(a.pending.is_some());
        // Typing writes nothing until applied; an empty field is unset.
        let _ = a.update(Message::Setting(Setting::Server(
            "http://box:11434/v1".into(),
        )));
        let _ = a.update(Message::Setting(Setting::ServerModel("  ".into())));
        assert_eq!(Config::load(&path).unwrap().semantic.server, None);
        let _ = a.update(Message::Setting(Setting::ApplyServer));
        let c = Config::load(&path).unwrap().semantic;
        assert_eq!(
            (c.server.as_deref(), c.server_model),
            (Some("http://box:11434/v1"), None)
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\n# server_model = \"nomic-embed-text\"\n"),
            "{text}"
        );
        // Reopening the panel fills the fields from the config.
        let _ = a.update(Message::Setting(Setting::Close));
        let _ = a.update(Message::Setting(Setting::Open));
        assert_eq!(
            a.settings_panel.as_ref().unwrap().server,
            "http://box:11434/v1"
        );
        assert!(a.settings_view().is_some());
    }

    #[test]
    fn the_top_bar_searches_and_hides_the_sidebar() {
        let mut a = app();
        a.root = Some(PathBuf::from("/v"));
        assert!(a.sidebar);
        let _ = a.update(Message::ToggleSidebar);
        assert!(!a.sidebar);
        let _ = a.view();
        let _ = a.update(Message::ToggleSidebar);
        assert!(a.sidebar);
        // Typing in the field shows the search screen for it.
        assert_eq!(a.view, View::Files);
        let _ = a.update(Message::SearchInput("t.work".into()));
        assert_eq!((a.view, a.search_input.as_str()), (View::Search, "t.work"));
        let _ = a.view();
    }

    #[test]
    fn new_file_form_creates_and_opens_the_file() {
        use blossom_vault::ops::{Change, Outcome};
        let mut a = app();
        a.root = Some(PathBuf::from("/v"));
        a.files = Arc::new(vec![FileInfo {
            path: "inbox.org".into(),
            headings: 0,
            open_tasks: 0,
            conflict: false,
        }]);
        a.sidebar = false;
        let _ = a.update(Message::NewFile(NewFileMsg::Open));
        assert!(a.sidebar, "the form is in the sidebar");
        // An existing name isn't created.
        let _ = a.update(Message::NewFile(NewFileMsg::Name("Inbox".into())));
        let _ = a.update(Message::NewFile(NewFileMsg::Create));
        assert_eq!(a.awaiting, Awaiting::Nothing);
        assert!(a.status.as_deref().unwrap().contains("exists"));
        let _ = a.view();
        let _ = a.update(Message::NewFile(NewFileMsg::Name("work/garden".into())));
        let _ = a.update(Message::NewFile(NewFileMsg::Title("Garden".into())));
        let _ = a.update(Message::NewFile(NewFileMsg::Create));
        assert_eq!(a.awaiting, Awaiting::NewFile);
        let _ = a.update(Message::Worker(Event::Done(Arc::new(Outcome {
            changes: vec![Change {
                path: "work/garden.org".into(),
                before: None,
                after: "#+TITLE: Garden\n".into(),
            }],
            path: "work/garden.org".into(),
            heading: None,
            line: 1,
            message: "Created work/garden.org".into(),
        }))));
        assert!(a.new_file.is_none());
        assert_eq!(
            a.file.as_ref().map(|f| f.path.as_str()),
            Some("work/garden.org")
        );
        assert!(a.notice.is_some(), "with undo");
        // Cancel closes the form.
        let _ = a.update(Message::NewFile(NewFileMsg::Open));
        let _ = a.update(Message::NewFile(NewFileMsg::Cancel));
        assert!(a.new_file.is_none());
    }

    #[test]
    fn a_folder_opens_on_its_last_file_else_the_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.toml");
        let infos = |paths: &[&str]| {
            Arc::new(
                paths
                    .iter()
                    .map(|p| FileInfo {
                        path: (*p).into(),
                        headings: 0,
                        open_tasks: 0,
                        conflict: false,
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let open = |a: &mut App, files: &[&str]| {
            let _ = a.update(Message::Worker(Event::Opened(PathBuf::from("/v"))));
            let _ = a.update(Message::Worker(Event::Files(infos(files))));
        };
        // First time: the inbox.
        let mut a = app().with_state(Some(state_path.clone()));
        open(&mut a, &["inbox.org", "work.org"]);
        assert_eq!(a.file.as_ref().map(|f| f.path.as_str()), Some("inbox.org"));
        // Choosing another file is remembered for next time.
        let _ = a.update(Message::SelectFile("work.org".into()));
        let mut a = app().with_state(Some(state_path.clone()));
        open(&mut a, &["inbox.org", "work.org"]);
        assert_eq!(a.file.as_ref().map(|f| f.path.as_str()), Some("work.org"));
        // A later file list (after a save) doesn't switch files.
        let _ = a.update(Message::SelectFile("inbox.org".into()));
        let _ = a.update(Message::Worker(Event::Files(infos(&[
            "inbox.org",
            "work.org",
        ]))));
        assert_eq!(a.file.as_ref().map(|f| f.path.as_str()), Some("inbox.org"));
        // Neither there: nothing opens.
        let mut a = app().with_state(Some(state_path));
        open(&mut a, &["notes.org"]);
        assert!(a.file.is_none());
    }

    #[test]
    fn a_folder_opened_in_the_app_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        edit::create(&path).unwrap();
        let mut a = app();
        a.config_path = Some(path.clone());
        let notes = dir.path().join("notes");
        let _ = a.update(Message::Picked(Some(notes.clone())));
        let config = Config::load(&path).unwrap();
        assert_eq!(config.vault.as_deref(), Some(notes.to_str().unwrap()));
        assert_eq!(
            a.pending.as_ref().map(|o| o.root.clone()),
            Some(notes.clone())
        );
        // Reading the app's own write back doesn't open it again.
        a.pending = None;
        let _ = a.update(Message::ConfigChanged);
        assert!(a.pending.is_none());
        // A hand edit to `vault` opens the new folder.
        let other = dir.path().join("other");
        edit::apply(
            &path,
            &[edit::Change::set(None, "vault", other.to_str().unwrap())],
        )
        .unwrap();
        let _ = a.update(Message::ConfigChanged);
        assert_eq!(a.pending.as_ref().map(|o| o.root.clone()), Some(other));
    }

    #[test]
    fn theme_follows_the_system_unless_chosen() {
        use crate::style::ModeChoice;
        let mut a = app();
        assert!(a.theme().is_none(), "system mode not known yet");
        let _ = a.update(Message::Mode(Mode::Dark));
        assert_eq!(a.mode(), Mode::Dark);
        let _ = a.update(Message::Setting(Setting::Mode(ModeChoice::Light)));
        let _ = a.update(Message::Mode(Mode::Dark));
        assert_eq!(a.mode(), Mode::Light);
        let _ = a.update(Message::Setting(Setting::Colours(Colours::Gray)));
        assert_eq!(a.theme().map(|t| t.to_string()), Some("Gray Light".into()));
        let _ = a.update(Message::Setting(Setting::Mode(ModeChoice::System)));
        assert_eq!(a.mode(), Mode::Dark);
    }

    #[test]
    fn font_choices_reach_the_read_view_and_are_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut a = app();
        a.config_path = Some(path.clone());
        assert_eq!(a.ink().body, Font::DEFAULT);
        assert_eq!(a.ink().code, Font::MONOSPACE);
        let _ = a.update(Message::Setting(Setting::ReadFont(fonts::Choice(Some(
            "Inter".into(),
        )))));
        let _ = a.update(Message::Setting(Setting::ReadSize(18)));
        let _ = a.update(Message::Setting(Setting::EditorFont(fonts::Choice(Some(
            "Fira Code".into(),
        )))));
        let ink = a.ink();
        assert_eq!(ink.body.family, iced::font::Family::Name("Inter"));
        assert_eq!(ink.code.family, iced::font::Family::Name("Fira Code"));
        assert_eq!(ink.size, 18);
        assert_eq!(saved(&path), a.settings);
        let _ = a.update(Message::Setting(Setting::ReadFont(fonts::Choice(None))));
        assert_eq!(a.ink().body, Font::DEFAULT);
    }

    #[test]
    fn appearance_choices_are_saved_and_escape_closes_the_panel() {
        use crate::style::ModeChoice;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut a = app();
        a.config_path = Some(path.clone());
        let _ = a.update(Message::Setting(Setting::Open));
        assert!(a.settings_panel.is_some());
        let _ = a.update(Message::Setting(Setting::Colours(Colours::Solarized)));
        let _ = a.update(Message::Setting(Setting::Mode(ModeChoice::Dark)));
        assert_eq!(saved(&path), a.settings);
        let _ = a.update(Message::Key(keyboard::Event::KeyPressed {
            key: Key::Named(keyboard::key::Named::Escape),
            modified_key: Key::Named(keyboard::key::Named::Escape),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        }));
        assert!(a.settings_panel.is_none());
    }

    fn doc(s: &str) -> Arc<Document> {
        Arc::new(Document::parse(s, Keywords::default()))
    }

    fn rev(n: u64) -> Revision {
        Revision {
            mtime_ns: n,
            size: n,
        }
    }

    /// Loads `text` as `a.org` at revision 1.
    fn loaded(text: &str) -> App {
        let mut a = app();
        let _ = a.update(Message::Worker(Event::Opened(PathBuf::from("/v"))));
        let _ = a.update(Message::SelectFile("a.org".into()));
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc(text),
            rev(1),
        )));
        a
    }

    fn text_of(a: &App) -> String {
        a.file
            .as_ref()
            .unwrap()
            .doc
            .as_ref()
            .unwrap()
            .text()
            .to_string()
    }

    #[test]
    fn search_screen_keeps_only_current_results() {
        let mut a = loaded("* one\n");
        let _ = a.update(Message::ShowSearch);
        assert_eq!(a.view, View::Search);
        let _ = a.update(Message::SearchInput("on".into()));
        let _ = a.update(Message::SearchInput("one ".into()));
        let results = |q: &str| {
            Message::Worker(Event::Search(Arc::new(SearchResults {
                query: q.into(),
                ..SearchResults::default()
            })))
        };
        // Results for earlier typing are dropped; the trimmed query matches.
        let _ = a.update(results("on"));
        assert!(a.search.is_none());
        let _ = a.update(results("one"));
        assert_eq!(a.search.as_ref().unwrap().query, "one");
        // By meaning: only results by meaning count.
        let _ = a.update(Message::SearchMeaning(true));
        assert!(a.search.is_none(), "words results are dropped");
        let _ = a.update(results("one"));
        assert!(a.search.is_none());
        let _ = a.update(Message::Worker(Event::Search(Arc::new(SearchResults {
            query: "one".into(),
            meaning: true,
            ..SearchResults::default()
        }))));
        assert!(a.search.as_ref().unwrap().meaning);
        let _ = a.update(Message::Worker(Event::Meaning(meaning::Status::Ready)));
        assert_eq!(a.meaning, Some(meaning::Status::Ready));
        // Opening a result switches to the file; the query stays.
        let _ = a.update(Message::OpenHeading("a.org".into(), 0));
        assert_eq!(a.view, View::Files);
        assert_eq!(a.search_input, "one ");
    }

    #[test]
    fn welcome_opens_or_creates_the_typed_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = app();
        assert_eq!(a.vault_input, "~/Notes");
        // A missing folder: flagged, then created and opened.
        let target = dir.path().join("Notes/work");
        let _ = a.update(Message::VaultInput(target.display().to_string()));
        assert!(!a.vault_exists);
        let _ = a.update(Message::OpenTyped);
        assert!(target.is_dir());
        assert_eq!(a.pending.as_ref().map(|o| &o.root), Some(&target));
        // An existing folder just opens.
        let _ = a.update(Message::VaultInput(target.display().to_string()));
        assert!(a.vault_exists);
        let _ = a.update(Message::OpenTyped);
        assert!(a.status.is_none());
        // A file in the way is reported.
        let file = dir.path().join("f");
        std::fs::write(&file, "").unwrap();
        let _ = a.update(Message::VaultInput(file.display().to_string()));
        assert!(!a.vault_exists);
        let _ = a.update(Message::OpenTyped);
        assert!(a.status.as_deref().unwrap().starts_with("Couldn't create"));
    }

    fn type_text(a: &mut App, s: &str) {
        for c in s.chars() {
            let edit = if c == '\n' {
                Edit::Enter
            } else {
                Edit::Insert(c)
            };
            let _ = a.update(Message::EditorAction(Action::Edit(edit)));
        }
    }

    #[test]
    fn agenda_opens_headings_and_notices_offer_undo() {
        let mut a = loaded("* one\n* two\n");
        let _ = a.update(Message::ShowAgenda);
        assert_eq!(a.view, View::Agenda);
        // A heading in another file: selected once that file arrives.
        let _ = a.update(Message::OpenHeading("b.org".into(), 1));
        assert_eq!(a.view, View::Files);
        let _ = a.update(Message::Worker(Event::Document(
            "b.org".into(),
            doc("* x\n* y\n"),
            rev(2),
        )));
        assert_eq!(a.file.as_ref().unwrap().selected, Selected::Heading(1));
        // In the open file: selected at once.
        let _ = a.update(Message::OpenHeading("b.org".into(), 0));
        assert_eq!(a.file.as_ref().unwrap().selected, Selected::Heading(0));
        // A search hit on the text before the first heading.
        let _ = a.update(Message::OpenIntro("b.org".into()));
        assert_eq!(a.file.as_ref().unwrap().selected, Selected::Intro);
        let _ = a.update(Message::OpenIntro("c.org".into()));
        let _ = a.update(Message::Worker(Event::Document(
            "c.org".into(),
            doc("* x\n"),
            rev(3),
        )));
        assert_eq!(a.file.as_ref().unwrap().selected, Selected::Intro);
        let _ = a.update(Message::OpenHeading("b.org".into(), 0));
        let out = blossom_vault::ops::Outcome {
            changes: vec![Change {
                path: "b.org".into(),
                before: Some("* x\n* y\n".into()),
                after: "* y\n".into(),
            }],
            path: "b.org".into(),
            heading: Some(0),
            line: 1,
            message: "Archived to b.org_archive".into(),
        };
        let _ = a.update(Message::Worker(Event::Done(Arc::new(out))));
        assert!(
            matches!(&a.notice, Some((m, Some(c))) if m.starts_with("Archived") && c.len() == 1)
        );
        let _ = a.update(Message::UndoOp);
        assert!(a.notice.is_none());
    }

    #[test]
    fn capture_panel_expands_and_keeps_typing() {
        let mut a = loaded("* one\n");
        let _ = a.update(Message::OpenCapture);
        let label = |t: &blossom_vault::config::Template| format!("{} — {}", t.key, t.name);
        let todo = label(&a.template.templates[1]);
        let _ = a.update(Message::CaptureTemplate(todo));
        let p = a.capture.as_ref().unwrap();
        assert_eq!(p.entry(), "* TODO ");
        // The cursor is where `%?` was: typing goes there.
        let _ = a.update(Message::CaptureAction(Action::Edit(Edit::Insert('x'))));
        let p = a.capture.as_ref().unwrap();
        assert_eq!(p.entry(), "* TODO x");
        assert!(p.edited);
        let _ = a.update(Message::CaptureSave);
        assert_eq!(a.awaiting, Awaiting::Capture);
        let _ = a.update(Message::Worker(Event::Error("disk full".into())));
        assert!(a.capture.is_some() && a.awaiting == Awaiting::Nothing);
        let _ = a.update(Message::CaptureCancel);
        assert!(a.capture.is_none());
    }

    #[test]
    fn refile_picker_filters_and_skips_itself() {
        let mut a = loaded("* one\n* two\n");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::StartRefile);
        let t = |path: &str, index: Option<usize>, outline: &str| worker::RefileTarget {
            path: path.into(),
            heading: index.map(|i| HeadingRef {
                path: path.into(),
                index: i,
                title: outline.rsplit(" / ").next().unwrap_or("").into(),
            }),
            outline: outline.into(),
        };
        let targets = vec![
            t("a.org", None, ""),
            t("a.org", Some(0), "one"),
            t("a.org", Some(1), "two"),
            t("work/b.org", Some(0), "Projects / Blossom"),
        ];
        let _ = a.update(Message::Worker(Event::Targets(Arc::new(targets))));
        let r = a.refile.as_ref().unwrap();
        assert_eq!(r.heading.title, "one");
        assert_eq!(r.matches(), [0, 2, 3]);
        let _ = a.update(Message::RefileFilter("work blos".into()));
        assert_eq!(a.refile.as_ref().unwrap().matches(), [3]);
        let _ = a.update(Message::RefilePick(3));
        assert!(a.refile.is_none());
    }

    #[test]
    fn emacs_kills_add_up_and_yank_back() {
        use crate::editor::Kill;
        let mut a = loaded("* one\nalpha beta\ngamma\n* two\n");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::StartEditing);
        let kill = |a: &mut App, k| {
            let _ = a.update(Message::EditorCommand(Command::Kill(k)));
        };
        let go = |a: &mut App, m| {
            let _ = a.update(Message::EditorAction(Action::Move(m)));
        };
        // To the start of "alpha beta".
        go(&mut a, Motion::DocumentStart);
        go(&mut a, Motion::Down);
        // C-k C-k C-k: the line, its break, the next line; one kill.
        kill(&mut a, Kill::Line);
        kill(&mut a, Kill::Line);
        kill(&mut a, Kill::Line);
        assert_eq!(text_of(&a), "* one\n\n* two\n");
        assert_eq!(a.kill_ring, "alpha beta\ngamma");
        // Moving ends the run: a new kill replaces it. M-d kills to the
        // end of the word.
        // C-y where it was killed puts it back.
        let _ = a.update(Message::EditorCommand(Command::Yank));
        assert_eq!(text_of(&a), "* one\nalpha beta\ngamma\n* two\n");
        go(&mut a, Motion::DocumentStart);
        go(&mut a, Motion::Down);
        kill(&mut a, Kill::WordForward);
        assert_eq!(text_of(&a), "* one\n beta\ngamma\n* two\n");
        assert_eq!(a.kill_ring, "alpha");
        // M-d again kills through the space to the next word's end.
        kill(&mut a, Kill::WordForward);
        assert_eq!(a.kill_ring, "alpha beta");
        let _ = a.update(Message::EditorCommand(Command::Yank));
        // M-DEL twice from the line's end: both words, in their order.
        go(&mut a, Motion::End);
        kill(&mut a, Kill::WordBackward);
        kill(&mut a, Kill::WordBackward);
        assert_eq!(a.kill_ring, "alpha beta");
        assert_eq!(text_of(&a), "* one\n\ngamma\n* two\n");
        // Undo brings the kill back.
        let _ = a.update(Message::EditorCommand(Command::Undo));
        assert!(text_of(&a).starts_with("* one\n"), "{}", text_of(&a));
    }

    #[test]
    fn images_load_once_and_attachments_are_linked() {
        let mut a = loaded("* one\n[[file:pic.png]]\n* two\n");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        // Without a worker nothing is requested, so mark it as a worker would.
        let f = a.file.as_mut().unwrap();
        assert_eq!(read::image_paths(&f.blocks), ["pic.png"]);
        let _ = a.update(Message::Worker(Event::Image {
            file: "a.org".into(),
            path: "pic.png".into(),
            data: None,
        }));
        assert!(matches!(
            a.file.as_ref().unwrap().pictures.get("pic.png"),
            Some(Picture::Missing)
        ));
        // Attached while reading: linked at the end of the selected heading.
        let _ = a.update(Message::Worker(Event::Attached {
            file: "a.org".into(),
            link: "attachments/a/x.png".into(),
        }));
        assert_eq!(
            text_of(&a),
            "* one\n[[file:pic.png]]\n[[file:attachments/a/x.png]]\n* two\n"
        );
        // While editing: at the cursor.
        let _ = a.update(Message::Select(Selected::Heading(1)));
        let _ = a.update(Message::StartEditing);
        let _ = a.update(Message::EditorAction(Action::Move(Motion::DocumentEnd)));
        let _ = a.update(Message::Worker(Event::Attached {
            file: "a.org".into(),
            link: "y.png".into(),
        }));
        assert!(
            text_of(&a).ends_with("* two\n[[file:y.png]]"),
            "{}",
            text_of(&a)
        );
        // Another file: not linked, reported.
        let _ = a.update(Message::Worker(Event::Attached {
            file: "b.org".into(),
            link: "z.png".into(),
        }));
        assert!(a.status.as_deref().unwrap().contains("isn't linked"));
    }

    #[test]
    fn diagram_editor_opens_saves_and_closes() {
        let mut a = loaded("* one\n");
        let _ = a.update(Message::NewDiagram);
        assert!(a.diagram.as_ref().is_some_and(|d| d.link.is_none()));
        let rect = crate::excalidraw::Element::new(
            crate::excalidraw::Kind::Rectangle,
            0.0,
            0.0,
            "#000000",
        );
        let _ = a.update(Message::Diagram(draw::Edit::Add(Box::new(rect))));
        // Cancel with changes asks once.
        let _ = a.update(Message::DiagramCancel);
        assert!(a.diagram.is_some());
        let _ = a.update(Message::DiagramSave);
        // No worker in this test: pretend it answered.
        a.awaiting = Awaiting::Diagram;
        let _ = a.update(Message::Worker(Event::Attached {
            file: "a.org".into(),
            link: "attachments/a/d.svg".into(),
        }));
        assert!(a.diagram.is_none());
        assert!(text_of(&a).contains("[[file:attachments/a/d.svg]]"));
        let _ = a.update(Message::Worker(Event::Diagram {
            file: "a.org".into(),
            path: "attachments/a/d.svg".into(),
            scene: Box::default(),
            base: None,
        }));
        assert!(a.diagram.as_ref().is_some_and(|d| d.link.is_some()));
        let _ = a.update(Message::DiagramCancel);
        assert!(a.diagram.is_none());
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
            rev(1),
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
            rev(1),
        )));
        assert_eq!(a.file.as_ref().unwrap().titles.len(), 2);
    }

    #[test]
    fn outside_edit_keeps_selection_when_possible() {
        let mut a = loaded("* one\n* two\nold\n");
        let _ = a.update(Message::Select(Selected::Heading(1)));
        let _ = a.update(Message::ToggleFold(1));
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* one\n* two\nnew body\n"),
            rev(2),
        )));
        let f = a.file.as_ref().unwrap();
        assert_eq!(f.selected, Selected::Heading(1));
        assert!(matches!(&f.blocks[..], [Block::Paragraph(s)] if s[0].text == "new body"));
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* only\n"),
            rev(3),
        )));
        assert_eq!(a.file.as_ref().unwrap().selected, Selected::Heading(0));
        assert!(a.file.as_ref().unwrap().folded.is_empty());
        let _ = a.update(Message::Worker(Event::Missing("a.org".into())));
        assert!(a.file.as_ref().unwrap().missing);
    }

    #[test]
    fn section_editing_splices_and_keeps_the_next_heading() {
        let mut a = loaded("* one\nbody\n* two\n");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::StartEditing);
        let ed = a.file.as_ref().unwrap().editor.as_ref().unwrap();
        assert_eq!(ed.content.text(), "* one\nbody\n");
        let _ = a.update(Message::EditorAction(Action::Move(Motion::DocumentEnd)));
        type_text(&mut a, "more");
        // Typing at the empty last line would join "* two"; a line break is kept.
        assert_eq!(text_of(&a), "* one\nbody\nmore\n* two\n");
        // Deleting the final line break keeps it in the file too.
        for _ in 0..5 {
            let _ = a.update(Message::EditorAction(Action::Edit(Edit::Backspace)));
        }
        assert_eq!(text_of(&a), "* one\nbody\n* two\n");
        // Typing a new heading in the editor shows up in the outline.
        let _ = a.update(Message::EditorAction(Action::Move(Motion::DocumentStart)));
        type_text(&mut a, "* zero\n");
        let f = a.file.as_ref().unwrap();
        assert_eq!(f.doc.as_ref().unwrap().headings().len(), 3);
        assert_eq!(f.selected, Selected::Heading(0));
        assert_eq!(a.save_state(f), "Saving…");
    }

    #[test]
    fn enter_continues_and_ends_lists() {
        let mut a = loaded("* h\n- [X] milk");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::StartEditing);
        let _ = a.update(Message::EditorAction(Action::Move(Motion::DocumentEnd)));
        let _ = a.update(Message::EditorCommand(Command::Enter));
        type_text(&mut a, "eggs");
        assert_eq!(text_of(&a), "* h\n- [X] milk\n- [ ] eggs");
        let _ = a.update(Message::EditorCommand(Command::Enter));
        let _ = a.update(Message::EditorCommand(Command::Enter));
        assert_eq!(text_of(&a), "* h\n- [X] milk\n- [ ] eggs\n");
        // Undo steps back over the list edits.
        let _ = a.update(Message::Undo);
        assert_eq!(text_of(&a), "* h\n- [X] milk\n- [ ] eggs\n- [ ] ");
    }

    #[test]
    fn metadata_panel_edits_the_heading() {
        let mut a = loaded("* buy milk\n");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::SetKeyword("TODO".into()));
        let _ = a.update(Message::SetPriority("A".into()));
        let _ = a.update(Message::Input(Field::Tags, "home, errand".into()));
        let _ = a.update(Message::Apply(Field::Tags));
        let _ = a.update(Message::Input(Field::Deadline, "tomorrow 09:30".into()));
        let _ = a.update(Message::Apply(Field::Deadline));
        assert_eq!(
            text_of(&a),
            "* TODO [#A] buy milk  :home:errand:\nDEADLINE: <2026-11-24 Tue 09:30>\n"
        );
        assert_eq!(
            a.file.as_ref().unwrap().inputs.deadline,
            "<2026-11-24 Tue 09:30>"
        );
        let _ = a.update(Message::ClearDate(PlanningKind::Deadline));
        let _ = a.update(Message::SetKeyword(NONE.into()));
        assert_eq!(text_of(&a), "* [#A] buy milk  :home:errand:\n");
        let _ = a.update(Message::Input(Field::Scheduled, "next week".into()));
        let _ = a.update(Message::Apply(Field::Scheduled));
        assert!(a.status.as_deref().unwrap().contains("Couldn't read"));
        // Each change is its own undo step.
        let _ = a.update(Message::Undo);
        let _ = a.update(Message::Undo);
        assert!(text_of(&a).contains("DEADLINE"));
    }

    #[test]
    fn checkboxes_toggle_from_the_read_view() {
        let mut a = loaded("* list\n- [ ] one\n- [ ] two\n");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let Block::ListItem { offset, .. } = a.file.as_ref().unwrap().blocks[0].clone() else {
            panic!("a list item");
        };
        let _ = a.update(Message::ToggleCheckbox(offset));
        assert_eq!(text_of(&a), "* list\n- [X] one\n- [ ] two\n");
    }

    #[test]
    fn outside_change_merges_with_unsaved_edits() {
        let mut a = loaded("* one\n* two\n* three\n");
        let _ = a.update(Message::Select(Selected::Heading(2)));
        let _ = a.update(Message::StartEditing);
        let _ = a.update(Message::EditorAction(Action::Move(Motion::End)));
        type_text(&mut a, "!");
        // Emacs edits heading one meanwhile.
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* one (emacs)\n* two\n* three\n"),
            rev(2),
        )));
        assert_eq!(text_of(&a), "* one (emacs)\n* two\n* three!\n");
        let ed = a.file.as_ref().unwrap().editor.as_ref().unwrap();
        assert_eq!(ed.content.text(), "* three!\n");
        let ds = &a.disk["a.org"];
        assert_eq!(ds.base, Some(rev(2)));
        assert!(ds.dirty());
        // The save lands.
        let _ = a.update(Message::Worker(Event::Saved {
            path: "a.org".into(),
            text: Arc::new("* one (emacs)\n* two\n* three!\n".into()),
            revision: rev(3),
        }));
        assert!(!a.disk["a.org"].dirty());
        assert_eq!(a.save_state(a.file.as_ref().unwrap()), "");
    }

    fn flushes(requests: &[Request]) -> usize {
        requests
            .iter()
            .filter(|r| matches!(r, Request::Flush))
            .count()
    }

    fn saves(requests: &[Request]) -> Vec<String> {
        requests
            .iter()
            .filter_map(|r| match r {
                Request::Save { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn closing_waits_for_a_save_merged_during_the_flush() {
        let (handle, sent) = crate::worker::Handle::recording();
        let mut a = loaded("* one\n* two\n");
        let _ = a.update(Message::Worker(Event::Ready(handle)));
        let _ = a.update(Message::Select(Selected::Heading(1)));
        let _ = a.update(Message::StartEditing);
        let _ = a.update(Message::EditorAction(Action::Move(Motion::End)));
        type_text(&mut a, "!");
        let _ = a.update(Message::CloseRequested);
        assert_eq!(flushes(&sent.take()), 1);
        // The flush's write was refused: Emacs changed heading one. The merge
        // is saved and flushed again before the app may exit.
        let _ = a.update(Message::Worker(Event::Stale {
            path: "a.org".into(),
            disk: Some((doc("* one (emacs)\n* two\n"), rev(2))),
        }));
        let sent_now = sent.take();
        assert_eq!(saves(&sent_now), ["* one (emacs)\n* two!\n"]);
        assert_eq!(flushes(&sent_now), 1);
        let _ = a.update(Message::Worker(Event::Flushed));
        assert!(a.closing && a.flushes == 1, "still waiting for the merge");
        let _ = a.update(Message::Worker(Event::Flushed));
        assert_eq!(a.flushes, 0);
    }

    #[test]
    fn a_stopped_worker_never_loses_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("a.org"), "* one\n").unwrap();
        let vault = blossom_vault::Vault::open(&root, Keywords::default(), &[]).unwrap();
        let read = || std::fs::read_to_string(root.join("a.org")).unwrap();
        let mut a = app();
        let (handle, _sent) = crate::worker::Handle::recording();
        let _ = a.update(Message::Worker(Event::Ready(handle)));
        let _ = a.update(Message::Worker(Event::Opened(root.clone())));
        let _ = a.update(Message::SelectFile("a.org".into()));
        let rev = vault.revision("a.org").unwrap();
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* one\n"),
            rev.unwrap(),
        )));
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::SetKeyword("TODO".into()));
        // The worker dies with the save still waiting: the app writes it and
        // starts a new worker, which reopens the vault and the file.
        let _ = a.update(Message::Worker(Event::Stopped("boom".into())));
        assert_eq!(read(), "* TODO one\n");
        assert!(a.worker.is_none());
        assert_eq!(a.worker_runs, 1);
        assert_eq!(a.pending.as_ref().map(|o| &o.root), Some(&root));
        assert!(a.status.as_deref().unwrap().contains("restarted"));
        let _ = a.update(Message::Worker(Event::Opened(root.clone())));
        assert_eq!(a.file.as_ref().map(|f| f.path.as_str()), Some("a.org"));
        assert!(a.status.as_deref().unwrap().contains("restarted"));
        let rev = vault.revision("a.org").unwrap();
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* TODO one\n"),
            rev.unwrap(),
        )));
        // It keeps stopping: no more restarts, edits are written at once.
        for _ in 0..WORKER_RESTARTS {
            let _ = a.update(Message::Worker(Event::Stopped("boom".into())));
        }
        assert!(a.worker_failed);
        assert_eq!(a.worker_runs, WORKER_RESTARTS);
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::SetKeyword(NONE.into()));
        assert_eq!(read(), "* one\n");
        // Another program changed the file: the text is kept beside it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("a.org"), "* emacs\n").unwrap();
        let _ = a.update(Message::SetKeyword("TODO".into()));
        assert_eq!(read(), "* emacs\n");
        assert!(a.status.as_deref().unwrap().contains("a.org.unsaved-"));
    }

    #[test]
    fn switching_vaults_writes_unsaved_edits_first() {
        let (handle, sent) = crate::worker::Handle::recording();
        let mut a = loaded("* one\n");
        let _ = a.update(Message::Worker(Event::Ready(handle)));
        let _ = a.update(Message::SetKeyword("TODO".into()));
        let _ = sent.take();
        a.open_folder(PathBuf::from("/other"));
        let first = sent.take();
        assert_eq!(flushes(&first), 1);
        assert!(!first.iter().any(|r| matches!(r, Request::Open(_))));
        let _ = a.update(Message::Worker(Event::Flushed));
        let opened = sent.take();
        assert!(
            opened
                .iter()
                .any(|r| matches!(r, Request::Open(o) if o.root == std::path::Path::new("/other")))
        );
    }

    #[test]
    fn conflicting_change_asks_and_keeps_both_reachable() {
        let mut a = loaded("* one\n");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::StartEditing);
        let _ = a.update(Message::EditorAction(Action::Move(Motion::End)));
        type_text(&mut a, " mine");
        let _ = a.update(Message::Worker(Event::Stale {
            path: "a.org".into(),
            disk: Some((doc("* one theirs\n"), rev(2))),
        }));
        assert!(a.disk["a.org"].conflict.is_some());
        assert_eq!(text_of(&a), "* one mine\n");
        assert_eq!(a.save_state(a.file.as_ref().unwrap()), "Not saved");
        // Closing is refused once.
        let _ = a.update(Message::CloseRequested);
        assert!(a.status.as_deref().unwrap().contains("close again"));
        let _ = a.update(Message::UseDisk);
        assert_eq!(text_of(&a), "* one theirs\n");
        assert!(!a.disk["a.org"].dirty());
        let _ = a.update(Message::Undo);
        assert_eq!(text_of(&a), "* one mine\n");
    }

    #[test]
    fn unsaved_text_survives_leaving_and_reopening_a_file() {
        let mut a = loaded("* one\n");
        let _ = a.update(Message::Select(Selected::Heading(0)));
        let _ = a.update(Message::SetKeyword("TODO".into()));
        let _ = a.update(Message::SelectFile("b.org".into()));
        let _ = a.update(Message::SelectFile("a.org".into()));
        // The disk still has the old text (the save is waiting).
        let _ = a.update(Message::Worker(Event::Document(
            "a.org".into(),
            doc("* one\n"),
            rev(1),
        )));
        assert_eq!(text_of(&a), "* TODO one\n");
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
        let when = |s: &str| parse_when(s, today).map(|t| t.map(|t| t.format()));
        assert_eq!(when("").unwrap(), None);
        assert_eq!(when("+3d").unwrap().unwrap(), "<2026-11-26 Thu>");
        assert_eq!(when("+2w").unwrap().unwrap(), "<2026-12-07 Mon>");
        assert_eq!(when("2026-12-01").unwrap().unwrap(), "<2026-12-01 Tue>");
        assert_eq!(
            when("[2026-12-01 Tue +1w]").unwrap().unwrap(),
            "<2026-12-01 Tue +1w>"
        );
        assert!(when("soon").is_err() && when("today 25:00").is_err());
        let mut a = app();
        let _ = a.update(Message::ToggleFolder("x".into()));
        assert!(a.collapsed_folders.contains("x"));
        let _ = a.update(Message::ToggleFolder("x".into()));
        assert!(a.collapsed_folders.is_empty());
    }
}
