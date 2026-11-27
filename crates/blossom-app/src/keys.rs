//! The app's own shortcuts. On Linux they take Ctrl+Shift, so plain Ctrl
//! keys belong to the editor (and its Emacs keys); macOS uses Cmd, which
//! the editor leaves alone. A key counts by the letter typed, or by its
//! place on a US keyboard when that isn't a Latin letter (a Russian
//! layout).
//!
//! | Action       | Linux        | macOS |
//! |--------------|--------------|-------|
//! | Capture      | Ctrl+Shift+N | ⌘N    |
//! | Edit         | Ctrl+Shift+E | ⌘E    |
//! | Open folder  | Ctrl+Shift+O | ⌘O    |
//! | Paste image  | Ctrl+Shift+V | ⌘V    |
//! | Agenda       | Ctrl+Shift+A | ⇧⌘A   |
//! | Search       | Ctrl+Shift+F | ⇧⌘F   |
//! | Settings     | Ctrl+,       | ⌘,    |
//! | Sidebar      | Ctrl+\       | ⌘\    |
//! | Undo / redo  | Ctrl+Z / Ctrl+Shift+Z (or Ctrl+Y) | ⌘Z / ⇧⌘Z |

use iced::keyboard::key::Physical;
use iced::keyboard::{Key, Modifiers};

/// An app action reached from the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKey {
    Capture,
    Edit,
    OpenFolder,
    PasteImage,
    Agenda,
    Search,
    Settings,
    Sidebar,
    Undo,
    Redo,
}

impl AppKey {
    /// Whether the key also works while the editor has the focus. Undo,
    /// redo and paste there are the editor's own.
    #[must_use]
    pub fn in_editor(self) -> bool {
        !matches!(self, AppKey::Undo | AppKey::Redo | AppKey::PasteImage)
    }

    /// The shortcut as this platform writes it, for hints.
    #[must_use]
    pub fn hint(self) -> String {
        let mac = cfg!(target_os = "macos");
        let (shift, key) = match self {
            AppKey::Capture => (!mac, "N"),
            AppKey::Edit => (!mac, "E"),
            AppKey::OpenFolder => (!mac, "O"),
            AppKey::PasteImage => (!mac, "V"),
            AppKey::Agenda => (true, "A"),
            AppKey::Search => (true, "F"),
            AppKey::Settings => (false, ","),
            AppKey::Sidebar => (false, "\\"),
            AppKey::Undo => (false, "Z"),
            AppKey::Redo => (true, "Z"),
        };
        shortcut(shift, key)
    }
}

/// A shortcut as this platform writes it: `shortcut(true, "A")` is
/// Ctrl+Shift+A, or ⇧⌘A on macOS.
#[must_use]
pub fn shortcut(shift: bool, key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("{}⌘{key}", if shift { "⇧" } else { "" })
    } else {
        format!("Ctrl+{}{key}", if shift { "Shift+" } else { "" })
    }
}

/// The app action a key press names, if any.
#[must_use]
pub fn app_key(key: &Key, physical: Physical, m: Modifiers) -> Option<AppKey> {
    let mac = cfg!(target_os = "macos");
    // The platform's command key alone, maybe with Shift.
    let command = if mac {
        m.logo() && !m.control()
    } else {
        m.control() && !m.logo()
    };
    if !command || m.alt() {
        return None;
    }
    let c = key.to_latin(physical)?.to_ascii_lowercase();
    // The keys that take Shift on Linux and not on macOS.
    let moved = m.shift() != mac;
    Some(match (c, m.shift()) {
        ('n', _) if moved => AppKey::Capture,
        ('e', _) if moved => AppKey::Edit,
        ('o', _) if moved => AppKey::OpenFolder,
        ('v', _) if moved => AppKey::PasteImage,
        ('a', true) => AppKey::Agenda,
        ('f', true) => AppKey::Search,
        (',', false) => AppKey::Settings,
        ('\\', false) => AppKey::Sidebar,
        ('z', false) => AppKey::Undo,
        ('z', true) | ('y', false) => AppKey::Redo,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::{Code, NativeCode};

    fn k(s: &str, m: Modifiers) -> Option<AppKey> {
        app_key(
            &Key::Character(s.into()),
            Physical::Unidentified(NativeCode::Unidentified),
            m,
        )
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_keys() {
        let ctrl = Modifiers::CTRL;
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        assert_eq!(k("N", both), Some(AppKey::Capture));
        assert_eq!(k("E", both), Some(AppKey::Edit));
        assert_eq!(k("O", both), Some(AppKey::OpenFolder));
        assert_eq!(k("V", both), Some(AppKey::PasteImage));
        assert_eq!(k("A", both), Some(AppKey::Agenda));
        assert_eq!(k("F", both), Some(AppKey::Search));
        assert_eq!(k("Z", both), Some(AppKey::Redo));
        assert_eq!(k(",", ctrl), Some(AppKey::Settings));
        assert_eq!(k("\\", ctrl), Some(AppKey::Sidebar));
        assert_eq!(k("z", ctrl), Some(AppKey::Undo));
        assert_eq!(k("y", ctrl), Some(AppKey::Redo));
        // Plain Ctrl letters are the editor's.
        for c in ["n", "e", "o", "v", "a", "f"] {
            assert_eq!(k(c, ctrl), None, "{c}");
        }
        assert_eq!(k("N", Modifiers::SHIFT), None);
        assert_eq!(k("N", both | Modifiers::ALT), None);
        // A Russian layout: Ctrl+Shift+Т is where N is.
        assert_eq!(
            app_key(
                &Key::Character("Т".into()),
                Physical::Code(Code::KeyN),
                both
            ),
            Some(AppKey::Capture)
        );
        assert_eq!(AppKey::Capture.hint(), "Ctrl+Shift+N");
        assert_eq!(AppKey::Settings.hint(), "Ctrl+,");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_keys() {
        let cmd = Modifiers::LOGO;
        let both = Modifiers::LOGO | Modifiers::SHIFT;
        assert_eq!(k("n", cmd), Some(AppKey::Capture));
        assert_eq!(k("A", both), Some(AppKey::Agenda));
        assert_eq!(k("Z", both), Some(AppKey::Redo));
        assert_eq!(k("n", Modifiers::CTRL), None);
        assert_eq!(AppKey::Capture.hint(), "⌘N");
    }

    #[test]
    fn editor_keeps_undo_and_paste() {
        assert!(AppKey::Capture.in_editor());
        assert!(!AppKey::Undo.in_editor());
        assert!(!AppKey::PasteImage.in_editor());
    }
}
