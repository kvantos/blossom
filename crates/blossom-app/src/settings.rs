//! The app's appearance choices (colours and fonts), kept in the
//! `[appearance]` table of `~/.config/blossom/config.toml`.
//!
//! Earlier versions kept them in `settings.toml` in the data folder; that
//! file is moved into the config once ([`migrate`]) and then deleted.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use blossom_vault::config::edit::Change;
use blossom_vault::config::{AppearanceConfig, Config};
use serde::de::{DeserializeOwned, IntoDeserializer as _};
use serde::{Deserialize, Serialize};

use crate::style::{Colours, ModeChoice};
use crate::topbar::Labels;

/// The font sizes on offer, in pixels.
pub const SIZES: RangeInclusive<u32> = 10..=28;

/// The config table these live in.
const TABLE: Option<&str> = Some("appearance");

/// The appearance choices.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Which colours.
    pub colours: Colours,
    /// Light, dark, or follow the system.
    pub mode: ModeChoice,
    /// The note text's font family; `None` is the default sans-serif.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_font: Option<String>,
    /// The note text's size in pixels.
    pub read_size: u32,
    /// The editor's font family; `None` is the default monospace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_font: Option<String>,
    /// The editor's size in pixels.
    pub editor_size: u32,
    /// Where the top bar's labels go.
    pub top_bar_labels: Labels,
    /// Emacs keys in the editor (`[editor] emacs_keys`).
    pub emacs_keys: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            colours: Colours::default(),
            mode: ModeChoice::default(),
            read_font: None,
            read_size: 15,
            editor_font: None,
            editor_size: 14,
            top_bar_labels: Labels::default(),
            emacs_keys: true,
        }
    }
}

/// A config name (`"solarized"`) as one of our choices.
fn parse<T: DeserializeOwned>(name: &str) -> Option<T> {
    let d: serde::de::value::StrDeserializer<'_, serde::de::value::Error> =
        name.into_deserializer();
    T::deserialize(d).ok()
}

/// The choice `value` names, if any; a name that isn't one adds a message.
fn choose<T: DeserializeOwned>(
    problems: &mut Vec<String>,
    key: &str,
    value: Option<&String>,
    all: &str,
) -> Option<T> {
    let v = value?;
    let parsed = parse(v);
    if parsed.is_none() {
        problems.push(format!(
            "appearance.{key} = {v:?} isn't one of {all}; using the default"
        ));
    }
    parsed
}

/// One of our choices as its config name.
fn name<T: Serialize>(choice: T) -> String {
    match toml::Value::try_from(choice) {
        Ok(toml::Value::String(s)) => s,
        _ => String::new(),
    }
}

impl Settings {
    /// The choices in the whole config: [`Settings::from_config`] plus
    /// `[editor]`.
    #[must_use]
    pub fn from_all(config: &Config) -> (Settings, Vec<String>) {
        let (mut s, problems) = Settings::from_config(&config.appearance);
        s.emacs_keys = config.editor.emacs_keys.unwrap_or(true);
        (s, problems)
    }

    /// The choices in `config`. A value that isn't one of the choices (or a
    /// size out of range) gives the default, or the nearest size, and a
    /// message saying so.
    #[must_use]
    pub fn from_config(config: &AppearanceConfig) -> (Settings, Vec<String>) {
        let d = Settings::default();
        let mut problems = Vec::new();
        let colours = choose(
            &mut problems,
            "colours",
            config.colours.as_ref(),
            "blossom, solarized or gray",
        )
        .unwrap_or(d.colours);
        let mode = choose(
            &mut problems,
            "mode",
            config.mode.as_ref(),
            "system, light or dark",
        )
        .unwrap_or(d.mode);
        let top_bar_labels = choose(
            &mut problems,
            "top_bar_labels",
            config.top_bar_labels.as_ref(),
            "none, below or beside",
        )
        .unwrap_or(d.top_bar_labels);
        let font = |f: &Option<String>| {
            f.as_deref()
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(String::from)
        };
        let mut size = |key: &str, value: Option<u32>, default: u32| match value {
            Some(n) if !SIZES.contains(&n) => {
                let near = n.clamp(*SIZES.start(), *SIZES.end());
                problems.push(format!(
                    "appearance.{key} = {n} is outside {}–{}; using {near}",
                    SIZES.start(),
                    SIZES.end()
                ));
                near
            }
            Some(n) => n,
            None => default,
        };
        let read_size = size("read_size", config.read_size, d.read_size);
        let editor_size = size("editor_size", config.editor_size, d.editor_size);
        (
            Settings {
                colours,
                mode,
                read_font: font(&config.read_font),
                read_size,
                editor_font: font(&config.editor_font),
                editor_size,
                top_bar_labels,
                emacs_keys: d.emacs_keys,
            },
            problems,
        )
    }

