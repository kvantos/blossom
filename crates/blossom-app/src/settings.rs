//! What the app remembers between runs (theme and fonts), in
//! `settings.toml` in the data folder. The app owns this file; people's own
//! settings stay in `~/.config/blossom/config.toml`, which it never writes.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::style::{Colours, ModeChoice};

/// The font sizes on offer, in pixels.
pub const SIZES: RangeInclusive<u32> = 10..=28;

/// The remembered choices. Missing or unknown values fall back to the defaults.
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
        }
    }
}

/// `settings.toml` in the platform data folder.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    blossom_vault::config::data_dir().map(|d| d.join("settings.toml"))
}

impl Settings {
    /// The settings at `path`: the defaults if the file is missing, and the
    /// defaults with a message if it can't be read.
    #[must_use]
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(mut s) => {
                    s.read_size = s.read_size.clamp(*SIZES.start(), *SIZES.end());
                    s.editor_size = s.editor_size.clamp(*SIZES.start(), *SIZES.end());
                    (s, None)
                }
                Err(e) => (
                    Settings::default(),
                    Some(format!("{}: {e}", path.display())),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (
                Settings::default(),
                Some(format!("{}: {e}", path.display())),
            ),
        }
    }

    /// Writes the settings to `path` (a temporary file, then a rename).
    ///
    /// # Errors
    /// When the folder or file can't be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let text = toml::to_string(self).map_err(|e| fail(&e))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(|e| fail(&e))?;
        std::fs::rename(&tmp, path).map_err(|e| fail(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/settings.toml");
        let s = Settings {
            colours: Colours::Solarized,
            mode: ModeChoice::Dark,
            editor_font: Some("Fira Code".into()),
            editor_size: 16,
            ..Settings::default()
        };
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            "colours = \"solarized\"\nmode = \"dark\"\nread_size = 15\n\
             editor_font = \"Fira Code\"\neditor_size = 16\n"
        );
        assert_eq!(Settings::load(&path), (s, None));
    }

    #[test]
    fn missing_or_broken_files_give_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert_eq!(Settings::load(&path), (Settings::default(), None));
        std::fs::write(&path, "colours = \"gray\"\n").unwrap();
        let (s, err) = Settings::load(&path);
        assert_eq!(
            (s.colours, s.mode, err),
            (Colours::Gray, ModeChoice::System, None)
        );
        std::fs::write(&path, "read_size = 99\neditor_size = 1\n").unwrap();
        let (s, err) = Settings::load(&path);
        assert_eq!((s.read_size, s.editor_size, err), (28, 10, None));
        std::fs::write(&path, "colours = \"purple\"\n").unwrap();
        let (s, err) = Settings::load(&path);
        assert_eq!(s, Settings::default());
        assert!(err.is_some());
    }
}
