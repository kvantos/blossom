//! Shared helpers for the round-trip and edit tests.
// Test-only helpers: failing loudly is the point.
#![allow(dead_code, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use orgize::{Org, SyntaxNode};

/// One fixture file: its path (for messages) and its exact text.
pub struct Fixture {
    pub path: PathBuf,
    pub text: String,
}

/// All `.org` fixtures under `tests/fixtures`, sorted by path.
pub fn fixtures() -> Vec<Fixture> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths = Vec::new();
    collect(&root, &mut paths);
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no fixtures found under {}",
        root.display()
    );
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            let text = String::from_utf8(bytes)
                .unwrap_or_else(|e| panic!("{} is not UTF-8: {e}", path.display()));
            Fixture { path, text }
        })
        .collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "org") {
            out.push(path);
        }
    }
}

/// Byte offset of the first difference between `a` and `b`, or `None` if equal.
pub fn first_diff(a: &str, b: &str) -> Option<usize> {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a == b {
        return None;
    }
    Some(
        a.iter()
            .zip(b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len())),
    )
}

/// A short, escaped window of `s` around byte `at`, for failure messages.
pub fn context(s: &str, at: usize) -> String {
    let bytes = s.as_bytes();
    let start = at.saturating_sub(20);
    let end = (at + 20).min(bytes.len());
    format!("{:?}", String::from_utf8_lossy(&bytes[start..end]))
}

/// Explains how `got` differs from `want`, or `None` if they are equal.
pub fn describe_diff(want: &str, got: &str) -> Option<String> {
    let at = first_diff(want, got)?;
    Some(format!(
        "first difference at byte {at} (want len {}, got len {})\n  want: {}\n  got:  {}",
        want.len(),
        got.len(),
        context(want, at),
        context(got, at),
    ))
}

/// The full syntax tree of `org`, as text, for comparing two parses.
pub fn tree(org: &Org) -> String {
    format!("{:#?}", SyntaxNode::new_root(org.green().clone()))
}
