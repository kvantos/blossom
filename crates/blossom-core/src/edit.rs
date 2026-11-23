//! Edits on a [`Document`], ported from Bloom's `OrgMutations.kt`.
//!
//! Every edit is one or more [`Document::replace_range`] splices of the
//! smallest range that changes: setting a keyword replaces the keyword, not
//! the heading line, so Emacs's tag alignment and anything else on the line
//! survive. New lines use the file's own line ending. Headings are addressed
//! by index; edits that move headings return the new index.

use std::fmt;
use std::ops::Range;

use chrono::NaiveDateTime;

use crate::document::{
    Document, Heading, PlanningKind, is_planning_line, parse_property, planning_parts,
};
use crate::text::{self, Line};
use crate::timestamp::Timestamp;

/// Why an edit was refused. The document is unchanged when one is returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// No heading has this index.
    NoSuchHeading(usize),
    /// The keyword is not configured (or not done-type where one is needed).
    UnknownKeyword(String),
    /// Tags may hold letters, digits and `_ @ # % -` only.
    InvalidTag(String),
    /// A priority is letters or digits, e.g. `A`.
    InvalidPriority(String),
    /// A property key is non-empty with no whitespace or colons.
    InvalidPropertyKey(String),
    /// Titles and property values must be a single line.
    MultilineText,
    /// The position is not on a checkbox list item.
    NotACheckbox,
    /// There is no sibling in that direction.
    NoSibling,
    /// A level-1 heading can't be promoted.
    AlreadyTopLevel,
    /// A heading needs a previous sibling to become its parent when demoted.
    NoPreviousSibling,
    /// A subtree can't be refiled under itself or its descendants.
    TargetInsideSubtree,
    /// Text to insert as a subtree must start with a heading line.
    NotASubtree,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::NoSuchHeading(i) => write!(f, "no heading at index {i}"),
            EditError::UnknownKeyword(k) => write!(f, "unknown keyword {k:?}"),
            EditError::InvalidTag(t) => {
                write!(f, "invalid tag {t:?}: use letters, numbers, _ @ # % or -")
            }
            EditError::InvalidPriority(p) => write!(f, "invalid priority {p:?}"),
            EditError::InvalidPropertyKey(k) => write!(f, "invalid property key {k:?}"),
            EditError::MultilineText => f.write_str("text must be a single line"),
            EditError::NotACheckbox => f.write_str("not a checkbox item"),
            EditError::NoSibling => f.write_str("no sibling in that direction"),
            EditError::AlreadyTopLevel => f.write_str("heading is already top level"),
            EditError::NoPreviousSibling => {
                f.write_str("heading has no previous sibling to nest under")
            }
            EditError::TargetInsideSubtree => f.write_str("can't refile a subtree under itself"),
            EditError::NotASubtree => f.write_str("text does not start with a heading"),
        }
    }
}

impl std::error::Error for EditError {}

/// Where [`Document::insert_heading`] and [`Document::insert_subtree`] put
/// new headings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAt {
    /// First child of the heading at this index.
    FirstChild(usize),
    /// Last child of the heading at this index.
    LastChild(usize),
    /// Sibling just above the heading at this index.
    Before(usize),
    /// Sibling just below the subtree of the heading at this index.
    After(usize),
    /// Level 1, above the first heading (below the preamble and intro).
    FirstTopLevel,
    /// Level 1, at the end of the file.
    LastTopLevel,
}

/// A heading to insert.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewHeading {
    /// The title; may be empty for a heading created for immediate editing.
    pub title: String,
    /// A configured TODO keyword.
    pub keyword: Option<String>,
    /// Adds an `:ID:` property (the caller makes the id; this crate is pure).
    pub id: Option<String>,
    /// Adds a `:CREATED:` inactive timestamp.
    pub created: Option<NaiveDateTime>,
    /// Body text under the heading, after any properties.
    pub body: Option<String>,
}

impl Document {
    // --- heading line ---

    /// Sets or removes the TODO keyword. This is a plain text change; use
    /// [`Document::change_keyword`] for org's done/repeat semantics.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`], or [`EditError::UnknownKeyword`] for a
    /// keyword that isn't configured.
    pub fn set_keyword(&mut self, index: usize, keyword: Option<&str>) -> Result<(), EditError> {
        let h = self.get(index)?;
        if let Some(k) = keyword
            && !self.keywords().contains(k)
        {
            return Err(EditError::UnknownKeyword(k.to_string()));
        }
        match (h.spans.keyword.clone(), keyword) {
            (Some(r), Some(k)) => self.replace_range(r, k),
            (Some(r), None) => {
                let end = self.skip_blanks(r.end, h.spans.line_content.end);
                self.replace_range(r.start..end, "");
            }
            (None, Some(k)) => self.insert_after_stars(&h, k),
            (None, None) => {}
        }
        Ok(())
    }

