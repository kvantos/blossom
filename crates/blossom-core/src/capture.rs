//! Capture templates: expanding org-capture placeholders and inserting the
//! entry into a file, ported from Bloom's `PlaceholderExpander.kt` and
//! `CaptureInserter.kt`.
//!
//! A template such as `* TODO %^{Title}\n%U\n%?` is expanded with
//! [`expand`] (the UI asks for each of [`prompts`] first, and puts the cursor
//! where `%?` was). The edited entry then goes into a document with
//! [`Document::capture`] at a [`Target`]: the top or bottom of the file, under
//! a heading, or in a date tree. Insertion is a splice: every other byte of
//! the file stays as it was.

use std::fmt;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};

use crate::document::Document;
use crate::edit::EditError;
use crate::text;
use crate::timestamp::Timestamp;

/// Where a capture goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Level 1, above the first heading (below the file's keywords and intro).
    Top,
    /// Level 1, at the end of the file.
    Bottom,
    /// Under a heading, found by `id` (`:ID:`, then `:CUSTOM_ID:`), else by
    /// its outline `path` (exact titles, outermost first, as org's
    /// `file+olp`).
    Heading {
        /// An `:ID:` or `:CUSTOM_ID:`.
        id: Option<String>,
        /// Outline path.
        path: Vec<String>,
        /// As the first child (or at the top of the body) instead of the last.
        first: bool,
    },
    /// A date tree for the capture's day, at the top level or inside the
    /// heading given by `id` / `path`, with the entry under its last node.
    Datetree {
        /// An `:ID:` or `:CUSTOM_ID:`.
        id: Option<String>,
        /// Outline path; empty with no `id` for the top level.
        path: Vec<String>,
        /// The tree's shape.
        tree: TreeType,
    },
}

impl Target {
    /// A short description, e.g. "under Projects / Blossom".
    #[must_use]
    pub fn describe(&self) -> String {
        let place = |id: &Option<String>, path: &[String]| {
            if path.is_empty() {
                id.as_ref().map(|i| format!("#{i}"))
            } else {
                Some(path.join(" / "))
            }
        };
        match self {
            Target::Top => "top of file".into(),
            Target::Bottom => "bottom of file".into(),
            Target::Heading { id, path, .. } => {
                format!("under {}", place(id, path).unwrap_or_default())
            }
            Target::Datetree { id, path, .. } => match place(id, path) {
                Some(p) => format!("date tree in {p}"),
                None => "date tree".into(),
            },
        }
    }
}

/// How a date tree groups entries (org's `:tree-type`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TreeType {
    /// Year, month, day (org's default).
    #[default]
    Day,
    /// Year, ISO week, day.
    Week,
    /// Year, month.
    Month,
}

/// Why a capture couldn't be inserted. The document is unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// The target heading isn't in the file.
    NoHeading(String),
    /// The entry is empty.
    Empty,
    /// An edit failed.
    Edit(EditError),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::NoHeading(h) => write!(f, "no heading {h} in the target file"),
            CaptureError::Empty => f.write_str("nothing to capture"),
            CaptureError::Edit(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<EditError> for CaptureError {
    fn from(e: EditError) -> Self {
        CaptureError::Edit(e)
    }
}

/// What a template expansion can use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// The capture time.
    pub now: NaiveDateTime,
    /// Answers to `%^{prompt}`, by prompt text; a missing one expands to nothing.
    pub answers: Vec<(String, String)>,
    /// For `%clipboard`.
    pub clipboard: String,
    /// For `%(id)`: made by the caller, as this crate is pure.
    pub id: String,
}

/// An expanded template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expanded {
    /// The text.
    pub text: String,
    /// Byte offset of the first `%?` / `%cursor`, else the end.
    pub cursor: usize,
}

/// The simple placeholders, longest first so `%time` wins over `%t`.
const KEYS: [&str; 14] = [
    "%clipboard",
    "%cursor",
    "%month",
    "%date",
    "%time",
    "%year",
    "%day",
    "%(id)",
    "%t",
    "%T",
    "%u",
    "%U",
    "%?",
    "%%",
];

