//! Archiving, ported from Bloom's `ArchiveLocation.kt` and
//! `ArchiveProperties.kt` after Emacs's `org-archive-subtree`.
//!
//! The destination comes from the nearest `:ARCHIVE:` property (the heading,
//! then its ancestors), then the file's `:ARCHIVE:` drawer property, then
//! `#+ARCHIVE:`, then the app's default (Emacs's is `%s_archive::`). The
//! moved subtree gets Emacs's `ARCHIVE_TIME`, `ARCHIVE_FILE`,
//! `ARCHIVE_OLPATH`, `ARCHIVE_CATEGORY` and `ARCHIVE_TODO` properties.
//! Moving text between two files is the caller's job: [`Document::take_subtree`]
//! on the source, [`Document::insert_under_path`] on the destination.

use chrono::NaiveDateTime;

use crate::document::Document;
use crate::edit::{EditError, InsertAt, NewHeading};
use crate::text;
use crate::timestamp::Timestamp;

/// Emacs's default `org-archive-location`.
pub const DEFAULT_LOCATION: &str = "%s_archive::";

/// A parsed archive location: a vault-relative file and a heading path in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveTarget {
    /// Vault-relative file.
    pub file: String,
    /// Heading titles, outermost first; empty for the top level.
    pub path: Vec<String>,
}

impl ArchiveTarget {
    /// Parses `file[::* Heading[/Sub...]]`. `%s` in the file part is the
    /// source file, an empty file part means the source file itself, a `./`
    /// prefix is dropped, and a bare name without an extension gets `.org`.
    /// `None` for a blank value.
    #[must_use]
    pub fn parse(raw: &str, source_file: &str) -> Option<ArchiveTarget> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let (file, heading) = raw.split_once("::").unwrap_or((raw, ""));
        let mut file = file
            .trim()
            .trim_start_matches("./")
            .replace("%s", source_file);
        if file.is_empty() {
            file = source_file.to_string();
        }
        let name = file.rsplit('/').next().unwrap_or("");
        if !name.get(1..).is_some_and(|n| n.contains('.')) {
            file.push_str(".org");
        }
        let path = heading
            .trim()
            .trim_start_matches('*')
            .split('/')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        Some(ArchiveTarget { file, path })
    }

    /// `work_archive › Done`, for messages.
    #[must_use]
    pub fn label(&self) -> String {
        let file = self.file.strip_suffix(".org").unwrap_or(&self.file);
        std::iter::once(file)
            .chain(self.path.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" › ")
    }
}

