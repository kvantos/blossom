//! `blsm search --semantic`: headings ranked by meaning.
//!
//! The query's plain words are the text to match by meaning; any Orgzly
//! filters (`t.work`, `i.todo`, …) narrow the candidates first
//! ([`Query::split_text`]). Vectors for every heading are kept beside the
//! index and brought up to date before each search, so only new and changed
//! headings are embedded. The built-in model is downloaded on first use.
//! Progress goes to stderr (live on a terminal), results to stdout.

use std::error::Error;
use std::io::{IsTerminal, Write};
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use blossom_index::{Note, Query};
use blossom_semantic::{Backend, Embedder, SharedEmbedder, VectorStore, open_embedder};
use blossom_vault::config;

use crate::{Session, fail, output};

/// Headings per embedding call.
const BATCH: usize = 32;

/// Vectors are saved at least this often while embedding, so stopping a
/// long first run (Ctrl-C) keeps most of the work.
const SAVE_EVERY: Duration = Duration::from_secs(15);

/// Runs a semantic search and prints at most `limit` headings, best first.
pub fn search(s: &Session, query: &str, limit: usize) -> Result<(), Box<dyn Error>> {
    let (words, filter) = Query::parse(query).split_text();
    if words.is_empty() {
        return Err(fail(
            "--semantic needs words to match by meaning, e.g. `blsm search --semantic fix the bike`",
        ));
    }
    let backend = Backend::from_config(&s.config.semantic).map_err(fail)?;
    let embedder = embedder(&backend)?;
    let mut store = VectorStore::open(&s.index_dir, embedder.as_ref());
    embed_all(&mut store, embedder.as_ref(), &s.index.all()?)?;
    let candidates = s.index.search(&filter, &s.ctx)?;
    let hits = store.search(embedder.as_ref(), &words, &candidates, limit)?;
    let ranked: Vec<(f32, &Note)> = hits
        .iter()
        .map(|h| (h.score, &candidates[h.index]))
        .collect();
    output::scored(&ranked, s.json)
}

/// The configured embedder, downloading the built-in model if needed.
fn embedder(backend: &Backend) -> Result<SharedEmbedder, Box<dyn Error>> {
    let data_dir = match backend {
        Backend::Builtin(_) => config::data_dir()
            .ok_or_else(|| fail("no data folder for the model (is $HOME set?)"))?,
        Backend::Server(_) => PathBuf::new(),
    };
    #[cfg(feature = "builtin-model")]
    if let Backend::Builtin(m) = backend
        && !m.is_downloaded(&data_dir)
    {
        download(*m, &data_dir)?;
    }
    Ok(open_embedder(backend, &data_dir)?)
}

#[cfg(feature = "builtin-model")]
fn download(m: blossom_semantic::Model, data_dir: &std::path::Path) -> Result<(), Box<dyn Error>> {
    eprintln!(
        "blsm: downloading the {} model ({} MB, only once) to {}",
        m.display_name(),
        m.download_mb(),
        m.dir(data_dir).display()
    );
    let mut meter = Meter::new();
    let result = blossom_semantic::download(m, data_dir, &mut |p| {
        meter.show(|| {
            format!(
                "blsm: downloaded {} of {} MB",
                p.done / 1_000_000,
                m.download_mb()
            )
        });
        ControlFlow::Continue(())
    });
    meter.finish();
    Ok(result?)
}

/// Embeds every heading that has no vector yet, saving as it goes.
fn embed_all(
    store: &mut VectorStore,
    embedder: &dyn Embedder,
    notes: &[Note],
) -> Result<(), Box<dyn Error>> {
    let pending = store.pending(notes);
    if pending == 0 {
        // Nothing to embed; drop vectors of headings that are gone.
        let r = store.sync(embedder, notes, BATCH, &mut |_| ControlFlow::Continue(()))?;
        if r.removed > 0 {
            store.save()?;
        }
        return Ok(());
    }
    eprintln!(
        "blsm: embedding {pending} {} with {} (next time only new and changed ones)",
        if pending == 1 { "heading" } else { "headings" },
        embedder.id()
    );
    let mut meter = Meter::new();
    let mut done = 0;
    loop {
        let started = Instant::now();
        let result = store.sync(embedder, notes, BATCH, &mut |p| {
            meter.show(|| format!("blsm: embedded {} of {pending}", done + p.done));
            if started.elapsed() > SAVE_EVERY {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        // Keep what was embedded, even when the embedder failed part-way.
        let saved = store.save();
        let report = match result.and_then(|r| saved.map(|()| r)) {
            Ok(r) => r,
            Err(e) => {
                meter.finish();
                return Err(e.into());
            }
        };
        done += report.added;
        if report.left == 0 {
            break;
        }
    }
    meter.finish();
    Ok(())
}

/// A progress line on stderr, redrawn in place, only on a terminal.
struct Meter {
    tty: bool,
    last: Option<Instant>,
    shown: bool,
}

impl Meter {
    fn new() -> Meter {
        Meter {
            tty: std::io::stderr().is_terminal(),
            last: None,
            shown: false,
        }
    }

    /// Redraws with `line()`, at most five times a second.
    fn show(&mut self, line: impl FnOnce() -> String) {
        if !self.tty
            || self
                .last
                .is_some_and(|t| t.elapsed() < Duration::from_millis(200))
        {
            return;
        }
        self.last = Some(Instant::now());
        self.shown = true;
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "\r\x1b[2K{}", line());
        let _ = err.flush();
    }

    /// Clears the line.
    fn finish(&mut self) {
        if self.shown {
            self.shown = false;
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[2K");
            let _ = err.flush();
        }
    }
}
