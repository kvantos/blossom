//! Founding rule 2: parsing is lossless, `parse(x).to_org() == x` byte for byte.

// Test helpers may unwrap (docs/PLAN.md section 2).
#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use orgize::Org;

#[test]
fn every_fixture_round_trips() {
    let mut failures = Vec::new();
    for fixture in common::fixtures() {
        let out = Org::parse(&fixture.text).to_org();
        if let Some(diff) = common::describe_diff(&fixture.text, &out) {
            failures.push(format!("{}: {diff}", fixture.path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} fixture(s) failed to round-trip:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
