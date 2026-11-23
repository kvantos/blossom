//! `blossom-core` edits: one exact-output test per behaviour, plus a sweep
//! over every fixture checking that each edit keeps the document consistent
//! with a fresh parse and leaves every byte outside the heading untouched.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use blossom_core::{Document, EditError, InsertAt, Keywords, NewHeading, Timestamp};
use chrono::{NaiveDate, NaiveDateTime};

fn kw() -> Keywords {
    Keywords::parse("TODO NEXT | DONE CANCELLED")
}

fn doc(s: &str) -> Document {
    Document::parse(s, kw())
}

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, 23)
        .unwrap()
        .and_hms_opt(10, 0, 42)
        .unwrap()
}

fn ts(s: &str) -> Timestamp {
    Timestamp::parse(s).unwrap()
}

/// Applies `f` to `input` and returns the new text.
fn edit(input: &str, f: impl FnOnce(&mut Document) -> Result<(), EditError>) -> String {
    let mut d = doc(input);
    f(&mut d).unwrap();
    d.text().to_string()
}

const LOG: &str = "- State \"DONE\"       from \"TODO\"       [2026-11-23 Mon 10:00]";

// --- heading line ---

#[test]
fn keyword_insert_replace_remove() {
    assert_eq!(
        edit("* Title\n", |d| d.set_keyword(0, Some("TODO"))),
        "* TODO Title\n"
    );
    assert_eq!(
        edit("* TODO Title\n", |d| d.set_keyword(0, Some("NEXT"))),
        "* NEXT Title\n"
    );
    assert_eq!(
        edit("* TODO Title\n", |d| d.set_keyword(0, None)),
        "* Title\n"
    );
    assert_eq!(edit("* \n", |d| d.set_keyword(0, Some("TODO"))), "* TODO\n");
    assert_eq!(edit("* TODO\n", |d| d.set_keyword(0, None)), "* \n");
    assert_eq!(
        edit("** [#A] x\n", |d| d.set_keyword(0, Some("DONE"))),
        "** DONE [#A] x\n"
    );
}

#[test]
fn keyword_keeps_tag_alignment() {
    let input = "* Title                                      :tag:\n";
    assert_eq!(
        edit(input, |d| d.set_keyword(0, Some("TODO"))),
        "* TODO Title                                      :tag:\n"
    );
}

#[test]
fn unknown_keyword_is_refused() {
    let mut d = doc("* Title\n");
    assert_eq!(
        d.set_keyword(0, Some("WAIT")),
        Err(EditError::UnknownKeyword("WAIT".into()))
    );
    assert_eq!(d.set_keyword(5, None), Err(EditError::NoSuchHeading(5)));
    assert_eq!(d.text(), "* Title\n");
}

#[test]
fn priority() {
    assert_eq!(
        edit("* TODO Title\n", |d| d.set_priority(0, Some("A"))),
        "* TODO [#A] Title\n"
    );
    assert_eq!(
        edit("* Title\n", |d| d.set_priority(0, Some("B"))),
        "* [#B] Title\n"
    );
    assert_eq!(
        edit("* [#B] Title\n", |d| d.set_priority(0, Some("C"))),
        "* [#C] Title\n"
    );
    assert_eq!(
        edit("* TODO [#A] Title\n", |d| d.set_priority(0, None)),
        "* TODO Title\n"
    );
    let mut d = doc("* x\n");
    assert!(matches!(
        d.set_priority(0, Some("A B")),
        Err(EditError::InvalidPriority(_))
    ));
}

