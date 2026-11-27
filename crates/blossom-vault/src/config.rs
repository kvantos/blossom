//! `~/.config/blossom/config.toml`, shared by `blsm` and the app.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use blossom_core::capture::{Target, TreeType};
use serde::Deserialize;

pub mod edit;

/// The settings file. Every field is optional.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The vault folder (`~` is expanded).
    pub vault: Option<String>,
    /// TODO keywords, `"TODO NEXT | DONE CANCELLED"`.
    pub keywords: Option<String>,
    /// Org's default priority for `p.X` and `o.p`.
    pub default_priority: Option<String>,
    /// Where `blsm capture` writes, vault-relative. Default `inbox.org`.
    pub inbox: Option<String>,
    /// Extra ignore globs, as in `.blsmignore`.
    pub ignore: Vec<String>,
    /// Days before a DEADLINE that the agenda lists it as due soon, when it
    /// has no `-Nd` of its own (Emacs's `org-deadline-warning-days`). Default 14.
    pub deadline_warning_days: Option<u32>,
    /// Where headings are archived when the file doesn't say (Emacs's
    /// `org-archive-location`). Default `%s_archive::`.
    pub archive: Option<String>,
    /// Capture templates; none means the built-in ones.
    pub capture: Vec<CaptureConfig>,
    /// The attachments folder name, beside each org file. Default `attachments`.
    pub attachments: Option<String>,
    /// Semantic search (`[semantic]`).
    pub semantic: SemanticConfig,
    /// The app's colours and fonts (`[appearance]`).
    pub appearance: AppearanceConfig,
}

/// The `[appearance]` table, read by the app. Every field is optional; the
/// app checks the values and falls back to its defaults.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AppearanceConfig {
    /// `blossom` (default), `solarized` or `gray`.
    pub colours: Option<String>,
    /// `system` (default), `light` or `dark`.
    pub mode: Option<String>,
    /// The note text's font family; none or empty for the default.
    pub read_font: Option<String>,
    /// The note text's size in pixels.
    pub read_size: Option<u32>,
    /// The editor's font family; none or empty for the default.
    pub editor_font: Option<String>,
    /// The editor's size in pixels.
    pub editor_size: Option<u32>,
    /// The top bar's labels: `none` (default; a hint on hover), `below` or
    /// `beside` the icons.
    pub top_bar_labels: Option<String>,
}

/// The `[semantic]` table. Every field is optional; with none, search by
/// meaning uses the built-in English model.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SemanticConfig {
    /// `english` (default), `multilingual` or `server`.
    pub model: Option<String>,
    /// For `server`: an OpenAI-compatible API base, default
    /// `http://localhost:11434/v1` (Ollama).
    pub server: Option<String>,
    /// For `server`: the embedding model name, default `nomic-embed-text`.
    pub server_model: Option<String>,
    /// For `server`: the environment variable holding an API key, if the
    /// server needs one (e.g. `OPENAI_API_KEY`).
    pub api_key_env: Option<String>,
}

/// One `[[capture]]` table.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CaptureConfig {
    /// A short key, e.g. `t`, for `blsm capture -t t`.
    pub key: String,
    /// The name shown in the app.
    pub name: String,
    /// Vault-relative target file; default: `inbox`.
    pub file: Option<String>,
    /// `bottom` (default), `top`, `heading` or `datetree`.
    pub target: Option<String>,
    /// For `heading` and `datetree`: an outline path, `Projects/Blossom`.
    pub heading: Option<String>,
    /// For `heading` and `datetree`: an `:ID:` or `:CUSTOM_ID:` (tried before `heading`).
    pub id: Option<String>,
    /// For `heading`: insert as the first child instead of the last.
    pub first: bool,
    /// For `datetree`: `day` (default), `week` or `month`.
    pub tree: Option<String>,
    /// The template text, e.g. `* TODO %?`.
    pub template: String,
}

/// A capture template ready to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    /// Short key.
    pub key: String,
    /// Display name.
    pub name: String,
    /// Vault-relative target file.
    pub file: String,
    /// Where in the file.
    pub target: Target,
    /// The template text.
    pub template: String,
}

impl Config {
    /// The configured TODO keywords, or the default.
    #[must_use]
    pub fn keywords(&self) -> blossom_core::Keywords {
        self.keywords.as_deref().map_or_else(
            blossom_core::Keywords::default,
            blossom_core::Keywords::parse,
        )
    }
}

