# Upstream

- Source: https://github.com/PoiScript/orgize
- Commit: `5f26c94dcec2a33b37b1c880ace053b29b5d021e` (2024-07-22)
- Version: `0.10.0-alpha.10`
- License: MIT (see `LICENSE`)
- Copied: 2026-11-21, without `.git/`, `.github/`, `target/`, `wasm/`, `fuzz/`, `development.md`

## Local patches

1. `Cargo.toml`: removed `[workspace]` (now a member of the Blossom workspace), moved
   `[profile.dev.package]` to the workspace root, added `publish = false`.
2. `Cargo.toml`: `[lints]` table allowing `clippy::all` and
   `mismatched_lifetime_syntaxes` (warnings from newer rustc/clippy on unchanged
   upstream code), so `cargo clippy --all-targets -- -D warnings` passes.
   No compiler-required fixes were needed: `cargo test -p orgize` passed as copied
   (rustc 1.99.1).
3. `Cargo.toml`: removed the `[[bench]]` target and set `autobenches = false`.
   `benches/parse.rs` needs corpus files that upstream downloads from orgmode.org;
   the source file is kept.
4. **Bug fix**, `src/replace.rs` `replace_headline`: the incremental path re-parsed
   only the edited headline and kept the first headline `headline_node` returned,
   silently **dropping** any remaining text (e.g. inserting `"* "` at a line start
   inside a headline). It also kept a whole-headline replacement at a deeper level
   (`* b` → `** b`) as a sibling where a fresh parse nests it under the previous
   headline, and it panicked on an `unwrap` when the replacement didn't start with
   stars (`"x\n** y\n"`). Now `reparse_headline` accepts the fast path only if the
   text parses as exactly one headline, still ends with a newline (unless last in
   the document), and any level change can't re-parent it; otherwise it falls back
   to `full_parse`. Regression cases added at the end of the `replace` test; found
   by `blossom-core/tests/edits.rs` (91 of 8885 edits failed before).
5. **Bug fix**, `src/syntax/element.rs` `element_nodes`: `debug_assert!` on empty
   input fired for an empty greater block (`#+begin_quote\n#+end_quote`). Empty input
   now returns no elements (what release builds already did). Found by proptest;
   regression fixture `blossom-core/tests/fixtures/edge/empty-block.org`.
