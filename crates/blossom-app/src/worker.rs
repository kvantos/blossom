//! The background worker. One thread owns the vault, the index and the file
//! watcher, so the UI never blocks on disk. The UI sends [`Request`]s through
//! a [`Handle`]; the worker answers with [`Event`]s through an iced
//! subscription. External changes (Emacs, Syncthing, git) arrive from the
//! watcher and refresh the file list and the open file.
//!
//! Saves are debounced here: the UI sends the latest text after every edit
//! and the worker writes it once typing pauses, with the vault's stale-write
//! guard. A write refused because another program changed the file comes
//! back as [`Event::Stale`] with the disk version, for the UI to merge.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use blossom_core::capture::Target;
use blossom_core::{Document, Keywords};
use blossom_index::NoteIndex;
use blossom_index::agenda::{self, Agenda};
use blossom_vault::config::Template;
use blossom_vault::ops::{Change, Complete, HeadingRef, OpError, Outcome};
use blossom_vault::{Revision, Vault, WriteError, config};
use iced::futures::{SinkExt, StreamExt};
use iced::{Subscription, futures};

use crate::tree::FileInfo;

/// How long the watcher waits for a burst of changes to settle.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// How long a save waits for typing to pause.
pub const SAVE_DELAY: Duration = Duration::from_millis(700);

/// What to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenVault {
    /// The vault folder.
    pub root: PathBuf,
    /// TODO keywords.
    pub keywords: Keywords,
    /// Extra ignore globs.
    pub ignore: Vec<String>,
    /// Index folder; `None` for the platform cache folder.
    pub index_dir: Option<PathBuf>,
    /// Default archive location.
    pub archive: String,
    /// Deadline warning days for the agenda.
    pub warning_days: i64,
    /// Capture templates.
    pub templates: Vec<Template>,
    /// The attachments folder name.
    pub attachments: String,
}

impl OpenVault {
    /// Settings for `root`, the rest from `config`.
    #[must_use]
    pub fn new(root: PathBuf, config: &config::Config) -> OpenVault {
        OpenVault {
            root,
            keywords: config.keywords(),
            ignore: config.ignore.clone(),
            index_dir: None,
            archive: config.archive_location().to_string(),
            warning_days: config.warning_days(),
            attachments: config.attachments_folder().to_string(),
            templates: config.templates().unwrap_or_else(|e| {
                eprintln!("blossom: {e}; using the built-in capture templates");
                config::Config::default().templates().unwrap_or_default()
            }),
        }
    }
}

/// A file operation, run by the worker (see [`blossom_vault::ops`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Check off an agenda entry.
    Complete(HeadingRef, Complete),
    /// Move a heading under another, or to the end of a file.
    Refile {
        /// The heading.
        heading: HeadingRef,
        /// The destination file.
        file: String,
        /// The destination heading, or `None` for the file's top level.
        under: Option<HeadingRef>,
    },
    /// Archive a heading.
    Archive(HeadingRef),
    /// Add a capture entry.
    Capture {
        /// Target file.
        file: String,
        /// Where in it.
        target: Target,
        /// The entry text.
        entry: String,
    },
}

/// A heading the refile picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefileTarget {
    /// The file.
    pub path: String,
    /// The heading, or `None` for the file's top level.
    pub heading: Option<HeadingRef>,
    /// "Projects / Blossom" (empty for a file).
    pub outline: String,
}

/// A request from the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open a vault (closing any open one).
    Open(OpenVault),
    /// Load a file and keep it fresh when it changes.
    Load(String),
    /// Write `text` to `path` once edits pause. `base` is the disk revision
    /// the text was made from (`None` for a new file). A newer save for the
    /// same path replaces a waiting one.
    Save {
        /// Vault-relative path.
        path: String,
        /// The whole new text.
        text: Arc<String>,
        /// The revision the text is based on.
        base: Option<Revision>,
    },
    /// Write every waiting save now, then answer [`Event::Flushed`].
    Flush,
    /// Send the agenda for this many days now and after every change.
    Agenda(u32),
    /// Send the refile targets.
    Targets,
    /// Run a file operation, after writing waiting saves.
    Run(Op),
    /// Undo an operation's changes.
    Restore(Vec<Change>),
    /// Read the image that `path` (a link path as written) in `file` points at.
    LoadImage {
        /// The org file.
        file: String,
        /// The link path.
        path: String,
    },
    /// Store `bytes` as an attachment of `file`.
    Attach {
        /// The org file.
        file: String,
        /// The wanted file name.
        name: String,
        /// The content.
        bytes: Arc<Vec<u8>>,
    },
    /// Copy a file (dropped on the window) in as an attachment of `file`.
    AttachFile {
        /// The org file.
        file: String,
        /// The file to copy.
        source: PathBuf,
    },
    /// Read the diagram that `path` in `file` points at.
    OpenDiagram {
        /// The org file.
        file: String,
        /// The link path.
        path: String,
    },
    /// Save a diagram: its source, then its SVG; over the one whose SVG is
    /// at link `path`, or as a new pair of attachments.
    SaveDiagram {
        /// The org file.
        file: String,
        /// The SVG's link path, for an existing diagram.
        path: Option<String>,
        /// The source's revision when it was opened (stale-write guard).
        base: Option<Revision>,
        /// The `.excalidraw` text.
        source: Arc<String>,
        /// The SVG.
        svg: Arc<String>,
    },
}