impl Config {
    /// The inbox file: `inbox`, else `inbox.org`.
    #[must_use]
    pub fn inbox(&self) -> String {
        self.inbox
            .clone()
            .unwrap_or_else(|| "inbox.org".to_string())
    }

    /// The attachments folder name.
    #[must_use]
    pub fn attachments_folder(&self) -> &str {
        self.attachments
            .as_deref()
            .filter(|f| !f.trim().is_empty())
            .unwrap_or(crate::attach::DEFAULT_FOLDER)
    }

    /// The deadline warning window in days.
    #[must_use]
    pub fn warning_days(&self) -> i64 {
        i64::from(self.deadline_warning_days.unwrap_or(14))
    }

    /// The default archive location.
    #[must_use]
    pub fn archive_location(&self) -> &str {
        self.archive
            .as_deref()
            .unwrap_or(blossom_core::archive::DEFAULT_LOCATION)
    }

    /// The capture templates, or the built-in ones (a note and a TODO at the
    /// bottom of the inbox, a journal entry in a date tree of `journal.org`)
    /// when none are configured.
    ///
    /// # Errors
    ///
    /// A message naming the template with a bad `target` or `tree`, a
    /// missing key or a repeated one.
    pub fn templates(&self) -> Result<Vec<Template>, String> {
        if self.capture.is_empty() {
            let t = |key: &str, name: &str, file: String, target, template: &str| Template {
                key: key.into(),
                name: name.into(),
                file,
                target,
                template: template.into(),
            };
            return Ok(vec![
                t("n", "Note", self.inbox(), Target::Bottom, "* %?"),
                t("t", "TODO", self.inbox(), Target::Bottom, "* TODO %?"),
                t(
                    "j",
                    "Journal",
                    "journal.org".into(),
                    Target::Datetree {
                        id: None,
                        path: Vec::new(),
                        tree: TreeType::Day,
                    },
                    "* %U %?",
                ),
            ]);
        }
        let mut out: Vec<Template> = Vec::new();
        for c in &self.capture {
            let who = if c.name.is_empty() { &c.key } else { &c.name };
            if c.key.trim().is_empty() {
                return Err(format!("capture template {who:?} has no key"));
            }
            if out.iter().any(|o| o.key == c.key) {
                return Err(format!("two capture templates have the key {:?}", c.key));
            }
            let path: Vec<String> = c
                .heading
                .as_deref()
                .unwrap_or("")
                .split('/')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            let id = c.id.clone().filter(|i| !i.trim().is_empty());
            let target = match c.target.as_deref().unwrap_or("bottom") {
                "bottom" => Target::Bottom,
                "top" => Target::Top,
                "heading" if id.is_none() && path.is_empty() => {
                    return Err(format!(
                        "capture template {who:?}: target \"heading\" needs `heading` or `id`"
                    ));
                }
                "heading" => Target::Heading {
                    id,
                    path,
                    first: c.first,
                },
                "datetree" => Target::Datetree {
                    id,
                    path,
                    tree: match c.tree.as_deref().unwrap_or("day") {
                        "day" => TreeType::Day,
                        "week" => TreeType::Week,
                        "month" => TreeType::Month,
                        other => {
                            return Err(format!(
                                "capture template {who:?}: unknown tree {other:?} (day, week or month)"
                            ));
                        }
                    },
                },
                other => {
                    return Err(format!(
                        "capture template {who:?}: unknown target {other:?} (bottom, top, heading or datetree)"
                    ));
                }
            };
            out.push(Template {
                key: c.key.clone(),
                name: if c.name.is_empty() {
                    c.key.clone()
                } else {
                    c.name.clone()
                },
                file: c.file.clone().unwrap_or_else(|| self.inbox()),
                target,
                template: c.template.clone(),
            });
        }
        Ok(out)
    }
}

impl Config {
    /// Reads `path`; a missing file is an empty config.
    ///
    /// # Errors
    ///
    /// A message naming the file when it can't be read or parsed.
    pub fn load(path: &Path) -> Result<Config, String> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// `$XDG_CONFIG_HOME/blossom/config.toml`, else `~/.config/blossom/config.toml`
/// (on macOS too, as is usual for command-line tools).
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(".config")))?;
    Some(base.join("blossom").join("config.toml"))
}

fn home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf())
}

