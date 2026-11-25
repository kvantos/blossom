//! Index and query behaviour on real vaults.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::Path;

use blossom_core::Keywords;
use blossom_index::{AgendaReason, MatchContext, NoteIndex, Query, agenda, filter_and_sort};
use blossom_vault::Vault;
use chrono::{NaiveDate, NaiveDateTime};

fn now() -> NaiveDateTime {
    // Monday 2026-11-23, 10:30.
    NaiveDate::from_ymd_opt(2026, 11, 23)
        .unwrap()
        .and_hms_opt(10, 30, 0)
        .unwrap()
}

fn keywords() -> Keywords {
    Keywords::parse("TODO NEXT | DONE CANCELLED")
}

fn ctx() -> MatchContext {
    MatchContext {
        now: now(),
        state_order: keywords().all().map(String::from).collect(),
        default_priority: None,
    }
}

fn put(root: &Path, path: &str, text: &str) {
    let full = root.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}

const WORK: &str = "\
#+FILETAGS: :work:
* TODO [#A] Call the committee about budget :phone:
SCHEDULED: <2026-11-23 Mon 09:00>
* NEXT Write report
DEADLINE: <2026-11-25 Wed>
:PROPERTIES:
:ID: abc-123
:CREATED: [2026-11-01 Sun 08:00]
:END:
Draft copy goes here.
* DONE Old task
CLOSED: [2026-11-23 Mon 08:00]
* Meeting notes
<2026-11-24 Tue 14:00-15:00>
** Weekly sync :someday:
<2026-11-16 Mon 10:00 +1w>
";

const HOME: &str = "\
* TODO Buy milk
SCHEDULED: <2026-11-20 Fri>
* TODO [#C] Fix bike :someday:
* Trip
<2026-11-27 Fri>--<2026-11-29 Sun>
";

fn sample() -> (tempfile::TempDir, Vault, NoteIndex) {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "work.org", WORK);
    put(dir.path(), "home/home.org", HOME);
    let vault = Vault::open(dir.path(), keywords(), &[]).unwrap();
    let mut index = NoteIndex::in_memory().unwrap();
    index.sync(&vault).unwrap();
    (dir, vault, index)
}

fn titles(index: &NoteIndex, q: &str) -> Vec<String> {
    index
        .search(&Query::parse(q), &ctx())
        .unwrap()
        .into_iter()
        .map(|n| n.record.title)
        .collect()
}

#[test]
fn text_terms_are_substrings_of_title_body_and_own_tags() {
    let (_d, _v, index) = sample();
    assert_eq!(titles(&index, "meet"), ["Meeting notes"]);
    assert_eq!(
        titles(&index, "mitt"),
        ["Call the committee about budget"],
        "inside a word"
    );
    assert_eq!(titles(&index, "DRAFT"), ["Write report"]);
    assert_eq!(titles(&index, "\"draft copy\""), ["Write report"]);
    assert_eq!(
        titles(&index, "pho"),
        ["Call the committee about budget"],
        "own tag"
    );
    assert_eq!(
        titles(&index, "work"),
        Vec::<String>::new(),
        "file tags are not text"
    );
    assert_eq!(
        titles(&index, "mi"),
        ["Buy milk", "Call the committee about budget"],
        "short terms scan"
    );
}

#[test]
fn filters() {
    let (_d, _v, index) = sample();
    // Default order: notebook, then priority (none last), then position.
    assert_eq!(
        titles(&index, "i.todo"),
        ["Fix bike", "Buy milk", "Call the committee about budget"]
    );
    assert_eq!(titles(&index, "it.done"), ["Old task"]);
    assert_eq!(
        titles(&index, "i.none b.work"),
        ["Meeting notes", "Weekly sync"]
    );
    assert_eq!(
        titles(&index, "t.work .t.someday it.todo"),
        ["Call the committee about budget", "Write report"]
    );
    assert_eq!(titles(&index, "tn.someday"), ["Fix bike", "Weekly sync"]);
    assert_eq!(titles(&index, "p.a"), ["Call the committee about budget"]);
    assert_eq!(
        titles(&index, "b.home i.todo .(p.c OR s.none)"),
        ["Buy milk"]
    );
    assert_eq!(titles(&index, "id.ABC"), ["Write report"]);
    assert_eq!(
        titles(&index, "b.home/home"),
        ["Fix bike", "Buy milk", "Trip"]
    );
}

#[test]
fn dates() {
    let (_d, _v, index) = sample();
    assert_eq!(
        titles(&index, "s.today"),
        ["Buy milk", "Call the committee about budget"],
        "le by default"
    );
    assert_eq!(
        titles(&index, "s.eq.today"),
        ["Call the committee about budget"]
    );
    assert_eq!(titles(&index, "s.overdue"), ["Buy milk"]);
    assert_eq!(
        titles(&index, "s.ge.now"),
        Vec::<String>::new(),
        "09:00 is past at 10:30"
    );
    assert_eq!(
        titles(&index, "s.le.now"),
        ["Buy milk", "Call the committee about budget"]
    );
    assert_eq!(titles(&index, "d.2d"), ["Write report"]);
    assert_eq!(titles(&index, "d.1d"), Vec::<String>::new());
    assert_eq!(titles(&index, "c.today"), ["Old task"]);
    assert_eq!(titles(&index, "cr.ge.-1m"), ["Write report"]);
    assert_eq!(titles(&index, "e.tomorrow"), ["Meeting notes"]);
    assert_eq!(
        titles(&index, "e.eq.2026-11-28"),
        ["Trip"],
        "ranges count every day"
    );
    assert_eq!(titles(&index, "e.ge.now b.work"), ["Meeting notes"]);
}

#[test]
fn sorting() {
    let (_d, _v, index) = sample();
    assert_eq!(
        titles(&index, "it.todo o.p"),
        [
            "Call the committee about budget",
            "Fix bike",
            "Buy milk",
            "Write report"
        ],
        "missing priority sorts last"
    );
    assert_eq!(
        titles(&index, "it.todo .o.title"),
        [
            "Write report",
            "Fix bike",
            "Call the committee about budget",
            "Buy milk"
        ]
    );
    assert_eq!(
        titles(&index, "it.todo o.st o.t"),
        [
            "Buy milk",
            "Call the committee about budget",
            "Fix bike",
            "Write report"
        ]
    );
}

/// An agenda section as (date, [(title, reason)]).
type Section = (Option<String>, Vec<(String, AgendaReason)>);

#[test]
fn agenda_view() {
    use AgendaReason::{Deadline, Event, Scheduled};
    let (_d, _v, index) = sample();
    let q = Query::parse("ad.7");
    let hits = index.search(&q, &ctx()).unwrap();
    let days = agenda(&hits, now().date(), 7);
    let rendered: Vec<Section> = days
        .iter()
        .map(|d| {
            (
                d.date.map(|x| x.to_string()),
                d.items
                    .iter()
                    .map(|i| (i.note.record.title.clone(), i.reason))
                    .collect(),
            )
        })
        .collect();
    let s = |x: &str| x.to_string();
    assert_eq!(
        rendered,
        [
            (None, vec![(s("Buy milk"), Scheduled)]),
            (
                Some(s("2026-11-23")),
                vec![
                    (s("Call the committee about budget"), Scheduled),
                    (s("Weekly sync"), Event)
                ]
            ),
            (Some(s("2026-11-24")), vec![(s("Meeting notes"), Event)]),
            (Some(s("2026-11-25")), vec![(s("Write report"), Deadline)]),
            (Some(s("2026-11-27")), vec![(s("Trip"), Event)]),
            (Some(s("2026-11-28")), vec![(s("Trip"), Event)]),
            (Some(s("2026-11-29")), vec![(s("Trip"), Event)]),
        ]
    );
}

#[test]
fn sync_tracks_revisions() {
    let (dir, vault, mut index) = sample();
    let again = index.sync(&vault).unwrap();
    assert!(again.added.is_empty() && again.updated.is_empty() && again.removed.is_empty());
    std::thread::sleep(std::time::Duration::from_millis(20));
    put(dir.path(), "work.org", "* Only one\n");
    fs::remove_file(dir.path().join("home/home.org")).unwrap();
    put(dir.path(), "new.org", "* Fresh\n");
    put(
        dir.path(),
        "new.sync-conflict-20261122-101010-ABC.org",
        "* Fresh'\n",
    );
    let r = index.sync(&vault).unwrap();
    assert_eq!(r.added, ["new.org"]);
    assert_eq!(r.updated, ["work.org"]);
    assert_eq!(r.removed, ["home/home.org"]);
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(titles(&index, ""), ["Fresh", "Only one"]);
}

#[test]
fn on_disk_index_persists_and_rebuilds_on_settings_change() {
    let (_d, vault, _) = sample();
    let cache = tempfile::tempdir().unwrap();
    let dir = cache.path().join("idx");
    {
        let mut index = NoteIndex::open(&dir, "TODO NEXT | DONE CANCELLED").unwrap();
        assert_eq!(index.sync(&vault).unwrap().added.len(), 2);
    }
    {
        let mut index = NoteIndex::open(&dir, "TODO NEXT | DONE CANCELLED").unwrap();
        assert_eq!(index.all().unwrap().len(), 8, "reopened");
        assert_eq!(index.sync(&vault).unwrap().added.len(), 0);
    }
    fs::create_dir_all(dir.join("semantic")).unwrap();
    fs::write(dir.join("semantic/v.bin"), "vectors").unwrap();
    let mut index = NoteIndex::open(&dir, "other settings").unwrap();
    assert!(dir.join("semantic/v.bin").exists(), "vectors are kept");
    assert_eq!(index.all().unwrap().len(), 0, "rebuilt from scratch");
    assert_eq!(index.sync(&vault).unwrap().added.len(), 2);
    index.clear().unwrap();
    assert_eq!(index.all().unwrap().len(), 0);
}

/// Narrowing through the trigram index must never drop a real match: for
/// many terms taken from the fixtures, indexed search equals a full scan.
#[test]
fn narrowing_matches_full_scan_on_fixtures() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../blossom-core/tests/fixtures");
    let vault = Vault::open(&fixtures, keywords(), &[]).unwrap();
    let mut index = NoteIndex::in_memory().unwrap();
    let report = index.sync(&vault).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let all = index.all().unwrap();
    assert!(all.len() > 100, "fixtures should give plenty of headings");
    // Terms: windows of 3..8 characters from titles and bodies, mixed case.
    let mut terms = vec![
        "ÜNICODE".to_string(),
        "emoji 🌸".to_string(),
        "x".to_string(),
    ];
    for (i, n) in all.iter().enumerate().step_by(3) {
        let chars: Vec<char> = n.record.text.chars().collect();
        if chars.len() < 3 {
            continue;
        }
        let len = 3 + i % 6;
        let start = (i * 7) % chars.len();
        let end = (start + len).min(chars.len());
        let term: String = chars[start..end].iter().collect();
        terms.push(if i % 2 == 0 {
            term.to_uppercase()
        } else {
            term
        });
    }
    let mut checked = 0;
    for term in terms {
        if term.contains('"') || term.trim().is_empty() {
            continue;
        }
        let q = Query::parse(&format!("\"{term}\""));
        let narrowed = index.search(&q, &ctx()).unwrap();
        let full = filter_and_sort(all.clone(), &q, &ctx());
        assert_eq!(narrowed, full, "term {term:?}");
        checked += 1;
    }
    assert!(checked > 30, "only {checked} terms checked");
}

