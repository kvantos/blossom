//! Search by meaning, in the background.
//!
//! A thread of its own (started by the worker the first time the search
//! screen's "By meaning" switch is on) owns the embedder and the vector
//! store. It downloads the built-in model when asked, loads it, embeds every
//! heading that has no vector yet, a batch at a time, and answers queries
//! with whatever vectors it has (again every [`ANSWER_EVERY`] while
//! embedding, and when done); new messages are looked at between batches,
//! so a query never waits for a long first run. Vectors are saved every
//! [`SAVE_EVERY`], when embedding finishes, when asked ([`In::Save`], before
//! the app closes) and when the thread stops.
//!
//! It reports through `out`: [`Event::Meaning`] with a [`Status`] when that
//! changes, and [`Event::Search`] with ranked results.

use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use blossom_index::Note;
use blossom_semantic::{Backend, SharedEmbedder, VectorStore, open_embedder};

use crate::worker::{Event, SearchResults};

/// Results shown at most.
pub const LIMIT: usize = 50;

/// Headings per embedding call.
const BATCH: usize = 16;

/// Vectors are saved at least this often while embedding.
const SAVE_EVERY: Duration = Duration::from_secs(15);

/// While embedding, the current query is answered again this often, with
/// the vectors made so far.
const ANSWER_EVERY: Duration = Duration::from_secs(3);

/// Progress is reported at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

/// Where search by meaning stands, for the search screen.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "builtin-model"), allow(dead_code))] // no downloads
pub enum Status {
    /// The built-in model isn't downloaded yet; the user decides.
    NeedsDownload {
        /// `English` or `multilingual`.
        model: String,
        /// Download size in MB.
        mb: u64,
    },
    /// Downloading the model, MB so far and in all.
    Downloading {
        /// MB received.
        done: u64,
        /// MB in all.
        total: u64,
    },
    /// Loading the model or connecting to the server.
    Loading,
    /// Reading headings for meaning: so many of so many.
    Embedding {
        /// Headings done in this run.
        done: usize,
        /// Headings to do in this run.
        total: usize,
    },
    /// Every heading has a vector.
    Ready,
    /// Something went wrong; the message says what.
    Failed(String),
}

/// A query to answer.
#[derive(Clone, Debug)]
pub struct Ask {
    /// The query as typed (results carry it back).
    pub query: String,
    /// The words to match by meaning.
    pub words: String,
    /// The headings the query's filters allow.
    pub candidates: Vec<Note>,
}

/// A message to the thread.
#[derive(Debug)]
pub enum In {
    /// Every heading in the vault (sent first, then after every change).
    Notes(Arc<Vec<Note>>),
    /// The query to answer now and after every improvement, or none.
    Ask(Option<Ask>),
    /// Download the built-in model.
    Download,
    /// Try again after a failure.
    Retry,
    /// Save the vectors now, then answer (the app is closing).
    Save(Sender<()>),
}

/// The running thread. Dropping it stops the thread (and a download).
pub struct Meaning {
    tx: Sender<In>,
    cancel: Arc<AtomicBool>,
}

impl Meaning {
    /// Starts the thread. `backend` is the `[semantic]` choice (or why it is
    /// unusable); a built-in model lives under `data_dir`; vectors go under
    /// `index_dir`.
    pub fn start(
        backend: Result<Backend, String>,
        data_dir: Option<PathBuf>,
        index_dir: PathBuf,
        out: impl FnMut(Event) + Send + 'static,
    ) -> Meaning {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut state = State {
            backend,
            data_dir,
            index_dir,
            out: Box::new(out),
            cancel: Arc::clone(&cancel),
            embedder: None,
            store: None,
            notes: None,
            ask: None,
            answered: true,
            blocked: false,
            status: None,
            last_progress: None,
            embedded: 0,
            dirty: false,
            last_save: Instant::now(),
            last_answer: Instant::now(),
        };
        thread::spawn(move || state.run(&rx));
        Meaning { tx, cancel }
    }

    /// Sends `message`; a stopped thread is ignored.
    pub fn send(&self, message: In) {
        let _ = self.tx.send(message);
    }

