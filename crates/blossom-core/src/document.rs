//! The parsed view of one org file.
//!
//! The text is the only state. Heading structure (where headings start, their
//! level, keyword, priority, title and tags) comes from orgize's syntax tree,
//! so it always agrees with the incremental re-parse that edits go through.
//! What follows a heading line (planning, the dedicated timestamp line,
//! `:PROPERTIES:` and `:LOGBOOK:` drawers) is read by a line scanner ported
//! from Bloom's `OrgParser.kt`, which accepts the layouts Bloom writes: drawers
//! in either order, and an active-timestamp line above the drawers.

use std::ops::Range;

use orgize::{Org, SyntaxKind, SyntaxNode, TextRange, TextSize};

use crate::keywords::Keywords;
use crate::text::{self, Line};
use crate::timestamp::Timestamp;

/// SCHEDULED / DEADLINE / CLOSED from a heading's planning line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Planning {
    /// `SCHEDULED: <…>`
    pub scheduled: Option<Timestamp>,
    /// `DEADLINE: <…>`
    pub deadline: Option<Timestamp>,
    /// `CLOSED: […]`
    pub closed: Option<Timestamp>,
}

/// Which planning entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanningKind {
    /// `SCHEDULED:`
    Scheduled,
    /// `DEADLINE:`
    Deadline,
    /// `CLOSED:`
    Closed,
}

impl PlanningKind {
    /// The keyword as written, without the colon.
    #[must_use]
    pub fn keyword(self) -> &'static str {
        match self {
            PlanningKind::Scheduled => "SCHEDULED",
            PlanningKind::Deadline => "DEADLINE",
            PlanningKind::Closed => "CLOSED",
        }
    }

    const ALL: [PlanningKind; 3] = [
        PlanningKind::Scheduled,
        PlanningKind::Deadline,
        PlanningKind::Closed,
    ];
}

/// Byte ranges of a heading's parts. Line ranges include their line ending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingSpans {
    /// The heading line.
    pub line: Range<usize>,
    /// The heading line without its line ending.
    pub line_content: Range<usize>,
    /// The leading `*` run.
    pub stars: Range<usize>,
    /// The TODO keyword.
    pub keyword: Option<Range<usize>>,
    /// The priority cookie, `[#A]`.
    pub priority: Option<Range<usize>>,
    /// The title, without surrounding whitespace (empty when there is none).
    pub title: Range<usize>,
    /// The tag group, `:a:b:`.
    pub tags: Option<Range<usize>>,
    /// The planning line.
    pub planning: Option<Range<usize>>,
    /// The dedicated active-timestamp line, in either position Bloom writes it.
    pub dedicated: Option<Range<usize>>,
    /// The `:PROPERTIES:` drawer, marker lines included.
    pub properties: Option<Range<usize>>,
    /// The `:LOGBOOK:` drawer, marker lines included.
    pub logbook: Option<Range<usize>>,
    /// Where the drawer run starts: after planning and a dedicated line above it.
    pub drawers_start: usize,
    /// The body: everything after the metadata, up to the next heading.
    pub body: Range<usize>,
    /// The heading's own section ends here: the next heading of any level.
    pub section_end: usize,
    /// The heading's subtree (itself and its descendants) ends here.
    pub subtree_end: usize,
}

/// One heading of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// Position in document order, from 0.
    pub index: usize,
    /// Number of stars.
    pub level: usize,
    /// The TODO keyword, if it is one of the configured keywords.
    pub keyword: Option<String>,
    /// The priority letter or number, `A` for `[#A]`.
    pub priority: Option<String>,
    /// The title as written, trimmed.
    pub title: String,
    /// The heading's own tags (not inherited).
    pub tags: Vec<String>,
    /// SCHEDULED / DEADLINE / CLOSED.
    pub planning: Planning,
    /// `:PROPERTIES:` entries in file order, as written.
    pub properties: Vec<(String, String)>,
    /// Raw lines inside the `:LOGBOOK:` drawer, without line endings.
    pub logbook: Vec<String>,
    /// Active timestamps in the heading's own body and dedicated line (events
    /// on their date). Planning stamps are not included.
    pub active_timestamps: Vec<Timestamp>,
    /// The stamps on the dedicated active-timestamp line only.
    pub dedicated_timestamps: Vec<Timestamp>,
    /// The nearest enclosing heading.
    pub parent: Option<usize>,
    /// Where each part is in the text.
    pub spans: HeadingSpans,
}

