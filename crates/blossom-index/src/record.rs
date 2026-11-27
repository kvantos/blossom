//! One index record per heading: everything search and the agenda need,
//! without the file.

use blossom_core::{Document, Keywords, Timestamp};
use serde::{Deserialize, Serialize};

/// A heading as stored in the index. Timestamps are kept as org text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRecord {
    /// Vault-relative path of the file.
    pub path: String,
    /// The heading's index in its file.
    pub heading: usize,
    /// 1-based line of the heading.
    pub line: usize,
    /// Number of stars; 0 for a file intro (see [`intro_record`]).
    pub level: usize,
    /// Title as written.
    pub title: String,
    /// TODO keyword.
    pub keyword: Option<String>,
    /// Whether the keyword is done-type.
    pub done: bool,
    /// Priority as written, e.g. `A`.
    pub priority: Option<String>,
    /// Own tags.
    pub tags: Vec<String>,
    /// Own, ancestor and file tags.
    pub inherited_tags: Vec<String>,
    /// `SCHEDULED:`
    pub scheduled: Option<String>,
    /// `DEADLINE:`
    pub deadline: Option<String>,
    /// `CLOSED:`
    pub closed: Option<String>,
    /// The `CREATED` property.
    pub created: Option<String>,
    /// Bare active timestamps (events).
    pub events: Vec<String>,
    /// `:ID:`
    pub id: Option<String>,
    /// `:CUSTOM_ID:`
    pub custom_id: Option<String>,
    /// The file's modification time (ns), the ranking tiebreaker.
    pub modified: u64,
    /// Title and body, for plain-text terms.
    pub text: String,
    /// Progress as (done, total): the title's `[n/m]` or `[n%]` (as n of
    /// 100), else the body's checkboxes, else the child headings' TODO
    /// keywords; `None` with none of them.
    #[serde(default)]
    pub progress: Option<(u32, u32)>,
}

/// Records for every heading of `doc`.
#[must_use]
pub fn records(path: &str, doc: &Document, keywords: &Keywords, modified: u64) -> Vec<NoteRecord> {
    let text = doc.text();
    let mut line = 1;
    let mut counted_to = 0;
    doc.headings()
        .iter()
        .map(|h| {
            let start = h.spans.line.start;
            line += text[counted_to..start].matches('\n').count();
            counted_to = start;
            let fmt = |t: &Option<Timestamp>| t.as_ref().map(Timestamp::format);
            NoteRecord {
                path: path.to_string(),
                heading: h.index,
                line,
                level: h.level,
                title: h.title.clone(),
                keyword: h.keyword.clone(),
                done: h.keyword.as_deref().is_some_and(|k| keywords.is_done(k)),
                priority: h.priority.clone(),
                tags: h.tags.clone(),
                inherited_tags: doc.inherited_tags(h.index),
                scheduled: fmt(&h.planning.scheduled),
                deadline: fmt(&h.planning.deadline),
                closed: fmt(&h.planning.closed),
                created: h.property("CREATED").map(String::from),
                events: h.active_timestamps.iter().map(Timestamp::format).collect(),
                id: h.id().map(String::from),
                custom_id: h.property("CUSTOM_ID").map(String::from),
                modified,
                text: format!("{}\n{}", h.title, doc.body_text(h.index)),
                progress: progress(doc, h.index, keywords),
            }
        })
        .collect()
}

/// A progress cookie in `title`: `[3/8]` is (3, 8), `[40%]` is (40, 100).
#[must_use]
pub fn cookie(title: &str) -> Option<(u32, u32)> {
    title.match_indices('[').find_map(|(i, _)| {
        let rest = &title[i + 1..];
        let inner = &rest[..rest.find(']')?];
        if let Some(pct) = inner.strip_suffix('%') {
            let n: u32 = pct.parse().ok()?;
            return Some((n.min(100), 100));
        }
        let (a, b) = inner.split_once('/')?;
        let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
        Some((a.min(b), b))
    })
}

