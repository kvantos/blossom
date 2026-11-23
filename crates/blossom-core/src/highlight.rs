//! Syntax classes for an org editor, one line at a time.
//!
//! [`highlight_line`] classifies the bytes of one line. The only thing
//! carried from line to line is a [`LineState`] (inside a source block or a
//! drawer), so an editor can re-highlight from any changed line onward. This
//! is a light scanner for colouring, not a parser: [`crate::Document`] stays
//! the authority on what the text means.

use std::ops::Range;

use crate::keywords::Keywords;
use crate::timestamp::Timestamp;

/// What a run of text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A heading line's stars and title, with its level (1 for `*`).
    Heading(usize),
    /// A not-done TODO keyword.
    Todo,
    /// A done-type keyword.
    Done,
    /// A priority cookie `[#A]`.
    Priority,
    /// Heading tags `:a:b:`.
    Tags,
    /// `SCHEDULED:`, `DEADLINE:`, `CLOSED:`.
    Planning,
    /// A timestamp.
    Timestamp,
    /// A drawer line such as `:PROPERTIES:` or `:END:`.
    Drawer,
    /// A property key `:KEY:` inside a drawer.
    Property,
    /// `#+KEY:` keywords and block delimiters.
    Meta,
    /// A `# comment` line.
    Comment,
    /// A list bullet.
    Bullet,
    /// A checkbox `[ ]`, `[X]`, `[-]`.
    Checkbox,
    /// `*bold*`.
    Bold,
    /// `/italic/`.
    Italic,
    /// `=verbatim=` or `~code~`.
    Code,
    /// `+strike+`.
    Strike,
    /// `_underline_`.
    Underline,
    /// `[[target]]` or `[[target][label]]`.
    Link,
    /// A line inside a source, example or export block.
    Block,
}

/// What carries over from one line to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineState {
    /// Inside `#+BEGIN_SRC`/`EXAMPLE`/`EXPORT` … `#+END_…`.
    pub in_block: bool,
    /// Inside a `:NAME:` … `:END:` drawer.
    pub in_drawer: bool,
}

/// Classifies `line` (with or without its line ending) and updates `state`
/// for the next line. Runs are in order, don't overlap and lie on char
/// boundaries; unclassified text is left out.
#[must_use]
pub fn highlight_line(
    line: &str,
    state: &mut LineState,
    keywords: &Keywords,
) -> Vec<(Range<usize>, Kind)> {
    let line = line.trim_end_matches(['\n', '\r']);
    let mut paint = Paint::new(line.len());
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let lower = trimmed.to_ascii_lowercase();

    if state.in_block {
        if lower.starts_with("#+end_") {
            state.in_block = false;
            paint.set(0..line.len(), Kind::Meta);
        } else {
            paint.set(0..line.len(), Kind::Block);
        }
        return paint.runs(line);
    }

    if let Some(level) = heading_level(line) {
        // A heading ends any drawer left open above it.
        state.in_drawer = false;
        heading(line, level, keywords, &mut paint);
        return paint.runs(line);
    }

    if let Some(kind) = lower.strip_prefix("#+begin_") {
        state.in_block = ["src", "example", "export"]
            .iter()
            .any(|k| kind.starts_with(k));
        paint.set(0..line.len(), Kind::Meta);
        return paint.runs(line);
    }
    if lower.starts_with("#+end_") {
        paint.set(0..line.len(), Kind::Meta);
        return paint.runs(line);
    }
    if trimmed.starts_with("#+") {
        let end = trimmed.find(':').map_or(line.len(), |i| indent + i + 1);
        paint.set(indent..end, Kind::Meta);
        inline(line, end, &mut paint);
        return paint.runs(line);
    }
    if trimmed == "#" || trimmed.starts_with("# ") {
        paint.set(0..line.len(), Kind::Comment);
        return paint.runs(line);
    }
    if let Some(name) = drawer_name(trimmed) {
        state.in_drawer = !name.eq_ignore_ascii_case("END");
        paint.set(indent..line.len(), Kind::Drawer);
        return paint.runs(line);
    }
    if state.in_drawer {
        if let Some(end) = property_key_end(trimmed) {
            paint.set(indent..indent + end, Kind::Property);
        }
        stamps(line, &mut paint);
        return paint.runs(line);
    }

    let mut from = 0;
    if let Some((bullet, checkbox)) = list_prefix(line) {
        paint.set(bullet.clone(), Kind::Bullet);
        from = bullet.end;
        if let Some(c) = checkbox {
            from = c.end;
            paint.set(c, Kind::Checkbox);
        }
    }
    planning_words(line, &mut paint);
    inline(line, from, &mut paint);
    paint.runs(line)
}

/// The heading level when `line` is a heading line (`* `, `** ` …).
fn heading_level(line: &str) -> Option<usize> {
    let stars = line.bytes().take_while(|&b| b == b'*').count();
    let rest = &line[stars..];
    (stars > 0 && (rest.starts_with(' ') || rest.starts_with('\t'))).then_some(stars)
}