impl Heading {
    /// The value of property `key` (case-insensitive, as in org).
    #[must_use]
    pub fn property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// The `:ID:` property.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.property("ID")
    }
}

/// What comes before the first heading.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preamble {
    /// Ends at the first heading (or the end of the text).
    pub range: Range<usize>,
    /// The leading `#+KEY: value` lines, keys as written without `#+` and `:`.
    pub keywords: Vec<(String, String)>,
    /// Tags from `#+FILETAGS:` lines.
    pub file_tags: Vec<String>,
    /// A file-level `:PROPERTIES:` drawer on the very first line, if closed.
    pub properties_drawer: Option<Range<usize>>,
    /// Its entries, in file order.
    pub properties: Vec<(String, String)>,
    /// Real content before the first heading (after the drawer and keyword
    /// lines), when there is any that isn't blank.
    pub intro: Option<Range<usize>>,
}

impl Preamble {
    /// The file-level `:ID:` property: an org-id target for the whole file.
    #[must_use]
    pub fn file_id(&self) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("ID"))
            .map(|(_, v)| v.as_str())
    }
}

/// A parsed org file. Its text is exactly the input: nothing is re-rendered.
#[derive(Debug)]
pub struct Document {
    text: String,
    org: Org,
    keywords: Keywords,
    headings: Vec<Heading>,
    preamble: Preamble,
}

impl Document {
    /// Parses `text` with the given TODO keywords.
    pub fn parse(text: impl Into<String>, keywords: Keywords) -> Document {
        let text = text.into();
        let org = keywords.parse_config().parse(&text);
        let mut doc = Document {
            text,
            org,
            keywords,
            headings: Vec::new(),
            preamble: Preamble::default(),
        };
        doc.reindex();
        doc
    }

    /// The full text, byte for byte.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The orgize syntax tree, for read-only use (rendering, highlighting).
    #[must_use]
    pub fn org(&self) -> &Org {
        &self.org
    }

    /// The TODO keywords this document was parsed with.
    #[must_use]
    pub fn keywords(&self) -> &Keywords {
        &self.keywords
    }

    /// Every heading, in document order.
    #[must_use]
    pub fn headings(&self) -> &[Heading] {
        &self.headings
    }

    /// The heading at `index`.
    #[must_use]
    pub fn heading(&self, index: usize) -> Option<&Heading> {
        self.headings.get(index)
    }

    /// The preamble: file keywords, file tags, file properties, intro.
    #[must_use]
    pub fn preamble(&self) -> &Preamble {
        &self.preamble
    }

    /// The text of the intro, if the file has one.
    #[must_use]
    pub fn intro_text(&self) -> Option<&str> {
        self.preamble.intro.clone().map(|r| &self.text[r])
    }

    /// Indexes of the direct children of heading `index`.
    #[must_use]
    pub fn children(&self, index: usize) -> Vec<usize> {
        self.descendants(index)
            .filter(|&i| self.headings[i].parent == Some(index))
            .collect()
    }

    /// Indexes of every descendant of heading `index` (its subtree without itself).
    #[must_use]
    pub fn descendants(&self, index: usize) -> Range<usize> {
        let Some(h) = self.headings.get(index) else {
            return 0..0;
        };
        let end = self.headings[index + 1..]
            .iter()
            .position(|o| o.spans.line.start >= h.spans.subtree_end)
            .map_or(self.headings.len(), |n| index + 1 + n);
        index + 1..end
    }

    /// Indexes of the headings sharing heading `index`'s parent, itself included.
    #[must_use]
    pub fn siblings(&self, index: usize) -> Vec<usize> {
        let Some(h) = self.headings.get(index) else {
            return Vec::new();
        };
        match h.parent {
            Some(p) => self.children(p),
            None => (0..self.headings.len())
                .filter(|&i| self.headings[i].parent.is_none())
                .collect(),
        }
    }

