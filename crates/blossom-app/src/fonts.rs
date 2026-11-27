//! Installed fonts for the Settings panel, and fonts by name.

use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fmt;
use std::sync::{Mutex, OnceLock, PoisonError};

use iced::Font;

/// The font families installed on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installed {
    /// Every family, sorted.
    pub all: Vec<String>,
    /// The monospaced ones, sorted.
    pub mono: Vec<String>,
}

impl Installed {
    /// The choices for the read view: Default, then every family.
    #[must_use]
    pub fn for_reading(&self) -> Vec<Choice> {
        std::iter::once(Choice(None))
            .chain(self.all.iter().map(|f| Choice(Some(f.clone()))))
            .collect()
    }

    /// The choices for the editor: Default, the monospaced families, then
    /// the rest.
    #[must_use]
    pub fn for_editing(&self) -> Vec<Choice> {
        let rest = self.all.iter().filter(|f| !self.mono.contains(f));
        std::iter::once(Choice(None))
            .chain(
                self.mono
                    .iter()
                    .chain(rest)
                    .map(|f| Choice(Some(f.clone()))),
            )
            .collect()
    }
}

/// Lists the installed families (slow: reads every font file's header).
#[must_use]
pub fn installed() -> Installed {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut all = BTreeSet::new();
    let mut mono = BTreeSet::new();
    for face in db.faces() {
        let Some((family, _)) = face.families.first() else {
            continue;
        };
        if family.is_empty() || family.starts_with('.') {
            continue; // macOS's hidden system faces
        }
        all.insert(family.clone());
        if face.monospaced {
            mono.insert(family.clone());
        }
    }
    Installed {
        all: all.into_iter().collect(),
        mono: mono.into_iter().collect(),
    }
}

/// A font choice in a dropdown: `None` is the app's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice(pub Option<String>);

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_deref().unwrap_or("Default"))
    }
}

/// The font named `name`, or `default` for none.
#[must_use]
pub fn named(name: Option<&str>, default: Font) -> Font {
    match name {
        Some(n) => Font {
            family: iced::font::Family::Name(intern(n)),
            ..default
        },
        None => default,
    }
}

/// iced wants `'static` family names; each distinct name is leaked once.
fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(n) = names.get(name) {
        return n;
    }
    let n: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(n);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_put_default_first_and_monospace_first_for_editing() {
        let i = Installed {
            all: vec!["Fira Code".into(), "Inter".into(), "Mono X".into()],
            mono: vec!["Fira Code".into(), "Mono X".into()],
        };
        let names = |v: Vec<Choice>| v.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            names(i.for_editing()),
            ["Default", "Fira Code", "Mono X", "Inter"]
        );
        assert_eq!(
            names(i.for_reading()),
            ["Default", "Fira Code", "Inter", "Mono X"]
        );
    }

    #[test]
    fn installed_fonts_are_sorted_and_monospace_is_a_subset() {
        // Whatever this machine has (CI may have few fonts).
        let i = installed();
        assert!(i.all.is_sorted());
        assert!(i.mono.is_sorted());
        assert!(i.mono.iter().all(|m| i.all.contains(m)));
    }

    #[test]
    fn named_fonts_keep_the_default_weight_and_share_names() {
        assert_eq!(named(None, Font::MONOSPACE), Font::MONOSPACE);
        let a = named(Some("Inter"), Font::DEFAULT);
        let b = named(Some(&String::from("Inter")), Font::DEFAULT);
        assert_eq!(a, b);
        assert_eq!(a.family, iced::font::Family::Name("Inter"));
        let (iced::font::Family::Name(x), iced::font::Family::Name(y)) = (a.family, b.family)
        else {
            panic!("named");
        };
        assert!(std::ptr::eq(x, y), "interned once");
    }
}
