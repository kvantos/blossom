//! The notebooks tree: vault files grouped into folders.

use std::collections::{BTreeMap, HashSet};

/// One vault file as the sidebar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    /// Vault-relative path.
    pub path: String,
    /// Number of headings.
    pub headings: usize,
    /// Headings with a not-done keyword.
    pub open_tasks: usize,
    /// Whether a Syncthing conflict copy exists.
    pub conflict: bool,
}

/// One visible row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A folder (`path` has no trailing `/`).
    Folder {
        /// Vault-relative folder path.
        path: String,
        /// Its own name.
        name: String,
        /// Nesting depth, 0 at the root.
        depth: usize,
        /// Whether its contents are shown.
        expanded: bool,
    },
    /// A file.
    File {
        /// Its info.
        info: FileInfo,
        /// File name without `.org`.
        name: String,
        /// Nesting depth.
        depth: usize,
    },
}

#[derive(Default)]
struct Node<'a> {
    folders: BTreeMap<String, Node<'a>>,
    files: Vec<&'a FileInfo>,
}

/// The visible rows: in each folder, sub-folders first, then files, each
/// sorted by name ignoring case. Folders in `collapsed` hide their contents.
#[must_use]
pub fn rows(files: &[FileInfo], collapsed: &HashSet<String>) -> Vec<Row> {
    let mut root = Node::default();
    for f in files {
        let mut node = &mut root;
        let mut parts: Vec<&str> = f.path.split('/').collect();
        parts.pop();
        for part in parts {
            node = node.folders.entry(part.to_string()).or_default();
        }
        node.files.push(f);
    }
    let mut out = Vec::new();
    walk(&root, "", 0, collapsed, &mut out);
    out
}

fn walk(
    node: &Node<'_>,
    prefix: &str,
    depth: usize,
    collapsed: &HashSet<String>,
    out: &mut Vec<Row>,
) {
    let mut folders: Vec<(&String, &Node<'_>)> = node.folders.iter().collect();
    folders.sort_by_key(|(name, _)| name.to_lowercase());
    for (name, child) in folders {
        let path = format!("{prefix}{name}");
        let expanded = !collapsed.contains(&path);
        out.push(Row::Folder {
            path: path.clone(),
            name: name.clone(),
            depth,
            expanded,
        });
        if expanded {
            walk(child, &format!("{path}/"), depth + 1, collapsed, out);
        }
    }
    let mut files = node.files.clone();
    files.sort_by_key(|f| f.path.to_lowercase());
    for f in files {
        let file_name = f.path.rsplit('/').next().unwrap_or(&f.path);
        out.push(Row::File {
            info: f.clone(),
            name: file_name
                .strip_suffix(".org")
                .unwrap_or(file_name)
                .to_string(),
            depth,
        });
    }
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

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Folder { name, depth, .. } => format!("{}{name}/", "  ".repeat(*depth)),
                Row::File { name, depth, .. } => format!("{}{name}", "  ".repeat(*depth)),
            })
            .collect()
    }

    #[test]
    fn folders_first_then_files() {
        let files = [
            info("zeta.org"),
            info("Inbox.org"),
            info("projects/b.org"),
            info("projects/old/a.org"),
            info("Archive/x.org"),
        ];
        assert_eq!(
            names(&rows(&files, &HashSet::new())),
            [
                "Archive/",
                "  x",
                "projects/",
                "  old/",
                "    a",
                "  b",
                "Inbox",
                "zeta"
            ]
        );
        let collapsed = HashSet::from(["projects".to_string()]);
        assert_eq!(
            names(&rows(&files, &collapsed)),
            ["Archive/", "  x", "projects/", "Inbox", "zeta"]
        );
    }
}