    /// Own tags, then ancestors' (nearest first), then file tags; no duplicates.
    #[must_use]
    pub fn inherited_tags(&self, index: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |tags: &[String]| {
            for t in tags {
                if !out.contains(t) {
                    out.push(t.clone());
                }
            }
        };
        let mut cur = self.headings.get(index);
        while let Some(h) = cur {
            add(&h.tags);
            cur = h.parent.and_then(|p| self.headings.get(p));
        }
        add(&self.preamble.file_tags);
        out
    }

    /// The body text of heading `index` (after its metadata, before the next heading).
    #[must_use]
    pub fn body_text(&self, index: usize) -> &str {
        self.headings
            .get(index)
            .map_or("", |h| &self.text[h.spans.body.clone()])
    }

    /// The whole subtree text of heading `index`.
    #[must_use]
    pub fn subtree_text(&self, index: usize) -> &str {
        self.headings
            .get(index)
            .map_or("", |h| &self.text[h.spans.line.start..h.spans.subtree_end])
    }

    /// The first heading whose `:ID:` is `id`.
    #[must_use]
    pub fn find_by_id(&self, id: &str) -> Option<&Heading> {
        self.headings.iter().find(|h| h.id() == Some(id))
    }

    /// Replaces the bytes in `range` with `with`, re-parsing incrementally.
    /// Every other byte stays as it was. This is the primitive every edit uses.
    /// A `range` out of bounds or off a char boundary is clamped to a valid one.
    pub fn replace_range(&mut self, range: Range<usize>, with: &str) {
        let end = floor_char_boundary(&self.text, range.end.min(self.text.len()));
        let start = floor_char_boundary(&self.text, range.start.min(end));
        self.text.replace_range(start..end, with);
        self.org
            .replace_range(TextRange::new(text_size(start), text_size(end)), with);
        self.reindex();
    }

    /// Rebuilds the headings and preamble from the current text and tree.
    fn reindex(&mut self) {
        let mut raw = Vec::new();
        collect_headlines(
            &SyntaxNode::new_root(self.org.green().clone()),
            None,
            &mut raw,
        );
        let mut headings = Vec::with_capacity(raw.len());
        for (i, r) in raw.iter().enumerate() {
            let next_start = raw.get(i + 1).map_or(r.end, |n| n.start);
            headings.push(build_heading(&self.text, i, r, next_start.min(r.end)));
        }
        let preamble_end = headings
            .first()
            .map_or(self.text.len(), |h| h.spans.line.start);
        self.preamble = parse_preamble(&self.text, preamble_end);
        self.headings = headings;
    }
}

fn text_size(n: usize) -> TextSize {
    // Org files over 4 GiB are not supported by orgize (rowan uses u32).
    TextSize::from(u32::try_from(n).unwrap_or(u32::MAX))
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn span(range: TextRange) -> Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}

/// The parts of a headline that orgize knows about.
struct RawHeadline {
    start: usize,
    end: usize,
    parent: Option<usize>,
    stars: Range<usize>,
    keyword: Option<(Range<usize>, String)>,
    priority: Option<(Range<usize>, String)>,
    title: Option<Range<usize>>,
    tags: Option<(Range<usize>, Vec<String>)>,
}

fn collect_headlines(node: &SyntaxNode, parent: Option<usize>, out: &mut Vec<RawHeadline>) {
    for child in node.children().filter(|c| c.kind() == SyntaxKind::HEADLINE) {
        let index = out.len();
        out.push(raw_headline(&child, parent));
        collect_headlines(&child, Some(index), out);
    }
}

fn raw_headline(node: &SyntaxNode, parent: Option<usize>) -> RawHeadline {
    let range = span(node.text_range());
    let mut raw = RawHeadline {
        start: range.start,
        end: range.end,
        parent,
        stars: range.start..range.start,
        keyword: None,
        priority: None,
        title: None,
        tags: None,
    };
    for el in node.children_with_tokens() {
        let r = span(el.text_range());
        match el.kind() {
            SyntaxKind::HEADLINE_STARS => raw.stars = r,
            SyntaxKind::HEADLINE_KEYWORD_TODO | SyntaxKind::HEADLINE_KEYWORD_DONE => {
                if let Some(t) = el.as_token() {
                    raw.keyword = Some((r, t.text().to_string()));
                }
            }
            SyntaxKind::HEADLINE_PRIORITY => {
                let value = el.as_node().and_then(|n| {
                    n.children_with_tokens()
                        .filter_map(orgize::rowan::NodeOrToken::into_token)
                        .find(|t| t.kind() == SyntaxKind::TEXT)
                        .map(|t| t.text().to_string())
                });
                raw.priority = Some((r, value.unwrap_or_default()));
            }
            SyntaxKind::HEADLINE_TITLE => raw.title = Some(r),
            SyntaxKind::HEADLINE_TAGS => {
                let tags = el.as_node().map_or_else(Vec::new, |n| {
                    n.children_with_tokens()
                        .filter_map(orgize::rowan::NodeOrToken::into_token)
                        .filter(|t| t.kind() == SyntaxKind::TEXT)
                        .map(|t| t.text().to_string())
                        .collect()
                });
                raw.tags = Some((r, tags));
            }
            SyntaxKind::NEW_LINE => break,
            _ => {}
        }
    }
    raw
}

