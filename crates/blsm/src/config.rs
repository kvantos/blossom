//! `~/.config/blossom/config.toml`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The settings file. Every field is optional.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
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
}

impl Config {
    /// Reads `path`; a missing file is an empty config.
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
pub fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), home()) {
        (Some(rest), Some(h)) => h.join(rest),
        _ if path == "~" => home().unwrap_or_else(|| PathBuf::from(path)),
        _ => PathBuf::from(path),
    }
}

/// The index folder for a vault: under the platform cache folder, one per
/// vault, named after the folder plus a stable hash of its full path.
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
    fn stable_hash() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
