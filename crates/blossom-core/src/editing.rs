//! Helpers for a text editor on top of a [`Document`]: which text range a
//! section editor shows, list continuation on Enter, and the three-way merge
//! that lets an autosave survive another program's edit elsewhere in the file.

use std::ops::Range;

use crate::document::Document;

impl Document {
    /// The text a section editor shows for heading `index`: its heading
    /// line, metadata and body, up to the next heading of any level.
    #[must_use]
    pub fn section_range(&self, index: usize) -> Option<Range<usize>> {
        self.heading(index)
            .map(|h| h.spans.line.start..h.spans.section_end)
    }

    /// Everything before the first heading: `#+` keywords, the file property
    /// drawer and the intro. Empty for a file that starts with a heading.
    #[must_use]
    pub fn top_range(&self) -> Range<usize> {
        0..self
            .headings()
            .first()
            .map_or(self.text().len(), |h| h.spans.line.start)
    }

    /// The heading whose section holds byte `offset`; `None` before the
    /// first heading.
    #[must_use]
    pub fn heading_at(&self, offset: usize) -> Option<usize> {
        self.headings()
            .iter()
            .rposition(|h| h.spans.line.start <= offset)
    }
}

/// The parts of a list item line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    /// Leading whitespace.
    pub indent: Range<usize>,
    /// `-`, `+`, `*` (indented only), `1.` or `1)`.
    pub bullet: Range<usize>,
    /// `[ ]`, `[X]` or `[-]`, when present.
    pub checkbox: Option<Range<usize>>,
    /// Where the item's text starts.
    pub content: usize,
}

/// Parses `line` as a list item, the way org does: a bullet followed by a
/// space (or the end of the line). `*` needs indentation, as a `*` in
/// column 0 starts a heading.
#[must_use]
pub fn list_item(line: &str) -> Option<ListItem> {
    let line = line.trim_end_matches(['\n', '\r']);
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let bullet_len = match rest.as_bytes().first()? {
        b'-' | b'+' => 1,
        b'*' if indent > 0 => 1,
        b'0'..=b'9' => {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            match rest.as_bytes().get(digits) {
                Some(b'.' | b')') => digits + 1,
                _ => return None,
            }
        }
        _ => return None,
    };
    let after = &rest[bullet_len..];
    if !(after.is_empty() || after.starts_with(' ') || after.starts_with('\t')) {
        return None;
    }
    let bullet = indent..indent + bullet_len;
    let mut content = (bullet.end + 1).min(line.len());
    let mut checkbox = None;
    let tail = &line[content..];
    if tail.len() >= 3
        && tail.starts_with('[')
        && matches!(tail.as_bytes()[1], b' ' | b'X' | b'x' | b'-')
        && tail.as_bytes()[2] == b']'
        && (tail.len() == 3 || tail[3..].starts_with(' '))
    {
        checkbox = Some(content..content + 3);
        content = (content + 4).min(line.len());
    }
    Some(ListItem {
        indent: 0..indent,
        bullet,
        checkbox,
        content,
    })
}

/// What Enter on a list item line should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListContinue {
    /// Start the next item: insert this prefix after the line break.
    Item(String),
    /// The item is empty: remove its bullet to end the list.
    End,
}

/// List continuation for Enter at the end of `line`: the next item gets the
/// same indent and bullet (numbers count up) and an empty checkbox when the
/// line has one; Enter on an empty item ends the list.
#[must_use]
pub fn continue_list(line: &str) -> Option<ListContinue> {
    let line = line.trim_end_matches(['\n', '\r']);
    let item = list_item(line)?;
    if line[item.content..].trim().is_empty() {
        return Some(ListContinue::End);
    }
    let bullet = &line[item.bullet.clone()];
    let next = match bullet.strip_suffix(['.', ')']) {
        Some(n) => {
            let close = &bullet[bullet.len() - 1..];
            let n: u64 = n.parse().ok()?;
            format!("{}{close}", n + 1)
        }
        None => bullet.to_string(),
    };
    let mut prefix = format!("{}{next} ", &line[item.indent]);
    if item.checkbox.is_some() {
        prefix.push_str("[ ] ");
    }
    Some(ListContinue::Item(prefix))
}

/// A successful [`merge3`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    /// The merged text.
    pub text: String,
    /// Offsets in `ours` at or after this moved by `delta` in `text`.
    pub shift_at: usize,
    /// How far they moved.
    pub delta: isize,
}

impl Merge {
    /// Where byte `offset` of `ours` ended up in the merged text.
    #[must_use]
    pub fn map(&self, offset: usize) -> usize {
        if offset >= self.shift_at {
            offset.saturating_add_signed(self.delta)
        } else {
            offset
        }
    }
}

/// Merges two edits of `base`: `ours` and `theirs`. Each side's change is
/// taken as one span of whole lines (from the first to the last changed
/// line); when the spans don't share a line, both changes are kept.
/// Otherwise `None`: the edits conflict.
#[must_use]
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Option<Merge> {
    let unchanged = |text: &str| Merge {
        text: text.to_string(),
        shift_at: usize::MAX,
        delta: 0,
    };
    if ours == theirs || theirs == base {
        return Some(unchanged(ours));
    }
    let d_theirs = len_delta(base, theirs);
    if ours == base {
        let (start, _) = changed_lines(base, theirs);
        return Some(Merge {
            text: theirs.to_string(),
            shift_at: start,
            delta: d_theirs,
        });
    }
    let (o_start, o_end) = changed_lines(base, ours);
    let (t_start, t_end) = changed_lines(base, theirs);
    let d_ours = len_delta(base, ours);
    if o_end <= t_start {
        // Ours first: our text up to the end of our change, then theirs.
        let cut = o_end.saturating_add_signed(d_ours);
        Some(Merge {
            text: format!("{}{}", &ours[..cut], &theirs[o_end..]),
            shift_at: cut,
            delta: d_theirs,
        })
    } else if t_end <= o_start {
        let cut = t_end.saturating_add_signed(d_theirs);
        Some(Merge {
            text: format!("{}{}", &theirs[..cut], &ours[t_end..]),
            shift_at: t_end,
            delta: d_theirs,
        })
    } else {
        None
    }
}

