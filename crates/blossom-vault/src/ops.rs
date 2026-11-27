//! Whole-file operations shared by `blsm` and the app: capture, completing an
//! agenda entry, refile and archive. Each one reads the files it touches,
//! edits them with `blossom-core`, writes them with the stale-write guard and
//! returns the [`Change`]s it made, which [`Vault::restore`] can undo.
//!
//! A heading is named by its index plus its title, so an operation on a file
//! that changed in the meantime fails with [`OpError::Moved`] instead of
//! touching the wrong heading. When two files change, the destination is
//! written first: a failure in between leaves a copy, never a loss.

use std::fmt;
use std::io;

use blossom_core::archive::stamp_subtree;
use blossom_core::capture::{CaptureError, Target};
use blossom_core::{Document, EditError, EventStamp, InsertAt};
use chrono::NaiveDateTime;

use crate::{Revision, Vault, WriteError};

/// One file an operation wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Vault-relative path.
    pub path: String,
    /// The text before; `None` when the operation created the file.
    pub before: Option<String>,
    /// The text written.
    pub after: String,
}

/// What an operation did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The files written, destination first.
    pub changes: Vec<Change>,
    /// The file the moved or new heading is in now.
    pub path: String,
    /// Its heading index there (for capture: the heading the entry is in).
    pub heading: Option<usize>,
    /// The 1-based line where it starts.
    pub line: usize,
    /// A short note, e.g. `Archived to work.org_archive`.
    pub message: String,
}

/// Why an operation failed. Nothing was written, except for
/// [`OpError::Write`] on the second file of a move (see the module docs).
#[derive(Debug)]
pub enum OpError {
    /// The heading isn't where it was: the file changed. Look again.
    Moved,
    /// An undo found the file changed since the operation.
    Changed(String),
    /// An edit was refused.
    Edit(EditError),
    /// A capture was refused.
    Capture(CaptureError),
    /// A write failed or was stale.
    Write(WriteError),
    /// Nothing to do, with why.
    Nothing(String),
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpError::Moved => f.write_str("the file changed; the heading isn't where it was"),
            OpError::Changed(p) => {
                write!(
                    f,
                    "{p} changed since then; undo would overwrite the newer version"
                )
            }
            OpError::Edit(e) => write!(f, "{e}"),
            OpError::Capture(e) => write!(f, "{e}"),
            OpError::Write(WriteError::Stale { .. }) => {
                f.write_str("a file changed while writing; nothing was lost, try again")
            }
            OpError::Write(e) => write!(f, "{e}"),
            OpError::Nothing(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for OpError {}

impl From<EditError> for OpError {
    fn from(e: EditError) -> Self {
        OpError::Edit(e)
    }
}

impl From<CaptureError> for OpError {
    fn from(e: CaptureError) -> Self {
        OpError::Capture(e)
    }
}

impl From<WriteError> for OpError {
    fn from(e: WriteError) -> Self {
        OpError::Write(e)
    }
}

impl From<io::Error> for OpError {
    fn from(e: io::Error) -> Self {
        OpError::Write(WriteError::Io(e))
    }
}

/// A heading by index, checked against its title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingRef {
    /// Vault-relative file.
    pub path: String,
    /// Index in the file.
    pub index: usize,
    /// Title as written.
    pub title: String,
}

/// What completing an agenda entry does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Complete {
    /// Mark the task done (repeating it if anything on it repeats), or
    /// reopen it when it is done already.
    Toggle,
    /// Move one repeating timestamp of an event on.
    Advance(EventStamp),
}