/// What the worker reports.
#[derive(Clone, Debug)]
pub enum Event {
    /// The worker is running; requests go through this handle.
    Ready(Handle),
    /// A vault is open.
    Opened(PathBuf),
    /// The vault's files, sorted by path.
    Files(Arc<Vec<FileInfo>>),
    /// A file's content on disk, at a revision.
    Document(String, Arc<Document>, Revision),
    /// A save was written.
    Saved {
        /// Vault-relative path.
        path: String,
        /// The text written.
        text: Arc<String>,
        /// The file's revision after the write.
        revision: Revision,
    },
    /// A save was refused: the file changed on disk since `base`. Carries
    /// the disk version, or `None` when the file is gone.
    Stale {
        /// Vault-relative path.
        path: String,
        /// The disk version.
        disk: Option<(Arc<Document>, Revision)>,
    },
    /// Every waiting save has been written (or failed).
    Flushed,
    /// The loaded file is gone.
    Missing(String),
    /// The agenda.
    Agenda(Arc<Agenda>),
    /// Every heading, for the refile picker.
    Targets(Arc<Vec<RefileTarget>>),
    /// An operation succeeded.
    Done(Arc<Outcome>),
    /// An undo succeeded.
    Restored(String),
    /// An image's bytes, or `None` when it can't be read.
    Image {
        /// The org file.
        file: String,
        /// The link path.
        path: String,
        /// The file's content.
        data: Option<Arc<Vec<u8>>>,
    },
    /// An attachment was stored.
    Attached {
        /// The org file.
        file: String,
        /// The link path to write in it.
        link: String,
    },
    /// A diagram to edit.
    Diagram {
        /// The org file.
        file: String,
        /// The SVG's link path.
        path: String,
        /// The scene.
        scene: Box<crate::excalidraw::Scene>,
        /// The source's revision.
        base: Option<Revision>,
    },
    /// An existing diagram was saved.
    DiagramSaved {
        /// The org file.
        file: String,
        /// The link path.
        path: String,
    },
    /// Something failed.
    Error(String),
}

/// Sends requests to the worker. Cheap to clone.
#[derive(Clone)]
pub struct Handle(Sender<Input>);

impl Handle {
    /// Sends `request`; a stopped worker is ignored.
    pub fn send(&self, request: Request) {
        let _ = self.0.send(Input::Request(request));
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Handle")
    }
}

/// The worker's inbox: UI requests plus watcher notices.
enum Input {
    Request(Request),
    /// Files changed in the vault opened as `generation`.
    Changed(u64),
}

/// The subscription that starts the worker and relays its events.
pub fn subscription() -> Subscription<Event> {
    Subscription::run(stream)
}

fn stream() -> impl futures::Stream<Item = Event> {
    iced::stream::channel(
        64,
        async |mut output: futures::channel::mpsc::Sender<Event>| {
            let (events_tx, mut events_rx) = futures::channel::mpsc::unbounded();
            let (input_tx, input_rx) = mpsc::channel();
            let looped = input_tx.clone();
            thread::spawn(move || {
                run(&input_rx, &looped, &mut |event| {
                    let _ = events_tx.unbounded_send(event);
                });
            });
            if output.send(Event::Ready(Handle(input_tx))).await.is_err() {
                return;
            }
            while let Some(event) = events_rx.next().await {
                if output.send(event).await.is_err() {
                    break;
                }
            }
        },
    )
}

struct Open {
    vault: Vault,
    index: NoteIndex,
    settings: OpenVault,
    /// Days of agenda to send after every change, once asked for.
    agenda: Option<u32>,
    generation: u64,
    /// Tells the watcher thread of this vault to stop.
    stop: Arc<AtomicBool>,
    /// Saves waiting for typing to pause.
    pending: BTreeMap<String, Pending>,
    /// Revisions this worker wrote, per path, oldest first, starting with
    /// the base the first write replaced. A save based on any of them is not
    /// stale as long as the disk is still at the last one: the UI just
    /// hadn't heard of the newer writes yet.
    written: HashMap<String, Vec<Option<Revision>>>,
}

