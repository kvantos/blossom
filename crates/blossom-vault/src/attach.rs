//! Attachments: files (images, diagrams) stored next to an org file and
//! linked from it with `[[file:…]]`.
//!
//! An attachment of `notes/work.org` goes in `notes/attachments/work/`
//! (the folder name is configurable), under its own name made safe and
//! unique (names from Bloom's `AttachmentNames.kt`). The link is relative
//! to the org file, so Emacs and other tools find it too.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{Vault, WriteError};

/// The default attachments folder name.
pub const DEFAULT_FOLDER: &str = "attachments";

/// `name` made safe for a file name and an org link: `[`, `]`, `/`, `\` and
/// control characters dropped, trimmed; `attachment` when nothing is left.
#[must_use]
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '/' | '\\') && !c.is_control())
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "attachment".to_string()
    } else {
        cleaned.to_string()
    }
}

/// `wanted`, or `name-1.ext`, `name-2.ext`, … when `existing` has it
/// (compared ignoring case, for case-insensitive file systems).
#[must_use]
pub fn unique_name(existing: &[String], wanted: &str) -> String {
    let taken: Vec<String> = existing.iter().map(|e| e.to_lowercase()).collect();
    let free = |n: &str| !taken.contains(&n.to_lowercase());
    if free(wanted) {
        return wanted.to_string();
    }
    let (stem, ext) = match wanted.rfind('.') {
        Some(i) if i > 0 => (&wanted[..i], &wanted[i..]),
        _ => (wanted, ""),
    };
    // At most one more candidate than there are taken names is needed.
    (1..=existing.len() + 1)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|c| free(c))
        .unwrap_or_else(|| wanted.to_string())
}

/// The vault-relative folder for `org_file`'s attachments: `folder/stem`
/// beside it.
#[must_use]
pub fn folder_for(org_file: &str, folder: &str) -> String {
    let (dir, name) = org_file.rsplit_once('/').unwrap_or(("", org_file));
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let folder = folder.trim_matches('/');
    if dir.is_empty() {
        format!("{folder}/{stem}")
    } else {
        format!("{dir}/{folder}/{stem}")
    }
}

/// The path of vault-relative `target` as written in a link from
/// vault-relative `org_file`: relative to the org file's folder.
#[must_use]
pub fn relative_link(org_file: &str, target: &str) -> String {
    let from: Vec<&str> = org_file.split('/').collect();
    let from = &from[..from.len() - 1];
    let to: Vec<&str> = target.split('/').collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    parts.join("/")
}

/// Where a link path written in vault-relative `org_file` points, for the
/// vault at `root`: `~/` and absolute paths as they are, anything else
/// relative to the org file's folder.
#[must_use]
pub fn resolve(root: &Path, org_file: &str, path: &str) -> PathBuf {
    if path.starts_with('~') || Path::new(path).is_absolute() {
        return crate::config::expand(path);
    }
    let dir = org_file.rsplit_once('/').map_or("", |(d, _)| d);
    root.join(dir).join(path)
}

impl Vault {
    /// Where a link path written in `org_file` points: `~/` and absolute
    /// paths as they are, anything else relative to the org file's folder.
    #[must_use]
    pub fn resolve_link(&self, org_file: &str, path: &str) -> PathBuf {
        resolve(self.root(), org_file, path)
    }

    /// The vault-relative path of absolute `full`, when it is inside the vault.
    #[must_use]
    pub fn relative_path(&self, full: &Path) -> Option<String> {
        let root = self.root().canonicalize().ok()?;
        let parent = full.parent()?.canonicalize().ok()?;
        let rel = parent.join(full.file_name()?);
        let rel = rel.strip_prefix(&root).ok()?;
        let parts: Option<Vec<&str>> = rel
            .components()
            .map(|c| match c {
                std::path::Component::Normal(s) => s.to_str(),
                _ => None,
            })
            .collect();
        Some(parts?.join("/"))
    }

    /// Stores `bytes` as a new attachment of `org_file` named after `name`
    /// in its attachments folder (`folder`, see [`folder_for`]). Returns the
    /// vault-relative path and the link path to write in `org_file`.
    ///
    /// # Errors
    ///
    /// On I/O errors.
    pub fn attach(
        &self,
        org_file: &str,
        folder: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<(String, String), WriteError> {
        let dir = folder_for(org_file, folder);
        let existing: Vec<String> = match fs::read_dir(self.full_path(&dir)?) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let file = unique_name(&existing, &sanitize_name(name));
        let path = format!("{dir}/{file}");
        self.write_bytes(&path, bytes, None)?;
        let link = relative_link(org_file, &path);
        Ok((path, link))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        assert_eq!(sanitize_name(" a[b]/c.png "), "abc.png");
        assert_eq!(sanitize_name(".."), "attachment");
        let existing = vec!["Shot.png".to_string(), "shot-1.png".to_string()];
        assert_eq!(unique_name(&existing, "shot.png"), "shot-2.png");
        assert_eq!(unique_name(&existing, "other"), "other");
        assert_eq!(
            folder_for("notes/work.org", "attachments"),
            "notes/attachments/work"
        );
        assert_eq!(folder_for("inbox.org", "att/"), "att/inbox");
        assert_eq!(
            relative_link("notes/work.org", "notes/attachments/work/a.png"),
            "attachments/work/a.png"
        );
        assert_eq!(relative_link("a/b.org", "c/d.png"), "../c/d.png");
        assert_eq!(relative_link("b.org", "c/d.png"), "c/d.png");
    }
}