#[test]
fn tags() {
    let t = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    assert_eq!(
        edit("* Title\n", |d| d.set_tags(0, &t(&["a", "b"]))),
        "* Title  :a:b:\n"
    );
    assert_eq!(
        edit("* Title   \n", |d| d.set_tags(0, &t(&["a"]))),
        "* Title  :a:\n"
    );
    assert_eq!(
        edit("* Title     :x:\n", |d| d.set_tags(0, &t(&["a"]))),
        "* Title     :a:\n"
    );
    assert_eq!(
        edit("* Title     :x:y:\n", |d| d.set_tags(0, &[])),
        "* Title\n"
    );
    assert_eq!(edit("* :x:\n", |d| d.set_tags(0, &[])), "* \n");
    assert_eq!(edit("* \n", |d| d.set_tags(0, &t(&["a"]))), "* :a:\n");
    assert_eq!(
        edit("* Ünïcode\n", |d| d.set_tags(0, &t(&["日本", "a_b@#%-"]))),
        "* Ünïcode  :日本:a_b@#%-:\n"
    );
    let mut d = doc("* x\n");
    assert_eq!(
        d.set_tags(0, &t(&["a b"])),
        Err(EditError::InvalidTag("a b".into()))
    );
    assert_eq!(
        d.set_tags(0, &t(&["a:b"])),
        Err(EditError::InvalidTag("a:b".into()))
    );
}

// --- planning ---

#[test]
fn scheduled_and_deadline() {
    let s = ts("<2026-11-24 Tue>");
    assert_eq!(
        edit("* a\nbody\n", |d| d.set_scheduled(0, Some(&s))),
        "* a\nSCHEDULED: <2026-11-24 Tue>\nbody\n"
    );
    assert_eq!(
        edit("* a\nDEADLINE: <2026-12-01 Tue>\n", |d| d
            .set_scheduled(0, Some(&s))),
        "* a\nDEADLINE: <2026-12-01 Tue> SCHEDULED: <2026-11-24 Tue>\n"
    );
    assert_eq!(
        edit(
            "* a\nSCHEDULED: <2026-11-01 Sun> DEADLINE: <2026-12-01 Di>\n",
            |d| d.set_scheduled(0, Some(&s))
        ),
        "* a\nSCHEDULED: <2026-11-24 Tue> DEADLINE: <2026-12-01 Di>\n",
        "the untouched entry keeps its exact text"
    );
    assert_eq!(
        edit(
            "* a\nSCHEDULED: <2026-11-01 Sun> DEADLINE: <2026-12-01 Tue>\n",
            |d| d.set_deadline(0, None)
        ),
        "* a\nSCHEDULED: <2026-11-01 Sun>\n"
    );
    assert_eq!(
        edit("* a\nSCHEDULED: <2026-11-01 Sun>\nbody\n", |d| d
            .set_scheduled(0, None)),
        "* a\nbody\n"
    );
}

#[test]
fn planning_at_end_without_newline() {
    let s = ts("<2026-11-24 Tue>");
    assert_eq!(
        edit("* a", |d| d.set_scheduled(0, Some(&s))),
        "* a\nSCHEDULED: <2026-11-24 Tue>"
    );
    assert_eq!(
        edit("* a\nSCHEDULED: <2026-11-24 Tue>", |d| d
            .set_scheduled(0, None)),
        "* a"
    );
}

#[test]
fn planning_follows_file_indent_and_eol() {
    let s = ts("<2026-11-24 Tue>");
    // Emacs's org-adapt-indentation: level + 1 spaces.
    let input = "* a\n  DEADLINE: <2026-12-01 Tue>\n** b\n";
    assert_eq!(
        edit(input, |d| d.set_scheduled(1, Some(&s))),
        "* a\n  DEADLINE: <2026-12-01 Tue>\n** b\n   SCHEDULED: <2026-11-24 Tue>\n"
    );
    assert_eq!(
        edit("* a\r\nbody\r\n", |d| d.set_scheduled(0, Some(&s))),
        "* a\r\nSCHEDULED: <2026-11-24 Tue>\r\nbody\r\n"
    );
}

// --- todo state ---

#[test]
fn mark_done_plain() {
    assert_eq!(
        edit("* TODO Task\nbody\n", |d| d.mark_done(0, "DONE", now())),
        format!("* DONE Task\nCLOSED: [2026-11-23 Mon 10:00]\n:LOGBOOK:\n{LOG}\n:END:\nbody\n")
    );
}