fn build_heading(text: &str, index: usize, raw: &RawHeadline, section_end: usize) -> Heading {
    let line = text::line_from(text, raw.start);
    let title_span = raw.title.clone().map_or_else(
        || {
            // No title: an empty span after the last part before the tags.
            let after = [
                Some(raw.stars.end),
                raw.keyword.as_ref().map(|k| k.0.end),
                raw.priority.as_ref().map(|p| p.0.end),
            ]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(raw.stars.end);
            after..after
        },
        |r| trim_span(text, r),
    );
    let meta = scan_metadata(text, line.end.min(section_end), section_end);
    Heading {
        index,
        level: raw.stars.len(),
        keyword: raw.keyword.as_ref().map(|k| k.1.clone()),
        priority: raw.priority.as_ref().map(|p| p.1.clone()),
        title: text[title_span.clone()].to_string(),
        tags: raw.tags.as_ref().map_or_else(Vec::new, |t| t.1.clone()),
        planning: meta.planning,
        properties: meta.properties,
        logbook: meta.logbook,
        active_timestamps: meta.active_timestamps,
        dedicated_timestamps: meta.dedicated_timestamps,
        parent: raw.parent,
        spans: HeadingSpans {
            line: line.range(),
            line_content: line.start..line.content_end,
            stars: raw.stars.clone(),
            keyword: raw.keyword.as_ref().map(|k| k.0.clone()),
            priority: raw.priority.as_ref().map(|p| p.0.clone()),
            title: title_span,
            tags: raw.tags.as_ref().map(|t| t.0.clone()),
            planning: meta.planning_line,
            dedicated: meta.dedicated_line,
            properties: meta.properties_drawer,
            logbook: meta.logbook_drawer,
            drawers_start: meta.drawers_start,
            body: meta.body_start..section_end,
            section_end,
            subtree_end: raw.end,
        },
    }
}

/// `range` without leading and trailing whitespace.
fn trim_span(text: &str, range: Range<usize>) -> Range<usize> {
    let s = &text[range.clone()];
    let start = range.start + (s.len() - s.trim_start().len());
    let end = range.end - (s.len() - s.trim_end().len());
    start..end.max(start)
}

/// Everything the line scanner reads from a heading's section.
struct Metadata {
    planning: Planning,
    planning_line: Option<Range<usize>>,
    dedicated_line: Option<Range<usize>>,
    dedicated_timestamps: Vec<Timestamp>,
    drawers_start: usize,
    properties: Vec<(String, String)>,
    properties_drawer: Option<Range<usize>>,
    logbook: Vec<String>,
    logbook_drawer: Option<Range<usize>>,
    body_start: usize,
    active_timestamps: Vec<Timestamp>,
}