/// A file as read for an operation.
struct Loaded {
    path: String,
    before: Option<String>,
    revision: Option<Revision>,
    doc: Document,
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

impl Vault {
    fn load(&self, path: &str) -> Result<Loaded, OpError> {
        let (before, revision) = match self.read(path) {
            Ok((t, r)) => (Some(t), Some(r)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (None, None),
            Err(e) => return Err(e.into()),
        };
        let doc = Document::parse(before.clone().unwrap_or_default(), self.keywords.clone());
        Ok(Loaded {
            path: path.to_string(),
            before,
            revision,
            doc,
        })
    }

    fn load_heading(&self, h: &HeadingRef) -> Result<Loaded, OpError> {
        let l = self.load(&h.path)?;
        if l.doc.heading(h.index).is_none_or(|x| x.title != h.title) {
            return Err(OpError::Moved);
        }
        Ok(l)
    }

    /// Writes the files that changed, in order; returns their changes.
    fn commit(&self, files: Vec<Loaded>) -> Result<Vec<Change>, OpError> {
        let mut changes = Vec::new();
        for f in files {
            if f.before.as_deref() == Some(f.doc.text()) {
                continue;
            }
            self.write(&f.path, f.doc.text(), f.revision)?;
            changes.push(Change {
                path: f.path,
                before: f.before,
                after: f.doc.text().to_string(),
            });
        }
        Ok(changes)
    }

    /// Adds a capture `entry` to `file` at `target`; `today` picks a date
    /// tree's day. The file is created if needed.
    ///
    /// # Errors
    ///
    /// [`OpError::Capture`] (e.g. the target heading is missing) or a write error.
    pub fn capture(
        &self,
        file: &str,
        target: &Target,
        entry: &str,
        today: chrono::NaiveDate,
    ) -> Result<Outcome, OpError> {
        let mut l = self.load(file)?;
        let at = l.doc.capture(target, entry, today)?;
        let heading = l
            .doc
            .headings()
            .iter()
            .rposition(|h| h.spans.line.start <= at);
        let line = line_of(l.doc.text(), at);
        let changes = self.commit(vec![l])?;
        Ok(Outcome {
            changes,
            path: file.to_string(),
            heading,
            line,
            message: format!("Captured to {file}"),
        })
    }

    /// Creates `file` (vault-relative, from [`new_file_path`]) holding
    /// `text`. Refused when it exists already, in any spelling the disk
    /// treats as the same. Undo ([`Vault::restore`]) deletes it again.
    ///
    /// # Errors
    ///
    /// [`OpError::Nothing`] when the file exists, or a write error.
    pub fn create_file(&self, file: &str, text: &str) -> Result<Outcome, OpError> {
        if self.revision(file)?.is_some() {
            return Err(OpError::Nothing(format!("{file} exists already")));
        }
        self.write(file, text, None).map_err(|e| match e {
            WriteError::Stale { .. } => OpError::Nothing(format!("{file} exists already")),
            e => OpError::Write(e),
        })?;
        Ok(Outcome {
            changes: vec![Change {
                path: file.to_string(),
                before: None,
                after: text.to_string(),
            }],
            path: file.to_string(),
            heading: None,
            line: 1,
            message: format!("Created {file}"),
        })
    }

    /// Completes an agenda entry: see [`Complete`].
    ///
    /// # Errors
    ///
    /// [`OpError::Moved`], an edit error (e.g. no done keyword configured),
    /// or a write error.
    pub fn complete(
        &self,
        h: &HeadingRef,
        how: Complete,
        now: NaiveDateTime,
    ) -> Result<Outcome, OpError> {
        let mut l = self.load_heading(h)?;
        let heading = l.doc.headings()[h.index].clone();
        let message = match how {
            Complete::Advance(which) => {
                l.doc.advance_event(h.index, which, now)?;
                "Moved to the next date".to_string()
            }
            Complete::Toggle => {
                let kw = self.keywords.clone();
                match heading.keyword.as_deref() {
                    Some(k) if kw.is_done(k) => {
                        l.doc
                            .reopen(h.index, kw.active.first().map(String::as_str))?;
                        "Reopened".to_string()
                    }
                    Some(_) => {
                        let done = kw
                            .done
                            .first()
                            .ok_or_else(|| {
                                OpError::Nothing("no done keyword is configured".into())
                            })?
                            .clone();
                        let repeats = l.doc.is_repeating(h.index);
                        l.doc.mark_done(h.index, &done, now)?;
                        if repeats {
                            "Repeated: moved to the next date".to_string()
                        } else {
                            format!("Marked {done}")
                        }
                    }
                    None => return Err(OpError::Nothing("an event has no TODO state".into())),
                }
            }
        };
        let line = line_of(l.doc.text(), l.doc.headings()[h.index].spans.line.start);
        let changes = self.commit(vec![l])?;
        Ok(Outcome {
            changes,
            path: h.path.clone(),
            heading: Some(h.index),
            line,
            message,
        })
    }

    /// Moves heading `h`'s subtree to be the last child of `to` (a heading,
    /// possibly in another file), or to the end of file `to_file` when `to`
    /// is `None`.
    ///
    /// # Errors
    ///
    /// [`OpError::Moved`], [`EditError::TargetInsideSubtree`], or a write error.
    pub fn refile(
        &self,
        h: &HeadingRef,
        to_file: &str,
        to: Option<&HeadingRef>,
    ) -> Result<Outcome, OpError> {
        let label = match to {
            Some(t) => format!("Refiled under {}", t.title),
            None => format!("Refiled to {to_file}"),
        };
        if self.same_file(to_file, &h.path) {
            let mut l = self.load_heading(h)?;
            if let Some(t) = to
                && l.doc.heading(t.index).is_none_or(|x| x.title != t.title)
            {
                return Err(OpError::Moved);
            }
            let index = l.doc.refile(h.index, to.map(|t| t.index))?;
            let line = line_of(l.doc.text(), l.doc.headings()[index].spans.line.start);
            let changes = self.commit(vec![l])?;
            return Ok(Outcome {
                changes,
                path: h.path.clone(),
                heading: Some(index),
                line,
                message: label,
            });
        }
        let mut src = self.load_heading(h)?;
        let mut dest = match to {
            Some(t) => self.load_heading(t)?,
            None => self.load(to_file)?,
        };
        let subtree = src.doc.take_subtree(h.index)?;
        let at = to.map_or(InsertAt::LastTopLevel, |t| InsertAt::LastChild(t.index));
        let index = dest.doc.insert_subtree(at, &subtree)?;
        let line = line_of(dest.doc.text(), dest.doc.headings()[index].spans.line.start);
        let changes = self.commit(vec![dest, src])?;
        Ok(Outcome {
            changes,
            path: to_file.to_string(),
            heading: Some(index),
            line,
            message: label,
        })
    }

    /// Archives heading `h`'s subtree to its archive location (see
    /// [`blossom_core::archive`]), `fallback` being the configured default,
    /// stamped with Emacs's `ARCHIVE_*` properties.
    ///
    /// # Errors
    ///
    /// [`OpError::Moved`], or a write error.
    pub fn archive(
        &self,
        h: &HeadingRef,
        fallback: &str,
        now: NaiveDateTime,
    ) -> Result<Outcome, OpError> {
        let mut src = self.load_heading(h)?;
        let mut target = src
            .doc
            .archive_target(h.index, &h.path, fallback)
            .ok_or_else(|| OpError::Nothing("no archive location".into()))?;
        target.file = self.archive_file(&target.file).ok_or_else(|| {
            OpError::Nothing(format!(
                "the archive location {} is outside the vault",
                target.file
            ))
        })?;
        let full = self.full_path(&h.path)?.display().to_string();
        let props = src.doc.archive_properties(h.index, &h.path, &full, now);
        let subtree = stamp_subtree(&src.doc.take_subtree(h.index)?, &self.keywords, &props)?;
        let message = format!("Archived to {}", target.label());
        let (files, index, path) = if self.same_file(&target.file, &h.path) {
            let index = src.doc.insert_under_path(&target.path, &subtree)?;
            (vec![src], index, h.path.clone())
        } else {
            let mut dest = self.load(&target.file)?;
            let index = dest.doc.insert_under_path(&target.path, &subtree)?;
            (vec![dest, src], index, target.file.clone())
        };
        let line = line_of(
            files[0].doc.text(),
            files[0].doc.headings()[index].spans.line.start,
        );
        let changes = self.commit(files)?;
        Ok(Outcome {
            changes,
            path,
            heading: Some(index),
            line,
            message,
        })
    }

    /// An archive location's file as a vault-relative path: relative ones
    /// as they are, `~/…` and absolute ones when they point into the vault.
    /// `None` for a file outside it.
    fn archive_file(&self, file: &str) -> Option<String> {
        if !(file.starts_with('~') || file.starts_with('/')) {
            return self.full_path(file).is_ok().then(|| file.to_string());
        }
        let full = crate::config::expand(file);
        let canonical = self.root().canonicalize().ok();
        let rel = full
            .strip_prefix(self.root())
            .ok()
            .or_else(|| full.strip_prefix(canonical.as_ref()?).ok())?;
        let parts: Option<Vec<&str>> = rel
            .components()
            .map(|c| match c {
                std::path::Component::Normal(s) => s.to_str(),
                _ => None,
            })
            .collect();
        let rel = parts?.join("/");
        self.full_path(&rel).is_ok().then_some(rel)
    }

    /// Undoes `changes` (from an [`Outcome`]): each file goes back to its
    /// `before` text (a created file is deleted), provided it still holds
    /// exactly the `after` text. Files are restored in reverse order.
    ///
    /// # Errors
    ///
    /// [`OpError::Changed`] when a file changed since (nothing is written
    /// then), or a write error.
    pub fn restore(&self, changes: &[Change]) -> Result<(), OpError> {
        let mut checked = Vec::new();
        for c in changes {
            let (text, rev) = self
                .read(&c.path)
                .map_err(|_| OpError::Changed(c.path.clone()))?;
            if text != c.after {
                return Err(OpError::Changed(c.path.clone()));
            }
            checked.push((c, rev));
        }
        for (c, rev) in checked.into_iter().rev() {
            if let Some(before) = &c.before {
                self.write(&c.path, before, Some(rev))?;
            } else {
                if self.revision(&c.path)? != Some(rev) {
                    return Err(OpError::Changed(c.path.clone()));
                }
                std::fs::remove_file(self.full_path(&c.path)?)?;
                self.lock_cache().remove(&c.path);
            }
        }
        Ok(())
    }
}

/// The vault-relative path for a new file typed as `input`: `garden` is
/// `garden.org`, `projects/garden` is `projects/garden.org` (the folder is
/// made when the file is written). Backslashes count as slashes and leading
/// slashes are dropped.
///
/// # Errors
///
/// A message for the person typing: nothing typed, a folder rather than a
/// file, `..`, or a name starting with `.` (such files and folders are
/// hidden from the file list).
pub fn new_file_path(input: &str) -> Result<String, String> {
    let input = input.trim().replace('\\', "/");
    let input = input.trim_start_matches('/');
    if input.is_empty() {
        return Err("Type a name for the file".to_string());
    }
    if input.ends_with('/') {
        return Err("Type a file name after the folder".to_string());
    }
    let mut parts = Vec::new();
    for part in input.split('/').filter(|p| !p.is_empty()) {
        let part = part.trim();
        if part.is_empty() {
            return Err("A folder name can't be only spaces".to_string());
        }
        if part == ".." || part == "." {
            return Err("Use folder names, not . or ..".to_string());
        }
        if part.starts_with('.') {
            return Err(format!(
                "{part} starts with a dot, which hides it from the file list"
            ));
        }
        if part.chars().any(char::is_control) {
            return Err("The name has a control character".to_string());
        }
        parts.push(part);
    }
    let mut path = parts.join("/");
    let has_org = path.len() > 4
        && path
            .get(path.len() - 4..)
            .is_some_and(|e| e.eq_ignore_ascii_case(".org"));
    if !has_org {
        path.push_str(".org");
    }
    Ok(path)
}

/// The text of a new file: a `#+TITLE:` line when `title` isn't blank,
/// else nothing.
#[must_use]
pub fn new_file_text(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        String::new()
    } else {
        format!("#+TITLE: {title}\n")
    }
}
