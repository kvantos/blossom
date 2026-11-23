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

use blossom_core::{Document, Keywords};
use blossom_index::NoteIndex;
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
            Input::Changed(g) => {
                // A notice from a vault that has since been closed is stale.
                let Some(o) = open.as_mut().filter(|o| o.generation == g) else {
                    continue;
                };
                // Extra notices are harmless: a sync with no changes is cheap.
                match o.index.sync(&o.vault) {
                    Ok(report) => {
                        emit(Event::Files(Arc::new(file_infos(o))));
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
            Ok(_) => emit(Event::Files(Arc::new(file_infos(o)))),
            Err(e) => emit(Event::Error(e.to_string())),
        }
    }
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
            ignore: vec![],
            index_dir: Some(dir.path().join("index")),
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
            fs::write(vault.join(p), t).unwrap();
        }
        let h = Harness::start();
        h.send(Request::Open(OpenVault {
            root: vault.clone(),
            keywords: Keywords::default(),
            ignore: vec![],
            index_dir: Some(dir.join("index")),
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
    fn reports_a_bad_vault() {
        let h = Harness::start();
        h.send(Request::Open(OpenVault {
            root: PathBuf::from("/nonexistent/blossom-vault"),
            keywords: Keywords::default(),
            ignore: vec![],
            index_dir: None,
        }));
        let msg = h.wait(|e| match e {
            Event::Error(m) => Some(m),
            _ => None,
        });
        assert!(msg.starts_with("/nonexistent/blossom-vault"), "{msg}");
    }
}