/// `%^{prompt}` at the start of `s`: the prompt text and the token length.
fn prompt_at(s: &str) -> Option<(&str, usize)> {
    let rest = s.strip_prefix("%^{")?;
    let close = rest.find('}')?;
    Some((&rest[..close], close + 4))
}

/// The prompts a template asks, in order, without repeats.
#[must_use]
pub fn prompts(template: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, _) in template.match_indices('%') {
        if let Some((p, _)) = prompt_at(&template[i..])
            && !out.iter().any(|o| o == p)
        {
            out.push(p.to_string());
        }
    }
    out
}

/// Expands `template`: `%t` `%T` `%u` `%U` (active / inactive date, with time
/// for the capitals), `%^{prompt}`, `%?` or `%cursor` (where the cursor goes),
/// `%date` `%time` `%year` `%month` `%day`, `%clipboard`, `%(id)`, and `%%`
/// for a percent sign. Anything else is kept as written.
#[must_use]
pub fn expand(template: &str, ctx: &Context) -> Expanded {
    let date = ctx.now.date();
    let stamp = |active: bool, timed: bool| {
        if timed {
            let mut t = Timestamp::inactive_at(ctx.now);
            t.active = active;
            t.format()
        } else {
            Timestamp::new(date, active).format()
        }
    };
    let mut out = String::new();
    let mut cursor = None;
    let mut rest = template;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some((p, len)) = prompt_at(rest) {
            if let Some((_, a)) = ctx.answers.iter().find(|(q, _)| q == p) {
                out.push_str(a);
            }
            rest = &rest[len..];
            continue;
        }
        let Some(key) = KEYS.iter().find(|k| rest.starts_with(**k)) else {
            out.push('%');
            rest = &rest[1..];
            continue;
        };
        match *key {
            "%?" | "%cursor" => {
                cursor.get_or_insert(out.len());
            }
            "%clipboard" => out.push_str(&ctx.clipboard),
            "%month" => out.push_str(month_name(date.month())),
            "%date" => out.push_str(&date.format("%Y-%m-%d").to_string()),
            "%time" => out.push_str(&ctx.now.format("%-H:%M").to_string()),
            "%year" => out.push_str(&date.year().to_string()),
            "%day" => out.push_str(day_name(date.weekday())),
            "%(id)" => out.push_str(&ctx.id),
            "%t" => out.push_str(&stamp(true, false)),
            "%T" => out.push_str(&stamp(true, true)),
            "%u" => out.push_str(&stamp(false, false)),
            "%U" => out.push_str(&stamp(false, true)),
            _ => out.push('%'),
        }
        rest = &rest[key.len()..];
    }
    out.push_str(rest);
    let cursor = cursor.unwrap_or(out.len());
    Expanded { text: out, cursor }
}

/// Whether `line` is a heading line: stars then a space or tab (or nothing).
fn heading_stars(line: &str) -> Option<usize> {
    let n = line.bytes().take_while(|&b| b == b'*').count();
    (n > 0 && matches!(line.as_bytes().get(n), None | Some(b' ' | b'\t'))).then_some(n)
}