#[test]
fn mark_done_logs_newest_first_after_properties() {
    let input = "* TODO Task\n:PROPERTIES:\n:ID: x\n:END:\n:LOGBOOK:\n- older\n:END:\n";
    assert_eq!(
        edit(input, |d| d.mark_done(0, "DONE", now())),
        format!(
            "* DONE Task\nCLOSED: [2026-11-23 Mon 10:00]\n:PROPERTIES:\n:ID: x\n:END:\n:LOGBOOK:\n{LOG}\n- older\n:END:\n"
        )
    );
}

#[test]
fn mark_done_repeating() {
    let input =
        "* TODO Water plants\nSCHEDULED: <2026-11-20 Fri +1w>\n:PROPERTIES:\n:ID: x\n:END:\n";
    assert_eq!(
        edit(input, |d| d.mark_done(0, "DONE", now())),
        format!(
            "* TODO Water plants\nSCHEDULED: <2026-11-27 Fri +1w>\n:PROPERTIES:\n:ID: x\n\
             :LAST_REPEAT: [2026-11-23 Mon 10:00]\n:END:\n:LOGBOOK:\n{LOG}\n:END:\n"
        )
    );
}

#[test]
fn mark_done_repeating_details() {
    // NEXT returns to the first active keyword; a body stamp's repeater counts;
    // a non-repeating SCHEDULED and an old CLOSED are dropped.
    let input = "* NEXT Pay rent\nSCHEDULED: <2026-11-20 Fri> CLOSED: [2026-10-01 Thu]\nOn <2026-11-01 Sun ++1m>.\n";
    let out = edit(input, |d| d.mark_done(0, "DONE", now()));
    assert!(
        out.starts_with("* TODO Pay rent\n:PROPERTIES:\n:LAST_REPEAT:"),
        "{out}"
    );
    assert!(out.contains("On <2026-12-01 Tue ++1m>."), "{out}");
    assert!(
        !out.contains("SCHEDULED") && !out.contains("CLOSED"),
        "{out}"
    );
    assert!(out.contains("- State \"DONE\"       from \"NEXT\"       [2026-11-23 Mon 10:00]"));
    // REPEAT_TO_STATE wins.
    let input =
        "* TODO x\nSCHEDULED: <2026-11-20 Fri .+1d>\n:PROPERTIES:\n:REPEAT_TO_STATE: NEXT\n:END:\n";
    let out = edit(input, |d| d.mark_done(0, "DONE", now()));
    assert!(
        out.starts_with("* NEXT x\nSCHEDULED: <2026-11-24 Tue .+1d>\n"),
        "{out}"
    );
}

#[test]
fn done_to_done_only_refreshes_closed() {
    let input = "* DONE Task\nCLOSED: [2026-01-01 Thu 09:00]\n";
    assert_eq!(
        edit(input, |d| d.mark_done(0, "CANCELLED", now())),
        "* CANCELLED Task\nCLOSED: [2026-11-23 Mon 10:00]\n"
    );
    let mut d = doc(input);
    assert_eq!(
        d.mark_done(0, "TODO", now()),
        Err(EditError::UnknownKeyword("TODO".into()))
    );
}

#[test]
fn change_keyword_routes() {
    let input = "* DONE Task\nCLOSED: [2026-01-01 Thu 09:00]\nbody\n";
    assert_eq!(
        edit(input, |d| d.change_keyword(0, Some("TODO"), now())),
        "* TODO Task\nbody\n"
    );
    assert_eq!(
        edit(input, |d| d.change_keyword(0, None, now())),
        "* Task\nbody\n"
    );
    assert_eq!(
        edit("* TODO a\n", |d| d.change_keyword(0, Some("NEXT"), now())),
        "* NEXT a\n"
    );
    let out = edit("* TODO a\n", |d| d.change_keyword(0, Some("DONE"), now()));
    assert!(out.starts_with("* DONE a\nCLOSED: [2026-11-23 Mon 10:00]\n:LOGBOOK:\n"));
}

// --- drawers ---