struct Pending {
    text: Arc<String>,
    base: Option<Revision>,
    due: Instant,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The worker loop: runs until the UI side goes away.
fn run(inbox: &Receiver<Input>, myself: &Sender<Input>, emit: &mut dyn FnMut(Event)) {
    let mut open: Option<Open> = None;
    let mut loaded: Option<String> = None;
    let mut generation = 0;
    loop {
        let due = open
            .as_ref()
            .and_then(|o| o.pending.values().map(|p| p.due).min());
        let input = match due {
            Some(due) => match inbox.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(input) => input,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(o) = open.as_mut() {
                        write_due(o, Instant::now(), emit);
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match inbox.recv() {
                Ok(input) => input,
                Err(_) => break,
            },
        };
        match input {
            Input::Request(Request::Open(req)) => {
                if let Some(o) = open.as_mut() {
                    write_due(o, far_future(), emit);
                }
                open = None;
                loaded = None;
                generation += 1;
                match open_vault(&req, generation, myself) {
                    Ok(o) => {
                        emit(Event::Opened(req.root.clone()));
                        emit(Event::Files(Arc::new(file_infos(&o))));
                        open = Some(o);
                    }
                    Err(e) => emit(Event::Error(e)),
                }
            }
            Input::Request(Request::Load(path)) => {
                if let Some(o) = &open {
                    emit_document(o, &path, emit);
                    loaded = Some(path);
                }
            }
            Input::Request(Request::Save { path, text, base }) => {
                if let Some(o) = open.as_mut() {
                    o.pending.insert(
                        path,
                        Pending {
                            text,
                            base,
                            due: Instant::now() + SAVE_DELAY,
                        },
                    );
                }
            }
            Input::Request(Request::Flush) => {
                if let Some(o) = open.as_mut() {
                    write_due(o, far_future(), emit);
                }
                emit(Event::Flushed);
            }
            Input::Request(r) => {
                if let Some(o) = open.as_mut() {
                    handle_other(o, r, loaded.as_deref(), emit);
                }
            }
            Input::Changed(g) => {
                // A notice from a vault that has since been closed is stale.
                let Some(o) = open.as_mut().filter(|o| o.generation == g) else {
                    continue;
                };
                // Extra notices are harmless: a sync with no changes is cheap.
                match o.index.sync(&o.vault) {
                    Ok(report) => {
                        emit(Event::Files(Arc::new(file_infos(o))));
                        if report.added.len() + report.updated.len() + report.removed.len() > 0 {
                            emit_agenda(o, emit);
                        }
                        if let Some(path) = &loaded {
                            if report.removed.contains(path) {
                                emit(Event::Missing(path.clone()));
                            } else if report.updated.contains(path) || report.added.contains(path) {
                                emit_document(o, path, emit);
                            }
                        }
                    }
                    Err(e) => emit(Event::Error(e.to_string())),
                }
            }
        }
    }
}

fn open_vault(req: &OpenVault, generation: u64, myself: &Sender<Input>) -> Result<Open, String> {
    let vault = Vault::open(&req.root, req.keywords.clone(), &req.ignore)
        .map_err(|e| format!("{}: {e}", req.root.display()))?;
    let dir = req
        .index_dir
        .clone()
        .or_else(|| config::index_dir(&req.root))
        .ok_or("no cache folder for the index")?;
    let settings = format!(
        "{}|{}",
        req.keywords.active.join(" "),
        req.keywords.done.join(" ")
    );
    let mut index = NoteIndex::open(&dir, &settings).map_err(|e| e.to_string())?;
    index.sync(&vault).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    match vault.watch(DEBOUNCE) {
        Ok(watcher) => {
            let (stop, myself) = (Arc::clone(&stop), myself.clone());
            thread::spawn(move || {
                for _batch in watcher.changes() {
                    if stop.load(Ordering::Relaxed)
                        || myself.send(Input::Changed(generation)).is_err()
                    {
                        break;
                    }
                }
            });
        }
        // Without a watcher the app still works; it just won't see outside edits.
        Err(e) => eprintln!("blossom: file watching is off: {e}"),
    }
    Ok(Open {
        vault,
        index,
        settings: req.clone(),
        agenda: None,
        generation,
        stop,
        pending: BTreeMap::new(),
        written: HashMap::new(),
    })
}

/// A time after every deadline, to write all waiting saves.
fn far_future() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

/// Writes every waiting save due by `now`.
fn write_due(o: &mut Open, now: Instant, emit: &mut dyn FnMut(Event)) {
    let due: Vec<String> = o
        .pending
        .iter()
        .filter(|(_, p)| p.due <= now)
        .map(|(k, _)| k.clone())
        .collect();
    let mut any = false;
    for path in due {
        if let Some(p) = o.pending.remove(&path) {
            any |= save(o, &path, p, emit);
        }
    }
    if any {
        // Refresh counts now rather than waiting for the watcher, which
        // will then find nothing new.
        match o.index.sync(&o.vault) {
            Ok(_) => {
                emit(Event::Files(Arc::new(file_infos(o))));
                emit_agenda(o, emit);
            }
            Err(e) => emit(Event::Error(e.to_string())),
        }
    }
}

/// Requests other than opening, loading, saving and flushing.
fn handle_other(o: &mut Open, request: Request, loaded: Option<&str>, emit: &mut dyn FnMut(Event)) {
    match request {
        Request::LoadImage { .. }
        | Request::Attach { .. }
        | Request::AttachFile { .. }
        | Request::OpenDiagram { .. }
        | Request::SaveDiagram { .. } => handle_file(o, request, emit),
        _ => handle_op(o, request, loaded, emit),
    }
}

/// The agenda, refile-target and operation requests.
fn handle_op(o: &mut Open, request: Request, loaded: Option<&str>, emit: &mut dyn FnMut(Event)) {
    match request {
        Request::Agenda(days) => {
            o.agenda = Some(days);
            emit_agenda(o, emit);
        }
        Request::Targets => emit(Event::Targets(Arc::new(targets(o)))),
        Request::Run(op) => {
            write_due(o, far_future(), emit);
            match run_op(o, op) {
                Ok(out) => {
                    let paths: Vec<String> = out.changes.iter().map(|c| c.path.clone()).collect();
                    after_change(o, &paths, loaded, emit);
                    emit(Event::Done(Arc::new(out)));
                }
                Err(e) => emit(Event::Error(e.to_string())),
            }
        }
        Request::Restore(changes) => {
            write_due(o, far_future(), emit);
            match o.vault.restore(&changes) {
                Ok(()) => {
                    let paths: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
                    after_change(o, &paths, loaded, emit);
                    emit(Event::Restored("Undone".into()));
                }
                Err(e) => emit(Event::Error(format!("Couldn't undo: {e}"))),
            }
        }
        _ => {}
    }
}

/// Largest image the read view loads, and largest file attached by drop.
const MAX_FILE: u64 = 64 * 1024 * 1024;

/// Reads `full` if it is a file no bigger than [`MAX_FILE`].
fn read_limited(full: &std::path::Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(full).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > MAX_FILE {
        return Err(format!("larger than {} MB", MAX_FILE / 1024 / 1024));
    }
    std::fs::read(full).map_err(|e| e.to_string())
}

/// The diagram source file's extension.
const SOURCE_EXT: &str = "excalidraw";

fn is_svg(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

fn modified(p: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The vault-relative path of `full`, or an error naming why it can't be written.
fn in_vault(o: &Open, full: &std::path::Path) -> Result<String, String> {
    o.vault
        .relative_path(full)
        .ok_or_else(|| "only diagrams inside the vault can be written".to_string())
}

/// When the SVG at `svg` has a `.excalidraw` source beside it that is newer
/// (or the SVG is missing), writes a fresh SVG from it and returns its bytes.
fn regenerate(o: &Open, svg: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    let source = svg.with_extension(SOURCE_EXT);
    let Some(src_time) = modified(&source) else {
        return Ok(None);
    };
    if modified(svg).is_some_and(|t| t >= src_time) {
        return Ok(None);
    }
    let text = String::from_utf8(read_limited(&source)?).map_err(|e| e.to_string())?;
    let scene = crate::excalidraw::Scene::parse(&text)?;
    let out = crate::excalidraw::to_svg(&scene).into_bytes();
    // Outside the vault it is shown but not written.
    if let Ok(rel) = in_vault(o, svg) {
        let rev = o.vault.revision(&rel).map_err(|e| e.to_string())?;
        o.vault
            .write_bytes(&rel, &out, rev)
            .map_err(|e| e.to_string())?;
    }
    Ok(Some(out))
}

/// The scene of the diagram whose SVG is at link `path`, and its source's
/// revision. An SVG saved by Blossom before source files existed gets its
/// source written now.
fn open_diagram(
    o: &Open,
    file: &str,
    path: &str,
) -> Result<(crate::excalidraw::Scene, Option<Revision>), String> {
    let svg = o.vault.resolve_link(file, path);
    let source = svg.with_extension(SOURCE_EXT);
    if source.exists() {
        let text = String::from_utf8(read_limited(&source)?).map_err(|e| e.to_string())?;
        let scene = crate::excalidraw::Scene::parse(&text)?;
        let base = match o.vault.relative_path(&source) {
            Some(rel) => o.vault.revision(&rel).map_err(|e| e.to_string())?,
            None => None,
        };
        return Ok((scene, base));
    }
    let old = std::fs::read_to_string(&svg).map_err(|e| e.to_string())?;
    let Some(shapes) = crate::diagram::from_svg(&old)? else {
        return Err(format!(
            "there is no .{SOURCE_EXT} file beside it, so it can't be edited here"
        ));
    };
    let json = crate::excalidraw::Scene::from_shapes(&shapes).to_json();
    let rel = in_vault(o, &source)?;
    let rev = o
        .vault
        .write_bytes(&rel, json.as_bytes(), None)
        .map_err(|e| e.to_string())?;
    Ok((crate::excalidraw::Scene::parse(&json)?, Some(rev)))
}

/// Writes an existing diagram's source (refused if it changed since `base`),
/// then its SVG.
fn save_diagram(
    o: &Open,
    file: &str,
    path: &str,
    base: Option<Revision>,
    source: &str,
    svg: &str,
) -> Result<(), String> {
    let svg_full = o.vault.resolve_link(file, path);
    let src_rel = in_vault(o, &svg_full.with_extension(SOURCE_EXT))?;
    let svg_rel = in_vault(o, &svg_full)?;
    o.vault
        .write_bytes(&src_rel, source.as_bytes(), base)
        .map_err(|e| match e {
            WriteError::Stale { .. } => format!(
                "the .{SOURCE_EXT} file changed since it was opened (another program?); nothing was saved"
            ),
            e => e.to_string(),
        })?;
    let rev = o.vault.revision(&svg_rel).map_err(|e| e.to_string())?;
    o.vault
        .write_bytes(&svg_rel, svg.as_bytes(), rev)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Images, attachments and diagrams.
fn handle_file(o: &Open, request: Request, emit: &mut dyn FnMut(Event)) {
    let folder = &o.settings.attachments;
    match request {
        Request::LoadImage { file, path } => {
            let full = o.vault.resolve_link(&file, &path);
            let fresh = if is_svg(&path) {
                match regenerate(o, &full) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        emit(Event::Error(format!("{path}: {e}")));
                        None
                    }
                }
            } else {
                None
            };
            let data = fresh.or_else(|| read_limited(&full).ok()).map(Arc::new);
            emit(Event::Image { file, path, data });
        }
        Request::Attach { file, name, bytes } => {
            match o.vault.attach(&file, folder, &name, &bytes) {
                Ok((_, link)) => emit(Event::Attached { file, link }),
                Err(e) => emit(Event::Error(format!("Couldn't attach {name}: {e}"))),
            }
        }
        Request::AttachFile { file, source } => {
            let name = source
                .file_name()
                .map_or_else(|| "attachment".into(), |n| n.to_string_lossy().into_owned());
            let stored = read_limited(&source).and_then(|bytes| {
                o.vault
                    .attach(&file, folder, &name, &bytes)
                    .map_err(|e| e.to_string())
            });
            match stored {
                Ok((_, link)) => emit(Event::Attached { file, link }),
                Err(e) => emit(Event::Error(format!("Couldn't attach {name}: {e}"))),
            }
        }
        Request::OpenDiagram { file, path } => match open_diagram(o, &file, &path) {
            Ok((scene, base)) => emit(Event::Diagram {
                file,
                path,
                scene: Box::new(scene),
                base,
            }),
            Err(e) => emit(Event::Error(format!("{path}: {e}"))),
        },
        Request::SaveDiagram {
            file,
            path,
            base,
            source,
            svg,
        } => {
            if let Some(path) = path {
                match save_diagram(o, &file, &path, base, &source, &svg) {
                    Ok(()) => emit(Event::DiagramSaved { file, path }),
                    Err(e) => emit(Event::Error(format!("Couldn't save {path}: {e}"))),
                }
            } else {
                let stem = chrono::Local::now()
                    .format("diagram-%Y%m%d-%H%M%S")
                    .to_string();
                let files: [(&str, &[u8]); 2] =
                    [(SOURCE_EXT, source.as_bytes()), ("svg", svg.as_bytes())];
                match o.vault.attach_set(&file, folder, &stem, &files) {
                    Ok(links) => emit(Event::Attached {
                        file,
                        link: links.last().cloned().unwrap_or_default(),
                    }),
                    Err(e) => emit(Event::Error(format!("Couldn't save the diagram: {e}"))),
                }
            }
        }
        _ => {}
    }
}

fn run_op(o: &Open, op: Op) -> Result<Outcome, OpError> {
    let now = chrono::Local::now().naive_local();
    match op {
        Op::Complete(h, how) => o.vault.complete(&h, how, now),
        Op::Refile {
            heading,
            file,
            under,
        } => o.vault.refile(&heading, &file, under.as_ref()),
        Op::Archive(h) => o.vault.archive(&h, &o.settings.archive, now),
        Op::Capture {
            file,
            target,
            entry,
        } => o.vault.capture(&file, &target, &entry, now.date()),
    }
}

/// After an operation wrote `paths`: forget our save chains for them (the
/// UI's base is now stale), refresh the index, the file list, the agenda and
/// the loaded file.
fn after_change(o: &mut Open, paths: &[String], loaded: Option<&str>, emit: &mut dyn FnMut(Event)) {
    for p in paths {
        o.written.remove(p);
    }
    match o.index.sync(&o.vault) {
        Ok(_) => {
            emit(Event::Files(Arc::new(file_infos(o))));
            emit_agenda(o, emit);
        }
        Err(e) => emit(Event::Error(e.to_string())),
    }
    if let Some(l) = loaded.filter(|l| paths.iter().any(|p| p == l)) {
        emit_document(o, l, emit);
    }
}

fn emit_agenda(o: &Open, emit: &mut dyn FnMut(Event)) {
    let Some(days) = o.agenda else { return };
    match o.index.all() {
        Ok(notes) => {
            let opts = agenda::Options {
                today: chrono::Local::now().date_naive(),
                days,
                warning_days: o.settings.warning_days,
                include_done: false,
            };
            emit(Event::Agenda(Arc::new(agenda::build(&notes, &opts))));
        }
        Err(e) => emit(Event::Error(e.to_string())),
    }
}

/// Every file (its top level) and every heading, in vault order.
fn targets(o: &Open) -> Vec<RefileTarget> {
    let notes = o.index.all().unwrap_or_default();
    let mut out: Vec<RefileTarget> = Vec::new();
    let mut path_titles: Vec<String> = Vec::new();
    for n in &notes {
        let r = &n.record;
        if out.last().is_none_or(|t| t.path != r.path) {
            out.push(RefileTarget {
                path: r.path.clone(),
                heading: None,
                outline: String::new(),
            });
            path_titles.clear();
        }
        path_titles.truncate(r.level.saturating_sub(1));
        path_titles.push(r.title.clone());
        out.push(RefileTarget {
            path: r.path.clone(),
            heading: Some(HeadingRef {
                path: r.path.clone(),
                index: r.heading,
                title: r.title.clone(),
            }),
            outline: path_titles.join(" / "),
        });
    }
    out
}

/// Writes one save; true when the file changed.
fn save(o: &mut Open, path: &str, p: Pending, emit: &mut dyn FnMut(Event)) -> bool {
    let disk = match o.vault.revision(path) {
        Ok(r) => r,
        Err(e) => {
            emit(Event::Error(format!("{path}: {e}")));
            return false;
        }
    };
    // A base we superseded ourselves still counts while the disk is at our
    // last write.
    let chain = o.written.get(path);
    let ours = chain.is_some_and(|c| c.last() == Some(&disk) && c.contains(&p.base));
    let expected = if ours { disk } else { p.base };
    match o.vault.write(path, &p.text, expected) {
        Ok(revision) => {
            let chain = o.written.entry(path.to_string()).or_default();
            if !ours {
                chain.clear();
                chain.push(p.base);
            }
            chain.push(Some(revision));
            emit(Event::Saved {
                path: path.to_string(),
                text: p.text,
                revision,
            });
            true
        }
        Err(WriteError::Stale { .. }) => {
            o.written.remove(path);
            let disk = match o.vault.document(path) {
                Ok(d) => Some(d),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    emit(Event::Error(format!("{path}: {e}")));
                    return false;
                }
            };
            emit(Event::Stale {
                path: path.to_string(),
                disk,
            });
            false
        }
        Err(e) => {
            emit(Event::Error(format!("Couldn't save {path}: {e}")));
            false
        }
    }
}

fn emit_document(o: &Open, path: &str, emit: &mut dyn FnMut(Event)) {
    match o.vault.document(path) {
        Ok((doc, rev)) => emit(Event::Document(path.to_string(), doc, rev)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            emit(Event::Missing(path.to_string()));
        }
        Err(e) => emit(Event::Error(format!("{path}: {e}"))),
    }
}

fn file_infos(o: &Open) -> Vec<FileInfo> {
    let Ok(listing) = o.vault.list() else {
        return Vec::new();
    };
    let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let notes = o.index.all().unwrap_or_default();
    for n in &notes {
        let c = counts.entry(n.record.path.as_str()).or_default();
        c.0 += 1;
        if n.record.keyword.is_some() && !n.record.done {
            c.1 += 1;
        }
    }
    listing
        .files
        .iter()
        .map(|f| {
            let (headings, open_tasks) = counts.get(f.path.as_str()).copied().unwrap_or_default();
            FileInfo {
                path: f.path.clone(),
                headings,
                open_tasks,
                conflict: listing.conflicts.iter().any(|c| c.path == f.path),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Runs the worker on its own thread and collects its events.
    struct Harness {
        tx: Sender<Input>,
        events: Receiver<Event>,
    }

    impl Harness {
        fn start() -> Harness {
            let (tx, rx) = mpsc::channel();
            let (etx, events) = mpsc::channel();
            let myself = tx.clone();
            thread::spawn(move || run(&rx, &myself, &mut |e| drop(etx.send(e))));
            Harness { tx, events }
        }

        fn send(&self, r: Request) {
            self.tx.send(Input::Request(r)).unwrap();
        }

        /// The next event matching `pick`, within a few seconds.
        fn wait<T>(&self, mut pick: impl FnMut(Event) -> Option<T>) -> T {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                let e = self.events.recv_timeout(left).expect("worker event");
                if let Some(t) = pick(e) {
                    return t;
                }
            }
        }
    }

    #[test]
    fn opens_loads_and_follows_outside_edits() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("v");
        fs::create_dir_all(vault.join("sub")).unwrap();
        fs::write(vault.join("a.org"), "* TODO one\n* two\n").unwrap();
        fs::write(vault.join("sub/b.org"), "* three\n").unwrap();
        let h = Harness::start();
        h.send(Request::Open(OpenVault {
            root: vault.clone(),
            keywords: Keywords::default(),
            index_dir: Some(dir.path().join("index")),
            ..OpenVault::new(PathBuf::new(), &config::Config::default())
        }));
        let files = h.wait(|e| match e {
            Event::Files(f) => Some(f),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        });
        assert_eq!(
            files
                .iter()
                .map(|f| (f.path.as_str(), f.headings, f.open_tasks))
                .collect::<Vec<_>>(),
            [("a.org", 2, 1), ("sub/b.org", 1, 0)]
        );
        h.send(Request::Load("a.org".into()));
        let doc = h.wait(|e| match e {
            Event::Document(p, d, _) if p == "a.org" => Some(d),
            _ => None,
        });
        assert_eq!(doc.headings().len(), 2);
        // Another program edits the open file: a fresh document arrives.
        thread::sleep(Duration::from_millis(50));
        fs::write(vault.join("a.org"), "* TODO one\n* two\n* added by emacs\n").unwrap();
        let doc = h.wait(|e| match e {
            Event::Document(p, d, _) if p == "a.org" => Some(d),
            _ => None,
        });
        assert_eq!(doc.headings().len(), 3);
        // And deletes it.
        fs::remove_file(vault.join("a.org")).unwrap();
        h.wait(|e| matches!(e, Event::Missing(p) if p == "a.org").then_some(()));
    }

    fn opened(dir: &std::path::Path, files: &[(&str, &str)]) -> (Harness, PathBuf) {
        let vault = dir.join("v");
        fs::create_dir_all(&vault).unwrap();
        for (p, t) in files {
            let full = vault.join(p);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, t).unwrap();
        }
        let h = Harness::start();
        h.send(Request::Open(OpenVault {
            root: vault.clone(),
            keywords: Keywords::default(),
            index_dir: Some(dir.join("index")),
            ..OpenVault::new(PathBuf::new(), &config::Config::default())
        }));
        h.wait(|e| matches!(e, Event::Files(_)).then_some(()));
        (h, vault)
    }

    fn save(h: &Harness, path: &str, text: &str, base: Option<Revision>) {
        h.send(Request::Save {
            path: path.into(),
            text: Arc::new(text.into()),
            base,
        });
    }

    #[test]
    fn saves_are_debounced_and_chained() {
        let dir = tempfile::tempdir().unwrap();
        let (h, vault) = opened(dir.path(), &[("a.org", "* one\n")]);
        h.send(Request::Load("a.org".into()));
        let base = h.wait(|e| match e {
            Event::Document(_, _, r) => Some(r),
            _ => None,
        });
        // Two quick saves: only the last is written.
        save(&h, "a.org", "* one\nx\n", Some(base));
        save(&h, "a.org", "* one\nxy\n", Some(base));
        let (text, rev) = h.wait(|e| match e {
            Event::Saved { text, revision, .. } => Some((text, revision)),
            _ => None,
        });
        assert_eq!(text.as_str(), "* one\nxy\n");
        assert_eq!(
            fs::read_to_string(vault.join("a.org")).unwrap(),
            "* one\nxy\n"
        );
        // A save still based on the old revision (the UI hadn't heard of the
        // write yet) is not stale.
        save(&h, "a.org", "* one\nxyz\n", Some(base));
        h.send(Request::Flush);
        let mut saved = None;
        h.wait(|e| match e {
            Event::Saved { revision, .. } => {
                saved = Some(revision);
                None
            }
            Event::Stale { .. } => panic!("not stale"),
            Event::Flushed => Some(()),
            _ => None,
        });
        assert!(saved.is_some_and(|r| r != rev));
        assert_eq!(
            fs::read_to_string(vault.join("a.org")).unwrap(),
            "* one\nxyz\n"
        );
    }

    #[test]
    fn outside_change_makes_a_save_stale() {
        let dir = tempfile::tempdir().unwrap();
        let (h, vault) = opened(dir.path(), &[("a.org", "* one\n")]);
        h.send(Request::Load("a.org".into()));
        let base = h.wait(|e| match e {
            Event::Document(_, _, r) => Some(r),
            _ => None,
        });
        thread::sleep(Duration::from_millis(20));
        fs::write(vault.join("a.org"), "* one\n* from emacs\n").unwrap();
        save(&h, "a.org", "* mine\n", Some(base));
        h.send(Request::Flush);
        let disk = h.wait(|e| match e {
            Event::Stale { disk, .. } => Some(disk),
            Event::Saved { .. } => panic!("must not overwrite"),
            _ => None,
        });
        let (doc, _) = disk.unwrap();
        assert_eq!(doc.text(), "* one\n* from emacs\n");
        assert_eq!(
            fs::read_to_string(vault.join("a.org")).unwrap(),
            "* one\n* from emacs\n"
        );
    }

    #[test]
    fn operations_flush_saves_then_run_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let (h, vault) = opened(
            dir.path(),
            &[("a.org", "* TODO one\nSCHEDULED: <2026-11-01 Sun>\n* two\n")],
        );
        h.send(Request::Agenda(3));
        let a = h.wait(|e| match e {
            Event::Agenda(a) => Some(a),
            _ => None,
        });
        assert_eq!(a.overdue.len(), 1);
        h.send(Request::Load("a.org".into()));
        let base = h.wait(|e| match e {
            Event::Document(_, _, r) => Some(r),
            _ => None,
        });
        // A waiting save is written before the operation reads the file.
        save(
            &h,
            "a.org",
            "* TODO one\nSCHEDULED: <2026-11-01 Sun>\n* two\n* three\n",
            Some(base),
        );
        h.send(Request::Run(Op::Archive(HeadingRef {
            path: "a.org".into(),
            index: 2,
            title: "three".into(),
        })));
        let out = h.wait(|e| match e {
            Event::Done(o) => Some(o),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        });
        assert_eq!(out.message, "Archived to a.org_archive");
        assert_eq!(
            fs::read_to_string(vault.join("a.org")).unwrap(),
            "* TODO one\nSCHEDULED: <2026-11-01 Sun>\n* two\n"
        );
        assert!(
            fs::read_to_string(vault.join("a.org_archive"))
                .unwrap()
                .starts_with("* three\n")
        );
        h.send(Request::Restore(out.changes.clone()));
        h.wait(|e| match e {
            Event::Restored(_) => Some(()),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        });
        assert!(
            fs::read_to_string(vault.join("a.org"))
                .unwrap()
                .ends_with("* three\n")
        );
        assert!(!vault.join("a.org_archive").exists());
        // Checking the task off from the agenda; the new agenda follows.
        h.send(Request::Run(Op::Complete(
            HeadingRef {
                path: "a.org".into(),
                index: 0,
                title: "one".into(),
            },
            Complete::Toggle,
        )));
        let a = h.wait(|e| match e {
            Event::Agenda(a) => Some(a),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        });
        assert_eq!(a.overdue, []);
        h.send(Request::Targets);
        let t = h.wait(|e| match e {
            Event::Targets(t) => Some(t),
            _ => None,
        });
        let outlines: Vec<&str> = t.iter().map(|t| t.outline.as_str()).collect();
        assert_eq!(outlines, ["", "one", "two", "three"]);
    }

    #[test]
    fn images_and_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let (h, _) = opened(dir.path(), &[("notes/a.org", "* one\n")]);
        let file = || "notes/a.org".to_string();
        h.send(Request::Attach {
            file: file(),
            name: "shot.png".into(),
            bytes: Arc::new(b"PNG".to_vec()),
        });
        let link = h.wait(|e| match e {
            Event::Attached { link, .. } => Some(link),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        });
        assert_eq!(link, "attachments/a/shot.png");
        h.send(Request::LoadImage {
            file: file(),
            path: link.clone(),
        });
        let data = h.wait(|e| match e {
            Event::Image { data, .. } => Some(data),
            _ => None,
        });
        assert_eq!(data.unwrap().as_slice(), b"PNG");
        h.send(Request::LoadImage {
            file: file(),
            path: "nope.png".into(),
        });
        assert!(h.wait(|e| match e {
            Event::Image { data, .. } => Some(data.is_none()),
            _ => None,
        }));
        // A dropped file is copied in under its own name.
        let outside = dir.path().join("photo.jpg");
        fs::write(&outside, b"JPG").unwrap();
        h.send(Request::AttachFile {
            file: file(),
            source: outside,
        });
        let link = h.wait(|e| match e {
            Event::Attached { link, .. } => Some(link),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        });
        assert_eq!(link, "attachments/a/photo.jpg");
    }