/// Bloom's `parseHeadline`, after the heading line: planning line, dedicated
/// timestamp line, then `:PROPERTIES:`/`:LOGBOOK:` drawers in either order.
/// An unclosed drawer is body text.
fn scan_metadata(text: &str, start: usize, end: usize) -> Metadata {
    let lines = text::lines_in(text, start..end);
    let mut i = 0;
    let mut meta = Metadata {
        planning: Planning::default(),
        planning_line: None,
        dedicated_line: None,
        dedicated_timestamps: Vec::new(),
        drawers_start: start,
        properties: Vec::new(),
        properties_drawer: None,
        logbook: Vec::new(),
        logbook_drawer: None,
        body_start: start,
        active_timestamps: Vec::new(),
    };
    if let Some(line) = lines.first()
        && is_planning_line(line.content(text))
    {
        meta.planning = parse_planning(line.content(text));
        meta.planning_line = Some(line.range());
        i += 1;
    }
    let mut canonical_dedicated = false;
    if let Some(line) = lines.get(i)
        && let Some(stamps) = dedicated_line_stamps(line.content(text))
    {
        meta.dedicated_timestamps = stamps;
        meta.dedicated_line = Some(line.range());
        canonical_dedicated = true;
        i += 1;
    }
    meta.drawers_start = lines.get(i).map_or(end, |l| l.start);
    while let Some(line) = lines.get(i) {
        let marker = line.content(text).trim();
        let is_props =
            marker.eq_ignore_ascii_case(":PROPERTIES:") && meta.properties_drawer.is_none();
        let is_log = marker.eq_ignore_ascii_case(":LOGBOOK:") && meta.logbook_drawer.is_none();
        if !is_props && !is_log {
            break;
        }
        let Some(close) = (i + 1..lines.len()).find(|&j| is_end_line(lines[j].content(text)))
        else {
            break;
        };
        let inner = &lines[i + 1..close];
        let range = line.start..lines[close].end;
        if is_props {
            meta.properties = inner
                .iter()
                .filter_map(|l| parse_property(l.content(text)))
                .collect();
            meta.properties_drawer = Some(range);
        } else {
            meta.logbook = inner.iter().map(|l| l.content(text).to_string()).collect();
            meta.logbook_drawer = Some(range);
        }
        i = close + 1;
    }
    meta.body_start = lines.get(i).map_or(end, |l| l.start);
    let body = &text[meta.body_start..end];
    let mut active: Vec<Timestamp> = Timestamp::scan(body)
        .into_iter()
        .map(|(_, ts)| ts)
        .filter(|ts| ts.active)
        .collect();
    // Bloom's older layout: the dedicated line below the drawers, as the
    // body's first line. Its stamps are already in `active` via the body.
    if !canonical_dedicated
        && let Some(line) = lines.get(i)
        && let Some(stamps) = dedicated_line_stamps(line.content(text))
    {
        meta.dedicated_timestamps = stamps;
        meta.dedicated_line = Some(line.range());
    }
    if canonical_dedicated {
        active.extend(meta.dedicated_timestamps.iter().cloned());
    }
    meta.active_timestamps = active;
    meta
}

pub(crate) fn is_planning_line(line: &str) -> bool {
    let t = line.trim_start();
    PlanningKind::ALL
        .iter()
        .any(|k| t.starts_with(k.keyword()) && t[k.keyword().len()..].starts_with(':'))
}

fn is_end_line(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case(":END:")
}

/// The entries of a planning line, in order: kind and raw value (trimmed).
pub(crate) fn planning_parts(line: &str) -> Vec<(PlanningKind, String)> {
    let mut starts: Vec<(usize, PlanningKind)> = PlanningKind::ALL
        .iter()
        .flat_map(|&k| {
            let pat = format!("{}:", k.keyword());
            line.match_indices(&pat)
                .map(move |(i, _)| (i, k))
                .collect::<Vec<_>>()
        })
        .collect();
    starts.sort_by_key(|(i, _)| *i);
    starts
        .iter()
        .enumerate()
        .map(|(n, &(i, kind))| {
            let value_start = i + kind.keyword().len() + 1;
            let value_end = starts.get(n + 1).map_or(line.len(), |s| s.0);
            (kind, line[value_start..value_end].trim().to_string())
        })
        .collect()
}

fn parse_planning(line: &str) -> Planning {
    let mut p = Planning::default();
    for (kind, value) in planning_parts(line) {
        let ts = Timestamp::scan(&value).into_iter().next().map(|(_, ts)| ts);
        match kind {
            PlanningKind::Scheduled => p.scheduled = ts,
            PlanningKind::Deadline => p.deadline = ts,
            PlanningKind::Closed => p.closed = ts,
        }
    }
    p
}