#[test]
fn upsert_property() {
    assert_eq!(
        edit("* a\n:PROPERTIES:\n:Effort: 1:00\n:END:\n", |d| d
            .upsert_property(0, "EFFORT", "2:00")),
        "* a\n:PROPERTIES:\n:EFFORT: 2:00\n:END:\n"
    );
    assert_eq!(
        edit("* a\n  :PROPERTIES:\n  :ID: x\n  :END:\n", |d| d
            .upsert_property(0, "B", "1")),
        "* a\n  :PROPERTIES:\n  :ID: x\n  :B: 1\n  :END:\n"
    );
    assert_eq!(
        edit("* a\nSCHEDULED: <2026-11-24 Tue>\nbody\n", |d| d
            .upsert_property(0, "ID", "x")),
        "* a\nSCHEDULED: <2026-11-24 Tue>\n:PROPERTIES:\n:ID: x\n:END:\nbody\n"
    );
    let mut d = doc("* a\n");
    assert!(matches!(
        d.upsert_property(0, "a b", "1"),
        Err(EditError::InvalidPropertyKey(_))
    ));
    assert_eq!(
        d.upsert_property(0, "A", "1\n2"),
        Err(EditError::MultilineText)
    );
}

// --- checkboxes ---

#[test]
fn checkbox_toggle_updates_parent() {
    let input = "* a\n- [ ] Pack [1/3]\n  - [X] socks\n  - [ ] shirt\n  - [ ] hat\nafter\n";
    let at = input.find("shirt").unwrap();
    let out = edit(input, |d| d.toggle_checkbox(at));
    assert_eq!(
        out,
        "* a\n- [ ] Pack [2/3]\n  - [X] socks\n  - [X] shirt\n  - [ ] hat\nafter\n"
    );
    let at = out.find("hat").unwrap();
    let out = edit(&out, |d| d.toggle_checkbox(at));
    assert_eq!(
        out,
        "* a\n- [X] Pack [3/3]\n  - [X] socks\n  - [X] shirt\n  - [X] hat\nafter\n"
    );
    let at = out.find("socks").unwrap();
    assert_eq!(
        edit(&out, |d| d.toggle_checkbox(at)),
        "* a\n- [ ] Pack [2/3]\n  - [ ] socks\n  - [X] shirt\n  - [X] hat\nafter\n"
    );
}

#[test]
fn checkbox_percent_and_plain() {
    let input = "- Trip [0%]\n  1. [ ] a\n  2. [x] b\n  3) [-] c\n";
    let at = input.find("] a").unwrap();
    assert_eq!(
        edit(input, |d| d.toggle_checkbox(at)),
        "- Trip [67%]\n  1. [X] a\n  2. [x] b\n  3) [-] c\n"
    );
    assert_eq!(edit("- [-] x", |d| d.toggle_checkbox(0)), "- [X] x");
    assert_eq!(edit("- [X]", |d| d.toggle_checkbox(0)), "- [ ]");
    let mut d = doc("- not a box\ntext\n");
    assert_eq!(d.toggle_checkbox(0), Err(EditError::NotACheckbox));
    assert_eq!(d.toggle_checkbox(13), Err(EditError::NotACheckbox));
    assert_eq!(d.toggle_checkbox(999), Err(EditError::NotACheckbox));
}

// --- structure ---

const TREE: &str = "#+TITLE: t\n* a\n** a1\n* b\nbody b\n** b1\n* c\n";

#[test]
fn move_subtree() {
    let mut d = doc(TREE);
    assert_eq!(d.move_subtree(2, true), Ok(0));
    assert_eq!(
        d.text(),
        "#+TITLE: t\n* b\nbody b\n** b1\n* a\n** a1\n* c\n"
    );
    assert_eq!(d.move_subtree(0, false), Ok(2));
    assert_eq!(d.text(), TREE);
    assert_eq!(d.move_subtree(0, true), Err(EditError::NoSibling));
    assert_eq!(d.move_subtree(1, false), Err(EditError::NoSibling));
    assert_eq!(
        edit("* a\n* b", |d| d.move_subtree(1, true).map(drop)),
        "* b\n* a"
    );
    assert_eq!(
        edit("* a\r\n* b", |d| d.move_subtree(0, false).map(drop)),
        "* b\r\n* a"
    );
}