    /// Waits for the diagram opened from `link`.
    fn open_diagram_event(h: &Harness, link: &str) -> (crate::excalidraw::Scene, Option<Revision>) {
        h.send(Request::OpenDiagram {
            file: "notes/a.org".into(),
            path: link.into(),
        });
        h.wait(|e| match e {
            Event::Diagram { scene, base, .. } => Some((*scene, base)),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        })
    }

    #[test]
    fn diagrams_are_a_source_and_a_generated_svg() {
        use crate::excalidraw::{Element, Kind, Scene, to_svg};
        let dir = tempfile::tempdir().unwrap();
        let (h, vault) = opened(dir.path(), &[("notes/a.org", "* one\n")]);
        let file = || "notes/a.org".to_string();
        let mut scene = Scene::new();
        let mut rect = Element::new(Kind::Rectangle, 0.0, 0.0, "#1e1e1e");
        rect.width = 10.0;
        rect.height = 10.0;
        scene.elements.push(rect);
        h.send(Request::SaveDiagram {
            file: file(),
            path: None,
            base: None,
            source: Arc::new(scene.to_json()),
            svg: Arc::new(to_svg(&scene)),
        });
        let link = h.wait(|e| match e {
            Event::Attached { link, .. } => Some(link),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        });
        let stem = link.strip_suffix(".svg").unwrap().to_string();
        assert!(stem.starts_with("attachments/a/diagram-"), "{link}");
        let source = vault.join("notes").join(format!("{stem}.excalidraw"));
        assert!(source.exists());
        // Edited and saved over: the source first, then the SVG.
        let (mut scene, base) = open_diagram_event(&h, &link);
        assert_eq!(scene.elements.len(), 1);
        let mut moved = scene.elements[0].clone();
        moved.shift(30.0, 0.0);
        scene.elements.push(moved);
        h.send(Request::SaveDiagram {
            file: file(),
            path: Some(link.clone()),
            base,
            source: Arc::new(scene.to_json()),
            svg: Arc::new(to_svg(&scene)),
        });
        h.wait(|e| match e {
            Event::DiagramSaved { .. } => Some(()),
            Event::Error(e) => panic!("{e}"),
            _ => None,
        });
        let text = fs::read_to_string(&source).unwrap();
        assert_eq!(Scene::parse(&text).unwrap().elements.len(), 2);
        // Saving again from the stale revision is refused.
        h.send(Request::SaveDiagram {
            file: file(),
            path: Some(link.clone()),
            base,
            source: Arc::new(scene.to_json()),
            svg: Arc::new(to_svg(&scene)),
        });
        let msg = h.wait(|e| match e {
            Event::Error(e) => Some(e),
            Event::DiagramSaved { .. } => panic!("must refuse"),
            _ => None,
        });
        assert!(msg.contains("changed since it was opened"), "{msg}");
        // Another tool edits the source: the SVG is regenerated on display.
        thread::sleep(Duration::from_millis(20));
        let mut outside = Scene::parse(&text).unwrap();
        outside.elements.truncate(1);
        fs::write(&source, outside.to_json()).unwrap();
        h.send(Request::LoadImage {
            file: file(),
            path: link.clone(),
        });
        let data = h.wait(|e| match e {
            Event::Image { data, .. } => Some(data),
            _ => None,
        });
        let svg = String::from_utf8(data.unwrap().to_vec()).unwrap();
        assert_eq!(svg.matches("<rect").count(), 2, "{svg}");
        let on_disk = fs::read_to_string(vault.join("notes").join(&link)).unwrap();
        assert_eq!(on_disk, svg);
        // A missing SVG is made again too.
        fs::remove_file(vault.join("notes").join(&link)).unwrap();
        h.send(Request::LoadImage {
            file: file(),
            path: link.clone(),
        });
        h.wait(|e| match e {
            Event::Image { data, .. } => Some(data),
            _ => None,
        })
        .expect("regenerated");
        assert!(vault.join("notes").join(&link).exists());
    }