/// The stamps of a line that holds only active timestamps and whitespace.
pub(crate) fn dedicated_line_stamps(line: &str) -> Option<Vec<Timestamp>> {
    let found = Timestamp::scan(line);
    if found.is_empty() || found.iter().any(|(_, ts)| !ts.active) {
        return None;
    }
    let mut pos = 0;
    for (r, _) in &found {
        if !line[pos..r.start].trim().is_empty() {
            return None;
        }
        pos = r.end;
    }
    if !line[pos..].trim().is_empty() {
        return None;
    }
    Some(found.into_iter().map(|(_, ts)| ts).collect())
}

/// `:KEY: value` (Bloom's `PROPERTY_LINE`).
pub(crate) fn parse_property(line: &str) -> Option<(String, String)> {
    let rest = line.trim_start().strip_prefix(':')?;
    let colon = rest.find(':')?;
    let key = &rest[..colon];
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some((key.to_string(), rest[colon + 1..].trim().to_string()))
}

/// `#+KEY: value` (Bloom's `PREAMBLE_KEYWORD`), on a trimmed line.
fn parse_keyword_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("#+")?;
    let colon = rest.find(':')?;
    let key = &rest[..colon];
    let mut chars = key.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then(|| (key.to_string(), rest[colon + 1..].trim().to_string()))
}