#[test]
fn promote_and_demote() {
    assert_eq!(
        edit(TREE, |d| d.demote(2)),
        "#+TITLE: t\n* a\n** a1\n** b\nbody b\n*** b1\n* c\n"
    );
    assert_eq!(
        edit(TREE, |d| d.promote(3)),
        "#+TITLE: t\n* a\n** a1\n* b\nbody b\n* b1\n* c\n"
    );
    let mut d = doc(TREE);
    assert_eq!(d.demote(0), Err(EditError::NoPreviousSibling));
    assert_eq!(d.demote(1), Err(EditError::NoPreviousSibling));
    assert_eq!(d.promote(0), Err(EditError::AlreadyTopLevel));
    // Stars inside a block are not headings and are left alone.
    let input = "* a\n* b\n#+begin_example\n*bold* line\n#+end_example\n";
    assert_eq!(
        edit(input, |d| d.demote(1)),
        "* a\n** b\n#+begin_example\n*bold* line\n#+end_example\n"
    );
}

#[test]
fn delete_subtree() {
    assert_eq!(
        edit(TREE, |d| d.delete_subtree(2)),
        "#+TITLE: t\n* a\n** a1\n* c\n"
    );
    assert_eq!(edit("* a\n* b", |d| d.delete_subtree(1)), "* a");
    assert_eq!(edit("* a\n", |d| d.delete_subtree(0)), "");
}

#[test]
fn refile_within_file() {
    let mut d = doc(TREE);
    assert_eq!(d.refile(0, Some(4)), Ok(3));
    assert_eq!(
        d.text(),
        "#+TITLE: t\n* b\nbody b\n** b1\n* c\n** a\n*** a1\n"
    );
    let mut d = doc(TREE);
    assert_eq!(d.refile(4, Some(0)), Ok(2));
    assert_eq!(
        d.text(),
        "#+TITLE: t\n* a\n** a1\n** c\n* b\nbody b\n** b1\n"
    );
    let mut d = doc(TREE);
    assert_eq!(d.refile(3, None), Ok(4));
    assert_eq!(d.text(), "#+TITLE: t\n* a\n** a1\n* b\nbody b\n* c\n* b1\n");
    let mut d = doc(TREE);
    assert_eq!(d.refile(2, Some(3)), Err(EditError::TargetInsideSubtree));
    assert_eq!(d.refile(2, Some(2)), Err(EditError::TargetInsideSubtree));
    assert_eq!(d.text(), TREE);
    assert_eq!(
        edit("* a\n* b", |d| d.refile(1, Some(0)).map(drop)),
        "* a\n** b"
    );
}

#[test]
fn insert_subtree_from_other_file() {
    let other = doc("*** x\nbody\n**** y\n");
    let mut d = doc(TREE);
    assert_eq!(
        d.insert_subtree(InsertAt::LastChild(0), other.subtree_text(0)),
        Ok(2)
    );
    assert_eq!(
        d.text(),
        "#+TITLE: t\n* a\n** a1\n** x\nbody\n*** y\n* b\nbody b\n** b1\n* c\n"
    );
    assert_eq!(
        d.insert_subtree(InsertAt::LastTopLevel, "no heading"),
        Err(EditError::NotASubtree)
    );
}

