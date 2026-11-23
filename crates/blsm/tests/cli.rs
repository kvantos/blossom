//! Runs the `blsm` binary against temporary vaults.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new(files: &[(&str, &str)], config: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let full = dir.path().join("vault").join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, text).unwrap();
        }
        fs::create_dir_all(dir.path().join("vault")).unwrap();
        fs::write(dir.path().join("config.toml"), config).unwrap();
        Env { dir }
    }

    fn vault(&self) -> std::path::PathBuf {
        self.dir.path().join("vault")
    }

    fn run(&self, args: &[&str]) -> Output {
        let root = self.dir.path();
        Command::new(env!("CARGO_BIN_EXE_blsm"))
            .arg("--vault")
            .arg(root.join("vault"))
            .arg("--config")
            .arg(root.join("config.toml"))
            .arg("--index-dir")
            .arg(root.join("index"))
            .args(["--now", "2026-11-23T10:30"])
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "blsm {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

const WORK: &str = "\
* NEXT [#A] Call Bob :phone:
SCHEDULED: <2026-11-23 Mon 09:00>
* TODO Report
DEADLINE: <2026-11-25 Wed>
* DONE Filed
SCHEDULED: <2026-11-22 Sun>
";

const CONFIG: &str = "keywords = \"TODO NEXT | DONE\"\ninbox = \"in/box.org\"\n";

#[test]
fn agenda_text() {
    let env = Env::new(&[("work.org", WORK)], CONFIG);
    assert_eq!(
        env.ok(&["agenda", "--days", "3"]),
        "Mon 2026-11-23\n  09:00  scheduled  NEXT [#A] Call Bob  :phone:  (work.org:1)\n\n\
         Wed 2026-11-25\n         deadline   TODO Report  (work.org:3)\n"
    );
    // --all includes the done item, which is overdue.
    assert!(
        env.ok(&["agenda", "--all"])
            .starts_with("Overdue\n         scheduled  DONE Filed")
    );
}

#[test]
fn search_and_todo() {
    let env = Env::new(&[("work.org", WORK), ("b.org", "* bob's notes\n")], CONFIG);
    assert_eq!(
        env.ok(&["search", "bob"]),
        "b.org:1  bob's notes\nwork.org:1  NEXT [#A] Call Bob  :phone:  SCHEDULED <2026-11-23 Mon 09:00>\n"
    );
    assert_eq!(
        env.ok(&["todo", "p.a"]),
        "work.org:1  NEXT [#A] Call Bob  :phone:  SCHEDULED <2026-11-23 Mon 09:00>\n"
    );
    let json: serde_json::Value = serde_json::from_str(&env.ok(&["--json", "todo"])).unwrap();
    let titles: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Call Bob", "Report"]);
    assert_eq!(json[0]["scheduled"], "<2026-11-23 Mon 09:00>");
}

#[test]
fn capture_then_find() {
    let env = Env::new(&[("work.org", WORK)], CONFIG);
    assert_eq!(
        env.ok(&[
            "capture",
            "-k",
            "TODO",
            "Ring",
            "Alice",
            "-b",
            "about lunch"
        ]),
        "in/box.org:1\n"
    );
    assert_eq!(
        fs::read_to_string(env.vault().join("in/box.org")).unwrap(),
        "* TODO Ring Alice\n:PROPERTIES:\n:CREATED: [2026-11-23 Mon 10:30]\n:END:\nabout lunch\n"
    );
    assert_eq!(
        env.ok(&["capture", "--file", "work.org", "Second"]),
        "work.org:7\n"
    );
    assert_eq!(
        env.ok(&["search", "lunch"]),
        "in/box.org:1  TODO Ring Alice\n"
    );
    let bad = env.run(&["capture", "-k", "WAIT", "x"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown keyword"));
    assert!(
        !env.run(&["capture", "-f", "notes.txt", "x"])
            .status
            .success()
    );
}

#[test]
fn index_ls_and_conflicts() {
    let env = Env::new(
        &[
            ("work.org", WORK),
            ("work.sync-conflict-20261122-101010-ABC.org", "* x\n"),
            (".stversions/old.org", "* old\n"),
        ],
        CONFIG,
    );
    let out = env.run(&["index"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "indexed: 1 added, 0 updated, 0 removed, 0 failed\n"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("work.org has a Syncthing conflict copy")
    );
    assert_eq!(
        env.ok(&["index"]),
        "indexed: 0 added, 0 updated, 0 removed, 0 failed\n"
    );
    assert_eq!(
        env.ok(&["index", "--rebuild"]),
        "indexed: 1 added, 0 updated, 0 removed, 0 failed\n"
    );
    assert_eq!(
        env.ok(&["ls"]),
        "work.org  3 headings, 2 open  (conflict copy)\n"
    );
}

#[test]
fn errors_are_reported() {
    let env = Env::new(&[], "nonsense = 1\n");
    let out = env.run(&["ls"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown field"));
    let out = Command::new(env!("CARGO_BIN_EXE_blsm"))
        .args(["--config", "/nonexistent/config.toml", "--vault"])
        .arg(Path::new("/nonexistent/vault"))
        .arg("ls")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("blsm: vault /nonexistent/vault"));
}