    #[test]
    fn old_blossom_svgs_get_a_source_on_edit() {
        let dir = tempfile::tempdir().unwrap();
        let old = "<svg><metadata><blossom-diagram xmlns=\"urn:blossom:diagram\">{\"version\":1,\"shapes\":[{\"kind\":\"rect\",\"x\":16.0,\"y\":16.0,\"w\":80.0,\"h\":40.0,\"color\":\"#1f1f1f\",\"width\":2.0}]}</blossom-diagram></metadata></svg>";
        let (h, vault) = opened(
            dir.path(),
            &[("notes/a.org", "* one\n"), ("notes/old.svg", old)],
        );
        let (scene, base) = open_diagram_event(&h, "old.svg");
        assert_eq!(scene.elements.len(), 1);
        assert!(base.is_some());
        let source = fs::read_to_string(vault.join("notes/old.excalidraw")).unwrap();
        assert!(source.contains("\"type\": \"rectangle\""), "{source}");
        // A plain SVG has nothing to edit.
        fs::write(vault.join("notes/plain.svg"), "<svg/>").unwrap();
        h.send(Request::OpenDiagram {
            file: "notes/a.org".into(),
            path: "plain.svg".into(),
        });
        let msg = h.wait(|e| match e {
            Event::Error(e) => Some(e),
            _ => None,
        });
        assert!(msg.contains("no .excalidraw file beside it"), "{msg}");
    }

    #[test]
    fn reports_a_bad_vault() {
        let h = Harness::start();
        h.send(Request::Open(OpenVault {
            root: PathBuf::from("/nonexistent/blossom-vault"),
            keywords: Keywords::default(),
            ..OpenVault::new(PathBuf::new(), &config::Config::default())
        }));
        let msg = h.wait(|e| match e {
            Event::Error(m) => Some(m),
            _ => None,
        });
        assert!(msg.starts_with("/nonexistent/blossom-vault"), "{msg}");
    }
}
