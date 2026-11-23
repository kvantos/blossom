//! Ignore rules (Bloom's `IgnorePatterns`): one glob per line, `#` comments.
//! `*` matches any run within one path segment, `?` one character. A pattern
//! containing `/` matches the vault-relative path, otherwise the file or
//! folder name. `!` lines are ignored: a folder that is never walked into
//! can't have files re-included.

/// A set of ignore globs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IgnorePatterns {
    patterns: Vec<String>,
}

impl IgnorePatterns {
    /// Parses ignore-file text.
    #[must_use]
    pub fn parse(rules: &str) -> IgnorePatterns {
        let mut p = IgnorePatterns::default();
        p.extend(rules.lines());
        p
    }

    /// Adds more patterns, one per item, with the same rules as [`IgnorePatterns::parse`].
    pub fn extend<'a>(&mut self, lines: impl IntoIterator<Item = &'a str>) {
        self.patterns.extend(
            lines
                .into_iter()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
                .map(String::from),
        );
    }

    /// Whether the file or folder `name`, at vault-relative `path`, is ignored.
    #[must_use]
    pub fn is_ignored(&self, name: &str, path: &str) -> bool {
        self.patterns.iter().any(|p| {
            let subject = if p.contains('/') { path } else { name };
            glob_match(p.as_bytes(), subject.as_bytes())
        })
    }
}

/// Whole-string glob match; `*` and `?` never match `/`.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    // Iterative matcher with single-star backtracking.
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(b'?') if text[t] != b'/' => {
                p += 1;
                t += 1;
            }
            Some(&c) if c != b'?' && c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                // Let the last star swallow one more byte, but never a `/`.
                Some((sp, st)) if text[st] != b'/' => {
                    star = Some((sp, st + 1));
                    p = sp + 1;
                    t = st + 1;
                }
                _ => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match(b"*.org", b"a.org"));
        assert!(glob_match(b"a?c", b"abc"));
        assert!(!glob_match(b"a?c", b"a/c"));
        assert!(glob_match(b"arch*", b"archive"));
        assert!(!glob_match(b"*.org", b"dir/a.org"));
        assert!(glob_match(b"dir/*.org", b"dir/a.org"));
        assert!(!glob_match(b"dir/*", b"dir/sub/a.org"));
        assert!(glob_match(b"**", b""));
        assert!(glob_match(b"a*b*c", b"axxbyyc"));
        assert!(!glob_match(b"a*b*c", b"axxbyy"));
    }

    #[test]
    fn name_or_path() {
        let p = IgnorePatterns::parse("# comment\nscratch*\n!keep.org\nprojects/old\n\n");
        assert!(p.is_ignored("scratch.org", "notes/scratch.org"));
        assert!(p.is_ignored("old", "projects/old"));
        assert!(!p.is_ignored("old", "other/old"));
        assert!(!p.is_ignored("keep.org", "keep.org"));
    }
}
