//! Property tests over generated org documents: lossless round-trip, and
//! `replace_range` agreeing with a naive splice plus fresh parse.

// Test helpers may unwrap (docs/PLAN.md section 2).
#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use orgize::{Org, TextRange, TextSize};
use proptest::prelude::*;

/// One org-ish line, without its line ending.
fn line() -> impl Strategy<Value = String> {
    let word = "[a-zA-Z0-9éß日本🌸]{1,8}";
    let words = proptest::collection::vec(word, 0..4).prop_map(|w| w.join(" "));
    prop_oneof![
        // Headings: stars, optional keyword, priority, title, tags.
        (
            1usize..5,
            prop::option::of(prop_oneof![Just("TODO"), Just("DONE"), Just("NEXT")]),
            prop::option::of(prop_oneof![Just("[#A]"), Just("[#C]")]),
            words.clone(),
            prop::option::of(prop_oneof![Just(":a:"), Just(":a:b:"), Just(":é:")]),
        )
            .prop_map(|(n, kw, pri, title, tags)| {
                let mut s = "*".repeat(n);
                for part in [kw, pri, Some(title.as_str()), tags].into_iter().flatten() {
                    s.push(' ');
                    s.push_str(part);
                }
                s
            }),
        // Fixed structural lines.
        prop_oneof![
            Just("SCHEDULED: <2026-11-21 Sat +1w>"),
            Just("DEADLINE: <2026-11-30 Mon -3d> CLOSED: [2026-11-20 Fri 10:00]"),
            Just(":PROPERTIES:"),
            Just(":ID: abc-123"),
            Just(":END:"),
            Just(":LOGBOOK:"),
            Just("- State \"DONE\" from \"TODO\" [2026-11-20 Fri 10:00]"),
            Just("#+TITLE: Generated"),
            Just("#+BEGIN_SRC rust"),
            Just("#+END_SRC"),
            Just("#+begin_quote"),
            Just("#+end_quote"),
            Just("| a | b |"),
            Just("|---+---|"),
            Just("<2026-11-21 Sat 10:00-11:00>"),
            Just("*bold* start"),
            Just(""),
            Just("   "),
            Just("\t"),
        ]
        .prop_map(str::to_string),
        // List items and plain text.
        (
            0usize..3,
            prop_oneof![Just("- "), Just("- [ ] "), Just("- [X] "), Just("1. ")],
            words.clone()
        )
            .prop_map(|(indent, bullet, w)| format!("{}{bullet}{w}", "  ".repeat(indent))),
        words,
    ]
}

/// A whole document: lines joined by `\n` or `\r\n`, optional final newline.
fn document() -> impl Strategy<Value = String> {
    (
        proptest::collection::vec(
            (line(), prop_oneof![3 => Just("\n"), 1 => Just("\r\n")]),
            0..25,
        ),
        any::<bool>(),
    )
        .prop_map(|(lines, trailing)| {
            let mut s = String::new();
            let n = lines.len();
            for (i, (l, eol)) in lines.into_iter().enumerate() {
                s.push_str(&l);
                if i + 1 < n || trailing {
                    s.push_str(eol);
                }
            }
            s
        })
}

/// Replacement text: a few generated lines, possibly unterminated.
fn replacement() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("\n".to_string()),
        Just("* ".to_string()),
        "[a-z ]{1,5}",
        document(),
    ]
}

/// Snaps `i` down to the nearest char boundary of `s`.
fn snap(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000)
}

fn size(n: usize) -> TextSize {
    TextSize::from(u32::try_from(n).unwrap())
}

proptest! {
    // 2000 cases by default; set PROPTEST_CASES for longer runs.
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    #[test]
    fn round_trips(doc in document()) {
        let out = Org::parse(&doc).to_org();
        prop_assert!(out == doc, "{}", common::describe_diff(&doc, &out).unwrap_or_default());
    }

    #[test]
    fn splice_matches_fresh_parse(
        doc in document(),
        a in any::<prop::sample::Index>(),
        b in any::<prop::sample::Index>(),
        with in replacement(),
    ) {
        let (a, b) = (snap(&doc, a.index(doc.len() + 1)), snap(&doc, b.index(doc.len() + 1)));
        let (start, end) = (a.min(b), a.max(b));
        let expected = format!("{}{with}{}", &doc[..start], &doc[end..]);

        let mut org = Org::parse(&doc);
        org.replace_range(TextRange::new(size(start), size(end)), &with);
        let got = org.to_org();
        prop_assert!(got == expected, "text: {}", common::describe_diff(&expected, &got).unwrap_or_default());

        let fresh = Org::parse(&expected);
        prop_assert!(
            org.green() == fresh.green(),
            "tree differs:\nwant {}\ngot  {}",
            common::tree(&fresh),
            common::tree(&org)
        );
    }
}
