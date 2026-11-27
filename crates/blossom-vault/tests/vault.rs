//! Vault behaviour against real temporary folders.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::Path;
use std::time::Duration;

use blossom_core::{EditError, Keywords};
use blossom_vault::{Conflict, Vault, WriteError};

fn put(root: &Path, path: &str, text: &str) {
    let full = root.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}

fn vault(root: &Path) -> Vault {
    Vault::open(root, Keywords::default(), &[]).unwrap()
}

#[test]
fn lists_org_files_with_skip_and_ignore_rules() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "inbox.org", "* a\n");
    put(r, "projects/acme.org", "* b\n");
    put(r, "projects/deep/x.org", "");
    put(r, "notes.txt", "not org");
    put(r, ".stversions/inbox.org", "old");
    put(r, ".git/x.org", "");
    put(r, "archive/old.org", "");
    put(r, "scratch-1.org", "");
    put(
        r,
        "projects/acme.sync-conflict-20261121-143200-ABCDEF.org",
        "* b'\n",
    );
    put(r, ".blsmignore", "archive\n");
    put(r, ".orgzlyignore", "scratch-*\n");
    let v = Vault::open(r, Keywords::default(), &["projects/deep".to_string()]).unwrap();
    let listing = v.list().unwrap();
    let paths: Vec<&str> = listing.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["inbox.org", "projects/acme.org"]);
    assert_eq!(
        listing.conflicts,
        [Conflict {
            path: "projects/acme.org".into(),
            copy: "projects/acme.sync-conflict-20261121-143200-ABCDEF.org".into()
        }]
    );
    assert_eq!(listing.files[0].revision.size, 4);
}

#[test]
fn rejects_paths_outside_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(dir.path());
    for bad in ["../x.org", "/etc/x.org", "", "a/../../x.org"] {
        assert!(v.full_path(bad).is_err(), "{bad}");
        assert!(
            matches!(v.write(bad, "x", None), Err(WriteError::InvalidPath(_))),
            "{bad}"
        );
    }
}

#[test]
fn write_is_atomic_and_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(dir.path());
    let rev = v.write("new/dir/a.org", "* a\r\nbody\r\n", None).unwrap();
    assert_eq!(
        fs::read(dir.path().join("new/dir/a.org")).unwrap(),
        b"* a\r\nbody\r\n"
    );
    assert_eq!(v.revision("new/dir/a.org").unwrap(), Some(rev));
    // Creating over an existing file is stale.
    assert!(matches!(
        v.write("new/dir/a.org", "x", None),
        Err(WriteError::Stale { .. })
    ));
    // Another program changes the file: our write based on `rev` is refused.
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.path().join("new/dir/a.org"), "* changed by emacs\n").unwrap();
    match v.write("new/dir/a.org", "* mine\n", Some(rev)) {
        Err(WriteError::Stale { expected, found }) => {
            assert_eq!(expected, Some(rev));
            assert_ne!(found, Some(rev));
        }
        other => panic!("expected stale, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("new/dir/a.org")).unwrap(),
        "* changed by emacs\n"
    );
    // No temp files are left behind.
    let names: Vec<String> = fs::read_dir(dir.path().join("new/dir"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["a.org"]);
}

#[cfg(unix)]
#[test]
fn write_keeps_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.org", "x");
    let full = dir.path().join("a.org");
    fs::set_permissions(&full, fs::Permissions::from_mode(0o600)).unwrap();
    let v = vault(dir.path());
    let rev = v.revision("a.org").unwrap();
    v.write("a.org", "y", rev).unwrap();
    assert_eq!(
        fs::metadata(&full).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn document_cache_follows_revisions() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.org", "* one\n");
    let v = vault(dir.path());
    let (d1, r1) = v.document("a.org").unwrap();
    let (d2, _) = v.document("a.org").unwrap();
    assert!(std::sync::Arc::ptr_eq(&d1, &d2), "second read is cached");
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.path().join("a.org"), "* one\n* two\n").unwrap();
    let (d3, r3) = v.document("a.org").unwrap();
    assert_ne!(r1, r3);
    assert_eq!(d3.headings().len(), 2);
}

