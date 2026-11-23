//! `blsm`: the Blossom command-line tool.
//!
//! Every command first brings the search index up to date with the vault
//! (only changed files are re-read), so results always reflect the files.

mod config;
mod output;

use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use blossom_core::{EditError, InsertAt, Keywords, NewHeading};
use blossom_index::{MatchContext, NoteIndex, Query, SyncReport, agenda};
use blossom_vault::{Vault, WriteError};
use chrono::NaiveDateTime;
use clap::{Parser, Subcommand};

use crate::config::Config;

/// Search, agenda and capture for a folder of org files.
#[derive(Parser, Debug)]
#[command(name = "blsm", version, about)]
struct Cli {
    /// The vault folder (default: `vault` in the config file).
    #[arg(long, global = true, value_name = "DIR")]
    vault: Option<PathBuf>,
    /// The config file (default: ~/.config/blossom/config.toml).
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Where to keep the search index (default: the platform cache folder).
    #[arg(long, global = true, value_name = "DIR")]
    index_dir: Option<PathBuf>,
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    /// Pretend it is this local time, `YYYY-MM-DDTHH:MM` (for tests).
    #[arg(long, global = true, hide = true, value_parser = parse_now)]
    now: Option<NaiveDateTime>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Search headings (Orgzly query syntax, e.g. `i.todo s.today t.work`).
    Search {
        /// The query; several words are joined with spaces.
        query: Vec<String>,
    },
    /// Scheduled, deadline and event items by day, overdue first.
    Agenda {
        /// How many days, starting today.
        #[arg(long, default_value_t = 7)]
        days: u32,
        /// Include done items.
        #[arg(long)]
        all: bool,
        /// Extra query terms, e.g. `t.work`.
        query: Vec<String>,
    },
    /// Open tasks (any not-done keyword), optionally narrowed by a query.
    Todo {
        /// Extra query terms.
        query: Vec<String>,
    },
    /// Add a heading at the end of the inbox file.
    Capture {
        /// The heading title.
        #[arg(required = true)]
        title: Vec<String>,
        /// Vault-relative file to add to (default: `inbox` in the config, else inbox.org).
        #[arg(long, short)]
        file: Option<String>,
        /// A TODO keyword for the new heading.
        #[arg(long, short)]
        keyword: Option<String>,
        /// Body text under the heading.
        #[arg(long, short)]
        body: Option<String>,
    },
    /// Update the search index (it is also updated before every command).
    Index {
        /// Throw the index away and re-read every file.
        #[arg(long)]
        rebuild: bool,
    },
    /// List the vault's files with heading and open-task counts.
    Ls,
}