    /// The config changes that turn `old` into `self`: one per field that
    /// differs. A font back to the default is unset (commented out).
    #[must_use]
    pub fn changes_from(&self, old: &Settings) -> Vec<Change> {
        let mut out = Vec::new();
        if self.colours != old.colours {
            out.push(Change::set(TABLE, "colours", name(self.colours)));
        }
        if self.mode != old.mode {
            out.push(Change::set(TABLE, "mode", name(self.mode)));
        }
        let font = |key, f: &Option<String>| match f {
            Some(f) => Change::set(TABLE, key, f.as_str()),
            None => Change::unset(TABLE, key),
        };
        if self.read_font != old.read_font {
            out.push(font("read_font", &self.read_font));
        }
        if self.read_size != old.read_size {
            out.push(Change::set(TABLE, "read_size", self.read_size));
        }
        if self.editor_font != old.editor_font {
            out.push(font("editor_font", &self.editor_font));
        }
        if self.editor_size != old.editor_size {
            out.push(Change::set(TABLE, "editor_size", self.editor_size));
        }
        if self.emacs_keys != old.emacs_keys {
            out.push(Change::set(Some("editor"), "emacs_keys", self.emacs_keys));
        }
        if self.top_bar_labels != old.top_bar_labels {
            out.push(Change::set(
                TABLE,
                "top_bar_labels",
                name(self.top_bar_labels),
            ));
        }
        out
    }
}

/// Where earlier versions kept the appearance: `settings.toml` in the
/// platform data folder.
#[must_use]
pub fn old_path() -> Option<PathBuf> {
    blossom_vault::config::data_dir().map(|d| d.join("settings.toml"))
}