impl Document {
    /// Where heading `index` archives to (see the module docs). `fallback`
    /// is the app's default location, used when the file names none.
    #[must_use]
    pub fn archive_target(
        &self,
        index: usize,
        source_file: &str,
        fallback: &str,
    ) -> Option<ArchiveTarget> {
        let mut cur = self.heading(index);
        while let Some(h) = cur {
            if let Some(t) = h
                .property("ARCHIVE")
                .and_then(|v| ArchiveTarget::parse(v, source_file))
            {
                return Some(t);
            }
            cur = h.parent.and_then(|p| self.heading(p));
        }
        let pre = self.preamble();
        let from = |entries: &[(String, String)]| {
            entries
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("ARCHIVE"))
                .find_map(|(_, v)| ArchiveTarget::parse(v, source_file))
        };
        from(&pre.properties)
            .or_else(|| from(&pre.keywords))
            .or_else(|| ArchiveTarget::parse(fallback, source_file))
    }

    /// Titles of heading `index`'s ancestors, outermost first.
    #[must_use]
    pub fn outline_path(&self, index: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = self.heading(index).and_then(|h| h.parent);
        while let Some(p) = cur.and_then(|p| self.heading(p)) {
            out.push(p.title.trim().to_string());
            cur = p.parent;
        }
        out.reverse();
        out
    }

    /// Org's category: the nearest `:CATEGORY:` on the heading or an
    /// ancestor, then the file drawer's, then the last `#+CATEGORY:`, then
    /// the file name without folder and extension.
    #[must_use]
    pub fn category(&self, index: usize, source_file: &str) -> String {
        let mut cur = self.heading(index);
        while let Some(h) = cur {
            if let Some(c) = h
                .property("CATEGORY")
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                return c.to_string();
            }
            cur = h.parent.and_then(|p| self.heading(p));
        }
        let pre = self.preamble();
        let is_cat =
            |(k, v): &&(String, String)| k.eq_ignore_ascii_case("CATEGORY") && !v.trim().is_empty();
        if let Some((_, v)) = pre.properties.iter().find(is_cat) {
            return v.trim().to_string();
        }
        if let Some((_, v)) = pre.keywords.iter().rev().find(is_cat) {
            return v.trim().to_string();
        }
        let name = source_file.rsplit('/').next().unwrap_or(source_file);
        name.rsplit_once('.')
            .map_or(name, |(stem, _)| stem)
            .to_string()
    }

    /// Emacs's archive context properties for heading `index`, in Emacs's
    /// order, blank ones left out. `full_path` is the source file's absolute
    /// path, for `ARCHIVE_FILE`.
    #[must_use]
    pub fn archive_properties(
        &self,
        index: usize,
        source_file: &str,
        full_path: &str,
        now: NaiveDateTime,
    ) -> Vec<(String, String)> {
        let stamp = Timestamp::inactive_at(now).format();
        let time = stamp
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let todo = self
            .heading(index)
            .and_then(|h| h.keyword.clone())
            .unwrap_or_default();
        [
            ("ARCHIVE_TIME", time),
            ("ARCHIVE_FILE", full_path.to_string()),
            ("ARCHIVE_OLPATH", self.outline_path(index).join("/")),
            ("ARCHIVE_CATEGORY", self.category(index, source_file)),
            ("ARCHIVE_TODO", todo),
        ]
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    /// Removes heading `index`'s subtree and returns its text, ending with a
    /// line ending.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchHeading`].
    pub fn take_subtree(&mut self, index: usize) -> Result<String, EditError> {
        self.heading(index).ok_or(EditError::NoSuchHeading(index))?;
        let mut subtree = self.subtree_text(index).to_string();
        if !subtree.ends_with('\n') {
            subtree.push_str(text::eol(self.text()));
        }
        self.delete_subtree(index)?;
        Ok(subtree)
    }

    /// The heading at outline `path`, creating missing ones as last children
    /// (Bloom's `findOrCreateHeadingPath`). `None` for an empty path.
    ///
    /// # Errors
    ///
    /// Only on an internal edit failure.
    pub fn ensure_path(&mut self, path: &[String]) -> Result<Option<usize>, EditError> {
        let mut cur: Option<usize> = None;
        for title in path {
            let candidates = match cur {
                Some(p) => self.children(p),
                None => self.top_level(),
            };
            let found = candidates
                .into_iter()
                .find(|&i| self.headings()[i].title == *title);
            cur = Some(if let Some(i) = found {
                i
            } else {
                let new = NewHeading {
                    title: title.clone(),
                    ..NewHeading::default()
                };
                let at = cur.map_or(InsertAt::LastTopLevel, InsertAt::LastChild);
                self.insert_heading(at, &new)?
            });
        }
        Ok(cur)
    }

    /// Inserts `subtree` as the last child of the heading at `path` (created
    /// as needed), or at the end of the file at level 1 for an empty path.
    /// Returns the root's new index.
    ///
    /// # Errors
    ///
    /// [`EditError::NotASubtree`] when `subtree` doesn't start with a heading.
    pub fn insert_under_path(
        &mut self,
        path: &[String],
        subtree: &str,
    ) -> Result<usize, EditError> {
        let parent = self.ensure_path(path)?;
        let at = parent.map_or(InsertAt::LastTopLevel, InsertAt::LastChild);
        self.insert_subtree(at, subtree)
    }
}

