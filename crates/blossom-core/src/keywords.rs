//! TODO keyword configuration.

/// The TODO keyword sequence, in the org/Orgzly settings format
/// `"TODO NEXT | DONE CANCELLED"`: keywords after `|` are done-type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keywords {
    /// Not-done keywords, in cycling order.
    pub active: Vec<String>,
    /// Done-type keywords, in cycling order.
    pub done: Vec<String>,
}

impl Default for Keywords {
    /// Bloom's default: `TODO IN-PROGRESS | DONE CANCELLED`.
    fn default() -> Self {
        Keywords::parse("TODO IN-PROGRESS | DONE CANCELLED")
    }
}

impl Keywords {
    /// Parses `"TODO NEXT | DONE CANCELLED"`.
    ///
    /// Without a `|`, the last keyword is the only done-type one, as in Emacs
    /// (`org-todo-keywords`). Bloom treats every keyword as active in that case;
    /// we follow Emacs so a file shared with it marks the same headings done.
    #[must_use]
    pub fn parse(config: &str) -> Keywords {
        let words = |s: &str| -> Vec<String> { s.split_whitespace().map(String::from).collect() };
        if let Some((active, done)) = config.split_once('|') {
            Keywords {
                active: words(active),
                done: words(done),
            }
        } else {
            let mut active = words(config);
            let done = active.pop().into_iter().collect();
            Keywords { active, done }
        }
    }

    /// All keywords: active first, then done.
    pub fn all(&self) -> impl Iterator<Item = &str> {
        self.active.iter().chain(&self.done).map(String::as_str)
    }

    /// Whether `keyword` is one of the configured keywords.
    #[must_use]
    pub fn contains(&self, keyword: &str) -> bool {
        self.all().any(|k| k == keyword)
    }

    /// Whether `keyword` is a done-type keyword.
    #[must_use]
    pub fn is_done(&self, keyword: &str) -> bool {
        self.done.iter().any(|k| k == keyword)
    }

    /// The next state when cycling: none → active… → done… → none.
    /// An unknown keyword cycles to none.
    #[must_use]
    pub fn next(&self, current: Option<&str>) -> Option<String> {
        let all: Vec<&str> = self.all().collect();
        let next = match current {
            None => all.first(),
            Some(kw) => all
                .iter()
                .position(|k| *k == kw)
                .and_then(|i| all.get(i + 1)),
        };
        next.map(|k| (*k).to_string())
    }

    /// The parse configuration that makes orgize recognise these keywords.
    pub(crate) fn parse_config(&self) -> orgize::ParseConfig {
        orgize::ParseConfig {
            todo_keywords: (self.active.clone(), self.done.clone()),
            ..orgize::ParseConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_sides_of_the_bar() {
        let k = Keywords::parse("TODO NEXT | DONE CANCELLED");
        assert_eq!(k.active, ["TODO", "NEXT"]);
        assert_eq!(k.done, ["DONE", "CANCELLED"]);
        assert!(k.is_done("CANCELLED"));
        assert!(!k.is_done("NEXT"));
        assert!(k.contains("NEXT"));
        assert!(!k.contains("WAIT"));
    }

    #[test]
    fn without_bar_last_is_done() {
        let k = Keywords::parse("TODO WAIT DONE");
        assert_eq!(k.active, ["TODO", "WAIT"]);
        assert_eq!(k.done, ["DONE"]);
        assert_eq!(Keywords::parse("  ").all().count(), 0);
    }

    #[test]
    fn next_cycles_through_none() {
        let k = Keywords::parse("TODO | DONE");
        assert_eq!(k.next(None).as_deref(), Some("TODO"));
        assert_eq!(k.next(Some("TODO")).as_deref(), Some("DONE"));
        assert_eq!(k.next(Some("DONE")), None);
        assert_eq!(k.next(Some("WAT")), None);
        assert_eq!(Keywords::parse("").next(None), None);
    }

    #[test]
    fn default_is_blooms() {
        let k = Keywords::default();
        assert_eq!(k.active, ["TODO", "IN-PROGRESS"]);
        assert_eq!(k.done, ["DONE", "CANCELLED"]);
    }
}
