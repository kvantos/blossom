//! Writing `config.toml`: the commented file made on first launch, and
//! changes to single values that keep everything else (comments, order,
//! blank lines, the user's own formatting) exactly as it was.
//!
//! A change works on the text. A value already set is replaced where it
//! stands; a value the file only mentions in a comment (`# colours = "blossom"`,
//! as the first-launch file has every option) is set by uncommenting that
//! line; anything else is added at the end of its table. Unsetting a value
//! comments its line out, so the option and its description stay in view.

use std::fs;
use std::io;
use std::ops::Range;
use std::path::Path;

use toml_edit::{Document, Item, Table};

use super::Config;

/// The file made on first launch: every option, commented out at its
/// default, each with a line saying what it does.
pub const TEMPLATE: &str = r#"# Blossom's settings, read by the app and by `blsm`.
#
# Every option is listed at its default value. To change one, remove the `#`
# in front of it and edit the value. The app writes what you change in its
# Settings panel here too, and keeps your comments. Changes made here while
# the app runs are applied at once.

# The notes folder (`~` is your home folder). Set when you choose a folder.
# vault = "~/Notes"

# TODO keywords: the states before `|` are open, the ones after it are done.
# keywords = "TODO IN-PROGRESS | DONE CANCELLED"

# The priority a heading without one has, for search's `p.X` and `o.p`.
# default_priority = "B"

# Where captures go when a template doesn't name a file (relative to the notes folder).
# inbox = "inbox.org"

# Files and folders to skip, as in `.blsmignore` (e.g. ["drafts/**", "*.tmp.org"]).
# ignore = []

# Days before a DEADLINE that the agenda shows it as due soon.
# deadline_warning_days = 14

# Where archived headings go when the file doesn't say (Emacs's `org-archive-location`).
# archive = "%s_archive::"

# The attachments folder, created beside each note that has attachments.
# attachments = "attachments"

# Capture templates. With none, there are three: Note and TODO at the bottom of
# the inbox, and Journal in a date tree of journal.org. Each one looks like this:
# [[capture]]
# key = "t"                # the key, also for `blsm capture -t t`
# name = "TODO"            # the name shown in the app
# file = "inbox.org"       # the file it goes to (default: `inbox`)
# target = "bottom"        # bottom, top, heading or datetree
# heading = "Projects"     # for heading and datetree: an outline path, "Projects/Blossom"
# id = ""                  # for heading and datetree: an :ID: or :CUSTOM_ID:, tried first
# first = false            # for heading: add as the first child, not the last
# tree = "day"             # for datetree: day, week or month
# template = "* TODO %?"   # the text; %? is where the cursor goes

[appearance]
# Colours: blossom (pink), solarized or gray.
# colours = "blossom"

# Light or dark: system (follow the system), light or dark.
# mode = "system"

# The note text's font family, e.g. "Inter" (default: the system's sans-serif).
# read_font = ""

# The note text's size in pixels, 10 to 28.
# read_size = 15

# The editor's font family, e.g. "Fira Code" (default: the system's monospace).
# editor_font = ""

# The editor's size in pixels, 10 to 28.
# editor_size = 14

# Names in the top bar: none (icons only, a hint on hover), below or beside the icons.
# top_bar_labels = "none"

[semantic]
# Search by meaning: english (built in), multilingual (built in, downloaded
# on first use) or server (an OpenAI-compatible server such as Ollama).
# model = "english"

# For server: the server's API address.
# server = "http://localhost:11434/v1"

# For server: the embedding model's name.
# server_model = "nomic-embed-text"

# For server: the environment variable holding an API key, if the server needs one.
# api_key_env = "OPENAI_API_KEY"
"#;

/// A value to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A string.
    Str(String),
    /// A whole number.
    Int(i64),
    /// A switch.
    Bool(bool),
}

