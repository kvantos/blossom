//! What the app remembers between runs (now only the theme), in
//! `settings.toml` in the data folder. The app owns this file; people's own
//! settings stay in `~/.config/blossom/config.toml`, which it never writes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::style::{Colours, ModeChoice};

/// The remembered choices. Missing or unknown values fall back to the defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Which colours.
    pub colours: Colours,
    /// Light, dark, or follow the system.
    pub mode: ModeChoice,
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
            Ok(text) => match toml::from_str(&text) {
                Ok(s) => (s, None),
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
    pub fn save(self, path: &Path) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
        let text = toml::to_string(&self).map_err(|e| fail(&e))?;
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
        };
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "colours = \"solarized\"\nmode = \"dark\"\n");
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
        std::fs::write(&path, "colours = \"purple\"\n").unwrap();
        let (s, err) = Settings::load(&path);
        assert_eq!(s, Settings::default());
        assert!(err.is_some());
    }
}