    /// Sets or removes the priority cookie, `[#A]`.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`], or [`EditError::InvalidPriority`] unless
    /// the priority is ASCII letters or digits.
    pub fn set_priority(&mut self, index: usize, priority: Option<&str>) -> Result<(), EditError> {
        let h = self.get(index)?;
        if let Some(p) = priority
            && (p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return Err(EditError::InvalidPriority(p.to_string()));
        }
        let cookie = priority.map(|p| format!("[#{p}]"));
        match (h.spans.priority.clone(), cookie) {
            (Some(r), Some(c)) => self.replace_range(r, &c),
            (Some(r), None) => {
                let end = self.skip_blanks(r.end, h.spans.line_content.end);
                self.replace_range(r.start..end, "");
            }
            (None, Some(c)) => match h.spans.keyword.clone() {
                Some(k) => self.replace_range(k.end..k.end, &format!(" {c}")),
                None => self.insert_after_stars(&h, &c),
            },
            (None, None) => {}
        }
        Ok(())
    }

    /// Replaces the heading's own tags. An empty list removes the tag group.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`], or [`EditError::InvalidTag`].
    pub fn set_tags(&mut self, index: usize, tags: &[String]) -> Result<(), EditError> {
        let h = self.get(index)?;
        if let Some(bad) = tags.iter().find(|t| !is_valid_tag(t)) {
            return Err(EditError::InvalidTag(bad.clone()));
        }
        let group = format!(":{}:", tags.join(":"));
        let content = h.spans.line_content.clone();
        match (h.spans.tags.clone(), tags.is_empty()) {
            (Some(r), false) => self.replace_range(r, &group),
            (Some(r), true) => {
                let start = self
                    .trimmed_end(content.start, r.start)
                    .max(h.spans.stars.end + 1);
                self.replace_range(start.min(r.start)..content.end, "");
            }
            (None, false) => {
                let end = self.trimmed_end(content.start, content.end);
                if end <= h.spans.stars.end {
                    self.replace_range(h.spans.stars.end..content.end, &format!(" {group}"));
                } else {
                    self.replace_range(end..content.end, &format!("  {group}"));
                }
            }
            (None, true) => {}
        }
        Ok(())
    }

    // --- planning ---

    /// Sets or removes `SCHEDULED:`.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`].
    pub fn set_scheduled(&mut self, index: usize, ts: Option<&Timestamp>) -> Result<(), EditError> {
        self.write_planning(
            index,
            &[(PlanningKind::Scheduled, ts.map(Timestamp::format))],
        )
    }

    /// Sets or removes `DEADLINE:`.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`].
    pub fn set_deadline(&mut self, index: usize, ts: Option<&Timestamp>) -> Result<(), EditError> {
        self.write_planning(
            index,
            &[(PlanningKind::Deadline, ts.map(Timestamp::format))],
        )
    }

    /// Rewrites the planning line with `changes` applied (`None` removes an
    /// entry). Entries keep their order and untouched ones their exact text;
    /// new ones go at the end. The line is created or removed as needed.
    fn write_planning(
        &mut self,
        index: usize,
        changes: &[(PlanningKind, Option<String>)],
    ) -> Result<(), EditError> {
        let h = self.get(index)?;
        let line = h
            .spans
            .planning
            .clone()
            .map(|r| text::line_from(self.text(), r.start));
        let mut parts = line.map_or_else(Vec::new, |l| planning_parts(l.content(self.text())));
        for (kind, value) in changes {
            let pos = parts.iter().position(|(k, _)| k == kind);
            match (pos, value) {
                (Some(p), Some(v)) => parts[p].1.clone_from(v),
                (Some(p), None) => {
                    parts.remove(p);
                }
                (None, Some(v)) => parts.push((*kind, v.clone())),
                (None, None) => {}
            }
        }
        let joined = parts
            .iter()
            .map(|(k, v)| format!("{}: {v}", k.keyword()))
            .collect::<Vec<_>>()
            .join(" ");
        match line {
            Some(l) if parts.is_empty() => {
                let r = text::removal_range(self.text(), l.range());
                self.replace_range(r, "");
            }
            Some(l) => {
                let new = format!("{}{joined}", text::indent(l.content(self.text())));
                self.replace_range(l.start..l.content_end, &new);
            }
            None if parts.is_empty() => {}
            None => {
                let indent = self.metadata_indent(&h);
                self.insert_lines(h.spans.line.end, &[format!("{indent}{joined}")]);
            }
        }
        Ok(())
    }

