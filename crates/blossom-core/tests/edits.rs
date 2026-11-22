//! Founding rule 3: edits are text splices. For every fixture and a fixed set
//! of ranges, `Org::replace_range` must give the same text as a naive string
//! splice, and the same syntax tree as parsing that text from scratch.

// Test helpers may unwrap (docs/PLAN.md section 2).
#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use orgize::{Org, TextRange, TextSize};

/// Replacement texts chosen to hit headline, drawer, block and EOL handling.
const REPLACEMENTS: &[&str] = &[
    "",
    "x",
    "\n",
    "\r\n",
    "* ",
    "** New\n",
    "\n* Split\n",
    "TODO ",
    "[#A] ",
    " :tag:",
    ":PROPERTIES:\n:ID: 1\n:END:\n",
    ":END:\n",
    "#+BEGIN_SRC\n",
    "#+END_SRC\n",
    "SCHEDULED: <2026-11-21 Sat +1w>\n",
    "- [ ] item\n",
    "🌸",
];

/// Ranges per fixture beyond the line-based ones.
const RANDOM_RANGES: usize = 150;

/// Small deterministic PRNG (`SplitMix64`) so failures reproduce exactly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n as u64)).unwrap()
    }
}

/// Byte offsets that fall on char boundaries, including 0 and `len`.
fn boundaries(s: &str) -> Vec<usize> {
    (0..=s.len()).filter(|&i| s.is_char_boundary(i)).collect()
}

/// Offsets of every line start, plus `len`.
fn line_starts(s: &str) -> Vec<usize> {
    let mut v = vec![0];
    v.extend(s.match_indices('\n').map(|(i, _)| i + 1));
    if *v.last().unwrap() != s.len() {
        v.push(s.len());
    }
    v
}

/// Deterministic `(start, end)` ranges for one text.
fn ranges(text: &str, seed: u64) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    // Whole lines, single and pairs, plus empty ranges at line starts.
    let lines = line_starts(text);
    for w in lines.windows(2) {
        out.push((w[0], w[1]));
        out.push((w[0], w[0]));
    }
    for w in lines.windows(3) {
        out.push((w[0], w[2]));
    }
    // Arbitrary char-boundary ranges.
    let b = boundaries(text);
    let mut rng = Rng(seed);
    for _ in 0..RANDOM_RANGES {
        let i = rng.below(b.len());
        let j = rng.below(b.len());
        out.push((b[i.min(j)], b[i.max(j)]));
    }
    out
}

fn size(n: usize) -> TextSize {
    TextSize::from(u32::try_from(n).unwrap())
}

#[test]
fn replace_range_matches_splice_and_fresh_parse() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for (n, fixture) in common::fixtures().into_iter().enumerate() {
        let text = &fixture.text;
        let base = Org::parse(text);
        for (k, (start, end)) in ranges(text, n as u64).into_iter().enumerate() {
            let with = REPLACEMENTS[k % REPLACEMENTS.len()];
            let expected = format!("{}{with}{}", &text[..start], &text[end..]);

            let mut org = Org::parse(text);
            debug_assert_eq!(org.to_org(), base.to_org());
            org.replace_range(TextRange::new(size(start), size(end)), with);
            checked += 1;

            let what = format!("{} [{start}..{end}] <- {with:?}", fixture.path.display());
            if let Some(diff) = common::describe_diff(&expected, &org.to_org()) {
                failures.push(format!("{what}: text differs from splice: {diff}"));
                continue;
            }
            let fresh = Org::parse(&expected);
            if org.green() != fresh.green() {
                let (got, want) = (common::tree(&org), common::tree(&fresh));
                let at = common::first_diff(&want, &got).unwrap_or(0);
                failures.push(format!(
                    "{what}: tree differs from fresh parse\n  want: {}\n  got:  {}",
                    common::context(&want, at),
                    common::context(&got, at),
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} edits failed (set EDITS_ALL=1 to show all):\n{}",
        failures.len(),
        failures
            .iter()
            .take(if std::env::var_os("EDITS_ALL").is_some() {
                usize::MAX
            } else {
                20
            })
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
