//! Folder access for Blossom.
//!
//! A [`Vault`] is a folder of `.org` files: the only real state. It lists them
//! (skipping dot-folders such as Syncthing's `.stversions`, ignored paths and
//! conflict copies), identifies each version by a [`Revision`] (`mtime:size`),
//! caches parsed documents per revision, and writes atomically with a
//! stale-write guard, so a change made by another program (Emacs, Syncthing,
//! git) is never silently overwritten. Paths are vault-relative with `/`.

pub mod attach;
pub mod config;
mod conflict;
mod ignore;
pub mod ops;
mod watch;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use blossom_core::{Document, Keywords};

pub use conflict::conflict_base;
pub use ignore::IgnorePatterns;
pub use watch::VaultWatcher;

/// Ignore files read from the vault root, in this order.
pub const IGNORE_FILES: [&str; 2] = [".blsmignore", ".orgzlyignore"];

/// Identifies one version of a file: modification time and size. Cheap to
/// read from a listing and enough to notice an outside change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Revision {
    /// Modification time, nanoseconds since the Unix epoch.
    pub mtime_ns: u64,
    /// Size in bytes.
    pub size: u64,
}

impl Revision {
    fn of(meta: &fs::Metadata) -> Revision {
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        Revision {
            mtime_ns,
            size: meta.len(),
        }
    }

    /// Parses the [`Display`](fmt::Display) form, `mtime:size`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Revision> {
        let (m, s) = s.split_once(':')?;
        Some(Revision {
            mtime_ns: m.parse().ok()?,
            size: s.parse().ok()?,
        })
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.mtime_ns, self.size)
    }
}

/// One `.org` file in the vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Vault-relative path, `/`-separated.
    pub path: String,
    /// Its current revision.
    pub revision: Revision,
}

/// A Syncthing conflict copy and the file it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The file the copy conflicts with.
    pub path: String,
    /// The conflict copy.
    pub copy: String,
}

/// The result of walking the vault.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// Every `.org` file, sorted by path.
    pub files: Vec<FileEntry>,
    /// Conflict copies, sorted by copy path.
    pub conflicts: Vec<Conflict>,
}

/// Why a write was refused.
#[derive(Debug)]
pub enum WriteError {
    /// The file changed on disk since it was read (or appeared or vanished).
    /// Re-read, re-apply the edit and write again.
    Stale {
        /// The revision the caller read.
        expected: Option<Revision>,
        /// The revision on disk now.
        found: Option<Revision>,
    },
    /// The path is not a vault-relative path.
    InvalidPath(String),
    /// An I/O error.
    Io(io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Stale { .. } => f.write_str("the file changed on disk since it was read"),
            WriteError::InvalidPath(p) => write!(f, "not a vault-relative path: {p:?}"),
            WriteError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WriteError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> Self {
        WriteError::Io(e)
    }
}

struct Cached {
    revision: Revision,
    doc: Arc<Document>,
}

/// A folder of org files.
pub struct Vault {
    root: PathBuf,
    keywords: Keywords,
    ignore: IgnorePatterns,
    cache: Mutex<HashMap<String, Cached>>,
}

impl Vault {
    /// Opens the folder `root`. Ignore rules come from [`IGNORE_FILES`] in
    /// the root plus `extra_ignore`.
    ///
    /// # Errors
    ///
    /// When `root` is not a readable folder.
    pub fn open(
        root: impl Into<PathBuf>,
        keywords: Keywords,
        extra_ignore: &[String],
    ) -> io::Result<Vault> {
        let root = root.into();
        if !fs::metadata(&root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "vault root is not a folder",
            ));
        }
        let mut ignore = IgnorePatterns::default();
        for name in IGNORE_FILES {
            match fs::read_to_string(root.join(name)) {
                Ok(text) => ignore.extend(text.lines()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        ignore.extend(extra_ignore.iter().map(String::as_str));
        Ok(Vault {
            root,
            keywords,
            ignore,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// The vault folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The TODO keywords documents are parsed with.
    #[must_use]
    pub fn keywords(&self) -> &Keywords {
        &self.keywords
    }

    /// Walks the vault. Dot-folders are never entered; symlinked folders are
    /// not followed (no cycles); names that aren't UTF-8 are skipped.
    ///
    /// # Errors
    ///
    /// When the root can't be read. Unreadable subfolders are skipped.
    pub fn list(&self) -> io::Result<Listing> {
        let mut listing = Listing::default();
        let mut stack = vec![(self.root.clone(), String::new())];
        let mut first = true;
        while let Some((dir, prefix)) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) if first => return Err(e),
                Err(_) => continue,
            };
            first = false;
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let path = format!("{prefix}{name}");
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if !name.starts_with('.') && !self.ignore.is_ignored(&name, &path) {
                        stack.push((entry.path(), format!("{path}/")));
                    }
                    continue;
                }
                if Path::new(&name).extension().is_none_or(|e| e != "org")
                    || self.ignore.is_ignored(&name, &path)
                {
                    continue;
                }
                if let Some(base) = conflict_base(&path) {
                    listing.conflicts.push(Conflict {
                        path: base,
                        copy: path,
                    });
                    continue;
                }
                // `metadata` follows a symlinked file to its target.
                if let Ok(meta) = fs::metadata(entry.path())
                    && meta.is_file()
                {
                    listing.files.push(FileEntry {
                        path,
                        revision: Revision::of(&meta),
                    });
                }
            }
        }
        listing.files.sort_by(|a, b| a.path.cmp(&b.path));
        listing.conflicts.sort_by(|a, b| a.copy.cmp(&b.copy));
        Ok(listing)
    }