#[test]
fn insert_heading_positions() {
    let new = |t: &str| NewHeading {
        title: t.to_string(),
        ..NewHeading::default()
    };
    let cases = [
        (
            InsertAt::FirstChild(2),
            "#+TITLE: t\n* a\n** a1\n* b\nbody b\n** N\n** b1\n* c\n",
            3,
        ),
        (
            InsertAt::LastChild(2),
            "#+TITLE: t\n* a\n** a1\n* b\nbody b\n** b1\n** N\n* c\n",
            4,
        ),
        (
            InsertAt::Before(2),
            "#+TITLE: t\n* a\n** a1\n* N\n* b\nbody b\n** b1\n* c\n",
            2,
        ),
        (
            InsertAt::After(0),
            "#+TITLE: t\n* a\n** a1\n* N\n* b\nbody b\n** b1\n* c\n",
            2,
        ),
        (
            InsertAt::FirstTopLevel,
            "#+TITLE: t\n* N\n* a\n** a1\n* b\nbody b\n** b1\n* c\n",
            0,
        ),
        (
            InsertAt::LastTopLevel,
            "#+TITLE: t\n* a\n** a1\n* b\nbody b\n** b1\n* c\n* N\n",
            5,
        ),
    ];
    for (at, want, index) in cases {
        let mut d = doc(TREE);
        assert_eq!(d.insert_heading(at, &new("N")), Ok(index), "{at:?}");
        assert_eq!(d.text(), want, "{at:?}");
    }
    let mut d = doc("text");
    assert_eq!(d.insert_heading(InsertAt::LastTopLevel, &new("")), Ok(0));
    assert_eq!(d.text(), "text\n* ");
    assert_eq!(
        d.insert_heading(InsertAt::LastTopLevel, &new("x\ny")),
        Err(EditError::MultilineText)
    );
}

#[test]
fn insert_heading_with_options() {
    let full = NewHeading {
        title: "Call Bob".into(),
        keyword: Some("TODO".into()),
        id: Some("abc".into()),
        created: Some(now()),
        body: Some("Line 1\nLine 2\n".into()),
    };
    assert_eq!(
        edit("\n\n", |d| d
            .insert_heading(InsertAt::LastTopLevel, &full)
            .map(drop)),
        "* TODO Call Bob\n:PROPERTIES:\n:ID: abc\n:CREATED: [2026-11-23 Mon 10:00]\n:END:\nLine 1\nLine 2\n"
    );
    assert_eq!(
        edit("* a\r\n", |d| d
            .insert_heading(
                InsertAt::LastChild(0),
                &NewHeading {
                    title: "b".into(),
                    body: Some("x".into()),
                    ..NewHeading::default()
                }
            )
            .map(drop)),
        "* a\r\n** b\r\nx\r\n"
    );
    let mut d = doc("* a\n");
    let bad = NewHeading {
        keyword: Some("WAIT".into()),
        ..NewHeading::default()
    };
    assert!(matches!(
        d.insert_heading(InsertAt::LastTopLevel, &bad),
        Err(EditError::UnknownKeyword(_))
    ));
}

// --- sweep over all fixtures ---

/// One edit to run on a heading, with how far its changes may reach.
struct Case {
    name: &'static str,
    /// The bytes that may change: `true` = the heading's whole subtree,
    /// `false` = anywhere (moves and refiles).
    local: bool,
    run: fn(&mut Document, usize) -> Result<(), EditError>,
}