/// `subtree` (text starting with its root heading) with `props` set in the
/// root's `:PROPERTIES:` drawer.
///
/// # Errors
///
/// [`EditError::NotASubtree`] when `subtree` doesn't start with a heading,
/// or an invalid property.
pub fn stamp_subtree(
    subtree: &str,
    keywords: &crate::Keywords,
    props: &[(String, String)],
) -> Result<String, EditError> {
    let mut doc = Document::parse(subtree, keywords.clone());
    if doc.heading(0).is_none_or(|h| h.spans.line.start != 0) {
        return Err(EditError::NotASubtree);
    }
    for (k, v) in props {
        doc.upsert_property(0, k, v)?;
    }
    Ok(doc.text().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Keywords;
    use chrono::NaiveDate;

    fn t(file: &str, path: &[&str]) -> ArchiveTarget {
        ArchiveTarget {
            file: file.into(),
            path: path.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn parses_locations() {
        assert_eq!(
            ArchiveTarget::parse("%s_archive::", "work/a.org"),
            Some(t("work/a.org_archive", &[]))
        );
        assert_eq!(
            ArchiveTarget::parse("::* Archive", "a.org"),
            Some(t("a.org", &["Archive"]))
        );
        assert_eq!(
            ArchiveTarget::parse("./archive::* Done/2026", "a.org"),
            Some(t("archive.org", &["Done", "2026"]))
        );
        assert_eq!(ArchiveTarget::parse("  ", "a.org"), None);
        assert_eq!(t("x.org", &["A", "B"]).label(), "x › A › B");
    }

    #[test]
    fn resolves_nearest_first() {
        let text =
            "#+ARCHIVE: kw.org::\n* A\n:PROPERTIES:\n:ARCHIVE: parent.org::\n:END:\n** B\n* C\n";
        let d = Document::parse(text, Keywords::default());
        assert_eq!(
            d.archive_target(1, "s.org", DEFAULT_LOCATION),
            Some(t("parent.org", &[]))
        );
        assert_eq!(
            d.archive_target(2, "s.org", DEFAULT_LOCATION),
            Some(t("kw.org", &[]))
        );
        let d = Document::parse("* C\n", Keywords::default());
        assert_eq!(
            d.archive_target(0, "s.org", DEFAULT_LOCATION),
            Some(t("s.org_archive", &[]))
        );
    }

    #[test]
    fn properties_and_moves() {
        let text = "#+CATEGORY: work\n* Proj\n** TODO Task :x:\nbody\n*** sub\n* Next\n";
        let mut d = Document::parse(text, Keywords::default());
        let now = NaiveDate::from_ymd_opt(2026, 11, 23)
            .unwrap()
            .and_hms_opt(14, 5, 0)
            .unwrap();
        let props = d.archive_properties(1, "a.org", "/v/a.org", now);
        assert_eq!(
            props,
            [
                (
                    "ARCHIVE_TIME".to_string(),
                    "2026-11-23 Mon 14:05".to_string()
                ),
                ("ARCHIVE_FILE".into(), "/v/a.org".into()),
                ("ARCHIVE_OLPATH".into(), "Proj".into()),
                ("ARCHIVE_CATEGORY".into(), "work".into()),
                ("ARCHIVE_TODO".into(), "TODO".into()),
            ]
        );
        let sub = d.take_subtree(1).unwrap();
        assert_eq!(d.text(), "#+CATEGORY: work\n* Proj\n* Next\n");
        let stamped = stamp_subtree(&sub, &Keywords::default(), &props[..1]).unwrap();
        assert_eq!(
            stamped,
            "** TODO Task :x:\n:PROPERTIES:\n:ARCHIVE_TIME: 2026-11-23 Mon 14:05\n:END:\nbody\n*** sub\n"
        );
        let mut dest = Document::parse("* Archive\n", Keywords::default());
        let i = dest
            .insert_under_path(&["Archive".into(), "2026".into()], &stamped)
            .unwrap();
        assert_eq!(i, 2);
        assert_eq!(
            dest.text(),
            "* Archive\n** 2026\n*** TODO Task :x:\n:PROPERTIES:\n:ARCHIVE_TIME: 2026-11-23 Mon 14:05\n:END:\nbody\n**** sub\n"
        );
        let mut empty = Document::parse("", Keywords::default());
        empty.insert_under_path(&[], &sub).unwrap();
        assert_eq!(empty.text(), "* TODO Task :x:\nbody\n** sub\n");
    }
}