fn parse_preamble(text: &str, end: usize) -> Preamble {
    let lines: Vec<Line> = text::lines_in(text, 0..end);
    let content = |i: usize| lines[i].content(text);
    let mut pre = Preamble {
        range: 0..end,
        ..Preamble::default()
    };
    let mut i = 0;
    if !lines.is_empty()
        && content(0).trim().eq_ignore_ascii_case(":PROPERTIES:")
        && let Some(close) = (1..lines.len()).find(|&j| is_end_line(content(j)))
    {
        pre.properties = (1..close)
            .filter_map(|j| parse_property(content(j)))
            .collect();
        pre.properties_drawer = Some(0..lines[close].end);
        i = close + 1;
    }
    while i < lines.len() {
        let line = content(i).trim();
        if let Some(kw) = parse_keyword_line(line) {
            pre.keywords.push(kw);
        } else if !line.is_empty() {
            break;
        }
        i += 1;
    }
    let intro_start = lines.get(i).map_or(end, |l| l.start);
    if !text[intro_start..end].trim().is_empty() {
        pre.intro = Some(intro_start..end);
    }
    for j in 0..lines.len() {
        if let Some((key, value)) = parse_keyword_line(content(j).trim())
            && key.eq_ignore_ascii_case("FILETAGS")
        {
            pre.file_tags.extend(
                value
                    .split([':', ' '])
                    .filter(|t| !t.is_empty())
                    .map(String::from),
            );
        }
    }
    pre
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(s: &str) -> Document {
        Document::parse(s, Keywords::parse("TODO NEXT | DONE"))
    }

    const SAMPLE: &str = "\
:PROPERTIES:
:ID: file-1
:END:
#+TITLE: Sample
#+FILETAGS: :work:home:

Intro line.
* TODO [#A] First *bold*   :x:y:
SCHEDULED: <2026-11-21 Sat +1w> DEADLINE: <2026-11-22 Sun>
:LOGBOOK:
- Note
:END:
:PROPERTIES:
:ID: h1
:Effort: 1:00
:END:
Body <2026-12-01 Tue> and [2026-12-02 Wed].
** NEXT Child :c:
<2026-11-25 Wed>
:PROPERTIES:
:A: 1
:END:
text
*** Grandchild
* Second
";

    #[test]
    fn preamble() {
        let d = doc(SAMPLE);
        let p = d.preamble();
        assert_eq!(p.file_id(), Some("file-1"));
        assert_eq!(
            p.keywords,
            [
                ("TITLE".to_string(), "Sample".to_string()),
                ("FILETAGS".to_string(), ":work:home:".to_string())
            ]
        );
        assert_eq!(p.file_tags, ["work", "home"]);
        assert_eq!(d.intro_text(), Some("Intro line.\n"));
    }

    #[test]
    fn heading_fields() {
        let d = doc(SAMPLE);
        assert_eq!(d.headings().len(), 4);
        let h = &d.headings()[0];
        assert_eq!(h.level, 1);
        assert_eq!(h.keyword.as_deref(), Some("TODO"));
        assert_eq!(h.priority.as_deref(), Some("A"));
        assert_eq!(h.title, "First *bold*");
        assert_eq!(h.tags, ["x", "y"]);
        assert_eq!(
            h.planning
                .scheduled
                .as_ref()
                .map(Timestamp::format)
                .as_deref(),
            Some("<2026-11-21 Sat +1w>")
        );
        assert_eq!(
            h.planning
                .deadline
                .as_ref()
                .map(Timestamp::format)
                .as_deref(),
            Some("<2026-11-22 Sun>")
        );
        assert_eq!(h.planning.closed, None);
        assert_eq!(h.logbook, ["- Note"]);
        assert_eq!(h.id(), Some("h1"));
        assert_eq!(h.property("effort"), Some("1:00"));
        assert_eq!(h.active_timestamps.len(), 1);
        assert_eq!(
            d.body_text(0),
            "Body <2026-12-01 Tue> and [2026-12-02 Wed].\n"
        );
        assert_eq!(&d.text()[h.spans.tags.clone().unwrap()], ":x:y:");
        assert_eq!(&d.text()[h.spans.keyword.clone().unwrap()], "TODO");
        assert_eq!(&d.text()[h.spans.priority.clone().unwrap()], "[#A]");
    }

    #[test]
    fn dedicated_line_above_drawers() {
        let d = doc(SAMPLE);
        let h = &d.headings()[1];
        assert_eq!(h.keyword.as_deref(), Some("NEXT"));
        assert_eq!(h.dedicated_timestamps.len(), 1);
        assert_eq!(h.active_timestamps.len(), 1);
        assert_eq!(h.property("A"), Some("1"));
        assert_eq!(d.body_text(1), "text\n");
    }

    #[test]
    fn tree_navigation() {
        let d = doc(SAMPLE);
        assert_eq!(d.headings()[2].parent, Some(1));
        assert_eq!(d.children(0), [1]);
        assert_eq!(d.descendants(0), 1..3);
        assert_eq!(d.descendants(3), 4..4);
        assert_eq!(d.siblings(0), [0, 3]);
        assert_eq!(d.inherited_tags(2), ["c", "x", "y", "work", "home"]);
        assert!(d.subtree_text(1).starts_with("** NEXT Child"));
        assert!(d.subtree_text(1).ends_with("*** Grandchild\n"));
        assert_eq!(d.find_by_id("h1").map(|h| h.index), Some(0));
    }

    #[test]
    fn unclosed_drawer_is_body() {
        let d = doc("* a\n:PROPERTIES:\n:ID: 1\nno end\n");
        let h = &d.headings()[0];
        assert_eq!(h.properties.len(), 0);
        assert_eq!(d.body_text(0), ":PROPERTIES:\n:ID: 1\nno end\n");
    }

    #[test]
    fn preflip_dedicated_line() {
        let d = doc("* a\n:PROPERTIES:\n:ID: 1\n:END:\n<2026-11-21 Sat> <2026-11-22 Sun>\nrest\n");
        let h = &d.headings()[0];
        assert_eq!(h.dedicated_timestamps.len(), 2);
        assert_eq!(h.active_timestamps.len(), 2, "not counted twice");
    }

    #[test]
    fn crlf_and_unconfigured_keyword() {
        let d = doc("* WAIT thing :t:\r\nSCHEDULED: <2026-11-21 Sat>\r\nbody\r\n");
        let h = &d.headings()[0];
        assert_eq!(h.keyword, None);
        assert_eq!(h.title, "WAIT thing");
        assert_eq!(h.tags, ["t"]);
        assert!(h.planning.scheduled.is_some());
        assert_eq!(d.body_text(0), "body\r\n");
    }

    #[test]
    fn empty_and_headingless() {
        let d = doc("");
        assert_eq!(d.headings().len(), 0);
        assert_eq!(d.preamble().intro, None);
        let d = doc("#+TITLE: x\n\n");
        assert_eq!(d.preamble().intro, None);
        let d = doc("just text");
        assert_eq!(d.intro_text(), Some("just text"));
    }

    #[test]
    fn replace_range_reindexes() {
        let mut d = doc("* a\n* b\n");
        d.replace_range(4..4, "** child\n");
        assert_eq!(d.text(), "* a\n** child\n* b\n");
        assert_eq!(d.headings().len(), 3);
        assert_eq!(d.headings()[1].parent, Some(0));
    }
}