    // --- todo state ---

    /// Changes the TODO state the way every UI action should: a done-type
    /// keyword gets [`Document::mark_done`]; leaving a done-type keyword
    /// drops the stale `CLOSED:` ([`Document::reopen`]); anything else is a
    /// plain [`Document::set_keyword`].
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`] or [`EditError::UnknownKeyword`].
    pub fn change_keyword(
        &mut self,
        index: usize,
        keyword: Option<&str>,
        now: NaiveDateTime,
    ) -> Result<(), EditError> {
        let h = self.get(index)?;
        let is_done = |k: Option<&str>| k.is_some_and(|k| self.keywords().is_done(k));
        if let Some(k) = keyword.filter(|k| is_done(Some(k))) {
            self.mark_done(index, k, now)
        } else if is_done(h.keyword.as_deref()) {
            self.reopen(index, keyword)
        } else {
            self.set_keyword(index, keyword)
        }
    }

    /// Marks a heading done per Emacs's `org-auto-repeat-maybe`, as Bloom does.
    ///
    /// A repeating entry (any active timestamp in its own section has a
    /// non-zero repeater) is not closed: every repeating active stamp moves to
    /// its next date, a SCHEDULED without a repeater and any CLOSED are
    /// dropped, the keyword goes back to the `REPEAT_TO_STATE` property or the
    /// first active keyword, and the change is logged in `:LOGBOOK:` and
    /// stamped as `:LAST_REPEAT:`. Otherwise the keyword becomes `done` and
    /// `CLOSED:` is set; leaving a not-done state also logs it.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`], or [`EditError::UnknownKeyword`] when
    /// `done` isn't a done-type keyword.
    pub fn mark_done(
        &mut self,
        index: usize,
        done: &str,
        now: NaiveDateTime,
    ) -> Result<(), EditError> {
        let h = self.get(index)?;
        if !self.keywords().is_done(done) {
            return Err(EditError::UnknownKeyword(done.to_string()));
        }
        let was_done = h
            .keyword
            .as_deref()
            .is_some_and(|k| self.keywords().is_done(k));
        let stamp = Timestamp::inactive_at(now);
        let log = state_change_line(done, h.keyword.as_deref().unwrap_or(""), &stamp);
        if !was_done && self.is_repeating(index) {
            let mut changes = vec![(PlanningKind::Closed, None)];
            if h.planning.scheduled.as_ref().is_some_and(|s| !s.repeats()) {
                changes.push((PlanningKind::Scheduled, None));
            }
            self.write_planning(index, &changes)?;
            self.shift_repeaters(index, now)?;
            let to = self.repeat_to_state(&h);
            if self.get(index)?.keyword != to {
                self.set_keyword(index, to.as_deref())?;
            }
            self.append_logbook_entry(index, &log)?;
            self.upsert_property(index, "LAST_REPEAT", &stamp.format())?;
        } else {
            self.write_planning(index, &[(PlanningKind::Closed, Some(stamp.format()))])?;
            self.set_keyword(index, Some(done))?;
            if !was_done {
                self.append_logbook_entry(index, &log)?;
            }
        }
        Ok(())
    }

    /// Undoes a non-repeating [`Document::mark_done`]: drops `CLOSED:` and
    /// sets `keyword`.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`] or [`EditError::UnknownKeyword`].
    pub fn reopen(&mut self, index: usize, keyword: Option<&str>) -> Result<(), EditError> {
        if let Some(k) = keyword
            && !self.keywords().contains(k)
        {
            return Err(EditError::UnknownKeyword(k.to_string()));
        }
        self.write_planning(index, &[(PlanningKind::Closed, None)])?;
        self.set_keyword(index, keyword)
    }

    /// Whether marking the heading done repeats it instead of closing it
    /// (Emacs's `org-get-repeat`): an active stamp anywhere in its own section,
    /// heading line included, has a non-zero repeater.
    #[must_use]
    pub fn is_repeating(&self, index: usize) -> bool {
        self.heading(index).is_some_and(|h| {
            Timestamp::scan(&self.text()[h.spans.line.start..h.spans.section_end])
                .iter()
                .any(|(_, ts)| ts.active && ts.repeats())
        })
    }

    fn shift_repeaters(&mut self, index: usize, now: NaiveDateTime) -> Result<(), EditError> {
        let h = self.get(index)?;
        let start = h.spans.line.start;
        let found = Timestamp::scan(&self.text()[start..h.spans.section_end]);
        // Back to front, so earlier offsets stay valid.
        for (r, ts) in found.into_iter().rev() {
            if ts.active && ts.repeats() {
                self.replace_range(start + r.start..start + r.end, &ts.advance(now).format());
            }
        }
        Ok(())
    }

    fn repeat_to_state(&self, h: &Heading) -> Option<String> {
        if let Some(state) = h.property("REPEAT_TO_STATE").map(str::trim)
            && self.keywords().contains(state)
        {
            return Some(state.to_string());
        }
        h.keyword.as_ref()?;
        self.keywords()
            .active
            .first()
            .cloned()
            .or_else(|| h.keyword.clone())
    }

    // --- drawers ---

    /// Sets property `key` in the heading's `:PROPERTIES:` drawer, replacing
    /// an entry with that key (any case) or adding one, and creating the
    /// drawer at the top of the drawer run if there is none.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`], [`EditError::InvalidPropertyKey`], or
    /// [`EditError::MultilineText`] for a value with a line break.
    pub fn upsert_property(
        &mut self,
        index: usize,
        key: &str,
        value: &str,
    ) -> Result<(), EditError> {
        let h = self.get(index)?;
        if key.is_empty() || key.contains(|c: char| c.is_whitespace() || c == ':') {
            return Err(EditError::InvalidPropertyKey(key.to_string()));
        }
        if value.contains(['\n', '\r']) {
            return Err(EditError::MultilineText);
        }
        let entry = format!(":{key}: {value}");
        if let Some(r) = h.spans.properties.clone() {
            let lines = text::lines_in(self.text(), r);
            let (Some(marker), Some(end)) = (lines.first().copied(), lines.last().copied()) else {
                return Ok(());
            };
            let existing = lines[1..lines.len() - 1].iter().copied().find(|l| {
                parse_property(l.content(self.text()))
                    .is_some_and(|(k, _)| k.eq_ignore_ascii_case(key))
            });
            if let Some(l) = existing {
                let new = format!("{}{entry}", text::indent(l.content(self.text())));
                self.replace_range(l.start..l.content_end, &new);
            } else {
                let indent = text::indent(marker.content(self.text())).to_string();
                self.insert_lines(end.start, &[format!("{indent}{entry}")]);
            }
        } else {
            let indent = self.metadata_indent(&h);
            let lines = [":PROPERTIES:", &entry, ":END:"].map(|l| format!("{indent}{l}"));
            self.insert_lines(h.spans.drawers_start, &lines);
        }
        Ok(())
    }

    /// Adds `entry` as the first line of the heading's `:LOGBOOK:` (newest
    /// first, Emacs's default), creating the drawer after `:PROPERTIES:` (or
    /// at the top of the drawer run) when there is none.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`] or [`EditError::MultilineText`].
    pub fn append_logbook_entry(&mut self, index: usize, entry: &str) -> Result<(), EditError> {
        let h = self.get(index)?;
        if entry.contains(['\n', '\r']) {
            return Err(EditError::MultilineText);
        }
        if let Some(r) = h.spans.logbook.clone() {
            let marker = text::line_from(self.text(), r.start);
            let indent = text::indent(marker.content(self.text())).to_string();
            self.insert_lines(marker.end, &[format!("{indent}{entry}")]);
        } else {
            let at = h
                .spans
                .properties
                .as_ref()
                .map_or(h.spans.drawers_start, |p| p.end);
            let indent = self.metadata_indent(&h);
            let lines = [":LOGBOOK:", entry, ":END:"].map(|l| format!("{indent}{l}"));
            self.insert_lines(at, &lines);
        }
        Ok(())
    }

    // --- checkboxes ---

    /// Toggles the checkbox of the list item on the line containing byte
    /// `offset`: `[X]` becomes `[ ]`, anything else `[X]`. Then, as Bloom does,
    /// refreshes the immediate parent item: a `[n/m]` or `[n%]` cookie it
    /// already has, and its own checkbox (checked when all its direct
    /// checkbox children are).
    ///
    /// # Errors
    ///
    /// [`EditError::NotACheckbox`].
    pub fn toggle_checkbox(&mut self, offset: usize) -> Result<(), EditError> {
        let text = self.text();
        if offset > text.len() || !text.is_char_boundary(offset) {
            return Err(EditError::NotACheckbox);
        }
        let lines = text::lines_in(text, 0..text.len());
        let start = text::line_at(text, offset).start;
        let idx = lines
            .iter()
            .position(|l| l.start == start)
            .ok_or(EditError::NotACheckbox)?;
        let item = list_item(lines[idx].content(text)).ok_or(EditError::NotACheckbox)?;
        let (mark_at, mark) = item.checkbox.ok_or(EditError::NotACheckbox)?;
        let new_mark = if mark == 'X' { ' ' } else { 'X' };
        let mut splices = vec![(lines[idx].start + mark_at, new_mark)];
        let mut cookie = None;
        if let Some(parent) = (0..idx)
            .rev()
            .map_while(|j| list_item(lines[j].content(text)).map(|it| (j, it)))
            .find(|(_, it)| it.indent < item.indent)
        {
            let (p, parent_item) = parent;
            // Checkbox state of the parent's direct children, with the toggle applied.
            let below: Vec<(usize, Option<char>)> = (p + 1..lines.len())
                .map_while(|j| {
                    let it = list_item(lines[j].content(text))?;
                    (it.indent > parent_item.indent).then(|| {
                        let state = if j == idx {
                            Some(new_mark)
                        } else {
                            it.checkbox.map(|c| c.1)
                        };
                        (it.indent, state)
                    })
                })
                .collect();
            let direct_indent = below.iter().map(|b| b.0).min();
            let direct: Vec<char> = below
                .iter()
                .filter(|b| Some(b.0) == direct_indent)
                .filter_map(|b| b.1)
                .collect();
            if !direct.is_empty() {
                let done = direct.iter().filter(|c| **c == 'X').count();
                let total = direct.len();
                let line = lines[p];
                let content = line.content(text);
                if let Some((r, percent)) = find_cookie(&content[parent_item.text_start..]) {
                    let r = line.start + parent_item.text_start + r.start
                        ..line.start + parent_item.text_start + r.end;
                    let new = if percent {
                        format!("[{}%]", (done * 100 + total / 2) / total)
                    } else {
                        format!("[{done}/{total}]")
                    };
                    cookie = Some((r, new));
                }
                if let Some((at, _)) = parent_item.checkbox {
                    splices.push((line.start + at, if done == total { 'X' } else { ' ' }));
                }
            }
        }
        // Back to front: the toggled line, then the parent's cookie, then its box.
        self.replace_char(splices[0].0, splices[0].1);
        if let Some((r, new)) = cookie {
            self.replace_range(r, &new);
        }
        if let Some(&(at, c)) = splices.get(1) {
            self.replace_char(at, c);
        }
        Ok(())
    }

    fn replace_char(&mut self, at: usize, c: char) {
        let len = self.text()[at..].chars().next().map_or(0, char::len_utf8);
        self.replace_range(at..at + len, c.encode_utf8(&mut [0; 4]));
    }

    // --- structure ---

    /// Swaps the heading's subtree with its previous (`up`) or next sibling's.
    /// Returns the heading's new index.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`] or [`EditError::NoSibling`].
    pub fn move_subtree(&mut self, index: usize, up: bool) -> Result<usize, EditError> {
        self.get(index)?;
        let siblings = self.siblings(index);
        let pos = siblings
            .iter()
            .position(|&s| s == index)
            .ok_or(EditError::NoSibling)?;
        let other = if up {
            pos.checked_sub(1).and_then(|p| siblings.get(p))
        } else {
            siblings.get(pos + 1)
        };
        let other = *other.ok_or(EditError::NoSibling)?;
        let (first, second) = (self.get(index.min(other))?, self.get(index.max(other))?);
        let text = self.text();
        let a = &text[first.spans.line.start..first.spans.subtree_end];
        let gap = &text[first.spans.subtree_end..second.spans.line.start];
        let b = &text[second.spans.line.start..second.spans.subtree_end];
        // The last subtree may lack a final line ending; keep that at the end.
        let eol = text::eol(text);
        let (b, a) = if b.ends_with('\n') {
            (b.to_string(), a.to_string())
        } else {
            (format!("{b}{eol}"), strip_eol(a).to_string())
        };
        let moved_start = if up {
            first.spans.line.start
        } else {
            first.spans.line.start + b.len() + gap.len()
        };
        let new = format!("{b}{gap}{a}");
        self.replace_range(first.spans.line.start..second.spans.subtree_end, &new);
        Ok(self.heading_index_at(moved_start))
    }

    /// Makes the heading and its subtree one level shallower.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`] or [`EditError::AlreadyTopLevel`].
    pub fn promote(&mut self, index: usize) -> Result<(), EditError> {
        if self.get(index)?.level <= 1 {
            return Err(EditError::AlreadyTopLevel);
        }
        self.shift_levels(index, -1);
        Ok(())
    }

    /// Makes the heading and its subtree one level deeper, under its previous
    /// sibling.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`] or [`EditError::NoPreviousSibling`].
    pub fn demote(&mut self, index: usize) -> Result<(), EditError> {
        self.get(index)?;
        if self.siblings(index).first() == Some(&index) {
            return Err(EditError::NoPreviousSibling);
        }
        self.shift_levels(index, 1);
        Ok(())
    }

    fn shift_levels(&mut self, index: usize, delta: isize) {
        let Some(h) = self.heading(index) else { return };
        let range = h.spans.line.start..h.spans.subtree_end;
        let new = self.releveled(index, delta);
        self.replace_range(range, &new);
    }

    /// The subtree text of heading `index` with every heading's stars changed
    /// by `delta` (never below one). Only real heading lines are touched.
    fn releveled(&self, index: usize, delta: isize) -> String {
        let Some(h) = self.heading(index) else {
            return String::new();
        };
        let text = self.text();
        let mut out = String::new();
        let mut pos = h.spans.line.start;
        for i in std::iter::once(index).chain(self.descendants(index)) {
            let stars = self.headings()[i].spans.stars.clone();
            out.push_str(&text[pos..stars.start]);
            let level = self.headings()[i].level.saturating_add_signed(delta).max(1);
            out.push_str(&"*".repeat(level));
            pos = stars.end;
        }
        out.push_str(&text[pos..h.spans.subtree_end]);
        out
    }

    /// Deletes the heading and its subtree.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`].
    pub fn delete_subtree(&mut self, index: usize) -> Result<(), EditError> {
        let h = self.get(index)?;
        let r = text::removal_range(self.text(), h.spans.line.start..h.spans.subtree_end);
        self.replace_range(r, "");
        Ok(())
    }

    /// Moves the heading's subtree to be the last child of `target`, or to the
    /// end of the file at level 1 when `target` is `None`, re-levelled.
    /// Returns its new index.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`], or [`EditError::TargetInsideSubtree`]
    /// when `target` is the heading or one of its descendants.
    pub fn refile(&mut self, index: usize, target: Option<usize>) -> Result<usize, EditError> {
        self.get(index)?;
        let descendants = self.descendants(index);
        if let Some(t) = target {
            self.get(t)?;
            if t == index || descendants.contains(&t) {
                return Err(EditError::TargetInsideSubtree);
            }
        }
        let mut subtree = self.subtree_text(index).to_string();
        if !subtree.ends_with('\n') {
            subtree.push_str(text::eol(self.text()));
        }
        self.delete_subtree(index)?;
        let count = descendants.len() + 1;
        let at = match target {
            Some(t) if t > index => InsertAt::LastChild(t - count),
            Some(t) => InsertAt::LastChild(t),
            None => InsertAt::LastTopLevel,
        };
        self.insert_subtree(at, &subtree)
    }

    /// Inserts `subtree` (text starting with a heading line, e.g. from
    /// [`Document::subtree_text`] of another file) at `at`, re-levelled so its
    /// root sits at that position's level. Returns the root's new index.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`] or [`EditError::NotASubtree`].
    pub fn insert_subtree(&mut self, at: InsertAt, subtree: &str) -> Result<usize, EditError> {
        let (offset, level) = self.resolve(at)?;
        let mut snippet = Document::parse(subtree, self.keywords().clone());
        let root = snippet.heading(0).ok_or(EditError::NotASubtree)?;
        if root.spans.line.start != 0 {
            return Err(EditError::NotASubtree);
        }
        let delta =
            isize::try_from(level).unwrap_or(isize::MAX) - isize::try_from(root.level).unwrap_or(0);
        // Every top-level heading of the snippet moves by the same amount.
        let roots: Vec<usize> = (0..snippet.headings().len())
            .filter(|&i| snippet.headings()[i].parent.is_none())
            .collect();
        for &r in roots.iter().rev() {
            snippet.shift_levels(r, delta);
        }
        let body = snippet.text();
        let lines: Vec<String> = body
            .strip_suffix('\n')
            .unwrap_or(body)
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        let start = self.insert_lines(offset, &lines);
        Ok(self.heading_index_at(start))
    }

    /// Inserts a new heading at `at`. Returns its index.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`], [`EditError::UnknownKeyword`] or
    /// [`EditError::MultilineText`] for a title with a line break.
    pub fn insert_heading(&mut self, at: InsertAt, new: &NewHeading) -> Result<usize, EditError> {
        if new.title.contains(['\n', '\r']) {
            return Err(EditError::MultilineText);
        }
        if let Some(k) = &new.keyword
            && !self.keywords().contains(k)
        {
            return Err(EditError::UnknownKeyword(k.clone()));
        }
        let (offset, level) = self.resolve(at)?;
        let mut header = "*".repeat(level);
        for part in [new.keyword.as_deref(), Some(new.title.as_str())]
            .into_iter()
            .flatten()
        {
            if !part.is_empty() {
                header.push(' ');
                header.push_str(part);
            }
        }
        if header.len() == level {
            // A blank heading keeps its space, ready for typing.
            header.push(' ');
        }
        let mut lines = vec![header];
        if new.id.is_some() || new.created.is_some() {
            lines.push(":PROPERTIES:".to_string());
            if let Some(id) = &new.id {
                lines.push(format!(":ID: {id}"));
            }
            if let Some(created) = new.created {
                lines.push(format!(":CREATED: {}", Timestamp::inactive_at(created)));
            }
            lines.push(":END:".to_string());
        }
        if let Some(body) = new.body.as_deref().filter(|b| !b.trim().is_empty()) {
            lines.extend(
                body.trim_end_matches(['\n', '\r'])
                    .split('\n')
                    .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()),
            );
        }
        if self.text().trim().is_empty() {
            // A blank file is replaced rather than kept as leading blank lines.
            let eol = text::eol(self.text());
            let all = lines.iter().fold(String::new(), |acc, l| acc + l + eol);
            let len = self.text().len();
            self.replace_range(0..len, &all);
            return Ok(0);
        }
        let start = self.insert_lines(offset, &lines);
        Ok(self.heading_index_at(start))
    }

    // --- helpers ---

    fn get(&self, index: usize) -> Result<Heading, EditError> {
        self.heading(index)
            .cloned()
            .ok_or(EditError::NoSuchHeading(index))
    }

    /// Byte offset and heading level for an insert position.
    fn resolve(&self, at: InsertAt) -> Result<(usize, usize), EditError> {
        Ok(match at {
            InsertAt::FirstChild(p) => {
                let h = self.get(p)?;
                (h.spans.section_end, h.level + 1)
            }
            InsertAt::LastChild(p) => {
                let h = self.get(p)?;
                (h.spans.subtree_end, h.level + 1)
            }
            InsertAt::Before(i) => {
                let h = self.get(i)?;
                (h.spans.line.start, h.level)
            }
            InsertAt::After(i) => {
                let h = self.get(i)?;
                (h.spans.subtree_end, h.level)
            }
            InsertAt::FirstTopLevel => (self.preamble().range.end, 1),
            InsertAt::LastTopLevel => (self.text().len(), 1),
        })
    }

    /// Inserts `lines` as whole lines at `at` (a line start or the end).
    /// Returns where the first inserted line starts.
    fn insert_lines(&mut self, at: usize, lines: &[String]) -> usize {
        let block = text::lines_block(self.text(), at, lines);
        let start = if block.starts_with(['\r', '\n']) {
            at + text::eol(self.text()).len()
        } else {
            at
        };
        self.replace_range(at..at, &block);
        start
    }

    /// The index of the last heading starting at or before `offset`.
    fn heading_index_at(&self, offset: usize) -> usize {
        self.headings()
            .iter()
            .rposition(|h| h.spans.line.start <= offset)
            .unwrap_or(0)
    }

    /// Inserts `word` right after the heading's stars and their space.
    fn insert_after_stars(&mut self, h: &Heading, word: &str) {
        let stars_end = h.spans.stars.end;
        let content_end = h.spans.line_content.end;
        let has_space = matches!(self.text().as_bytes().get(stars_end), Some(b' ' | b'\t'));
        if !has_space {
            self.replace_range(stars_end..stars_end, &format!(" {word}"));
        } else if stars_end + 1 >= content_end {
            self.replace_range(stars_end + 1..content_end, word);
        } else {
            self.replace_range(stars_end + 1..stars_end + 1, &format!("{word} "));
        }
    }

    /// The first offset at or after `from` (up to `limit`) that isn't a space or tab.
    fn skip_blanks(&self, from: usize, limit: usize) -> usize {
        let rest = &self.text()[from..limit];
        from + (rest.len() - rest.trim_start_matches([' ', '\t']).len())
    }

    /// The end of `start..end` with trailing whitespace removed.
    fn trimmed_end(&self, start: usize, end: usize) -> usize {
        start + self.text()[start..end].trim_end().len()
    }

    /// The first non-blank line of the heading's section, when it is a
    /// metadata line (planning or a drawer marker).
    fn first_metadata_line(&self, h: &Heading) -> Option<Line> {
        let line = text::lines_in(
            self.text(),
            h.spans.line.end.min(h.spans.section_end)..h.spans.section_end,
        )
        .into_iter()
        .find(|l| !l.content(self.text()).trim().is_empty())?;
        let content = line.content(self.text());
        let marker = content.trim();
        (is_planning_line(content)
            || marker.eq_ignore_ascii_case(":PROPERTIES:")
            || marker.eq_ignore_ascii_case(":LOGBOOK:"))
        .then_some(line)
    }

    /// Indentation for a new metadata line under `h`, matching the file
    /// (Bloom's `metadataIndent`): `h`'s own metadata first, else the first
    /// other heading's. An indent of level + 1 spaces (Emacs's
    /// `org-adapt-indentation`) adapts to `h`'s level; any other is copied.
    fn metadata_indent(&self, h: &Heading) -> String {
        if let Some(l) = self.first_metadata_line(h) {
            return text::indent(l.content(self.text())).to_string();
        }
        for other in self.headings().iter().filter(|o| o.index != h.index) {
            let Some(l) = self.first_metadata_line(other) else {
                continue;
            };
            let indent = text::indent(l.content(self.text()));
            return if !indent.is_empty()
                && indent.bytes().all(|b| b == b' ')
                && indent.len() == other.level + 1
            {
                " ".repeat(h.level + 1)
            } else {
                indent.to_string()
            };
        }
        String::new()
    }
}

