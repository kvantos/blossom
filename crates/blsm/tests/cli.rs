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
        "Due soon\n         in 2d      TODO Report  (work.org:3)\n\n\
         Mon 2026-11-23\n  09:00  scheduled  NEXT [#A] Call Bob  :phone:  (work.org:1)\n\n\
         Wed 2026-11-25\n         deadline   TODO Report  (work.org:3)\n"
    );
    // --all includes the done item, which is overdue.
    assert!(
        env.ok(&["agenda", "--all"])
            .starts_with("Overdue\n         scheduled  DONE Filed")
    );
    // Repeats show ahead; a warning window of 1 day drops "due soon".
    let env = Env::new(
        &[(
            "r.org",
            "* TODO Water plants\nSCHEDULED: <2026-11-21 Sat +3d>\n* bins\n<2026-11-17 Tue +1w>\n",
        )],
        "deadline_warning_days = 1\n",
    );
    assert_eq!(
        env.ok(&["agenda", "--days", "4"]),
        "Overdue\n         scheduled  TODO Water plants  (r.org:1)\n\n\
         Mon 2026-11-23\n\n\
         Tue 2026-11-24\n         event      bins  (r.org:3)\n         repeat     TODO Water plants  (r.org:1)\n"
    );
    // Query terms narrow it.
    assert_eq!(
        env.ok(&["agenda", "--days", "4", "bins"]),
        "Mon 2026-11-23\n\nTue 2026-11-24\n         event      bins  (r.org:3)\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&env.ok(&["--json", "agenda", "--days", "2"])).unwrap();
    assert_eq!(json["days"][1]["items"][1]["placement"], "scheduled-repeat");
}

#[test]
fn capture_with_templates() {
    let config = "\
[[capture]]
key = \"t\"
file = \"tasks.org\"
target = \"heading\"
heading = \"Inbox\"
template = \"* TODO %^{What}\\n%U %?\"
[[capture]]
key = \"j\"
file = \"journal.org\"
target = \"datetree\"
template = \"* %?\"
";
    let env = Env::new(&[("tasks.org", "* Inbox\n* Done\n")], config);
    assert_eq!(
        env.ok(&["capture", "-t", "t", "--set", "What=Buy milk", "2 litres"]),
        "tasks.org:2\n"
    );
    assert_eq!(
        fs::read_to_string(env.vault().join("tasks.org")).unwrap(),
        "* Inbox\n** TODO Buy milk\n[2026-11-23 Mon 10:30] 2 litres\n* Done\n"
    );
    assert_eq!(env.ok(&["capture", "-t", "j", "Rainy"]), "journal.org:4\n");
    assert_eq!(
        fs::read_to_string(env.vault().join("journal.org")).unwrap(),
        "* 2026\n** 2026-11 November\n*** 2026-11-23 Monday\n**** Rainy\n"
    );
    let missing = env.run(&["capture", "-t", "t", "x"]);
    assert!(String::from_utf8_lossy(&missing.stderr).contains("asks for What"));
    let unknown = env.run(&["capture", "-t", "zz", "x"]);
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("there are: t, j"));
    assert!(
        !env.run(&["capture", "-t", "t", "-f", "x.org", "x"])
            .status
            .success()
    );
}