/// Expands a leading `~/`.
#[must_use]
pub fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), home()) {
        (Some(rest), Some(h)) => h.join(rest),
        _ if path == "~" => home().unwrap_or_else(|| PathBuf::from(path)),
        _ => PathBuf::from(path),
    }
}

/// `path` as written in the config: `~/…` when it is in the home folder,
/// so the file still works after the home folder moves.
#[must_use]
pub fn contract(path: &Path) -> String {
    contract_in(path, home().as_deref())
}

fn contract_in(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) if rest.to_str().is_some() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// The index folder for a vault: under the platform cache folder, one per
/// vault, named after the folder plus a stable hash of its full path.
#[must_use]
pub fn index_dir(vault: &Path) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "blossom")?;
    let full = vault.canonicalize().unwrap_or_else(|_| vault.to_path_buf());
    let name: String = full
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("vault")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let hash = fnv1a(full.to_string_lossy().as_bytes());
    Some(
        dirs.cache_dir()
            .join("index")
            .join(format!("{name}-{hash:016x}")),
    )
}

/// The platform data folder (`~/.local/share/blossom` on Linux,
/// `~/Library/Application Support/blossom` on macOS), for downloaded models.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "blossom").map(|d| d.data_dir().to_path_buf())
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semantic_table() {
        let c: Config = toml::from_str(
            "[semantic]\nmodel = \"server\"\nserver = \"http://h/v1\"\nserver_model = \"m\"\napi_key_env = \"KEY\"\n",
        )
        .unwrap();
        assert_eq!(
            c.semantic,
            SemanticConfig {
                model: Some("server".into()),
                server: Some("http://h/v1".into()),
                server_model: Some("m".into()),
                api_key_env: Some("KEY".into()),
            }
        );
        assert_eq!(Config::default().semantic, SemanticConfig::default());
        assert!(toml::from_str::<Config>("[semantic]\nmodle = \"x\"\n").is_err());
    }

    #[test]
    fn parses_all_fields() {
        let c: Config = toml::from_str(
            "vault = \"~/org\"\nkeywords = \"TODO | DONE\"\ndefault_priority = \"B\"\ninbox = \"in.org\"\nignore = [\"a*\"]\n",
        )
        .unwrap();
        assert_eq!(c.vault.as_deref(), Some("~/org"));
        assert_eq!(c.ignore, ["a*"]);
        assert!(toml::from_str::<Config>("typo = 1").is_err());
    }

    #[test]
    fn capture_templates() {
        let c = Config::default();
        let keys: Vec<String> = c.templates().unwrap().into_iter().map(|t| t.key).collect();
        assert_eq!(keys, ["n", "t", "j"]);
        let c: Config = toml::from_str(
            "inbox = \"in.org\"\n[[capture]]\nkey = \"p\"\ntarget = \"heading\"\nheading = \"Projects / Blossom\"\ntemplate = \"* %?\"\n[[capture]]\nkey = \"w\"\nname = \"Week\"\nfile = \"j.org\"\ntarget = \"datetree\"\ntree = \"week\"\n",
        )
        .unwrap();
        let t = c.templates().unwrap();
        assert_eq!(t[0].file, "in.org");
        assert_eq!(t[0].name, "p");
        assert_eq!(
            t[0].target,
            Target::Heading {
                id: None,
                path: vec!["Projects".into(), "Blossom".into()],
                first: false
            }
        );
        assert!(matches!(
            t[1].target,
            Target::Datetree {
                tree: TreeType::Week,
                ..
            }
        ));
        let bad: Config =
            toml::from_str("[[capture]]\nkey = \"x\"\ntarget = \"middle\"\n").unwrap();
        assert!(bad.templates().unwrap_err().contains("middle"));
        let bad: Config =
            toml::from_str("[[capture]]\nkey = \"x\"\ntarget = \"heading\"\n").unwrap();
        assert!(bad.templates().is_err());
    }

    #[test]
    fn contract_home() {
        let home = Path::new("/home/me");
        assert_eq!(
            contract_in(Path::new("/home/me/Notes"), Some(home)),
            "~/Notes"
        );
        assert_eq!(contract_in(Path::new("/home/me"), Some(home)), "~");
        assert_eq!(
            contract_in(Path::new("/home/meg/x"), Some(home)),
            "/home/meg/x"
        );
        assert_eq!(contract_in(Path::new("/srv/n"), None), "/srv/n");
        assert_eq!(expand(&contract(Path::new("/srv/n"))), Path::new("/srv/n"));
    }

    #[test]
    fn stable_hash() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