/// Emacs's `org-log-note-headings` state note: `- State %-12s from %-12s %t`.
fn state_change_line(new: &str, old: &str, at: &Timestamp) -> String {
    let pad = |s: &str| format!("{:<12}", format!("\"{s}\""));
    format!("- State {} from {} {at}", pad(new), pad(old))
}

/// Bloom's tag charset: letters, marks and digits in any script, `_ @ # % -`.
fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '%' | '-'))
}

fn strip_eol(s: &str) -> &str {
    s.strip_suffix('\n')
        .map_or(s, |s| s.strip_suffix('\r').unwrap_or(s))
}

/// A plain list item line: `- x`, `+ x`, `1. x`, `1) x`, maybe with a checkbox.
struct ListItem {
    /// Leading whitespace length.
    indent: usize,
    /// Offset of the mark inside `[ ]` in the line, and the mark (`x` read as `X`).
    checkbox: Option<(usize, char)>,
    /// Where the item's text starts in the line.
    text_start: usize,
}

fn list_item(line: &str) -> Option<ListItem> {
    let indent = text::indent(line).len();
    let rest = &line[indent..];
    let bullet = if rest.starts_with(['-', '+']) {
        1
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) {
            digits + 1
        } else {
            return None;
        }
    };
    let after = &rest[bullet..];
    let ws = after.len() - after.trim_start_matches([' ', '\t']).len();
    if ws == 0 {
        return None;
    }
    let mut pos = indent + bullet + ws;
    let mut checkbox = None;
    let b = line.as_bytes();
    if b.get(pos) == Some(&b'[')
        && b.get(pos + 2) == Some(&b']')
        && let Some(&mark) = b.get(pos + 1)
        && matches!(mark, b' ' | b'X' | b'x' | b'-')
        && matches!(b.get(pos + 3), None | Some(b' ' | b'\t'))
    {
        let mark = if mark == b'x' { 'X' } else { char::from(mark) };
        checkbox = Some((pos + 1, mark));
        pos += 3;
        pos += line[pos..].len() - line[pos..].trim_start_matches([' ', '\t']).len();
    }
    Some(ListItem {
        indent,
        checkbox,
        text_start: pos,
    })
}

/// The first statistics cookie in `s`: `[n/m]` (preferred) or `[n%]`.
/// Returns its range and whether it is a percent cookie.
fn find_cookie(s: &str) -> Option<(Range<usize>, bool)> {
    let cookie = |percent: bool| {
        s.match_indices('[').find_map(|(i, _)| {
            let inner = &s[i + 1..];
            let close = inner.find(']')?;
            let body = &inner[..close];
            let ok = if percent {
                body.strip_suffix('%')
                    .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
            } else {
                body.split_once('/').is_some_and(|(a, b)| {
                    a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
                })
            };
            ok.then_some(i..i + close + 2)
        })
    };
    cookie(false)
        .map(|r| (r, false))
        .or_else(|| cookie(true).map(|r| (r, true)))
}