fn len_delta(base: &str, other: &str) -> isize {
    // Text lengths are far below isize::MAX.
    #[allow(clippy::cast_possible_wrap)]
    let d = other.len() as isize - base.len() as isize;
    d
}

/// The changed span of `base` → `other` in `base` offsets, widened to whole
/// lines: from the start of the first changed line to the end (after the
/// line break) of the last.
fn changed_lines(base: &str, other: &str) -> (usize, usize) {
    let (a, b) = (base.as_bytes(), other.as_bytes());
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let start = base[..floor(base, prefix)].rfind('\n').map_or(0, |i| i + 1);
    let end_at = floor(base, a.len() - suffix);
    let end = if start == end_at && end_at > 0 && a[end_at - 1] == b'\n' {
        // A pure insertion at a line start touches only that point.
        end_at
    } else {
        base[end_at..]
            .find('\n')
            .map_or(base.len(), |i| end_at + i + 1)
    };
    (start, end.max(start))
}

fn floor(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Keywords;

    #[test]
    fn ranges_for_section_editing() {
        let doc = Document::parse(
            "#+TITLE: x\nintro\n* a\nbody\n** b\n* c",
            Keywords::default(),
        );
        assert_eq!(&doc.text()[doc.top_range()], "#+TITLE: x\nintro\n");
        assert_eq!(&doc.text()[doc.section_range(0).unwrap()], "* a\nbody\n");
        assert_eq!(&doc.text()[doc.section_range(2).unwrap()], "* c");
        assert_eq!(doc.heading_at(3), None);
        assert_eq!(doc.heading_at(17), Some(0));
        assert_eq!(doc.heading_at(28), Some(1));
        assert_eq!(doc.heading_at(doc.text().len()), Some(2));
    }

    #[test]
    fn list_items() {
        let item = list_item("  - [X] done").unwrap();
        assert_eq!(
            (item.bullet, item.checkbox, item.content),
            (2..3, Some(4..7), 8)
        );
        assert_eq!(list_item("12) x").unwrap().bullet, 0..3);
        assert_eq!(list_item("-").unwrap().content, 1);
        for not in ["* heading", "-x", "1x", "text", "", "1.5 apples"] {
            assert_eq!(list_item(not), None, "{not}");
        }
        assert!(list_item("  * star bullet").is_some());
    }

    #[test]
    fn enter_continues_lists() {
        let go = |l: &str| continue_list(l);
        assert_eq!(go("- milk"), Some(ListContinue::Item("- ".into())));
        assert_eq!(
            go("  + [X] eggs\r\n"),
            Some(ListContinue::Item("  + [ ] ".into()))
        );
        assert_eq!(go("9. nine"), Some(ListContinue::Item("10. ".into())));
        assert_eq!(go("3) c"), Some(ListContinue::Item("4) ".into())));
        assert_eq!(go("- "), Some(ListContinue::End));
        assert_eq!(go("  - [ ]"), Some(ListContinue::End));
        assert_eq!(go("plain"), None);
    }

    #[test]
    fn merges_edits_on_different_lines() {
        let base = "a\nb\nc\nd\n";
        // Ours edits line b, theirs line d.
        let m = merge3(base, "a\nB!\nc\nd\n", "a\nb\nc\nD\n").unwrap();
        assert_eq!(m.text, "a\nB!\nc\nD\n");
        // Theirs first: offsets in ours after their change move.
        let m = merge3(base, "a\nb\nc\nD\n", "A long\nb\nc\nd\n").unwrap();
        assert_eq!(m.text, "A long\nb\nc\nD\n");
        assert_eq!(m.map(6), 11);
        assert_eq!(m.map(0), 0);
        // Ours first: offsets after our change move by their delta.
        let m = merge3(base, "x\nb\nc\nd\n", "a\nb\nc\nd\ne\n").unwrap();
        assert_eq!((m.text.as_str(), m.map(1)), ("x\nb\nc\nd\ne\n", 1));
        // Insertions at different line starts.
        let m = merge3(base, "a\nnew\nb\nc\nd\n", "a\nb\nc\nalso\nd\n").unwrap();
        assert_eq!(m.text, "a\nnew\nb\nc\nalso\nd\n");
        // Same line: conflict.
        assert_eq!(merge3(base, "a\nb1\nc\nd\n", "a\nb2\nc\nd\n"), None);
        // Changes on neighbouring lines merge.
        assert!(merge3(base, "A\nb\nc\nd\n", "a\nB\nc\nd\n").is_some());
        // Identical changes and one-sided changes.
        assert_eq!(merge3(base, "z", "z").unwrap().text, "z");
        assert_eq!(merge3(base, base, "q\n").unwrap().text, "q\n");
        assert_eq!(merge3(base, "q\n", base).unwrap().text, "q\n");
    }

    #[test]
    fn merge_handles_unicode_and_missing_final_newline() {
        let base = "é1\né2";
        let m = merge3(base, "é1!\né2", "é1\né2 end").unwrap();
        assert_eq!(m.text, "é1!\né2 end");
        assert_eq!(merge3("ab", "ax", "ay"), None);
    }
}
