# Blossom implementation plan

Version 1, 2026-11-21. A new session reads this first and updates the checkboxes as work completes.

## 0. Context and decisions (final unless the user says otherwise)
- **Product:** Blossom, a desktop org-mode note-taking app for macOS and Linux, modeled on the Android app Bloom. The command-line tool is `blsm`. No mobile.
- **Language:** Rust only. Core, CLI and GUI all in Rust.
- **GUI:** iced (0.14).
- **Org parser:** a fork of orgize, copied into the workspace so upstream changes can't affect us.
- **Search:** tantivy. File watching: notify. CLI: clap.
- **Founding rules** (from Bloom's `docs/architecture.md`):
  1. The `.org` files are the only real state. Any index or cache can be deleted and rebuilt from them.
  2. Parsing is lossless: `parse(x).to_org() == x`, byte for byte.
  3. Edits are text splices (replace a range and re-parse), never a re-render of the whole document.
  4. Other programs (Emacs, Syncthing, git) can change files at any time. Detect changes by mtime+size and file watching.
- **Images:** linked with `[[file:...]]`.
- **Diagrams:** drawn in a built-in canvas, saved as `.svg` with the shape data embedded, so Emacs can display them and Blossom can re-edit them.
- **Distribution:** outside the Mac App Store.
- **Reference material (read-only, never modify):**
  - Bloom (Kotlin Android app, Apache-2.0) at `/home/kvantos/Projects/bloom`, commit `11e0ce1de0f4925303994c3733471413fc80c69b` (2026-11-18). Key docs: `docs/architecture.md`, `docs/search-syntax.md`, `docs/tasks-and-events.md`, `docs/terminology.md`. Key code: `app/src/main/java/com/rrajath/grove/org/OrgParser.kt`, `OrgMutations.kt`.
  - orgize upstream (MIT) at `/home/kvantos/Projects/orgize`, commit `5f26c94dcec2a33b37b1c880ace053b29b5d021e` (2024-07-22), version `0.10.0-alpha.10`.
- **Environment:** x86_64 Arch Linux, stable rustc/cargo, no rustup or nightly.

## 1. Workspace layout
```
blossom/
├── Cargo.toml              # [workspace], resolver 3, shared lints and dependency versions
├── CLAUDE.md               # points to docs/PLAN.md
├── docs/PLAN.md            # this file
├── crates/
│   ├── orgize/             # forked parser (MIT), publish = false
│   ├── blossom-core/       # our org API on top of orgize
│   ├── blossom-vault/      # folder access, file watching, ignore list, revisions
│   ├── blossom-index/      # tantivy index and query language
│   ├── blsm/               # CLI binary (clap)
│   └── blossom-app/        # iced GUI binary
└── fuzz/                   # cargo-fuzz targets (nightly only)
```

## 2. Quality rules
- Our crates use edition 2024. The orgize fork stays on edition 2021 and is exempt from our strict lints.
- Workspace lints: `unsafe_code = "forbid"`; clippy `unwrap_used`, `expect_used`, `panic`, `todo`, `dbg_macro` deny (tests may unwrap); `clippy::pedantic` warn.
- Must pass `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`.
- Reviewable code: plain structs/enums, clone rather than complex lifetimes, no clever generics or macros, doc comments on public items.
- `blossom-core` does no file I/O. Only `blossom-core` imports `orgize`.

## Phase 0: workspace, orgize fork, round-trip tests
- [x] 0.1 Workspace `Cargo.toml`, `.gitignore`, `CLAUDE.md`, `docs/PLAN.md`, `git init`. Ask the user before the first commit. *(First commit made 2026-11-23 with user approval.)*
- [x] 0.2 Fork orgize into `crates/orgize` (without `.git/`, `target/`, `wasm/`, `fuzz/`); strip its `[workspace]` and profiles; `publish = false`; keep `rowan 0.15`, `nom 7`; `UPSTREAM.md` with source, commit, and patch log.
- [x] 0.3 `cargo test -p orgize` passes; compiler-required fixes logged in `UPSTREAM.md`.
- [x] 0.4 Bloom fixtures in `crates/blossom-core/tests/fixtures/bloom/` with `ATTRIBUTION.md`.
- [x] 0.5 Own edge-case fixtures in `tests/fixtures/edge/` (empty, no trailing newline, CRLF, mixed EOL, BOM, tabs/trailing ws, unicode/emoji/RTL, intro only, file-level drawer, unclosed drawer, unclosed block, deep nesting, `*bold*` at line start, empty heading, repeaters/warnings/ranges, non-English day names).
- [x] 0.6 Round-trip test `tests/roundtrip.rs`: raw bytes, `to_org() == s`, report first differing offset.
- [x] 0.7 Edit test: deterministic ranges, `replace_range` == naive splice == fresh parse.
- [x] 0.8 proptest grammar-based round-trip and splice properties.
- [x] 0.9 cargo-fuzz targets `roundtrip`, `splice` (run requires nightly via rustup, user approval). *(Targets in `fuzz/`, `cargo check` passes on stable; not yet run.)*
- [x] 0.10 Round-trip failures fixed in the fork, with regression fixtures. Done when all pass. *(Two fork bugs fixed, see `crates/orgize/UPSTREAM.md` patches 4–5.)*

## Phase 1: `blossom-core`
Model on Bloom's `OrgParser.kt` / `OrgMutations.kt`, built on orgize's tree.
- [x] `Document`: text, flat headings, file tags, preamble `#+KEY:` lines, file property drawer, file id, intro.
- [x] `Heading`: index, ranges, level, keyword, priority, title, tags, planning, properties (ordered), logbook, active timestamps, body range; parent/children/subtree/inherited tags.
- [x] `Timestamp`: date, times, active, repeater (`+ ++ .+`), warning/delay, range; parse and Emacs-style format.
- [x] `Keywords`: parse `"TODO NEXT | DONE CANCELLED"`, `is_done`, `next`.
- [x] Edits: set keyword/priority/tags/scheduled/deadline, mark_done (repeater + LOGBOOK), toggle_checkbox, upsert_property, move up/down, promote/demote, delete, refile, insert heading.
- [x] Unit tests per edit; lines outside the edited range unchanged. *(`tests/mutations.rs`: exact-output tests, plus a sweep running every edit on every fixture heading, checking the tree and index against a fresh parse and that no byte outside the heading changed.)*

Notes from Phase 1:
- Heading structure (start, level, keyword, priority, title, tags, subtree end) comes from orgize's tree; the metadata after the heading line uses a line scanner ported from Bloom, which also accepts Bloom's layouts (drawers in either order, a dedicated active-timestamp line above the drawers).
- Edits splice the smallest range (e.g. only the keyword), not a re-rendered heading line as Bloom does, so Emacs tag alignment survives. New lines use the file's line ending; a file without a final newline keeps it that way.
- Dates use the `chrono` crate (pure; `now` is always passed in).
- Departures from Bloom: `Keywords` without `|` makes the last keyword done-type (Emacs rule); timestamps are written with English day names regardless of locale (any day name is read).
- `Document::org()` exposes the orgize tree read-only; only tests use it now (the GUI uses `blossom_core::render`).
- Agenda occurrence logic (`occurrenceCovering`) was added in Phase 2; warning days are left for Phase 5.

## Phase 2: `blossom-vault` + `blossom-index` + `blsm`
- [x] Vault: recursive `.org` walk (skip dot-folders, `.stversions`), ignore file (`.orgzlyignore`/`.blsmignore`), `mtime:size` revisions, parse cache, atomic writes preserving CRLF, stale-write guard, `.sync-conflict-*` detection, debounced notify watcher.
- [x] Index: tantivy per-heading schema; incremental by revision; in the platform cache dir; rebuildable.
- [x] Query language from Bloom `docs/search-syntax.md`.
- [x] `blsm search|agenda|capture|todo|index --rebuild|ls`, `--json`, config `~/.config/blossom/config.toml`.
- [x] CI on Linux and macOS. *(`.github/workflows/ci.yml`; not run yet: the repo has no remote.)*

Notes from Phase 2:
- The index only narrows: plain-text terms of 3+ characters are looked up as lower-cased trigrams, then Bloom's matcher (`blossom-index/src/matcher.rs`) decides every result. `tests/index.rs` checks narrowed search equals a full scan on all fixtures.
- The index folder holds a meta file (schema version + keyword settings); any mismatch deletes and rebuilds it.
- Watcher batches are hints to re-list, not exact: a file created inside a brand-new folder may be reported as the folder (inotify race).
- Symlinked folders are not followed (no cycles); symlinked files are.
- `b.NAME` also matches a nested file by its name (`b.home` finds `home/home.org`); Bloom compares the full path only.
- Departures/omissions vs Bloom search: no file-name result rows, no snippets, no intro note, no saved searches (GUI concerns).
- `blsm capture` is a plain "heading at the end of the inbox"; templates come in Phase 5. `blsm agenda` is the `ad.N` view (minus done items unless `--all`); Phase 5 adds Bloom's full agenda rules (repeats ahead for SCHEDULED/DEADLINE, due soon).
- Occurrence logic (`Timestamp::occurrence_covering`, `occurs_between`) moved into core now, as `ad.N` needs it.
- New dependencies: tantivy 0.27, notify 9, clap 4, serde/serde_json, toml 1, directories 6; tempfile for tests.

## Phase 3: `blossom-app` shell (iced)
- [x] Notebooks tree, outline with chips, read view.
- [x] Background worker owning vault and index (Task/Subscription); watcher refreshes views.
- [x] Light/dark following system.

Notes from Phase 3:
- `blossom [VAULT]` opens VAULT or `vault` from the shared config (now `blossom_vault::config`, used by `blsm` too); otherwise a welcome screen takes a folder path. No native folder picker yet; it is planned for Phase 4.
- The worker is one thread owning `Vault`, `NoteIndex` and the watcher; the UI talks to it through `worker::Handle` requests and receives `worker::Event`s via `Subscription::run`. Outside edits re-sync the index, refresh the file list and reload the open file, keeping the selection where possible.
- Read view uses `blossom_core::render` (blocks of styled spans built from orgize's tree), so the app never touches orgize. Checkboxes are shown read-only; toggling is Phase 4.
- Light/dark: `iced::system::theme()` / `theme_changes()` pick Blossom's own light or dark palette.
- Links: http(s)/mailto open with the system opener; `file:` links to vault `.org` files open in the app; others (`id:`, images) show "not supported yet".
- Dev hook `BLOSSOM_SCREENSHOT=out.png` (+ `_FILE`, `_HEADING`, `BLOSSOM_THEME`) saves a window screenshot and exits, for checking the UI without a screenshot tool.

## Phase 4: editing
- [ ] `text_editor` with org highlighter; per-heading section editing.
- [ ] Metadata panel, list continuation, autosave with stale guard, undo.
- [ ] Native folder picker (e.g. `rfd`) on the welcome screen and as an "Open vault…" action.

## Phase 5: agenda, capture, refile, archive
- [ ] Agenda rules from Bloom (one day per item: scheduled else deadline; bare timestamps are events; due soon; repeats ahead).
- [ ] Capture templates (`%U %t %^{prompt} %?`; file top/bottom, under heading, date tree).
- [ ] Refile and archive with undo.

## Phase 6: images and diagrams
- [ ] Inline `[[file:…png|jpg|svg]]` in read view.
- [ ] Paste/drag image → attachments folder + link.
- [ ] Diagram canvas (pen, line, arrow, rect, ellipse, text, select/move, delete, undo, snap); SVG with JSON `<metadata>`.

## Later
rowan/nom upgrade; nightly fuzzing; packaging (.app signing/notarization, AppImage/Flatpak/AUR); `blsm` name-clash check; semantic search.
