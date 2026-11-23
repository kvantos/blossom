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