/// The entry's lines, without line endings or trailing blank lines.
fn entry_lines(entry: &str) -> Vec<String> {
    let mut lines: Vec<String> = entry
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// The entry as a subtree at `level`, like org-capture: its first line
/// becomes a heading at `level` (a line without stars is the heading's
/// text), and every heading below moves by the same amount, never above
/// `level`. Other lines are kept as written.
fn relevel(lines: &[String], level: usize) -> Vec<String> {
    let Some((first, rest)) = lines.split_first() else {
        return Vec::new();
    };
    let (own, title) = match heading_stars(first) {
        Some(n) => (n, first[n..].trim()),
        None => (1, first.trim()),
    };
    let shift = isize::try_from(level).unwrap_or(isize::MAX) - isize::try_from(own).unwrap_or(0);
    let mut out = vec![format!("{} {title}", "*".repeat(level))];
    for line in rest {
        match heading_stars(line) {
            Some(n) => {
                let new = n.saturating_add_signed(shift).max(level);
                out.push(format!("{}{}", "*".repeat(new), &line[n..]));
            }
            None => out.push(line.clone()),
        }
    }
    out
}

/// English month names, as org's date tree writes them by default.
fn month_name(m: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES[(m.clamp(1, 12) - 1) as usize]
}

fn day_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

/// The node titles of `day`'s branch of a date tree, outermost first. A week
/// tree uses the ISO week-based year, as org does.
#[must_use]
pub fn datetree_titles(day: NaiveDate, tree: TreeType) -> Vec<String> {
    let month = format!(
        "{:04}-{:02} {}",
        day.year(),
        day.month(),
        month_name(day.month())
    );
    let date = format!("{} {}", day.format("%Y-%m-%d"), day_name(day.weekday()));
    match tree {
        TreeType::Day => vec![day.year().to_string(), month, date],
        TreeType::Week => {
            let w = day.iso_week();
            vec![
                w.year().to_string(),
                format!("{:04}-W{:02}", w.year(), w.week()),
                date,
            ]
        }
        TreeType::Month => vec![day.year().to_string(), month],
    }
}

/// A date-tree node's date part: `2026-11` of `2026-11 November`, or `None`
/// when the title isn't date-shaped (`2026`, `2026-11`, `2026-W48`,
/// `2026-11-23`, each optionally followed by a space and anything).
fn date_part(title: &str) -> Option<&str> {
    let part = title.split([' ', '\t']).next().unwrap_or("");
    let b = part.as_bytes();
    let digits = |r: std::ops::Range<usize>| {
        b.get(r.clone())
            .is_some_and(|s| s.len() == r.len() && s.iter().all(u8::is_ascii_digit))
    };
    let ok = digits(0..4)
        && match b.len() {
            4 => true,
            7 => b[4] == b'-' && digits(5..7),
            8 => b[4] == b'-' && b[5] == b'W' && digits(6..8),
            10 => b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10),
            _ => false,
        };
    ok.then_some(part)
}

impl Document {
    /// The heading with `:ID:` or `:CUSTOM_ID:` `id`.
    #[must_use]
    pub fn find_by_any_id(&self, id: &str) -> Option<usize> {
        self.find_by_id(id).map(|h| h.index).or_else(|| {
            self.headings()
                .iter()
                .find(|h| h.property("CUSTOM_ID") == Some(id))
                .map(|h| h.index)
        })
    }

    /// The heading at outline `path`: exact titles, outermost first, each a
    /// direct child of the one before. `None` for an empty path or a missing step.
    #[must_use]
    pub fn find_by_path(&self, path: &[String]) -> Option<usize> {
        let mut cur: Option<usize> = None;
        for title in path {
            let candidates = match cur {
                Some(p) => self.children(p),
                None => self.top_level(),
            };
            cur = Some(
                candidates
                    .into_iter()
                    .find(|&i| self.headings()[i].title == *title)?,
            );
        }
        cur
    }

    /// Indexes of the level-1 (parentless) headings.
    #[must_use]
    pub fn top_level(&self) -> Vec<usize> {
        self.headings()
            .iter()
            .filter(|h| h.parent.is_none())
            .map(|h| h.index)
            .collect()
    }

    fn capture_anchor(
        &self,
        id: Option<&String>,
        path: &[String],
    ) -> Result<Option<usize>, CaptureError> {
        if id.is_none() && path.is_empty() {
            return Ok(None);
        }
        id.and_then(|i| self.find_by_any_id(i))
            .or_else(|| self.find_by_path(path))
            .map(Some)
            .ok_or_else(|| {
                CaptureError::NoHeading(if path.is_empty() {
                    format!("with ID {:?}", id.map_or("", String::as_str))
                } else {
                    format!("{:?}", path.join(" / "))
                })
            })
    }