    /// Stops a running download.
    pub fn cancel_download(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Meaning {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg_attr(not(feature = "builtin-model"), allow(dead_code))] // no downloads
struct State {
    backend: Result<Backend, String>,
    data_dir: Option<PathBuf>,
    index_dir: PathBuf,
    out: Box<dyn FnMut(Event) + Send>,
    cancel: Arc<AtomicBool>,
    embedder: Option<SharedEmbedder>,
    store: Option<VectorStore>,
    notes: Option<Arc<Vec<Note>>>,
    ask: Option<Ask>,
    /// Whether the current ask has been answered with the current vectors.
    answered: bool,
    /// Waiting for the user: to download the model, or to retry.
    blocked: bool,
    /// The last status sent.
    status: Option<Status>,
    last_progress: Option<Instant>,
    /// Headings embedded since the last `Ready`.
    embedded: usize,
    /// Vectors not saved yet.
    dirty: bool,
    last_save: Instant,
    last_answer: Instant,
}

impl State {
    fn run(&mut self, rx: &Receiver<In>) {
        let mut stash: VecDeque<In> = VecDeque::new();
        loop {
            let message = if let Some(m) = stash.pop_front() {
                Some(m)
            } else if self.has_work() {
                match rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break,
                }
            } else {
                match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => break,
                }
            };
            match message {
                Some(m) => self.handle(m),
                None => self.work(rx, &mut stash),
            }
        }
        self.save();
    }

    fn has_work(&self) -> bool {
        if self.blocked || self.notes.is_none() {
            return false;
        }
        match (&self.embedder, &self.store) {
            (Some(_), Some(store)) => {
                (self.ask.is_some() && !self.answered)
                    || self.notes.as_ref().is_some_and(|n| store.pending(n) > 0)
                    || self.status != Some(Status::Ready)
            }
            // Load on first ask.
            _ => self.ask.is_some(),
        }
    }

    fn handle(&mut self, message: In) {
        match message {
            In::Notes(notes) => self.notes = Some(notes),
            In::Ask(ask) => {
                self.ask = ask;
                self.answered = false;
            }
            In::Download => self.download(),
            In::Retry => {
                self.blocked = false;
                self.answered = false;
            }
            In::Save(done) => {
                self.save();
                let _ = done.send(());
            }
        }
    }

    fn emit_status(&mut self, status: Status) {
        if self.status.as_ref() != Some(&status) {
            self.status = Some(status.clone());
            (self.out)(Event::Meaning(status));
        }
    }

    fn fail(&mut self, message: String) {
        self.blocked = true;
        self.emit_status(Status::Failed(message));
    }

    fn work(&mut self, rx: &Receiver<In>, stash: &mut VecDeque<In>) {
        if self.embedder.is_none() {
            self.load();
            return;
        }
        if self.ask.is_some() && !self.answered {
            self.answer();
            return;
        }
        self.embed_round(rx, stash);
    }

