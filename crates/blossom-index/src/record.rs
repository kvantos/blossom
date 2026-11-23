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
    /// Number of stars.
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
            }
        })
        .collect()
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