impl Value {
    /// The value in TOML syntax.
    fn to_toml(&self) -> String {
        let v: toml_edit::Value = match self {
            Value::Str(s) => s.as_str().into(),
            Value::Int(n) => (*n).into(),
            Value::Bool(b) => (*b).into(),
        };
        v.to_string().trim().to_string()
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::Str(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::Str(s)
    }
}

impl From<u32> for Value {
    fn from(n: u32) -> Value {
        Value::Int(i64::from(n))
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

/// One change: set `key` in `table` (`None` for the top level) to `value`,
/// or unset it with `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The table, e.g. `appearance`; `None` for the top level.
    pub table: Option<&'static str>,
    /// The key.
    pub key: &'static str,
    /// The new value; `None` comments the key out.
    pub value: Option<Value>,
}

impl Change {
    /// Sets `key` in `table`.
    #[must_use]
    pub fn set(table: Option<&'static str>, key: &'static str, value: impl Into<Value>) -> Change {
        Change {
            table,
            key,
            value: Some(value.into()),
        }
    }

    /// Unsets `key` in `table`.
    #[must_use]
    pub fn unset(table: Option<&'static str>, key: &'static str) -> Change {
        Change {
            table,
            key,
            value: None,
        }
    }
}

/// Writes [`TEMPLATE`] to `path` if there is no file there yet (making its
/// folder). Returns whether it did.
///
/// # Errors
///
/// A message naming the file when it can't be written.
pub fn create(path: &Path) -> Result<bool, String> {
    if fs::symlink_metadata(path).is_ok() {
        return Ok(false);
    }
    write_atomic(path, TEMPLATE)?;
    Ok(true)
}

/// Applies `changes` to the file at `path` (a missing file starts as
/// [`TEMPLATE`]) and writes it back. Nothing is written when there is nothing
/// to change, or when the result wouldn't be a valid config.
///
/// # Errors
///
/// A message naming the file when it can't be read, parsed or written.
pub fn apply(path: &Path, changes: &[Change]) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let old = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => TEMPLATE.to_string(),
        Err(e) => return Err(fail(&e)),
    };
    let mut text = old.clone();
    for change in changes {
        text = change_text(&text, change).map_err(|e| fail(&e))?;
    }
    if text == old && fs::symlink_metadata(path).is_ok() {
        return Ok(());
    }
    toml::from_str::<Config>(&text).map_err(|e| fail(&e))?;
    write_atomic(path, &text)
}

/// Writes `text` to `path` through a temporary file and a rename. A symlink
/// at `path` (dotfiles kept in a repository) is followed, so the link stays.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text).map_err(|e| fail(&e))?;
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        fail(&e)
    })
}

/// `text` with one change made.
///
/// # Errors
///
/// When `text` isn't valid TOML, or the key sits somewhere a line edit can't
/// reach (an inline table, or a dotted key outside its table).
pub fn change_text(text: &str, change: &Change) -> Result<String, String> {
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let table = match change.table {
        Some(name) => doc.as_table().get(name),
        None => Some(doc.as_item()),
    };
    if table.is_some_and(|t| t.as_table_like().is_none()) {
        return Err(format!(
            "`{}` is not a table; change it by hand",
            change.table.unwrap_or_default()
        ));
    }
    let table = table.and_then(Item::as_table_like);
    let found = table.and_then(|t| t.get_key_value(change.key));
    let section = section(&doc, change.table);
    let unreachable = || {
        format!(
            "`{}` is written in a form the app doesn't change; change it by hand",
            dotted(change)
        )
    };
    match (found, &change.value) {
        // Set where it stands: only the value's text changes.
        (Some((_, item)), Some(value)) => {
            let span = item.span().ok_or_else(unreachable)?;
            let new = value.to_toml();
            if text[span.clone()] == new {
                return Ok(text.to_string());
            }
            Ok(splice(text, span, &new))
        }
        // Unset: comment out the whole line.
        (Some((key, item)), None) => {
            let (Some(key_span), Some(value_span)) = (key.span(), item.span()) else {
                return Err(unreachable());
            };
            let start = line_start(text, key_span.start);
            let end = line_end(text, value_span.end);
            if !text[start..key_span.start].trim().is_empty()
                || section
                    .as_ref()
                    .is_none_or(|s| !s.contains(&key_span.start))
            {
                return Err(unreachable());
            }
            let mut lines = String::new();
            for line in text[start..end].split_inclusive('\n') {
                lines.push_str("# ");
                lines.push_str(line);
            }
            Ok(splice(text, start..end, &lines))
        }
        (None, None) => Ok(text.to_string()),
        // Set a key the file doesn't have yet.
        (None, Some(value)) => {
            let line = format!("{} = {}", key_repr(change.key), value.to_toml());
            let Some(range) = section else {
                let name = change.table.unwrap_or_default();
                let mut out = text.to_string();
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push('[');
                out.push_str(&key_repr(name));
                out.push_str("]\n");
                out.push_str(&line);
                out.push('\n');
                return Ok(out);
            };
            if let Some(commented) = commented_line(text, range.clone(), change.key) {
                let body = &text[commented.clone()];
                let indent = &body[..body.len() - body.trim_start().len()];
                return Ok(splice(text, commented.clone(), &format!("{indent}{line}")));
            }
            Ok(insert_at_end(text, range, &line))
        }
    }
}