/// Moves the choices in the old `settings.toml` at `old` into the config at
/// `config`, then deletes the old file. A value the config already sets is
/// kept; so is the old file, if anything goes wrong. Returns what happened
/// worth telling, if anything.
pub fn migrate(old: &Path, config: &Path, current: &AppearanceConfig) -> Option<String> {
    let text = match std::fs::read_to_string(old) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(format!("{}: {e}", old.display())),
    };
    let saved = match toml::from_str::<Settings>(&text) {
        Ok(s) => s,
        Err(e) => {
            // Nothing usable in it: the defaults are as good.
            let _ = std::fs::remove_file(old);
            return Some(format!("{}: {e}; removed it", old.display()));
        }
    };
    // Only what the config doesn't set yet; defaults aren't written.
    let d = Settings::default();
    let mut base = d.clone();
    if current.colours.is_none() {
        base.colours = saved.colours;
    }
    if current.mode.is_none() {
        base.mode = saved.mode;
    }
    if current.read_font.is_none() {
        base.read_font = saved.read_font;
    }
    if current.read_size.is_none() {
        base.read_size = saved.read_size.clamp(*SIZES.start(), *SIZES.end());
    }
    if current.editor_font.is_none() {
        base.editor_font = saved.editor_font;
    }
    if current.editor_size.is_none() {
        base.editor_size = saved.editor_size.clamp(*SIZES.start(), *SIZES.end());
    }
    let changes = base.changes_from(&d);
    if let Err(e) = blossom_vault::config::edit::apply(config, &changes) {
        return Some(format!(
            "couldn't move {} into the config: {e}",
            old.display()
        ));
    }
    match std::fs::remove_file(old) {
        Ok(()) => None,
        Err(e) => Some(format!("{}: {e}", old.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appearance(path: &Path) -> AppearanceConfig {
        Config::load(path).unwrap().appearance
    }

    #[test]
    fn read_from_the_config() {
        let (s, problems) = Settings::from_config(&AppearanceConfig::default());
        assert_eq!((s, problems.len()), (Settings::default(), 0));
        let (s, problems) = Settings::from_config(&AppearanceConfig {
            colours: Some("solarized".into()),
            mode: Some("dark".into()),
            read_font: Some(" Inter ".into()),
            editor_font: Some(String::new()),
            read_size: Some(99),
            editor_size: Some(16),
            top_bar_labels: Some("beside".into()),
        });
        assert_eq!(
            s,
            Settings {
                colours: Colours::Solarized,
                mode: ModeChoice::Dark,
                read_font: Some("Inter".into()),
                read_size: 28,
                editor_font: None,
                editor_size: 16,
                top_bar_labels: Labels::Beside,
                emacs_keys: true,
            }
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        let (s, problems) = Settings::from_config(&AppearanceConfig {
            colours: Some("purple".into()),
            ..AppearanceConfig::default()
        });
        assert_eq!(s.colours, Colours::Blossom);
        assert!(problems[0].contains("purple"), "{problems:?}");
    }

    #[test]
    fn changes_round_trip_through_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        blossom_vault::config::edit::create(&path).unwrap();
        let d = Settings::default();
        let s = Settings {
            colours: Colours::Gray,
            mode: ModeChoice::Light,
            read_font: Some("Inter".into()),
            read_size: 18,
            editor_font: Some("Fira Code".into()),
            editor_size: 12,
            top_bar_labels: Labels::Below,
            emacs_keys: false,
        };
        blossom_vault::config::edit::apply(&path, &s.changes_from(&d)).unwrap();
        let all = |path: &Path| Settings::from_all(&Config::load(path).unwrap());
        assert_eq!(all(&path), (s.clone(), vec![]));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\nemacs_keys = false\n"), "{text}");
        let back = Settings {
            read_font: None,
            ..s.clone()
        };
        let changes = back.changes_from(&s);
        assert_eq!(changes, [Change::unset(TABLE, "read_font")]);
        blossom_vault::config::edit::apply(&path, &changes).unwrap();
        assert_eq!(all(&path).0, back);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\n# read_font = \"Inter\"\n"), "{text}");
        assert!(text.contains("\ncolours = \"gray\"\n"), "{text}");
    }

    #[test]
    fn the_old_settings_file_moves_into_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("settings.toml");
        let path = dir.path().join("config.toml");
        assert_eq!(migrate(&old, &path, &AppearanceConfig::default()), None);
        assert!(!path.exists(), "nothing to move, nothing written");
        std::fs::write(
            &old,
            "colours = \"solarized\"\nmode = \"dark\"\nread_size = 15\neditor_size = 16\n",
        )
        .unwrap();
        // The config already chose its mode by hand: that stays.
        std::fs::write(&path, "[appearance]\nmode = \"light\"\n").unwrap();
        assert_eq!(migrate(&old, &path, &appearance(&path)), None);
        assert!(!old.exists());
        let (s, _) = Settings::from_config(&appearance(&path));
        assert_eq!(
            (s.colours, s.mode, s.editor_size),
            (Colours::Solarized, ModeChoice::Light, 16)
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("read_size"),
            "defaults aren't written: {text}"
        );
        // A broken old file is dropped.
        std::fs::write(&old, "colours = \"purple\"\n").unwrap();
        assert!(migrate(&old, &path, &appearance(&path)).is_some());
        assert!(!old.exists());
    }
}