fn heading(line: &str, level: usize, keywords: &Keywords, paint: &mut Paint) {
    paint.set(0..line.len(), Kind::Heading(level));
    let mut pos = level;
    let mut rest_start = skip_space(line, pos);
    // The keyword: the first word, when configured.
    let word_end = line[rest_start..]
        .find([' ', '\t'])
        .map_or(line.len(), |i| rest_start + i);
    let word = &line[rest_start..word_end];
    if keywords.contains(word) {
        let kind = if keywords.is_done(word) {
            Kind::Done
        } else {
            Kind::Todo
        };
        paint.set(rest_start..word_end, kind);
        pos = word_end;
        rest_start = skip_space(line, pos);
    }
    // The priority cookie.
    let rest = &line[rest_start..];
    if rest.starts_with("[#")
        && let Some(close) = rest.find(']')
        && close > 2
    {
        paint.set(rest_start..rest_start + close + 1, Kind::Priority);
        pos = rest_start + close + 1;
    }
    // Trailing tags.
    let mut title_end = line.len();
    if let Some(tags) = tags_range(line) {
        title_end = tags.start;
        paint.set(tags, Kind::Tags);
    }
    if pos < title_end {
        inline(&line[..title_end], pos, paint);
    }
}

fn skip_space(line: &str, from: usize) -> usize {
    line[from..]
        .find(|c: char| c != ' ' && c != '\t')
        .map_or(line.len(), |i| from + i)
}

/// `:a:b:` at the end of a heading line, after whitespace.
fn tags_range(line: &str) -> Option<Range<usize>> {
    let end = line.trim_end().len();
    let body = &line[..end];
    if !body.ends_with(':') {
        return None;
    }
    let start = body.rfind([' ', '\t'])? + 1;
    let tags = &body[start..];
    let ok = tags.len() > 2
        && tags.starts_with(':')
        && tags[1..tags.len() - 1]
            .split(':')
            .all(|t| !t.is_empty() && t.chars().all(|c| c.is_alphanumeric() || "_@#%".contains(c)));
    ok.then_some(start..end)
}

/// `NAME` for a drawer line `:NAME:`.
fn drawer_name(trimmed: &str) -> Option<&str> {
    let t = trimmed.trim_end();
    let name = t.strip_prefix(':')?.strip_suffix(':')?;
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-'))
    .then_some(name)
}

/// The end of `:KEY:` at the start of a property line.
fn property_key_end(trimmed: &str) -> Option<usize> {
    let rest = trimmed.strip_prefix(':')?;
    let close = rest.find(':')?;
    let key = &rest[..close];
    (!key.is_empty() && !key.contains(char::is_whitespace)).then_some(close + 2)
}

/// The bullet (and checkbox) of a list item line.
fn list_prefix(line: &str) -> Option<(Range<usize>, Option<Range<usize>>)> {
    let item = crate::editing::list_item(line)?;
    Some((item.bullet, item.checkbox))
}

fn planning_words(line: &str, paint: &mut Paint) {
    for word in ["SCHEDULED:", "DEADLINE:", "CLOSED:"] {
        let mut from = 0;
        while let Some(i) = line[from..].find(word) {
            let at = from + i;
            let before_ok = at == 0 || line[..at].ends_with([' ', '\t']);
            if before_ok {
                paint.set(at..at + word.len(), Kind::Planning);
            }
            from = at + word.len();
        }
    }
}

fn stamps(line: &str, paint: &mut Paint) {
    for (range, _) in Timestamp::scan(line) {
        paint.set(range, Kind::Timestamp);
    }
}

/// Emphasis, links and timestamps in `line[from..]`.
fn inline(line: &str, from: usize, paint: &mut Paint) {
    emphasis(line, from, paint);
    stamps_from(line, from, paint);
    links(line, from, paint);
}

fn stamps_from(line: &str, from: usize, paint: &mut Paint) {
    for (range, _) in Timestamp::scan(&line[from..]) {
        paint.set(from + range.start..from + range.end, Kind::Timestamp);
    }
}

fn links(line: &str, from: usize, paint: &mut Paint) {
    let mut at = from;
    while let Some(i) = line[at..].find("[[") {
        let start = at + i;
        let Some(j) = line[start..].find("]]") else {
            break;
        };
        let end = start + j + 2;
        paint.set(start..end, Kind::Link);
        at = end;
    }
}

/// Org emphasis: a marker after start/space/opening punctuation, content not
/// starting or ending with space, a closing marker before end/space/punctuation.
fn emphasis(line: &str, from: usize, paint: &mut Paint) {
    let chars: Vec<(usize, char)> = line[from..]
        .char_indices()
        .map(|(i, c)| (from + i, c))
        .collect();
    let kind_of = |c: char| match c {
        '*' => Some(Kind::Bold),
        '/' => Some(Kind::Italic),
        '=' | '~' => Some(Kind::Code),
        '+' => Some(Kind::Strike),
        '_' => Some(Kind::Underline),
        _ => None,
    };
    let pre_ok = |k: usize| {
        k == 0 || {
            let p = chars[k - 1].1;
            p.is_whitespace() || "-('\"{".contains(p)
        }
    };
    let post_ok = |k: usize| {
        k + 1 >= chars.len() || {
            let n = chars[k + 1].1;
            n.is_whitespace() || "-.,;:!?')}\"[".contains(n)
        }
    };
    let mut k = 0;
    while k < chars.len() {
        let (start, c) = chars[k];
        let Some(kind) = kind_of(c) else {
            k += 1;
            continue;
        };
        let opens = pre_ok(k) && chars.get(k + 1).is_some_and(|(_, n)| !n.is_whitespace());
        if !opens {
            k += 1;
            continue;
        }
        let close = (k + 2..chars.len())
            .find(|&m| chars[m].1 == c && !chars[m - 1].1.is_whitespace() && post_ok(m));
        match close {
            Some(m) => {
                paint.set(start..chars[m].0 + c.len_utf8(), kind);
                k = m + 1;
            }
            None => k += 1,
        }
    }
}

