//! Line-level helpers over byte offsets. A line's content excludes its
//! line ending (`\n` or `\r\n`); its `end` includes it.

use std::ops::Range;

/// One line of a text, as byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    /// First byte of the line.
    pub start: usize,
    /// End of the content, before any `\r\n` or `\n`.
    pub content_end: usize,
    /// End of the line including its line ending (or the text's end).
    pub end: usize,
}

impl Line {
    /// The whole line, line ending included.
    pub fn range(self) -> Range<usize> {
        self.start..self.end
    }

    /// The line's text without its line ending.
    pub fn content(self, text: &str) -> &str {
        &text[self.start..self.content_end]
    }
}

/// The line starting at byte `start` (which must be a line start).
pub(crate) fn line_from(text: &str, start: usize) -> Line {
    match text[start..].find('\n') {
        Some(i) => {
            let nl = start + i;
            let content_end = if nl > start && text.as_bytes()[nl - 1] == b'\r' {
                nl - 1
            } else {
                nl
            };
            Line {
                start,
                content_end,
                end: nl + 1,
            }
        }
        None => Line {
            start,
            content_end: text.len(),
            end: text.len(),
        },
    }
}

/// The line containing byte `offset`.
pub(crate) fn line_at(text: &str, offset: usize) -> Line {
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    line_from(text, start)
}

/// Every line that starts inside `range` (which must start at a line start).
pub(crate) fn lines_in(text: &str, range: Range<usize>) -> Vec<Line> {
    let mut out = Vec::new();
    let mut pos = range.start;
    while pos < range.end {
        let line = line_from(text, pos);
        out.push(line);
        pos = line.end;
    }
    out
}

/// The line ending new lines should use: whatever the text's first line
/// ending is, `\n` when it has none.
pub(crate) fn eol(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// The leading spaces and tabs of `s`.
pub(crate) fn indent(s: &str) -> &str {
    let len = s.len() - s.trim_start_matches([' ', '\t']).len();
    &s[..len]
}

/// The range to delete to remove `line` without changing whether the text
/// ends with a line ending: for a last line without one, the previous line's
/// ending goes instead.
pub(crate) fn removal_range(text: &str, line: Range<usize>) -> Range<usize> {
    if line.end == text.len() && !text.ends_with('\n') && line.start > 0 {
        let prev_eol = if text[..line.start].ends_with("\r\n") {
            2
        } else {
            1
        };
        (line.start - prev_eol)..line.end
    } else {
        line
    }
}

/// Text to insert at byte `at` (a line start, or the text's end) so that
/// `lines` become whole lines there. At the end of a text without a final
/// line ending, the ending goes before the new lines instead of after, so
/// the text still ends the same way.
pub(crate) fn lines_block(text: &str, at: usize, lines: &[String]) -> String {
    let eol = eol(text);
    if at == text.len() && !text.is_empty() && !text.ends_with('\n') {
        let mut s = String::new();
        for line in lines {
            s.push_str(eol);
            s.push_str(line);
        }
        s
    } else {
        lines.iter().fold(String::new(), |acc, l| acc + l + eol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_with_mixed_endings() {
        let text = "a\r\nbb\n\nc";
        let lines = lines_in(text, 0..text.len());
        let contents: Vec<&str> = lines.iter().map(|l| l.content(text)).collect();
        assert_eq!(contents, ["a", "bb", "", "c"]);
        assert_eq!(lines[3].end, lines[3].content_end);
        assert_eq!(lines[0].end, 3);
        assert_eq!(line_at(text, 4), lines[1]);
        assert_eq!(line_at(text, 0), lines[0]);
    }

    #[test]
    fn eol_follows_first_line() {
        assert_eq!(eol("a\r\nb\n"), "\r\n");
        assert_eq!(eol("a\nb\r\n"), "\n");
        assert_eq!(eol("a"), "\n");
    }

    #[test]
    fn removal_keeps_final_newline_state() {
        let text = "a\nb";
        assert_eq!(removal_range(text, 2..3), 1..3);
        let text = "a\nb\n";
        assert_eq!(removal_range(text, 2..4), 2..4);
        let text = "a\r\nb";
        assert_eq!(removal_range(text, 3..4), 1..4);
    }

    #[test]
    fn block_at_end_without_newline() {
        let lines = vec!["x".to_string(), "y".to_string()];
        assert_eq!(lines_block("a", 1, &lines), "\nx\ny");
        assert_eq!(lines_block("a\n", 2, &lines), "x\ny\n");
        assert_eq!(lines_block("a\r\n", 0, &lines), "x\r\ny\r\n");
        assert_eq!(lines_block("", 0, &lines), "x\ny\n");
    }
}
