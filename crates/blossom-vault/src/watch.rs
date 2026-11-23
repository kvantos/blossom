//! A debounced file watcher: bursts of filesystem events become one batch of
//! changed vault-relative paths.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

/// Watches a vault folder. Changes arrive on [`VaultWatcher::changes`] as
/// sorted, de-duplicated vault-relative paths, once no new event has come for
/// the debounce interval. Dropping the watcher stops it.
///
/// Treat a batch as a hint to re-list the vault and compare revisions, not as
/// a complete list: a path may be a folder, and a file written into a folder
/// the moment it is created may be reported only as that folder.
pub struct VaultWatcher {
    _watcher: RecommendedWatcher,
    changes: Receiver<Vec<String>>,
}

impl VaultWatcher {
    /// Starts watching `root` recursively.
    ///
    /// # Errors
    ///
    /// When the platform watcher can't be created or can't watch `root`.
    pub fn start(root: &Path, debounce: Duration) -> notify::Result<VaultWatcher> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let (raw_tx, raw_rx) = mpsc::channel::<Vec<PathBuf>>();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    // The receiver only goes away when the watcher is dropped.
                    let _ = raw_tx.send(event.paths);
                }
            })?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        let (tx, changes) = mpsc::channel();
        thread::spawn(move || debounce_loop(&root, &raw_rx, &tx, debounce));
        Ok(VaultWatcher {
            _watcher: watcher,
            changes,
        })
    }

    /// The batches of changed paths.
    #[must_use]
    pub fn changes(&self) -> &Receiver<Vec<String>> {
        &self.changes
    }
}

fn debounce_loop(
    root: &Path,
    raw: &Receiver<Vec<PathBuf>>,
    out: &mpsc::Sender<Vec<String>>,
    debounce: Duration,
) {
    // Block for the first event of a burst, then gather until it goes quiet.
    while let Ok(first) = raw.recv() {
        let mut batch = BTreeSet::new();
        batch.extend(first.iter().filter_map(|p| relevant(root, p)));
        loop {
            match raw.recv_timeout(debounce) {
                Ok(paths) => batch.extend(paths.iter().filter_map(|p| relevant(root, p))),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if !batch.is_empty() && out.send(batch.into_iter().collect()).is_err() {
            return;
        }
    }
}

/// The vault-relative path of `path`, unless it is outside the vault or under
/// a dot-folder or dot-file (Syncthing's `.stversions`, our own temp files).
fn relevant(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rel
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    if parts.is_empty() || parts.iter().any(|p| p.starts_with('.')) {
        return None;
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevant_paths() {
        let root = Path::new("/v");
        assert_eq!(
            relevant(root, Path::new("/v/a/b.org")).as_deref(),
            Some("a/b.org")
        );
        assert_eq!(relevant(root, Path::new("/v/.stversions/b.org")), None);
        assert_eq!(relevant(root, Path::new("/v/a/.b.org.tmp")), None);
        assert_eq!(relevant(root, Path::new("/other/b.org")), None);
        assert_eq!(relevant(root, Path::new("/v")), None);
    }
}
