//! What the app remembers between runs that isn't a setting: the file last
//! open in each notes folder, and how the agenda was last shown. Kept in `state.toml` in the platform data
//! folder; losing it only means starting on the inbox again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tree::FileInfo;

/// The remembered state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct State {
    /// Notes folder (as an absolute path) → the vault-relative file last open.
    pub last_file: BTreeMap<String, String>,
    /// The agenda's last scope: week, month, quarter or year.
    pub agenda_scope: crate::agenda::Scope,
    /// The agenda's timeline was closed.
    pub timeline_closed: bool,
    /// Notes folder → the files the agenda shows, when not all of them.
    pub agenda_files: BTreeMap<String, Vec<String>>,
}

/// `state.toml` in the platform data folder.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    blossom_vault::config::data_dir().map(|d| d.join("state.toml"))
}

impl State {
    /// The state at `path`; empty when missing or unreadable (it is only a
    /// convenience, so a broken file is not worth a message).
    #[must_use]
    pub fn load(path: &Path) -> State {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Writes the state to `path` (a temporary file, then a rename).
    ///
    /// # Errors
    /// When the folder or file can't be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let text = toml::to_string(self).map_err(|e| fail(&e))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| fail(&e))?;
        std::fs::rename(&tmp, path).map_err(|e| fail(&e))
    }

    /// The key for a notes folder.
    fn key(root: &Path) -> String {
        root.canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .display()
            .to_string()
    }

    /// The file last open in `root`, if remembered.
    #[must_use]
    pub fn last_file(&self, root: &Path) -> Option<&str> {
        self.last_file.get(&State::key(root)).map(String::as_str)
    }

    /// The files the agenda shows in `root`; `None` for all of them.
    #[must_use]
    pub fn agenda_files(&self, root: &Path) -> Option<&[String]> {
        self.agenda_files.get(&State::key(root)).map(Vec::as_slice)
    }

    /// Shows only `files` in `root`'s agenda (`None`: all of them).
    pub fn set_agenda_files(&mut self, root: &Path, files: Option<Vec<String>>) {
        let key = State::key(root);
        match files {
            Some(f) => {
                self.agenda_files.insert(key, f);
            }
            None => {
                self.agenda_files.remove(&key);
            }
        }
    }

    /// Remembers `file` as open in `root`; whether that changed anything.
    pub fn set_last_file(&mut self, root: &Path, file: &str) -> bool {
        let key = State::key(root);
        if self.last_file.get(&key).is_some_and(|f| f == file) {
            return false;
        }
        self.last_file.insert(key, file.to_string());
        true
    }
}

/// The file to open when a notes folder opens: the last one open there if
/// it still exists, else the inbox if it exists, else none.
#[must_use]
pub fn start_file(last: Option<&str>, inbox: &str, files: &[FileInfo]) -> Option<String> {
    let exists = |p: &str| files.iter().any(|f| f.path == p);
    last.filter(|p| exists(p))
        .or_else(|| Some(inbox).filter(|p| exists(p)))
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(path: &str) -> FileInfo {
        FileInfo {
            path: path.into(),
            headings: 0,
            open_tasks: 0,
            conflict: false,
        }
    }

    #[test]
    fn start_file_prefers_the_last_then_the_inbox() {
        let files = [info("inbox.org"), info("work/plan.org")];
        let pick = |last, inbox| start_file(last, inbox, &files);
        assert_eq!(
            pick(Some("work/plan.org"), "inbox.org").as_deref(),
            Some("work/plan.org")
        );
        assert_eq!(
            pick(Some("gone.org"), "inbox.org").as_deref(),
            Some("inbox.org")
        );
        assert_eq!(pick(None, "inbox.org").as_deref(), Some("inbox.org"));
        assert_eq!(pick(None, "in.org"), None);
    }

    #[test]
    fn save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/state.toml");
        assert_eq!(State::load(&path), State::default());
        let mut s = State::default();
        assert!(s.set_last_file(dir.path(), "a.org"));
        assert!(!s.set_last_file(dir.path(), "a.org"));
        s.save(&path).unwrap();
        let back = State::load(&path);
        assert_eq!(back.last_file(dir.path()), Some("a.org"));
        std::fs::write(&path, "not toml [").unwrap();
        assert_eq!(State::load(&path), State::default());
    }
}