    /// Inserts a capture `entry` at `target` (`today` picks the date-tree
    /// branch). An entry that starts with a heading is re-levelled for its
    /// place, and one that doesn't gets its first line as the heading, except
    /// under a heading, where text that isn't a heading is added to that
    /// heading's body (a `- [ ] item` under "Shopping"). Returns the byte
    /// offset where the entry starts.
    ///
    /// # Errors
    ///
    /// [`CaptureError::NoHeading`] when the target heading is missing,
    /// [`CaptureError::Empty`] for a blank entry.
    pub fn capture(
        &mut self,
        target: &Target,
        entry: &str,
        today: NaiveDate,
    ) -> Result<usize, CaptureError> {
        let lines = entry_lines(entry);
        let first = lines
            .iter()
            .position(|l| !l.trim().is_empty())
            .ok_or(CaptureError::Empty)?;
        let lines = &lines[first..];
        match target {
            Target::Top => {
                let at = self.preamble().range.end;
                Ok(self.insert_entry(at, &relevel(lines, 1)))
            }
            Target::Bottom => {
                let at = self.text().len();
                Ok(self.insert_entry(at, &relevel(lines, 1)))
            }
            Target::Heading { id, path, first } => {
                let Some(t) = self.capture_anchor(id.as_ref(), path)? else {
                    let at = self.text().len();
                    return Ok(self.insert_entry(at, &relevel(lines, 1)));
                };
                let h = self.headings()[t].clone();
                if heading_stars(&lines[0]).is_none() {
                    // Body text: above the first child, and after the body's
                    // last non-blank line so a list item joins its list.
                    let at = if *first {
                        h.spans.body.start
                    } else {
                        self.after_content(h.spans.body.clone())
                    };
                    return Ok(self.insert_entry(at, lines));
                }
                let at = if *first {
                    h.spans.section_end
                } else {
                    h.spans.subtree_end
                };
                Ok(self.insert_entry(at, &relevel(lines, h.level + 1)))
            }
            Target::Datetree { id, path, tree } => {
                let mut parent = self.capture_anchor(id.as_ref(), path)?;
                let mut level = parent.map_or(0, |p| self.headings()[p].level);
                for title in datetree_titles(today, *tree) {
                    level += 1;
                    parent = Some(self.date_node(parent, level, &title));
                }
                let at = parent.map_or(self.text().len(), |p| self.headings()[p].spans.subtree_end);
                Ok(self.insert_entry(at, &relevel(lines, level + 1)))
            }
        }
    }

    /// Finds or creates the date-tree node `title` under `parent` (or at the
    /// top level), matched and ordered by its date part only, as
    /// `org-datetree` does, so a node in another language is reused.
    fn date_node(&mut self, parent: Option<usize>, level: usize, title: &str) -> usize {
        let prefix = date_part(title).unwrap_or(title).to_string();
        let siblings = match parent {
            Some(p) => self.children(p),
            None => self.top_level(),
        };
        let dated: Vec<(usize, String)> = siblings
            .into_iter()
            .filter_map(|i| date_part(&self.headings()[i].title).map(|d| (i, d.to_string())))
            .collect();
        if let Some((i, _)) = dated.iter().find(|(_, d)| *d == prefix) {
            return *i;
        }
        let at = if let Some((i, _)) = dated.iter().find(|(_, d)| *d > prefix) {
            self.headings()[*i].spans.line.start
        } else if let Some((i, _)) = dated.last() {
            self.headings()[*i].spans.subtree_end
        } else if let Some(p) = parent {
            self.headings()[p].spans.subtree_end
        } else {
            self.text().len()
        };
        let start = self.insert_entry(at, &[format!("{} {title}", "*".repeat(level))]);
        self.headings()
            .iter()
            .position(|h| h.spans.line.start == start)
            .unwrap_or(0)
    }

    /// The start of the line after the last non-blank line in `range`, or
    /// `range.start` when it is all blank.
    fn after_content(&self, range: std::ops::Range<usize>) -> usize {
        let kept = self.text()[range.clone()]
            .trim_end_matches([' ', '\t', '\r', '\n'])
            .len();
        if kept == 0 {
            range.start
        } else {
            // The end of the content (a char boundary, unlike its last byte
            // when the text ends in a multi-byte letter) is on the same line.
            text::line_at(self.text(), range.start + kept).end
        }
    }