    /// Loads the embedder, or says the model needs downloading.
    fn load(&mut self) {
        let backend = match &self.backend {
            Ok(b) => b.clone(),
            Err(e) => {
                let e = e.clone();
                self.fail(e);
                return;
            }
        };
        let data_dir = self.data_dir.clone().unwrap_or_default();
        if let Backend::Builtin(m) = &backend {
            if self.data_dir.is_none() {
                self.fail("No data folder for the model (is $HOME set?)".into());
                return;
            }
            if cfg!(feature = "builtin-model") && !m.is_downloaded(&data_dir) {
                self.blocked = true;
                self.emit_status(Status::NeedsDownload {
                    model: m.display_name().into(),
                    mb: m.download_mb(),
                });
                return;
            }
        }
        self.emit_status(Status::Loading);
        match open_embedder(&backend, &data_dir) {
            Ok(e) => {
                self.store = Some(VectorStore::open(&self.index_dir, e.as_ref()));
                self.embedder = Some(e);
                self.answered = false;
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    #[cfg(feature = "builtin-model")]
    fn download(&mut self) {
        let (Ok(Backend::Builtin(m)), Some(data_dir)) = (&self.backend, self.data_dir.clone())
        else {
            return;
        };
        let m = *m;
        self.cancel.store(false, Ordering::Relaxed);
        let cancel = Arc::clone(&self.cancel);
        let mb = |b: u64| b / 1_000_000;
        let total = m.download_mb();
        self.emit_status(Status::Downloading { done: 0, total });
        let result = blossom_semantic::download(m, &data_dir, &mut |p| {
            if cancel.load(Ordering::Relaxed) {
                return ControlFlow::Break(());
            }
            if self
                .last_progress
                .is_none_or(|t| t.elapsed() >= PROGRESS_EVERY)
            {
                self.last_progress = Some(Instant::now());
                self.emit_status(Status::Downloading {
                    done: mb(p.done),
                    total,
                });
            }
            ControlFlow::Continue(())
        });
        match result {
            Ok(()) => {
                self.blocked = false;
                self.answered = false;
            }
            Err(_) if self.cancel.load(Ordering::Relaxed) => {
                self.emit_status(Status::NeedsDownload {
                    model: m.display_name().into(),
                    mb: m.download_mb(),
                });
            }
            Err(e) => self.fail(format!("Couldn't download the model: {e}")),
        }
    }

    #[cfg(not(feature = "builtin-model"))]
    #[allow(clippy::unused_self)] // the same call as with the feature
    fn download(&mut self) {}

    fn answer(&mut self) {
        self.answered = true;
        self.last_answer = Instant::now();
        let (Some(e), Some(store), Some(ask)) = (&self.embedder, &self.store, &self.ask) else {
            return;
        };
        match store.search(e.as_ref(), &ask.words, &ask.candidates, LIMIT) {
            Ok(hits) => {
                let results = SearchResults {
                    query: ask.query.clone(),
                    meaning: true,
                    notes: hits
                        .iter()
                        .map(|h| ask.candidates[h.index].clone())
                        .collect(),
                    scores: hits.iter().map(|h| h.score).collect(),
                };
                (self.out)(Event::Search(Arc::new(results)));
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    /// Embeds batches until done, a message arrives or it is time to save.
    fn embed_round(&mut self, rx: &Receiver<In>, stash: &mut VecDeque<In>) {
        let (Some(e), Some(notes)) = (&self.embedder, self.notes.clone()) else {
            return;
        };
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let pending = store.pending(&notes);
        if pending == 0 {
            self.embedded = 0;
            self.save();
            self.emit_status(Status::Ready);
            // Answer again with every vector in place.
            self.answered = self.ask.is_none();
            return;
        }
        let total = self.embedded + pending;
        let started = Instant::now();
        let embedded = self.embedded;
        let mut progress = Vec::new();
        let result = store.sync(e.as_ref(), &notes, BATCH, &mut |p| {
            progress.push(Status::Embedding {
                done: embedded + p.done,
                total,
            });
            match rx.try_recv() {
                Ok(m) => {
                    stash.push_back(m);
                    return ControlFlow::Break(());
                }
                Err(TryRecvError::Disconnected) => return ControlFlow::Break(()),
                Err(TryRecvError::Empty) => {}
            }
            if started.elapsed() >= PROGRESS_EVERY {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        if let Some(s) = progress.pop() {
            self.emit_status(s);
        } else if self.status.is_none() || self.status == Some(Status::Ready) {
            self.emit_status(Status::Embedding {
                done: embedded,
                total,
            });
        }
        match result {
            Ok(r) => {
                self.embedded += r.added;
                self.dirty |= r.added + r.removed > 0;
            }
            Err(e) => {
                self.save();
                self.fail(e.to_string());
                return;
            }
        }
        if self.last_save.elapsed() >= SAVE_EVERY {
            self.save();
        }
        if self.last_answer.elapsed() >= ANSWER_EVERY {
            // Better results so far.
            self.answered = self.ask.is_none();
        }
    }

    fn save(&mut self) {
        self.last_save = Instant::now();
        if !self.dirty {
            return;
        }
        if let Some(store) = &self.store {
            match store.save() {
                Ok(()) => self.dirty = false,
                Err(e) => (self.out)(Event::Error(format!("Couldn't save the vectors: {e}"))),
            }
        }
    }
}