/// `table.key`, for messages.
fn dotted(change: &Change) -> String {
    match change.table {
        Some(t) => format!("{t}.{}", change.key),
        None => change.key.to_string(),
    }
}

/// A key as TOML writes it: bare if it can be.
fn key_repr(key: &str) -> String {
    toml_edit::Key::new(key).display_repr().into_owned()
}

fn splice(text: &str, range: Range<usize>, new: &str) -> String {
    let mut out = String::with_capacity(text.len() + new.len());
    out.push_str(&text[..range.start]);
    out.push_str(new);
    out.push_str(&text[range.end..]);
    out
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// The end of the line holding `at`, including its newline.
fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

/// The byte range of a table's section: from its header (or the start of the
/// file for the top level) to the next header. `None` if the file has no
/// such table, or only defines it implicitly or inline.
fn section(doc: &Document<&str>, table: Option<&str>) -> Option<Range<usize>> {
    let mut headers = Vec::new();
    collect_headers(doc.as_table(), &mut headers);
    headers.sort_unstable();
    let text_len = doc.raw().len();
    let start = match table {
        None => 0,
        Some(name) => {
            let t = doc.as_table().get(name)?.as_table()?;
            if t.is_implicit() || t.is_dotted() {
                return None;
            }
            t.span()?.start
        }
    };
    let end = headers
        .iter()
        .copied()
        .find(|&h| {
            if table.is_some() {
                h > start
            } else {
                h >= start
            }
        })
        .unwrap_or(text_len);
    Some(start..end)
}

/// The start of every table header in the document.
fn collect_headers(table: &Table, out: &mut Vec<usize>) {
    for (_, item) in table {
        match item {
            Item::Table(t) => {
                if !t.is_implicit()
                    && !t.is_dotted()
                    && let Some(span) = t.span()
                {
                    out.push(span.start);
                }
                collect_headers(t, out);
            }
            Item::ArrayOfTables(a) => {
                for t in a {
                    if let Some(span) = t.span() {
                        out.push(span.start);
                    }
                    collect_headers(t, out);
                }
            }
            _ => {}
        }
    }
}

/// The first line in `range` that is `key = …` commented out (`# key = …`),
/// without its newline.
fn commented_line(text: &str, range: Range<usize>, key: &str) -> Option<Range<usize>> {
    let mut at = range.start;
    for line in text[range].split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        if let Some(rest) = body.trim_start().strip_prefix('#')
            && let Some(rest) = rest.trim_start().strip_prefix(key)
            && rest.trim_start().starts_with('=')
        {
            return Some(at..at + body.len());
        }
        at += line.len();
    }
    None
}