#[test]
fn edit_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a.org", "* TODO a\r\n");
    let v = vault(dir.path());
    v.edit("a.org", |d| {
        d.set_keyword(0, Some("DONE")).map_err(Failure::Edit)
    })
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("a.org")).unwrap(),
        "* DONE a\r\n"
    );
    // Editing a missing file creates it.
    v.edit("b.org", |d| {
        d.insert_heading(
            blossom_core::InsertAt::LastTopLevel,
            &blossom_core::NewHeading {
                title: "new".into(),
                ..Default::default()
            },
        )
        .map_err(Failure::Edit)
    })
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("b.org")).unwrap(),
        "* new\n"
    );
    // A failing edit writes nothing.
    let err = v.edit("a.org", |d| d.set_keyword(9, None).map_err(Failure::Edit));
    assert!(matches!(
        err,
        Err(Failure::Edit(EditError::NoSuchHeading(9)))
    ));
}

#[derive(Debug)]
enum Failure {
    Edit(EditError),
    Write,
}

impl From<WriteError> for Failure {
    fn from(_: WriteError) -> Self {
        Failure::Write
    }
}

#[test]
fn watcher_batches_changes() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(dir.path());
    let w = v.watch(Duration::from_millis(150)).unwrap();
    put(dir.path(), "a.org", "* a\n");
    put(dir.path(), "sub/b.org", "* b\n");
    put(dir.path(), ".stversions/c.org", "* c\n");
    let mut seen = std::collections::BTreeSet::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    // A file written into a brand-new folder may only show up as the folder.
    while !(seen.contains("a.org") && (seen.contains("sub/b.org") || seen.contains("sub"))) {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!left.is_zero(), "watcher saw only {seen:?}");
        if let Ok(batch) = w.changes().recv_timeout(left) {
            seen.extend(batch);
        }
    }
    assert!(!seen.iter().any(|p| p.contains(".stversions")), "{seen:?}");
}

// --- operations ---