/// Text before the first heading, and files without headings, are found
/// by plain words in `search_with_intros`, never by filters or `search`.
#[test]
fn intros_are_found_by_words_only() {
    let dir = tempfile::tempdir().unwrap();
    put(
        dir.path(),
        "trip.org",
        "#+TITLE: Lyon trip\n#+FILETAGS: :travel:\nBook the train early.\n* Packing\npassport\n",
    );
    put(dir.path(), "notes/loose.org", "Just a train of thought.\n");
    put(
        dir.path(),
        "plain.org",
        "#+TITLE: Only keywords\n* Heading\n",
    );
    let vault = Vault::open(dir.path(), keywords(), &[]).unwrap();
    let mut index = NoteIndex::in_memory().unwrap();
    index.sync(&vault).unwrap();
    let found = |q: &str| -> Vec<(String, usize, bool)> {
        index
            .search_with_intros(&Query::parse(q), &ctx())
            .unwrap()
            .into_iter()
            .map(|n| (n.record.title.clone(), n.record.line, n.record.is_intro()))
            .collect()
    };
    let mut train = found("train");
    train.sort();
    assert_eq!(
        train,
        [("Lyon trip".into(), 3, true), ("loose".into(), 1, true)]
    );
    assert_eq!(found("passport"), [("Packing".into(), 4, false)]);
    assert_eq!(found("lyon"), [("Lyon trip".into(), 3, true)], "the title");
    assert_eq!(found("train t.travel"), [("Lyon trip".into(), 3, true)]);
    assert_eq!(found("train b.loose").len(), 1);
    assert_eq!(found("train i.todo"), []);
    assert_eq!(
        found("t.travel"),
        [("Packing".into(), 4, false)],
        "no words"
    );
    assert!(titles(&index, "train").is_empty(), "plain search: headings");
    assert_eq!(index.all().unwrap().len(), 2, "all: headings only");
}

#[test]
fn open_never_empties_a_folder_that_is_not_an_index() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "note.org", "* keep me\n");
    let err = NoteIndex::open(dir.path(), "settings").err().unwrap();
    assert!(
        err.to_string().contains("isn't a Blossom index folder"),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("note.org")).unwrap(),
        "* keep me\n"
    );
    // A new folder becomes an index; reopening with other settings rebuilds it.
    let index_dir = dir.path().join("index");
    drop(NoteIndex::open(&index_dir, "a").unwrap());
    fs::create_dir_all(index_dir.join("semantic")).unwrap();
    fs::write(index_dir.join("semantic/x.bin"), "v").unwrap();
    drop(NoteIndex::open(&index_dir, "b").unwrap());
    assert!(index_dir.join("semantic/x.bin").exists());
    // An empty folder (or one holding only vectors) is fine too.
    let empty = dir.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    drop(NoteIndex::open(&empty, "a").unwrap());
}