// A table of cases; one entry per edit.
#[allow(clippy::too_many_lines)]
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "keyword",
            local: true,
            run: |d, i| d.set_keyword(i, Some("NEXT")),
        },
        Case {
            name: "no keyword",
            local: true,
            run: |d, i| d.set_keyword(i, None),
        },
        Case {
            name: "priority",
            local: true,
            run: |d, i| d.set_priority(i, Some("B")),
        },
        Case {
            name: "no priority",
            local: true,
            run: |d, i| d.set_priority(i, None),
        },
        Case {
            name: "tags",
            local: true,
            run: |d, i| d.set_tags(i, &["t1".into(), "t2".into()]),
        },
        Case {
            name: "no tags",
            local: true,
            run: |d, i| d.set_tags(i, &[]),
        },
        Case {
            name: "scheduled",
            local: true,
            run: |d, i| d.set_scheduled(i, Some(&ts("<2026-11-24 Tue>"))),
        },
        Case {
            name: "no scheduled",
            local: true,
            run: |d, i| d.set_scheduled(i, None),
        },
        Case {
            name: "deadline",
            local: true,
            run: |d, i| d.set_deadline(i, Some(&ts("<2026-11-24 Tue +1w -2d>"))),
        },
        Case {
            name: "property",
            local: true,
            run: |d, i| d.upsert_property(i, "ID", "sweep"),
        },
        Case {
            name: "logbook",
            local: true,
            run: |d, i| d.append_logbook_entry(i, "- sweep"),
        },
        Case {
            name: "done",
            local: true,
            run: |d, i| d.mark_done(i, "DONE", now()),
        },
        Case {
            name: "change",
            local: true,
            run: |d, i| d.change_keyword(i, Some("TODO"), now()),
        },
        Case {
            name: "promote",
            local: true,
            run: |d, i| d.promote(i),
        },
        Case {
            name: "demote",
            local: true,
            run: |d, i| d.demote(i),
        },
        Case {
            name: "delete",
            local: true,
            run: |d, i| d.delete_subtree(i),
        },
        Case {
            name: "up",
            local: false,
            run: |d, i| d.move_subtree(i, true).map(drop),
        },
        Case {
            name: "down",
            local: false,
            run: |d, i| d.move_subtree(i, false).map(drop),
        },
        Case {
            name: "refile top",
            local: false,
            run: |d, i| d.refile(i, None).map(drop),
        },
        Case {
            name: "refile first",
            local: false,
            run: |d, i| d.refile(i, Some(0)).map(drop),
        },
        Case {
            name: "child",
            local: true,
            run: |d, i| {
                d.insert_heading(InsertAt::LastChild(i), &NewHeading::default())
                    .map(drop)
            },
        },
        Case {
            name: "first child",
            local: true,
            run: |d, i| {
                d.insert_heading(InsertAt::FirstChild(i), &NewHeading::default())
                    .map(drop)
            },
        },
        Case {
            name: "repeat",
            local: true,
            run: |d, i| {
                d.set_scheduled(i, Some(&ts("<2026-11-24 Tue 09:00 +1d>")))?;
                d.mark_done(i, "DONE", now())
            },
        },
        Case {
            name: "both",
            local: true,
            run: |d, i| {
                d.set_deadline(i, Some(&ts("<2026-12-24 Thu>")))?;
                d.set_scheduled(i, Some(&ts("<2026-11-24 Tue 09:00 +1d>")))
            },
        },
    ]
}

/// Whether `after` is `before` with only bytes inside `range` replaced.
fn only_changed_within(before: &str, after: &str, range: std::ops::Range<usize>) -> bool {
    let (head, tail) = (&before[..range.start], &before[range.end..]);
    after.len() >= head.len() + tail.len() && after.starts_with(head) && after.ends_with(tail)
}

#[test]
fn every_edit_on_every_fixture_heading() {
    const MAX_HEADINGS: usize = 25;
    let mut failures = Vec::new();
    let mut ran = 0usize;
    for fixture in common::fixtures() {
        let base = doc(&fixture.text);
        let count = base.headings().len().min(MAX_HEADINGS);
        for i in 0..count {
            let h = &base.headings()[i];
            // Removing a final line may take the line ending before it.
            let mut start = h.spans.line.start.saturating_sub(2);
            while !fixture.text.is_char_boundary(start) {
                start += 1;
            }
            let allowed = start..h.spans.subtree_end;
            for case in cases() {
                let mut d = doc(&fixture.text);
                let what = format!("{} heading {i} {}", fixture.path.display(), case.name);
                let result = (case.run)(&mut d, i);
                ran += 1;
                if result.is_err() {
                    if d.text() != fixture.text {
                        failures.push(format!("{what}: error {result:?} but text changed"));
                    }
                    continue;
                }
                let fresh = doc(d.text());
                if d.org().green() != fresh.org().green() {
                    failures.push(format!("{what}: tree differs from a fresh parse"));
                } else if d.headings() != fresh.headings() || d.preamble() != fresh.preamble() {
                    failures.push(format!("{what}: index differs from a fresh parse"));
                }
                if case.local && !only_changed_within(&fixture.text, d.text(), allowed.clone()) {
                    failures.push(format!(
                        "{what}: bytes outside the heading's {allowed:?} changed"
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {ran} edits failed:\n{}",
        failures.len(),
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
