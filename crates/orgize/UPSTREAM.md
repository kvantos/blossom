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
6. `src/org.rs`: `Org` derives `Clone` (a `GreenNode` and a `ParseConfig`, both cheap
   to clone), so Blossom can copy a parsed document and splice edits into the copy.
7. **Dependency upgrade** (2026-11-24): `rowan` 0.15 → 0.17 (no code changes needed) and
   `nom` 7 → 8. `src/syntax/input.rs` implements nom 8's single `Input` trait in place of
   `InputLength`/`InputIter`/`InputTake`/`InputTakeAtPosition`/`Slice`, and `Input` gained
   inherent `slice` and `take_split` so call sites read as before. Parsers are no longer
   callable in nom 8, so `p(input)` became `p.parse(input)` (also in `lossless_parser!`),
   the deprecated `tuple((a, b))` became the tuple `(a, b)`, and `ParserIterator`s are
   iterated through `by_ref()` so `finish()` can still be called. The `criterion`
   dev-dependency was removed (the bench target is off, see 3).