    /// The absolute location of vault-relative `path`.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidInput`] for an absolute path or one with `..`.
    pub fn full_path(&self, path: &str) -> io::Result<PathBuf> {
        let rel = Path::new(path);
        let valid = !path.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a vault-relative path: {path:?}"),
            ));
        }
        Ok(self.root.join(rel))
    }

    /// The current revision of `path`, `None` when it doesn't exist.
    ///
    /// # Errors
    ///
    /// On an invalid path or an I/O error other than "not found".
    pub fn revision(&self, path: &str) -> io::Result<Option<Revision>> {
        match fs::metadata(self.full_path(path)?) {
            Ok(meta) => Ok(Some(Revision::of(&meta))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Reads `path` and the revision of exactly what was read (re-reading
    /// if the file changed while it was being read).
    ///
    /// # Errors
    ///
    /// On I/O errors, or [`io::ErrorKind::InvalidData`] for non-UTF-8 text.
    pub fn read(&self, path: &str) -> io::Result<(String, Revision)> {
        let full = self.full_path(path)?;
        let mut attempt = 0;
        loop {
            let before = Revision::of(&fs::metadata(&full)?);
            let bytes = fs::read(&full)?;
            let after = Revision::of(&fs::metadata(&full)?);
            attempt += 1;
            if before == after || attempt == 3 {
                let text = String::from_utf8(bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                return Ok((text, after));
            }
        }
    }

    /// The parsed document at `path`, from the cache when its revision is unchanged.
    ///
    /// # Errors
    ///
    /// As [`Vault::read`].
    pub fn document(&self, path: &str) -> io::Result<(Arc<Document>, Revision)> {
        if let Some(rev) = self.revision(path)?
            && let Some(hit) = self.lock_cache().get(path).filter(|c| c.revision == rev)
        {
            return Ok((Arc::clone(&hit.doc), rev));
        }
        let (text, rev) = self.read(path)?;
        let doc = Arc::new(Document::parse(text, self.keywords.clone()));
        self.lock_cache().insert(
            path.to_string(),
            Cached {
                revision: rev,
                doc: Arc::clone(&doc),
            },
        );
        Ok((doc, rev))
    }

    fn lock_cache(&self) -> std::sync::MutexGuard<'_, HashMap<String, Cached>> {
        // A panic elsewhere can't leave the cache inconsistent: it only maps
        // revisions to parses, so a poisoned lock is still usable.
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes `text` to `path` atomically (temp file in the same folder, then
    /// rename), creating folders as needed. `expected` is the revision the
    /// text was based on, `None` for a new file. The write is refused with
    /// [`WriteError::Stale`] when the file on disk is not at `expected`.
    /// Bytes are written as given, so line endings are kept.
    ///
    /// # Errors
    ///
    /// [`WriteError::Stale`], [`WriteError::InvalidPath`] or [`WriteError::Io`].
    pub fn write(
        &self,
        path: &str,
        text: &str,
        expected: Option<Revision>,
    ) -> Result<Revision, WriteError> {
        self.write_bytes(path, text.as_bytes(), expected)
    }

    /// [`Vault::write`] for any bytes (attachments, diagrams).
    ///
    /// # Errors
    ///
    /// As [`Vault::write`].
    pub fn write_bytes(
        &self,
        path: &str,
        bytes: &[u8],
        expected: Option<Revision>,
    ) -> Result<Revision, WriteError> {
        let full = self
            .full_path(path)
            .map_err(|_| WriteError::InvalidPath(path.to_string()))?;
        let found = self.revision(path)?;
        if found != expected {
            return Err(WriteError::Stale { expected, found });
        }
        let dir = full.parent().unwrap_or(&self.root).to_path_buf();
        fs::create_dir_all(&dir)?;
        let tmp = temp_path(&full);
        let result = write_and_rename(&tmp, &full, bytes);
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        sync_dir(&dir);
        let rev = Revision::of(&fs::metadata(&full)?);
        self.lock_cache().remove(path);
        Ok(rev)
    }

    /// Reads `path` (an empty document if it doesn't exist), applies `edit`,
    /// and writes the result with the stale-write guard. Nothing is written
    /// when `edit` fails or leaves the text unchanged.
    ///
    /// # Errors
    ///
    /// `edit`'s error, or a [`WriteError`] (converted with `From`).
    pub fn edit<T, E>(
        &self,
        path: &str,
        edit: impl FnOnce(&mut Document) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<WriteError>,
    {
        let (text, rev) = match self.read(path) {
            Ok((t, r)) => (t, Some(r)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (String::new(), None),
            Err(e) => return Err(WriteError::Io(e).into()),
        };
        let mut doc = Document::parse(text.clone(), self.keywords.clone());
        let out = edit(&mut doc)?;
        if doc.text() != text {
            self.write(path, doc.text(), rev)?;
        }
        Ok(out)
    }

    /// Starts a debounced watcher on the vault folder.
    ///
    /// # Errors
    ///
    /// When the platform watcher can't be started.
    pub fn watch(&self, debounce: Duration) -> notify::Result<VaultWatcher> {
        VaultWatcher::start(&self.root, debounce)
    }
}

/// `.name.blsm-<pid>-<n>.tmp` next to `full`: a dot-file, so the walk and the
/// watcher ignore it.
fn temp_path(full: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = full.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    full.with_file_name(format!(".{name}.blsm-{}-{n}.tmp", std::process::id()))
}

fn write_and_rename(tmp: &Path, full: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if let Ok(meta) = fs::metadata(full) {
        fs::set_permissions(tmp, meta.permissions())?;
    }
    fs::rename(tmp, full)
}

/// Makes the rename durable where the platform allows it; best effort.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}
