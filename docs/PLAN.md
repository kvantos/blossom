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
- **Diagrams:** drawn in a built-in canvas and stored as a pair of files side by side: `flow.excalidraw` (the source, Excalidraw's JSON, which Excalidraw tools can also edit) and `flow.svg` (generated from it). Notes link to the `.svg`, so Emacs, GitHub and any viewer show it; Blossom finds the source by name. Commit both. *(Revised 2026-11-24 with the user; it replaced "one `.svg` with the shape data embedded".)*
- **Distribution:** outside the Mac App Store.
- **Semantic search:** optional, `blsm search --semantic plain words` and a "By meaning" switch in the app's search screen; plain words are enough, Orgzly filters optional. Embeddings come from a built-in pure-Rust model (candle; English `bge-small-en-v1.5` by default, `multilingual-e5-small` as an option, downloaded once on first use) or an OpenAI-compatible server (e.g. Ollama). Vectors live beside the index, rebuildable. *(Agreed 2026-11-24 with the user; plan in Phase 7.)*
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
│   ├── blossom-semantic/   # embeddings and vector store (semantic search)
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
- [x] `text_editor` with org highlighter; per-heading section editing.
- [x] Metadata panel, list continuation, autosave with stale guard, undo.
- [x] Native folder picker (e.g. `rfd`) on the welcome screen and as an "Open vault…" action.

Notes from Phase 4:
- Section editing: "Edit" (or Ctrl/Cmd+E) opens iced's `text_editor` on the selected heading's own section (heading line, metadata, body; children keep their own sections) or on the top of the file (keywords, file drawer, intro; also how an empty file is started). Every keystroke splices the editor text into the working document (`Document::replace_range`), so the outline updates live; a section that has text after it always keeps its final line break, so the next heading can't join its last line. Esc or Done leaves the editor.
- Highlighting: `blossom_core::highlight` is a line scanner (state carried line to line: inside a block or a drawer) for headings, keywords, priority, tags, planning, timestamps, drawers, `#+` lines, comments, bullets, checkboxes, emphasis and links; the app adapts it to iced's `Highlighter`.
- Metadata panel (read and edit view): keyword and priority pickers, tags, SCHEDULED and DEADLINE fields (`2026-11-24`, `2026-11-24 09:30`, `today`, `tomorrow`, `+3d`, `+2w` or an org timestamp; × clears). Keyword changes use `change_keyword`, so DONE sets CLOSED and repeaters advance. Read-view checkboxes now toggle.
- List continuation (`blossom_core::editing::continue_list`): Enter at the end of a list item starts the next one (same indent and bullet, numbers count up, an empty `[ ]` if the item had a checkbox); Enter on an empty item removes its bullet.
- Undo/redo (Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z or Ctrl+Y, and buttons) per open file, up to 200 steps of whole-text snapshots; keystrokes less than 1 s apart are one step. An outside change clears the history (stepping back would silently revert the other program's edit).
- Autosave: the UI sends the full text after every change; the worker writes it once typing pauses for 0.7 s, with the vault's stale-write guard, and flushes on close (the window waits for it) and when another vault is opened. A save refused as stale, or an outside change while edits are unsaved, is merged with `blossom_core::editing::merge3` when the two sides changed different lines; otherwise a banner offers "Keep mine" or "Use the disk version" (the dropped edits stay one Undo away). Closing with an unresolved conflict asks once. Unsaved text of a file you moved away from is kept and shown again when you come back.
- `Document` and the fork's `Org` are now `Clone` (fork patch 6), so edits splice a copy instead of re-parsing.
- Folder picker: `rfd` 0.18 with its default XDG portal backend (no GTK or other system libraries on Linux). "Choose folder…" on the welcome screen, "Open…" above the notebooks tree, Ctrl/Cmd+O. The typed path stays as a fallback when no portal is running.
- The welcome screen's path field starts as `~/Notes`; if the typed folder doesn't exist, the button reads "Create and open" with a hint below, and opening creates it (`create_dir_all`). Some portal file choosers (e.g. COSMIC's) have no "New folder" button. *(Added 2026-11-24 at the user's request.)*
- New dependencies: `rfd`; iced's `advanced` feature (for the `Highlighter` trait).
- Screenshot hook: `BLOSSOM_SCREENSHOT_EDIT=1` opens the editor first. The GPU renderer's window screenshot leaves the editor's text out, so capture the editor with `ICED_BACKEND=tiny-skia`.

## Phase 5: agenda, capture, refile, archive
- [x] Agenda rules from Bloom (one day per item: scheduled else deadline; bare timestamps are events; due soon; repeats ahead).
- [x] Capture templates (`%U %t %^{prompt} %?`; file top/bottom, under heading, date tree).
- [x] Refile and archive with undo.

Notes from Phase 5:
- Agenda (`blossom_index::agenda`, ported from Bloom's `AgendaBuckets` and `dateGroups`): Overdue (past SCHEDULED, else DEADLINE), Due soon (DEADLINE after today within its `-Nd`, else `deadline_warning_days`, default 14), then today and each later day of the window. One day per planned heading; events (bare active stamps) on every day they cover and every repeat; repeating SCHEDULED/DEADLINE previewed on later repeats; SCHEDULED delay `-Nd` hides the plan (and its repeats; `--Nd` only the first). Kept Bloom's quirks: a heading with both dates also shows under Due soon; a delayed heading previews no repeats. `Entry::completion` says what its box does: a task is marked done (repeating if anything repeats), an event with a repeater advances only the stamp that placed it (`Document::advance_event`, new), a plain event has no box.
- `blsm agenda` uses the new layout (sections Overdue / Due soon / days; JSON `{overdue, due_soon, days}`); `blsm search ad.N` keeps Bloom's search agenda as before.
- Capture (`blossom_core::capture`, from Bloom's `PlaceholderExpander`/`CaptureInserter`): `%t %T %u %U %^{prompt} %? %cursor %date %time %year %month %day %clipboard %(id) %%`; targets top, bottom, under a heading (by `:ID:`/`:CUSTOM_ID:`, else outline path; first or last child; a non-heading entry joins the heading's body), date tree (day/week/month, matched by date part, inside a heading or at top level). Entries are re-levelled like org-capture. English month/day names, as in timestamps.
- Templates are `[[capture]]` tables in `config.toml` (`key name file target heading id first tree template`); without any, three built-ins: `n` Note and `t` TODO at the bottom of the inbox, `j` Journal in a date tree of `journal.org` (Bloom puts its TODO template in `tasks.org`). `blsm capture -t KEY [--set PROMPT=TEXT] TEXT` (TEXT goes where `%?` is).
- Archive (`blossom_core::archive`, from Bloom's `ArchiveLocation`/`ArchiveProperties`): nearest `:ARCHIVE:` property, file drawer, `#+ARCHIVE:`, then config `archive` (default Emacs's `%s_archive::`, so `a.org` archives to `a.org_archive`, which the vault doesn't list, as in Emacs). Missing heading path is created; `ARCHIVE_TIME/FILE/OLPATH/CATEGORY/TODO` are stamped. No auto-archive on done yet.
- `blossom_vault::ops`: capture, complete, refile, archive and `restore` (undo), shared by `blsm` and the app. Headings are named by index + title (a mismatch is refused as "moved"); the destination file is written before the source; undo restores only files still holding exactly what the operation wrote (a created file is deleted). New CLI: `blsm archive FILE:LINE`, `blsm refile FILE:LINE (FILE | FILE:LINE)`.
- App: "Agenda" (Ctrl/Cmd+Shift+A) and "Capture…" (Ctrl/Cmd+N) in the sidebar; Refile… and Archive in the read view. Agenda boxes, capture, refile and archive run as worker `Op`s after writing waiting saves; the open file then adopts the new disk text. A notice bar reports the result with Undo. The capture panel and refile picker take the main area instead of floating: iced's `stack` layer dropped the text editor's text.
- Screenshot hook: `BLOSSOM_SCREENSHOT_AGENDA=1`, `BLOSSOM_SCREENSHOT_CAPTURE=1`.

## Phase 6: images and diagrams
- [x] Inline `[[file:…png|jpg|svg]]` in read view.
- [x] Paste/drag image → attachments folder + link.
- [x] Diagram canvas (pen, line, arrow, rect, ellipse, text, select/move, delete, undo, snap); SVG with JSON `<metadata>`.

Notes from Phase 6:
- Inline images follow org (and Bloom's `ParagraphImages`): a link without a description to a local png/jpg/jpeg/gif/webp/bmp/svg (`file:x`, `./x`, `/x`, `~/x`; `::search` dropped) becomes `render::Block::Image`, splitting its paragraph. Links with a description, and remote images, stay links. Each image has a caption with its path and an Open (or "Edit diagram" for SVG) button.
- The worker reads image bytes (`Request::LoadImage`, up to 64 MB); the app keeps them per open file (`OpenFile::pictures`) and shows "Missing image" for one that can't be read. Handles are made from bytes, so a re-saved diagram shows its new content.
- Attachments (`blossom_vault::attach`, names from Bloom's `AttachmentNames`): `notes/work.org`'s go in `notes/attachments/work/` (config `attachments` sets the folder name), with a safe, unique name; the link is relative to the org file. Not org-attach (`:ID:` folders, `attachment:` links); `.org`-only listing means attachments never show as notes.
- Dropping files on the window attaches them to the open file; Ctrl/Cmd+V attaches a clipboard image as `pasted-YYYYMMDD-HHMMSS.png` (via `arboard`, encoded with `image`), else pastes text in the editor as before. The link goes at the editor's cursor, or at the end of the selected heading's own text (`Document::append_line`, new).
- *Superseded the same week by the next note:* diagrams (`diagram.rs` model and SVG, `draw.rs` iced canvas editor): "Diagram…" in the read view starts one; "Edit diagram" under an SVG reopens it if it has Blossom metadata (`<metadata><blossom-diagram xmlns="urn:blossom:diagram">{"version":1,"shapes":[…]}</blossom-diagram></metadata>`). Shapes are pen, line, arrow, rectangle, ellipse and text, five colours, 10 px snap (on by default), select and drag to move, Delete, Ctrl/Cmd+Z/Y. The canvas and the saved file are white, so drawings read the same in Emacs and in dark mode. Saved as `diagram-YYYYMMDD-HHMMSS.svg` in the attachments folder; re-saving overwrites in place (only inside the vault). The SVG is cropped to the drawing plus a 16 px margin, and the JSON uses the SVG's coordinates.
- Diagrams, revised (section 0): `excalidraw.rs` reads and writes Excalidraw scenes; `draw.rs` edits them. Each element keeps its JSON as read and is written back unchanged unless Blossom changed it (then its fields, `version`, `versionNonce` and `updated` are updated); unknown fields, `appState` and `files` are kept, and `serde_json`'s `preserve_order` keeps key order. Drawn: rectangle (rounded per `roundness`), ellipse, diamond, line, arrow (arrow/triangle heads), freedraw, text (multi-line, alignment, font family 3 as monospace, the rest sans-serif), with stroke colour, width, dashed/dotted, background fill (hatched fills as a light solid), opacity and rotation. Other types (image, frame, embeds) are kept and shown as a labelled dashed box. Blossom's look is clean, not Excalidraw's hand-drawn one; new elements get `roughness: 0` so Excalidraw draws them the same. Arrows bind as in Excalidraw (`startBinding`/`endBinding` on the arrow, `boundElements` on the shape): drawing an arrow from or to a shape binds it, moving the shape re-aims the bound end at its centre and stops it `gap` outside the outline (`focus` 0; an arrow's label moves with its middle), dragging the arrow off a shape unbinds it, and deleting either side clears the links. Elbow arrows only have their bound end moved with the shape; Excalidraw re-routes them when it next edits them. A label (`containerId`) moves and deletes with its shape. Rotated text works on the canvas too: iced draws text in a rotated frame as glyph outlines.
- Files: "Diagram…" saves `diagram-YYYYMMDD-HHMMSS.excalidraw` then `.svg` (one free stem, `Vault::attach_set`) and links the `.svg`. "Edit diagram" opens `x.excalidraw` beside `x.svg`; saving writes the source (refused if it changed since it was opened) and then regenerates the SVG. When the worker loads an `.svg` whose source is newer, or the SVG is missing, it regenerates the SVG first (inside the vault; outside it is only shown). Hand edits to a generated SVG are lost on the next regeneration. An SVG from the first Phase 6 format (shapes in `<metadata>`) gets its `.excalidraw` written on first edit; other SVGs without a source can't be edited.
- Canvas: tools select, pen, line, arrow, rectangle, ellipse, diamond, text; five colours (Excalidraw's palette); 10-unit snap; Delete; Ctrl/Cmd+Z/Y; wheel pans (Shift for sideways), Ctrl/Cmd+wheel zooms around the pointer, Fit resets.
- New dependencies: iced features `canvas`, `image`, `svg`; `arboard` 3.7 (`image-data`, `wayland-data-control`); `image` 0.25 (png only; the same version iced uses).
- Screenshot hook: `BLOSSOM_SCREENSHOT_DIAGRAM=<link path>` opens that diagram.


## Phase 7: search screen and semantic search
Agreed with the user 2026-11-24; starts after the dependency upgrade and Linux packages. Keep it simple for end users: no query syntax or config needed to start.
- [x] App search screen (plain query language): sidebar "Search", results grouped by file, click opens the heading.
- [x] `crates/blossom-semantic`: `Embedder` trait, vector store (`<index_dir>/semantic/<id>.bin`, brute-force cosine), incremental sync by text hash, test embedder.
- [x] `Query::split_text`: positive plain-text terms become the semantic text; everything else stays a filter.
- [ ] Built-in model (candle BERT, pinned revision + SHA-256, downloaded to the data dir; feature `builtin-model`) and OpenAI-compatible server backend; `[semantic]` config (`model = english|multilingual|server`, `server`, `server_model`, `api_key_env`).
- [ ] `blsm search --semantic [--limit N]`, progress on stderr, `index --rebuild` drops vectors.
- [ ] App "By meaning" switch: model download button, background embedding thread with progress.

Notes from Phase 7:
- Search screen (`search.rs`): "Search" in the sidebar or Ctrl/Cmd+Shift+F. Each keystroke sends `Request::Search(query)`; the worker answers with `Event::Search(SearchResults { query, notes })` from `NoteIndex::search` (the same matcher and ranking as `blsm search`) and re-sends after every vault change while a query is set (`emit_live`, next to the agenda); the app keeps only results whose query matches the field, so late answers to earlier typing are dropped. Files are listed in order of their best-ranked heading; at most 200 headings are shown, then "and N more". A heading found by its body shows the matching line, cut to 110 characters around the match. The empty field lists clickable examples (`i.todo`, `t.work`, `s.today`, …). `OpenVault` gained `default_priority` for the `MatchContext`. Screenshot hook: `BLOSSOM_SCREENSHOT_SEARCH=<query>`.
- `blossom-semantic` (depends only on `blossom-index`): `Embedder` has `id`, `dims`, `embed_passages` (batched) and `embed_query`, separate because e5/bge word queries and passages differently. `VectorStore` keys each vector by FNV-1a of the heading's text (`passage` = title + body, trimmed), not its position, so inserting a heading embeds only it and equal texts share a vector. `sync(embedder, notes, batch, progress)` drops unused vectors and embeds missing ones a batch at a time; `progress` can return `Break` to stop, keeping what was done (`SyncReport { added, removed, left }`); it doesn't save, `save()` does (temp file + rename). `search(embedder, query, notes, limit)` ranks the given candidate notes by cosine (vectors stored normalised, so a dot product), leaving out notes without a vector. File `<index_dir>/semantic/<id>.bin` (id sanitised): `BLSV`, version 1, dims, id, count, then hash + f32s, little-endian, sorted by hash; a missing, damaged, other-version or other-embedder file means an empty store, rebuilt by the next sync. `TestEmbedder` (bag of hashed words) is public for other crates' tests.
- `Query::split_text() -> (String, Query)`: the words are the positive text terms every match needs (top-level AND, recursing into nested AND groups), in order, space-joined; a quoted phrase stays one piece. Negated words and anything inside an OR stay in the filter query, since removing them would change what matches (`milk OR tea` gives no words). Sort (`o.`) and `ad.` stay with the filter. Empty words mean: plain search, nothing to rank by meaning.

## Later
Order agreed with the user on 2026-11-24: dependency upgrade, then Linux packages, then semantic search (planned first).
- [x] `blsm` name-clash check: the user found no `blsm` binary or package on crates.io, AUR, Debian or Homebrew (2026-11-24).
- [x] rowan/nom upgrade in the orgize fork (before packaging): rowan 0.17, nom 8 (see `crates/orgize/UPSTREAM.md` patch 7); `cargo update` for the rest. tantivy still pulls nom 7 for its own query grammar.
- [x] Releases (re-scoped with the user 2026-11-24: releases first, then the AUR). A `Makefile` is the one build path (`make`, `make install`/`uninstall` with `PREFIX` (default `~/.local` for a user, `/usr/local` as root; a direct install writes the absolute binary path into the desktop entry)/`DESTDIR`, `dist`, `deb`, `appimage`, `dmg`, `check`); `install` never runs cargo, so `sudo make install` leaves no root-owned files. `.github/workflows/release.yml` builds on a `v*` tag (tag must match the Cargo version) and attaches to a **draft** release: Linux `.tar.gz`, `.deb` and AppImage (built in an `ubuntu:22.04` container, glibc 2.35), Arch `.pkg.tar.zst` (makepkg in `archlinux:base-devel` from `packaging/arch/PKGBUILD`), and two unsigned macOS `.dmg`s (arm64, x86_64; deployment target 11.0), plus `SHA256SUMS`. "Run workflow" builds the same artifacts without a release. App id `io.github.kvantos.Blossom` (desktop entry, AppStream metainfo, Wayland app id, window icon); icon `assets/blossom.svg` with PNGs from `make icons`. Licence is MIT only (the user's choice; Cargo said "MIT OR Apache-2.0"). appimagetool 1.9.1 pinned by SHA-256.
- [ ] AUR: publish `packaging/arch/PKGBUILD` (fill in the tarball's sha256sum) from the user's AUR account. *On hold (user, 2026-11-24): distribute as GitHub releases for now; no patch release needed for the `~/Notes` welcome change.*
- [ ] Flatpak / Flathub, macOS signing and notarization: later, need the user's accounts.
- [ ] Semantic search: planned with the user, see Phase 7 (after packaging).
- Postponed: nightly fuzzing. The user doesn't want a nightly toolchain; revisit when `cargo-fuzz` works on stable.