fn parse_now(s: &str) -> Result<NaiveDateTime, String> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("blsm: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A plain message error.
#[derive(Debug)]
struct Message(String);

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Message {}

fn fail(msg: impl Into<String>) -> Box<dyn Error> {
    Box::new(Message(msg.into()))
}

/// Everything a command needs.
struct Session {
    vault: Vault,
    index: NoteIndex,
    ctx: MatchContext,
    config: Config,
    json: bool,
    report: SyncReport,
}

fn open(cli: &Cli, rebuild: bool) -> Result<Session, Box<dyn Error>> {
    let config_path = cli.config.clone().or_else(config::default_path);
    let config = match &config_path {
        Some(p) => Config::load(p).map_err(fail)?,
        None => Config::default(),
    };
    let root = cli
        .vault
        .clone()
        .or_else(|| config.vault.as_deref().map(config::expand))
        .ok_or_else(|| {
            fail(format!(
                "no vault: pass --vault DIR or set `vault` in {}",
                config_path.map_or_else(|| "the config file".into(), |p| p.display().to_string())
            ))
        })?;
    let keywords = config
        .keywords
        .as_deref()
        .map_or_else(Keywords::default, Keywords::parse);
    let vault = Vault::open(&root, keywords.clone(), &config.ignore)
        .map_err(|e| fail(format!("vault {}: {e}", root.display())))?;
    let index_dir = cli
        .index_dir
        .clone()
        .or_else(|| config::index_dir(&root))
        .ok_or_else(|| fail("no cache folder for the index; pass --index-dir DIR"))?;
    let settings = format!("{}|{}", keywords.active.join(" "), keywords.done.join(" "));
    let mut index = NoteIndex::open(&index_dir, &settings)?;
    if rebuild {
        index.clear()?;
    }
    let report = index.sync(&vault)?;
    for c in &report.conflicts {
        eprintln!(
            "blsm: warning: {} has a Syncthing conflict copy {}",
            c.path, c.copy
        );
    }
    for (path, why) in &report.failed {
        eprintln!("blsm: warning: could not read {path}: {why}");
    }
    let now = cli
        .now
        .unwrap_or_else(|| chrono::Local::now().naive_local());
    let ctx = MatchContext {
        now,
        state_order: keywords.all().map(String::from).collect(),
        default_priority: config.default_priority.clone(),
    };
    Ok(Session {
        vault,
        index,
        ctx,
        config,
        json: cli.json,
        report,
    })
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let rebuild = matches!(cli.command, Command::Index { rebuild: true });
    let mut s = open(&cli, rebuild)?;
    match cli.command {
        Command::Search { query } => {
            let q = Query::parse(&query.join(" "));
            let hits = s.index.search(&q, &s.ctx)?;
            if let Some(days) = q.agenda_days {
                output::agenda(&agenda(&hits, s.ctx.now.date(), days), s.json)
            } else {
                output::notes(&hits, s.json)
            }
        }
        Command::Agenda { days, all, query } => {
            let done = if all { "" } else { " .it.done" };
            let q = Query::parse(&format!("ad.{days}{done} {}", query.join(" ")));
            let hits = s.index.search(&q, &s.ctx)?;
            output::agenda(&agenda(&hits, s.ctx.now.date(), days), s.json)
        }
        Command::Todo { query } => {
            let q = Query::parse(&format!("it.todo {}", query.join(" ")));
            output::notes(&s.index.search(&q, &s.ctx)?, s.json)
        }
        Command::Capture {
            title,
            file,
            keyword,
            body,
        } => capture(&mut s, &title.join(" "), file, keyword, body),
        Command::Index { .. } => output::sync_report(&s.report, s.json),
        Command::Ls => output::files(&s.vault.list()?, &s.index.all()?, s.json),
    }
}

#[derive(Debug)]
enum CaptureError {
    Edit(EditError),
    Write(WriteError),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::Edit(e) => write!(f, "{e}"),
            CaptureError::Write(WriteError::Stale { .. }) => {
                f.write_str("the file changed while capturing; nothing was written, try again")
            }
            CaptureError::Write(e) => write!(f, "{e}"),
        }
    }
}

impl Error for CaptureError {}

impl From<WriteError> for CaptureError {
    fn from(e: WriteError) -> Self {
        CaptureError::Write(e)
    }
}

fn capture(
    s: &mut Session,
    title: &str,
    file: Option<String>,
    keyword: Option<String>,
    body: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let path = file
        .or_else(|| s.config.inbox.clone())
        .unwrap_or_else(|| "inbox.org".to_string());
    if std::path::Path::new(&path)
        .extension()
        .is_none_or(|e| e != "org")
    {
        return Err(fail(format!("{path}: capture target must be an .org file")));
    }
    let new = NewHeading {
        title: title.to_string(),
        keyword,
        created: Some(s.ctx.now),
        body,
        ..NewHeading::default()
    };
    let line = s.vault.edit(&path, |doc| {
        let index = doc
            .insert_heading(InsertAt::LastTopLevel, &new)
            .map_err(CaptureError::Edit)?;
        let start = doc.heading(index).map_or(0, |h| h.spans.line.start);
        Ok::<usize, CaptureError>(doc.text()[..start].matches('\n').count() + 1)
    })?;
    s.index.sync(&s.vault)?;
    output::captured(&path, line, s.json)
}