/// A byte-wise colouring; later paints win.
struct Paint(Vec<Option<Kind>>);

impl Paint {
    fn new(len: usize) -> Paint {
        Paint(vec![None; len])
    }

    fn set(&mut self, range: Range<usize>, kind: Kind) {
        let end = range.end.min(self.0.len());
        for slot in &mut self.0[range.start.min(end)..end] {
            *slot = Some(kind);
        }
    }

    /// Runs of equal kind, split only at char boundaries.
    fn runs(&self, line: &str) -> Vec<(Range<usize>, Kind)> {
        let mut out: Vec<(Range<usize>, Kind)> = Vec::new();
        for (i, c) in line.char_indices() {
            let Some(kind) = self.0[i] else { continue };
            let end = i + c.len_utf8();
            match out.last_mut() {
                Some((r, k)) if *k == kind && r.end == i => r.end = end,
                _ => out.push((i..end, kind)),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(lines: &str) -> Vec<Vec<(String, Kind)>> {
        let keywords = Keywords::default();
        let mut state = LineState::default();
        lines
            .lines()
            .map(|l| {
                highlight_line(l, &mut state, &keywords)
                    .into_iter()
                    .map(|(r, k)| (l[r].to_string(), k))
                    .collect()
            })
            .collect()
    }

    fn s(t: &str, k: Kind) -> (String, Kind) {
        (t.to_string(), k)
    }

    #[test]
    fn heading_parts() {
        let r = runs("** TODO [#A] Call *mum* about it   :home:fam:");
        assert_eq!(
            r[0],
            [
                s("** ", Kind::Heading(2)),
                s("TODO", Kind::Todo),
                s(" ", Kind::Heading(2)),
                s("[#A]", Kind::Priority),
                s(" Call ", Kind::Heading(2)),
                s("*mum*", Kind::Bold),
                s(" about it   ", Kind::Heading(2)),
                s(":home:fam:", Kind::Tags),
            ]
        );
        let r = runs("* DONE x\n*bold* not a heading\n*");
        assert_eq!(r[0][1], s("DONE", Kind::Done));
        assert_eq!(r[1], [s("*bold*", Kind::Bold)]);
        assert_eq!(r[2], []);
    }

    #[test]
    fn metadata_drawers_and_blocks() {
        let r = runs(concat!(
            "SCHEDULED: <2026-11-23 Mon>\n",
            ":PROPERTIES:\n",
            ":ID: abc\n",
            ":END:\n",
            "#+TITLE: Notes\n",
            "#+begin_src rust\n",
            "let x = *y*;\n",
            "#+end_src\n",
            "# a comment\n",
            "  - [X] done =code= and [[https://x.org][link]]\n",
            "a/b/c and 2*3*4 stay plain\n",
        ));
        assert_eq!(
            r[0],
            [
                s("SCHEDULED:", Kind::Planning),
                s("<2026-11-23 Mon>", Kind::Timestamp)
            ]
        );
        assert_eq!(r[1], [s(":PROPERTIES:", Kind::Drawer)]);
        assert_eq!(r[2], [s(":ID:", Kind::Property)]);
        assert_eq!(r[3], [s(":END:", Kind::Drawer)]);
        assert_eq!(r[4], [s("#+TITLE:", Kind::Meta)]);
        assert_eq!(r[5], [s("#+begin_src rust", Kind::Meta)]);
        assert_eq!(r[6], [s("let x = *y*;", Kind::Block)]);
        assert_eq!(r[7], [s("#+end_src", Kind::Meta)]);
        assert_eq!(r[8], [s("# a comment", Kind::Comment)]);
        assert_eq!(
            r[9],
            [
                s("-", Kind::Bullet),
                s("[X]", Kind::Checkbox),
                s("=code=", Kind::Code),
                s("[[https://x.org][link]]", Kind::Link)
            ]
        );
        assert_eq!(r[10], []);
    }

    #[test]
    fn unicode_and_crlf_stay_on_boundaries() {
        let keywords = Keywords::default();
        let mut state = LineState::default();
        let line = "* Привет /мир/ 🌸 :тег:\r\n";
        for (r, _) in highlight_line(line, &mut state, &keywords) {
            assert!(line.is_char_boundary(r.start) && line.is_char_boundary(r.end));
            assert!(!line[r].contains('\r'));
        }
    }
}