mod ops {
    use super::*;
    use blossom_core::EventStamp;
    use blossom_core::capture::Target;
    use blossom_vault::ops::{Complete, HeadingRef, OpError};
    use chrono::{NaiveDate, NaiveDateTime};

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 23)
            .unwrap()
            .and_hms_opt(9, 30, 0)
            .unwrap()
    }

    fn h(path: &str, index: usize, title: &str) -> HeadingRef {
        HeadingRef {
            path: path.into(),
            index,
            title: title.into(),
        }
    }

    fn read(r: &Path, p: &str) -> String {
        fs::read_to_string(r.join(p)).unwrap()
    }

    #[test]
    fn archive_to_a_new_file_and_undo() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(r, "a.org", "* Proj\n** DONE Task\nbody\n* Next\n");
        let v = vault(r);
        let out = v
            .archive(&h("a.org", 1, "Task"), "%s_archive::", now())
            .unwrap();
        assert_eq!(out.path, "a.org_archive");
        assert_eq!(out.message, "Archived to a.org_archive");
        assert_eq!(read(r, "a.org"), "* Proj\n* Next\n");
        let full = r.join("a.org").display().to_string();
        assert_eq!(
            read(r, "a.org_archive"),
            format!(
                "* DONE Task\n:PROPERTIES:\n:ARCHIVE_TIME: 2026-11-23 Mon 09:30\n:ARCHIVE_FILE: {full}\n:ARCHIVE_OLPATH: Proj\n:ARCHIVE_CATEGORY: a\n:ARCHIVE_TODO: DONE\n:END:\nbody\n"
            )
        );
        v.restore(&out.changes).unwrap();
        assert_eq!(read(r, "a.org"), "* Proj\n** DONE Task\nbody\n* Next\n");
        assert!(!r.join("a.org_archive").exists());
        // A stale reference is refused.
        assert!(matches!(
            v.archive(&h("a.org", 1, "Other"), "%s_archive::", now()),
            Err(OpError::Moved)
        ));
    }

    #[test]
    fn archive_under_a_heading_in_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(r, "a.org", "#+ARCHIVE: ::* Archive\n* DONE x\n* y\n");
        let v = vault(r);
        let out = v
            .archive(&h("a.org", 0, "x"), "%s_archive::", now())
            .unwrap();
        assert_eq!(out.changes.len(), 1);
        let text = read(r, "a.org");
        assert!(
            text.starts_with("#+ARCHIVE: ::* Archive\n* y\n* Archive\n** DONE x\n:PROPERTIES:\n"),
            "{text}"
        );
    }

    #[test]
    fn refile_across_files_undo_refuses_after_outside_edit() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(r, "a.org", "* move me\n** child\n* stay\n");
        put(r, "b.org", "* Target\n* Other\n");
        let v = vault(r);
        let out = v
            .refile(
                &h("a.org", 0, "move me"),
                "b.org",
                Some(&h("b.org", 0, "Target")),
            )
            .unwrap();
        assert_eq!(
            (out.path.as_str(), out.heading, out.line),
            ("b.org", Some(1), 2)
        );
        assert_eq!(read(r, "a.org"), "* stay\n");
        assert_eq!(
            read(r, "b.org"),
            "* Target\n** move me\n*** child\n* Other\n"
        );
        // Same file, to the end.
        let back = v.refile(&h("b.org", 1, "move me"), "b.org", None).unwrap();
        assert_eq!(read(r, "b.org"), "* Target\n* Other\n* move me\n** child\n");
        std::thread::sleep(Duration::from_millis(20));
        put(r, "b.org", "* changed by emacs\n");
        assert!(matches!(v.restore(&back.changes), Err(OpError::Changed(p)) if p == "b.org"));
        assert_eq!(read(r, "b.org"), "* changed by emacs\n");
    }

    #[test]
    fn archive_locations_follow_emacs() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let note = "#+ARCHIVE: archive.org::\n* DONE a\n* DONE b\n* DONE c\n";
        put(r, "projects/acme.org", note);
        let v = vault(r);
        // Relative to the note's folder.
        let out = v
            .archive(&h("projects/acme.org", 0, "a"), "%s_archive::", now())
            .unwrap();
        assert_eq!(out.path, "projects/archive.org");
        assert!(read(r, "projects/archive.org").starts_with("* DONE a\n"));
        // An absolute location inside the vault is mapped into it.
        let abs = format!("{}/done/%s_archive::", r.display());
        put(
            r,
            "projects/acme.org",
            &format!("#+ARCHIVE: {abs}\n* DONE b\n"),
        );
        let out = v
            .archive(&h("projects/acme.org", 0, "b"), "%s_archive::", now())
            .unwrap();
        assert_eq!(out.path, "done/acme.org_archive");
        // Outside the vault: refused, nothing written.
        put(
            r,
            "projects/acme.org",
            "#+ARCHIVE: ../../x.org::\n* DONE c\n",
        );
        let err = v
            .archive(&h("projects/acme.org", 0, "c"), "%s_archive::", now())
            .unwrap_err();
        assert!(err.to_string().contains("outside the vault"), "{err}");
        assert_eq!(
            read(r, "projects/acme.org"),
            "#+ARCHIVE: ../../x.org::\n* DONE c\n"
        );
    }

    #[test]
    fn new_file_names() {
        use blossom_vault::ops::{new_file_path, new_file_text};
        let ok = |s: &str| new_file_path(s).unwrap();
        assert_eq!(ok("garden"), "garden.org");
        assert_eq!(ok(" projects/garden "), "projects/garden.org");
        assert_eq!(ok("/projects//garden.ORG"), "projects/garden.ORG");
        assert_eq!(ok("a\\b"), "a/b.org");
        assert_eq!(ok("notes.txt"), "notes.txt.org");
        for bad in [
            "",
            "  ",
            ".org",
            "projects/",
            "../x",
            "a/./b",
            ".hidden",
            "a/.git/x",
            "a/ /b",
        ] {
            assert!(new_file_path(bad).is_err(), "{bad:?}");
        }
        assert_eq!(new_file_text(""), "");
        assert_eq!(new_file_text("  My   garden "), "#+TITLE: My garden\n");
    }

    #[test]
    fn refile_within_a_file_spelled_differently() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(r, "n/a.org", "* one\n* two\n");
        let v = vault(r);
        assert!(v.same_file("n/a.org", "n//a.org"));
        assert!(!v.same_file("n/a.org", "a.org"));
        assert!(!v.same_file("n/a.org", "../a.org"));
        let out = v
            .refile(
                &h("n/a.org", 0, "one"),
                "n//a.org",
                Some(&h("n//a.org", 1, "two")),
            )
            .unwrap();
        assert_eq!(out.changes.len(), 1);
        assert_eq!(read(r, "n/a.org"), "* two\n** one\n");
    }

    #[test]
    fn complete_and_capture() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(r, "a.org", "* TODO task\n* bins\n<2026-11-20 Fri +1w>\n");
        let v = vault(r);
        let out = v
            .complete(&h("a.org", 0, "task"), Complete::Toggle, now())
            .unwrap();
        assert_eq!(out.message, "Marked DONE");
        assert!(read(r, "a.org").starts_with("* DONE task\nCLOSED: [2026-11-23 Mon 09:30]\n"));
        let out = v
            .complete(&h("a.org", 0, "task"), Complete::Toggle, now())
            .unwrap();
        assert_eq!(out.message, "Reopened");
        let day = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        v.complete(
            &h("a.org", 1, "bins"),
            Complete::Advance(EventStamp::Active(day)),
            now(),
        )
        .unwrap();
        assert!(read(r, "a.org").ends_with("* bins\n<2026-11-27 Fri +1w>\n"));
        assert!(
            v.complete(&h("a.org", 1, "bins"), Complete::Toggle, now())
                .is_err()
        );

        let out = v
            .capture(
                "new/journal.org",
                &Target::Bottom,
                "* hello\nbody",
                now().date(),
            )
            .unwrap();
        assert_eq!((out.heading, out.line), (Some(0), 1));
        assert_eq!(read(r, "new/journal.org"), "* hello\nbody\n");
        v.restore(&out.changes).unwrap();
        assert!(!r.join("new/journal.org").exists());
        // A new file: created with its folder, undone by deleting it.
        let out = v
            .create_file("projects/garden.org", "#+TITLE: Garden\n")
            .unwrap();
        assert_eq!(out.message, "Created projects/garden.org");
        assert_eq!(read(r, "projects/garden.org"), "#+TITLE: Garden\n");
        let again = v.create_file("projects/garden.org", "").unwrap_err();
        assert!(again.to_string().contains("exists already"), "{again}");
        assert_eq!(read(r, "projects/garden.org"), "#+TITLE: Garden\n");
        v.restore(&out.changes).unwrap();
        assert!(!r.join("projects/garden.org").exists());
        let out = v.create_file("empty.org", "").unwrap();
        assert_eq!((read(r, "empty.org"), out.heading), (String::new(), None));
        let missing = Target::Heading {
            id: None,
            path: vec!["Nope".into()],
            first: false,
        };
        assert!(matches!(
            v.capture("a.org", &missing, "x", now().date()),
            Err(OpError::Capture(_))
        ));
    }
}