/// `text` with `line` added after the last non-blank, non-comment line of
/// `range` (or right after the header when the section has none).
fn insert_at_end(text: &str, range: Range<usize>, line: &str) -> String {
    let mut at = range.start;
    let mut after = None;
    for l in text[range].split_inclusive('\n') {
        let t = l.trim();
        at += l.len();
        if !t.is_empty() && !t.starts_with('#') {
            after = Some(at);
        }
    }
    let at = after.unwrap_or(at);
    let mut new = String::new();
    if at > 0 && !text[..at].ends_with('\n') {
        new.push('\n');
    }
    new.push_str(line);
    new.push('\n');
    splice(text, at..at, &new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(text: &str, c: &Change) -> String {
        change_text(text, c).unwrap()
    }

    #[test]
    fn the_template_is_the_default_config() {
        let c: Config = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(c, Config::default());
        // Uncommenting every option line gives a valid config too.
        let all: String = TEMPLATE
            .lines()
            .map(|l| match l.strip_prefix("# ") {
                Some(rest)
                    if rest.split_once(" = ").is_some_and(|(k, _)| {
                        !k.contains(' ') && !k.contains('`') && !k.starts_with('[')
                    }) && !rest.contains("  #") =>
                {
                    rest.to_string()
                }
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let c: Config = toml::from_str(&all).unwrap();
        assert_eq!(c.vault.as_deref(), Some("~/Notes"));
        assert_eq!(c.appearance.colours.as_deref(), Some("blossom"));
        assert_eq!(c.semantic.model.as_deref(), Some("english"));
        assert_eq!(c.warning_days(), 14);
        assert_eq!(c.inbox(), "inbox.org");
        assert_eq!(
            c.archive_location(),
            blossom_core::archive::DEFAULT_LOCATION
        );
        assert_eq!(c.attachments_folder(), crate::attach::DEFAULT_FOLDER);
        assert_eq!(c.keywords(), blossom_core::Keywords::default());
    }

    #[test]
    fn setting_uncomments_the_option_line() {
        let out = change(
            TEMPLATE,
            &Change::set(Some("appearance"), "colours", "solarized"),
        );
        assert!(
            out.contains(
                "# Colours: blossom (pink), solarized or gray.\ncolours = \"solarized\"\n"
            )
        );
        assert_eq!(
            out.len(),
            TEMPLATE.len() + "solarized".len() - "blossom".len() - 2
        );
        let out = change(&out, &Change::set(None, "vault", "~/org"));
        assert!(out.contains("Set when you choose a folder.\nvault = \"~/org\"\n"));
        let c: Config = toml::from_str(&out).unwrap();
        assert_eq!(c.vault.as_deref(), Some("~/org"));
        assert_eq!(c.appearance.colours.as_deref(), Some("solarized"));
        // Same key name in two tables: each finds its own.
        let text = "# model = \"top\"\n[semantic]\n# model = \"english\"\n";
        let out = change(text, &Change::set(Some("semantic"), "model", "server"));
        assert_eq!(out, "# model = \"top\"\n[semantic]\nmodel = \"server\"\n");
    }

    #[test]
    fn setting_again_replaces_only_the_value() {
        let text = "[appearance]\n  read_size   =   15 # mine\nmode = \"dark\"\n";
        let out = change(text, &Change::set(Some("appearance"), "read_size", 18u32));
        assert_eq!(
            out,
            "[appearance]\n  read_size   =   18 # mine\nmode = \"dark\"\n"
        );
        assert_eq!(
            change(&out, &Change::set(Some("appearance"), "read_size", 18u32)),
            out
        );
        // An inline table or a dotted key: its value is replaced too.
        let text = "appearance = { mode = \"dark\" }\n";
        let out = change(text, &Change::set(Some("appearance"), "mode", "light"));
        assert_eq!(out, "appearance = { mode = \"light\" }\n");
        let text = "appearance.mode = \"dark\"\n";
        let out = change(text, &Change::set(Some("appearance"), "mode", "light"));
        assert_eq!(out, "appearance.mode = \"light\"\n");
    }

    #[test]
    fn unsetting_comments_the_line_out() {
        let text = "[appearance]\n# The font.\nread_font = \"Inter\"\nmode = \"dark\"\n";
        let out = change(text, &Change::unset(Some("appearance"), "read_font"));
        assert_eq!(
            out,
            "[appearance]\n# The font.\n# read_font = \"Inter\"\nmode = \"dark\"\n"
        );
        // And setting it again uncomments the same line.
        let back = change(&out, &Change::set(Some("appearance"), "read_font", "Lora"));
        assert_eq!(back, text.replace("Inter", "Lora"));
        assert_eq!(
            change(&out, &Change::unset(Some("appearance"), "read_font")),
            out
        );
        // A key in an inline table can't be commented out.
        let text = "appearance = { mode = \"dark\" }\n";
        assert!(change_text(text, &Change::unset(Some("appearance"), "mode")).is_err());
    }

    #[test]
    fn new_keys_go_at_the_end_of_their_table() {
        let text = "vault = \"~/n\"\n\n[semantic]\nmodel = \"server\"\n\n# end\n";
        let out = change(text, &Change::set(None, "inbox", "in.org"));
        assert_eq!(
            out,
            "vault = \"~/n\"\ninbox = \"in.org\"\n\n[semantic]\nmodel = \"server\"\n\n# end\n"
        );
        let out = change(text, &Change::set(Some("semantic"), "server", "http://h"));
        assert_eq!(
            out,
            "vault = \"~/n\"\n\n[semantic]\nmodel = \"server\"\nserver = \"http://h\"\n\n# end\n"
        );
        let out = change(text, &Change::set(Some("appearance"), "mode", "dark"));
        assert_eq!(out, format!("{text}\n[appearance]\nmode = \"dark\"\n"));
        let out = change("", &Change::set(None, "vault", "~/n"));
        assert_eq!(out, "vault = \"~/n\"\n");
        let out = change("[semantic]\n", &Change::set(None, "vault", "~/n"));
        assert_eq!(out, "vault = \"~/n\"\n[semantic]\n");
        // Headers inside a multi-line string aren't headers.
        let text = "[[capture]]\nkey = \"l\"\ntemplate = '''\n* %?\n[[file:x]]\n'''\n[semantic]\n";
        let out = change(text, &Change::set(Some("semantic"), "model", "server"));
        assert_eq!(out, format!("{text}model = \"server\"\n"));
        toml::from_str::<Config>(&out).unwrap();
    }

    #[test]
    fn strings_are_quoted() {
        let out = change("", &Change::set(None, "vault", "C:\\notes \"x\""));
        let c: Config = toml::from_str(&out).unwrap();
        assert_eq!(c.vault.as_deref(), Some("C:\\notes \"x\""));
    }

    #[test]
    fn files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blossom/config.toml");
        assert!(create(&path).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), TEMPLATE);
        fs::write(&path, "# mine\n").unwrap();
        assert!(!create(&path).unwrap());
        apply(&path, &[Change::set(Some("appearance"), "mode", "dark")]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# mine\n\n[appearance]\nmode = \"dark\"\n"
        );
        // A change that would make the config invalid isn't written.
        let before = fs::read_to_string(&path).unwrap();
        assert!(apply(&path, &[Change::set(Some("appearance"), "mode", 3u32)]).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        // Nor is anything written into a file that doesn't parse.
        fs::write(&path, "vault = \n").unwrap();
        assert!(apply(&path, &[Change::set(None, "vault", "~/n")]).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "vault = \n");
        // A missing file starts as the template.
        let other = dir.path().join("other.toml");
        apply(&other, &[Change::set(None, "vault", "~/n")]).unwrap();
        let text = fs::read_to_string(&other).unwrap();
        assert!(text.starts_with("# Blossom's settings"));
        assert!(text.contains("\nvault = \"~/n\"\n"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_file_stays_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles.toml");
        fs::write(&real, "").unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        apply(&link, &[Change::set(None, "vault", "~/n")]).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "vault = \"~/n\"\n");
    }
}