    /// Adds `line` as a line of its own at the end of heading `index`'s own
    /// text (after its last non-blank line, above its children), or of the
    /// text before the first heading when `index` is `None`. Returns where
    /// the line starts.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`], or [`EditError::MultilineText`].
    pub fn append_line(&mut self, index: Option<usize>, line: &str) -> Result<usize, EditError> {
        if line.contains(['\n', '\r']) {
            return Err(EditError::MultilineText);
        }
        let range = match index {
            Some(i) => {
                let h = self.heading(i).ok_or(EditError::NoSuchHeading(i))?;
                // The metadata (planning, drawers) stays above the line.
                h.spans.body.start..h.spans.section_end
            }
            None => self.preamble().range.clone(),
        };
        let at = self.after_content(range);
        Ok(self.insert_lines(at, &[line.to_string()]))
    }

    /// Inserts `lines` at byte `at` (a line start or the end); returns
    /// where they start.
    fn insert_entry(&mut self, at: usize, lines: &[String]) -> usize {
        self.insert_lines(at, lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Keywords;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 23)
            .unwrap()
            .and_hms_opt(9, 5, 30)
            .unwrap()
    }

    fn today() -> NaiveDate {
        now().date()
    }

    fn cap(text: &str, target: &Target, entry: &str) -> String {
        let mut d = Document::parse(text, Keywords::default());
        d.capture(target, entry, today()).unwrap();
        d.text().to_string()
    }

    #[test]
    fn expands_placeholders() {
        let ctx = Context {
            now: now(),
            answers: vec![("Title".into(), "Buy milk".into())],
            clipboard: "clip".into(),
            id: "abc".into(),
        };
        let e = expand(
            "* TODO %^{Title} %^{Other}\n%U %t %T %u\n%?%date %time %year %month %day %clipboard %(id) 50%% %x %cursor",
            &ctx,
        );
        assert_eq!(
            e.text,
            "* TODO Buy milk \n[2026-11-23 Mon 09:05] <2026-11-23 Mon> <2026-11-23 Mon 09:05> [2026-11-23 Mon]\n2026-11-23 9:05 2026 November Monday clip abc 50% %x "
        );
        assert_eq!(&e.text[e.cursor..e.cursor + 4], "2026");
        assert_eq!(prompts("%^{A} %^{B} %^{A} %^{unclosed"), ["A", "B"]);
        let plain = expand("no cursor", &ctx);
        assert_eq!(plain.cursor, plain.text.len());
    }

    #[test]
    fn top_and_bottom() {
        let text = "#+TITLE: x\nintro\n* a\n* b\n";
        assert_eq!(
            cap(text, &Target::Top, "new\nbody"),
            "#+TITLE: x\nintro\n* new\nbody\n* a\n* b\n"
        );
        assert_eq!(
            cap(text, &Target::Bottom, "** TODO deep\n*** child\n"),
            "#+TITLE: x\nintro\n* a\n* b\n* TODO deep\n** child\n"
        );
        assert_eq!(cap("", &Target::Bottom, "x"), "* x\n");
        assert_eq!(cap("* a", &Target::Bottom, "x"), "* a\n* x");
        assert_eq!(
            cap("* a\r\n", &Target::Bottom, "x\ny"),
            "* a\r\n* x\r\ny\r\n"
        );
        let mut d = Document::parse("* a\n", Keywords::default());
        assert_eq!(
            d.capture(&Target::Bottom, " \n\n", today()),
            Err(CaptureError::Empty)
        );
    }

    #[test]
    fn under_a_heading() {
        let text = "* Projects\n:PROPERTIES:\n:ID: p1\n:END:\nnotes\n- one\n\n** Old\n* Other\n";
        let by_path = Target::Heading {
            id: None,
            path: vec!["Projects".into()],
            first: false,
        };
        assert_eq!(
            cap(text, &by_path, "* New\n** sub"),
            "* Projects\n:PROPERTIES:\n:ID: p1\n:END:\nnotes\n- one\n\n** Old\n** New\n*** sub\n* Other\n"
        );
        let by_id = Target::Heading {
            id: Some("p1".into()),
            path: vec![],
            first: true,
        };
        assert_eq!(
            cap(text, &by_id, "* New"),
            "* Projects\n:PROPERTIES:\n:ID: p1\n:END:\nnotes\n- one\n\n** New\n** Old\n* Other\n"
        );
        assert_eq!(
            cap(text, &by_id, "first line"),
            "* Projects\n:PROPERTIES:\n:ID: p1\n:END:\nfirst line\nnotes\n- one\n\n** Old\n* Other\n"
        );
        // Text that isn't a heading joins the body, after its last line.
        assert_eq!(
            cap(text, &by_path, "- two"),
            "* Projects\n:PROPERTIES:\n:ID: p1\n:END:\nnotes\n- one\n- two\n\n** Old\n* Other\n"
        );
        let mut d = Document::parse(text, Keywords::default());
        let missing = Target::Heading {
            id: None,
            path: vec!["Projects".into(), "Nope".into()],
            first: false,
        };
        assert!(matches!(
            d.capture(&missing, "x", today()),
            Err(CaptureError::NoHeading(_))
        ));
        assert_eq!(d.text(), text);
    }