#[test]
fn attachments_are_stored_beside_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "notes/work.org", "* a\n");
    let v = vault(r);
    let (path, link) = v
        .attach("notes/work.org", "attachments", "shot.png", b"png")
        .unwrap();
    assert_eq!(path, "notes/attachments/work/shot.png");
    assert_eq!(link, "attachments/work/shot.png");
    let (path2, _) = v
        .attach("notes/work.org", "attachments", "shot.png", b"x")
        .unwrap();
    assert_eq!(path2, "notes/attachments/work/shot-1.png");
    assert_eq!(fs::read(r.join(&path)).unwrap(), b"png");
    // Not listed as notes.
    assert_eq!(v.list().unwrap().files.len(), 1);
    let full = v.resolve_link("notes/work.org", &link);
    assert_eq!(fs::read(&full).unwrap(), b"png");
    assert_eq!(v.relative_path(&full).as_deref(), Some(path.as_str()));
    assert_eq!(v.relative_path(Path::new("/etc/hosts")), None);
    assert!(v.resolve_link("a.org", "/abs/x.png").ends_with("abs/x.png"));
    // A set shares one free stem.
    put(r, "notes/attachments/work/flow.excalidraw", "{}");
    let links = v
        .attach_set(
            "notes/work.org",
            "attachments",
            "flow",
            &[("excalidraw", b"{}"), ("svg", b"<svg/>")],
        )
        .unwrap();
    assert_eq!(
        links,
        [
            "attachments/work/flow-1.excalidraw",
            "attachments/work/flow-1.svg"
        ]
    );
}