#[test]
fn archive_and_refile() {
    let env = Env::new(
        &[
            (
                "work.org",
                "#+ARCHIVE: done.org::* Old\n* DONE Shipped\nnotes\n* TODO Keep\n",
            ),
            ("in.org", "* Idea\n"),
        ],
        CONFIG,
    );
    // Any line of the heading's section names it.
    assert_eq!(env.ok(&["archive", "work.org:3"]), "done.org:2\n");
    assert_eq!(
        fs::read_to_string(env.vault().join("work.org")).unwrap(),
        "#+ARCHIVE: done.org::* Old\n* TODO Keep\n"
    );
    let done = fs::read_to_string(env.vault().join("done.org")).unwrap();
    assert!(
        done.starts_with(
            "* Old\n** DONE Shipped\n:PROPERTIES:\n:ARCHIVE_TIME: 2026-11-23 Mon 10:30\n"
        ),
        "{done}"
    );
    assert_eq!(
        env.ok(&["refile", "in.org:1", "work.org:2"]),
        "work.org:3\n"
    );
    assert_eq!(
        fs::read_to_string(env.vault().join("work.org")).unwrap(),
        "#+ARCHIVE: done.org::* Old\n* TODO Keep\n** Idea\n"
    );
    assert_eq!(fs::read_to_string(env.vault().join("in.org")).unwrap(), "");
    assert!(env.ok(&["search", "idea"]).starts_with("work.org:3"));
    assert!(!env.run(&["archive", "work.org:1"]).status.success());
    assert!(!env.run(&["refile", "work.org", "in.org"]).status.success());
    // A line past the end is an error, not the file's first heading.
    let before = fs::read_to_string(env.vault().join("work.org")).unwrap();
    let out = env.run(&["archive", "work.org:99"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the file has 3 lines"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(env.vault().join("work.org")).unwrap(),
        before
    );
}

#[test]
fn index_dir_must_be_an_index_folder() {
    let env = Env::new(&[("a.org", "* a\n")], CONFIG);
    let keep = env.dir.path().join("elsewhere");
    fs::create_dir_all(&keep).unwrap();
    fs::write(keep.join("keep.txt"), "mine").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blsm"))
        .arg("--vault")
        .arg(env.vault())
        .arg("--config")
        .arg(env.dir.path().join("config.toml"))
        .arg("--index-dir")
        .arg(&keep)
        .arg("ls")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(fs::read_to_string(keep.join("keep.txt")).unwrap(), "mine");
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

/// A tiny OpenAI-style embeddings server: each text becomes counts of words
/// from three topics (food, bike, work) plus a constant, so related words
/// rank together. Runs until the test ends.
fn embed_server() -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let topics: [&[&str]; 3] = [
                &["milk", "bread", "food", "groceries", "eat"],
                &["bike", "bicycle", "brake", "chain", "cycling"],
                &["report", "budget", "meeting", "office"],
            ];
            let data: Vec<serde_json::Value> = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let t = t.as_str().unwrap().to_lowercase();
                    let mut v: Vec<f32> = topics
                        .iter()
                        .map(|words| {
                            f32::from(
                                u8::try_from(words.iter().filter(|w| t.contains(**w)).count())
                                    .unwrap(),
                            )
                        })
                        .collect();
                    v.push(0.1);
                    serde_json::json!({"index": i, "embedding": v})
                })
                .collect();
            let json = serde_json::json!({ "data": data }).to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                json.len()
            )
            .unwrap();
        }
    });
    url
}

const MEANING: &str = "\
* Groceries
milk and bread
* Fix the bicycle :work:
the brake squeaks
* Quarterly report :work:
* Ride to the office :work:
on the bike
";

fn semantic_env() -> Env {
    let config = format!(
        "[semantic]\nmodel = \"server\"\nserver = \"{}\"\nserver_model = \"t\"\n",
        embed_server()
    );
    Env::new(&[("m.org", MEANING)], &config)
}

#[test]
fn semantic_search_ranks_by_meaning_and_embeds_once() {
    let env = semantic_env();
    let out = env.run(&["search", "--semantic", "something", "to", "eat"]);
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("embedding 4 headings with server-t"),
        "{stderr}"
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().next(), Some("m.org:1  Groceries"));
    // Filters narrow; --limit cuts.
    assert_eq!(
        env.ok(&["search", "--semantic", "--limit", "2", "t.work", "cycling"]),
        "m.org:3  Fix the bicycle  :work:\nm.org:6  Ride to the office  :work:\n"
    );
    // Vectors are kept: nothing to embed the second time.
    let out = env.run(&["search", "--semantic", "eat"]);
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Only a new heading is embedded.
    fs::write(
        env.vault().join("m.org"),
        format!("{MEANING}* Bake bread\n"),
    )
    .unwrap();
    let out = env.run(&["search", "--semantic", "eat"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("embedding 1 heading "));
    // JSON adds the score.
    let json: serde_json::Value =
        serde_json::from_str(&env.ok(&["--json", "search", "--semantic", "bread"])).unwrap();
    assert_eq!(json[0]["title"], "Bake bread");
    assert!(json[0]["score"].as_f64().unwrap() > 0.9);
    // A rebuild drops the vectors too.
    env.ok(&["index", "--rebuild"]);
    assert!(!env.dir.path().join("index/semantic").exists());
    let out = env.run(&["search", "--semantic", "eat"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("embedding 5 headings"));
}

#[test]
fn semantic_search_explains_problems() {
    let env = semantic_env();
    let out = env.run(&["search", "--semantic", "t.work"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("needs words"));
    // Nothing listening.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let env = Env::new(
        &[("m.org", MEANING)],
        &format!("[semantic]\nmodel = \"server\"\nserver = \"http://127.0.0.1:{port}/v1\"\n"),
    );
    let out = env.run(&["search", "--semantic", "eat"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("127.0.0.1:{port}/v1/embeddings")),
        "{err}"
    );
}