    #[test]
    fn append_lines() {
        let mut d = Document::parse(
            "#+TITLE: t\n\n* a\nSCHEDULED: <2026-11-23 Mon>\nbody\n\n** child\n* b",
            crate::Keywords::default(),
        );
        d.append_line(Some(0), "[[file:x.png]]").unwrap();
        d.append_line(Some(2), "[[file:y.png]]").unwrap();
        d.append_line(None, "top").unwrap();
        assert_eq!(
            d.text(),
            "#+TITLE: t\ntop\n\n* a\nSCHEDULED: <2026-11-23 Mon>\nbody\n[[file:x.png]]\n\n** child\n* b\n[[file:y.png]]"
        );
        let mut e = Document::parse("* h\n", crate::Keywords::default());
        e.append_line(Some(0), "x").unwrap();
        assert_eq!(e.text(), "* h\nx\n");
        assert_eq!(
            e.append_line(Some(5), "x"),
            Err(EditError::NoSuchHeading(5))
        );
        // Text ending in a multi-byte letter (found by fuzzing: it panicked
        // slicing inside the letter), with and without a final line break.
        for (text, want) in [
            ("* a\nкофе\n* b\n", "* a\nкофе\n[[file:x.png]]\n* b\n"),
            ("* a\nكافيه", "* a\nكافيه\n[[file:x.png]]"),
            ("* a\ncafé 🌸  \n\n", "* a\ncafé 🌸  \n[[file:x.png]]\n\n"),
        ] {
            let mut d = Document::parse(text, crate::Keywords::default());
            d.append_line(Some(0), "[[file:x.png]]").unwrap();
            assert_eq!(d.text(), want);
        }
        let mut top = Document::parse("Заметка\n* a\n", crate::Keywords::default());
        top.append_line(None, "x").unwrap();
        assert_eq!(top.text(), "Заметка\nx\n* a\n");
    }

    #[test]
    fn date_trees() {
        let tree = Target::Datetree {
            id: None,
            path: vec![],
            tree: TreeType::Day,
        };
        let first = cap("", &tree, "[2026-11-23 Mon 09:05] hello");
        assert_eq!(
            first,
            "* 2026\n** 2026-11 November\n*** 2026-11-23 Monday\n**** [2026-11-23 Mon 09:05] hello\n"
        );
        // Reused, even in another language, and kept in date order.
        let existing = "* 2025\n* 2026\n** 2026-10 Oktober\n** 2026-11 November\n*** 2026-11-24 Tuesday\n* Notes\n";
        assert_eq!(
            cap(existing, &tree, "x"),
            "* 2025\n* 2026\n** 2026-10 Oktober\n** 2026-11 November\n*** 2026-11-23 Monday\n**** x\n*** 2026-11-24 Tuesday\n* Notes\n"
        );
        let week = Target::Datetree {
            id: None,
            path: vec!["Journal".into()],
            tree: TreeType::Week,
        };
        assert_eq!(
            cap("* Journal\n* Later\n", &week, "x"),
            "* Journal\n** 2026\n*** 2026-W48\n**** 2026-11-23 Monday\n***** x\n* Later\n"
        );
        assert_eq!(
            datetree_titles(NaiveDate::from_ymd_opt(2027, 1, 1).unwrap(), TreeType::Week)[..2],
            ["2026".to_string(), "2026-W53".to_string()]
        );
    }
}
