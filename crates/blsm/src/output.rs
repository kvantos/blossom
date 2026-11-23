//! Text and JSON output.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::io::{self, Write};

use blossom_index::{AgendaDay, AgendaReason, Note, SyncReport};
use blossom_vault::Listing;
use serde::Serialize;
use serde_json::json;

type Out = Result<(), Box<dyn Error>>;

/// The public JSON shape of a heading.
#[derive(Serialize)]
struct NoteJson<'a> {
    path: &'a str,
    line: usize,
    level: usize,
    title: &'a str,
    keyword: Option<&'a str>,
    done: bool,
    priority: Option<&'a str>,
    tags: &'a [String],
    inherited_tags: &'a [String],
    scheduled: Option<&'a str>,
    deadline: Option<&'a str>,
    closed: Option<&'a str>,
    created: Option<&'a str>,
    events: &'a [String],
    id: Option<&'a str>,
}

fn note_json(n: &Note) -> NoteJson<'_> {
    let r = &n.record;
    NoteJson {
        path: &r.path,
        line: r.line,
        level: r.level,
        title: &r.title,
        keyword: r.keyword.as_deref(),
        done: r.done,
        priority: r.priority.as_deref(),
        tags: &r.tags,
        inherited_tags: &r.inherited_tags,
        scheduled: r.scheduled.as_deref(),
        deadline: r.deadline.as_deref(),
        closed: r.closed.as_deref(),
        created: r.created.as_deref(),
        events: &r.events,
        id: r.id.as_deref(),
    }
}

fn print_json(value: &impl Serialize) -> Out {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}

/// `TODO [#A] Title  :a:b:`
fn heading_text(n: &Note) -> String {
    let r = &n.record;
    let mut s = String::new();
    if let Some(k) = &r.keyword {
        s.push_str(k);
        s.push(' ');
    }
    if let Some(p) = &r.priority {
        let _ = write!(s, "[#{p}] ");
    }
    s.push_str(&r.title);
    if !r.tags.is_empty() {
        let _ = write!(s, "  :{}:", r.tags.join(":"));
    }
    s
}

/// One line per heading: `path:line  heading  dates`.
pub fn notes(hits: &[Note], json: bool) -> Out {
    if json {
        return print_json(&hits.iter().map(note_json).collect::<Vec<_>>());
    }
    let mut out = io::stdout().lock();
    for n in hits {
        let r = &n.record;
        let mut line = format!("{}:{}  {}", r.path, r.line, heading_text(n));
        for (label, value) in [("SCHEDULED", &r.scheduled), ("DEADLINE", &r.deadline)] {
            if let Some(v) = value {
                let _ = write!(line, "  {label} {v}");
            }
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn reason_name(r: AgendaReason) -> &'static str {
    match r {
        AgendaReason::Scheduled => "scheduled",
        AgendaReason::Deadline => "deadline",
        AgendaReason::Event => "event",
    }
}

/// Sections by day, overdue first.
pub fn agenda(days: &[AgendaDay], json: bool) -> Out {
    if json {
        let value: Vec<_> = days
            .iter()
            .map(|d| {
                json!({
                    "date": d.date.map(|x| x.to_string()),
                    "items": d.items.iter().map(|i| json!({
                        "reason": reason_name(i.reason),
                        "time": i.time.map(|t| t.format("%H:%M").to_string()),
                        "note": note_json(&i.note),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        return print_json(&value);
    }
    let mut out = io::stdout().lock();
    for (n, day) in days.iter().enumerate() {
        if n > 0 {
            writeln!(out)?;
        }
        match day.date {
            Some(d) => writeln!(out, "{}", d.format("%a %Y-%m-%d"))?,
            None => writeln!(out, "Overdue")?,
        }
        for item in &day.items {
            let time = item
                .time
                .map_or_else(|| "     ".to_string(), |t| t.format("%H:%M").to_string());
            let r = &item.note.record;
            writeln!(
                out,
                "  {time}  {:<9}  {}  ({}:{})",
                reason_name(item.reason),
                heading_text(&item.note),
                r.path,
                r.line
            )?;
        }
    }
    Ok(())
}

/// What `blsm index` did.
pub fn sync_report(r: &SyncReport, json: bool) -> Out {
    if json {
        return print_json(&json!({
            "added": r.added,
            "updated": r.updated,
            "removed": r.removed,
            "failed": r.failed.iter().map(|(p, e)| json!({"path": p, "error": e})).collect::<Vec<_>>(),
            "conflicts": r.conflicts.iter().map(|c| json!({"path": c.path, "copy": c.copy})).collect::<Vec<_>>(),
        }));
    }
    println!(
        "indexed: {} added, {} updated, {} removed, {} failed",
        r.added.len(),
        r.updated.len(),
        r.removed.len(),
        r.failed.len()
    );
    Ok(())
}

/// Each file with its heading and open-task counts.
pub fn files(listing: &Listing, all: &[Note], json: bool) -> Out {
    let mut counts: BTreeMap<&str, (usize, usize)> = listing
        .files
        .iter()
        .map(|f| (f.path.as_str(), (0, 0)))
        .collect();
    for n in all {
        if let Some(c) = counts.get_mut(n.record.path.as_str()) {
            c.0 += 1;
            if n.record.keyword.is_some() && !n.record.done {
                c.1 += 1;
            }
        }
    }
    let conflicted = |p: &str| listing.conflicts.iter().any(|c| c.path == p);
    if json {
        let value: Vec<_> = counts
            .iter()
            .map(|(p, (h, t))| json!({"path": p, "headings": h, "open_tasks": t, "conflict": conflicted(p)}))
            .collect();
        return print_json(&value);
    }
    let mut out = io::stdout().lock();
    for (p, (h, t)) in &counts {
        let flag = if conflicted(p) {
            "  (conflict copy)"
        } else {
            ""
        };
        writeln!(out, "{p}  {h} headings, {t} open{flag}")?;
    }
    Ok(())
}

/// Where `blsm capture` wrote.
pub fn captured(path: &str, line: usize, json: bool) -> Out {
    if json {
        return print_json(&json!({"path": path, "line": line}));
    }
    println!("{path}:{line}");
    Ok(())
}