/// Heading `index`'s progress: its cookie, else its body's checkboxes,
/// else its child headings' TODO keywords.
fn progress(doc: &Document, index: usize, keywords: &Keywords) -> Option<(u32, u32)> {
    let h = &doc.headings()[index];
    if let Some(p) = cookie(&h.title) {
        return Some(p);
    }
    let (mut done, mut total) = (0u32, 0u32);
    for line in doc.body_text(index).lines() {
        let t = line.trim_start();
        let item = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("+ "))
            .or_else(|| {
                let digits = t.find(|c: char| !c.is_ascii_digit()).filter(|&n| n > 0)?;
                t[digits..]
                    .strip_prefix(". ")
                    .or_else(|| t[digits..].strip_prefix(") "))
            });
        let Some(item) = item else { continue };
        if item.starts_with("[ ]") || item.starts_with("[-]") {
            total += 1;
        } else if item.starts_with("[X]") || item.starts_with("[x]") {
            total += 1;
            done += 1;
        }
    }
    if total > 0 {
        return Some((done, total));
    }
    for c in doc.children(index) {
        if let Some(k) = &doc.headings()[c].keyword {
            total += 1;
            done += u32::from(keywords.is_done(k));
        }
    }
    (total > 0).then_some((done, total))
}

impl NoteRecord {
    /// Whether this is a file's intro rather than a heading.
    #[must_use]
    pub fn is_intro(&self) -> bool {
        self.level == 0
    }
}

/// The file's intro (real text before the first heading, or the whole of a
/// file without headings) as a level-0 record, so plain-text search finds
/// it. Its title is `#+TITLE`, else the file name; its tags are the file
/// tags. `None` when there is no such text.
#[must_use]
pub fn intro_record(path: &str, doc: &Document, modified: u64) -> Option<NoteRecord> {
    let pre = doc.preamble();
    let range = pre.intro.clone()?;
    let text = doc.text();
    let title = pre
        .keywords
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("TITLE"))
        .map(|(_, v)| v.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            let name = path.rsplit('/').next().unwrap_or(path);
            name.strip_suffix(".org").unwrap_or(name).to_string()
        });
    Some(NoteRecord {
        path: path.to_string(),
        heading: 0,
        line: 1 + text[..range.start].matches('\n').count(),
        level: 0,
        text: format!("{title}\n{}", &text[range]),
        title,
        keyword: None,
        done: false,
        priority: None,
        tags: pre.file_tags.clone(),
        inherited_tags: pre.file_tags.clone(),
        scheduled: None,
        deadline: None,
        closed: None,
        created: None,
        events: Vec::new(),
        id: pre.file_id().map(String::from),
        custom_id: None,
        modified,
        progress: None,
    })
}

/// A record with its timestamps parsed, as the matcher uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// The stored record.
    pub record: NoteRecord,
    /// Parsed `SCHEDULED:`.
    pub scheduled: Option<Timestamp>,
    /// Parsed `DEADLINE:`.
    pub deadline: Option<Timestamp>,
    /// Parsed `CLOSED:`.
    pub closed: Option<Timestamp>,
    /// Parsed `CREATED`.
    pub created: Option<Timestamp>,
    /// Parsed events.
    pub events: Vec<Timestamp>,
}

impl From<NoteRecord> for Note {
    fn from(record: NoteRecord) -> Note {
        let parse = |s: &Option<String>| {
            s.as_deref()
                .and_then(|s| Timestamp::scan(s).into_iter().next())
                .map(|(_, ts)| ts)
        };
        Note {
            scheduled: parse(&record.scheduled),
            deadline: parse(&record.deadline),
            closed: parse(&record.closed),
            created: parse(&record.created),
            events: record
                .events
                .iter()
                .filter_map(|e| Timestamp::parse(e))
                .collect(),
            record,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress_of(text: &str) -> Vec<Option<(u32, u32)>> {
        let doc = Document::parse(text, Keywords::default());
        records("a.org", &doc, &Keywords::default(), 0)
            .into_iter()
            .map(|r| r.progress)
            .collect()
    }

    #[test]
    fn cookies() {
        assert_eq!(cookie("Beta [3/8]"), Some((3, 8)));
        assert_eq!(cookie("Beta [40%]"), Some((40, 100)));
        assert_eq!(cookie("[a] then [1/2]"), Some((1, 2)));
        assert_eq!(cookie("Beta [x]"), None);
        assert_eq!(cookie("Beta [9/3]"), Some((3, 3)));
    }

    #[test]
    fn progress_from_cookie_then_checkboxes_then_children() {
        let text = "* A [1/4]\n- [X] one\n* B\n- [X] one\n- [ ] two\n1. [x] three\n\
                    * C\n** TODO c1\n** DONE c2\n** plain\n* D\nnothing\n";
        // A, B, C, then c1, c2 and plain (none of their own), then D.
        assert_eq!(
            progress_of(text),
            [
                Some((1, 4)),
                Some((2, 3)),
                Some((1, 2)),
                None,
                None,
                None,
                None
            ]
        );
    }
}
