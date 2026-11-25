//! blossom-core on any input:
//!
//! - a splice through `Document::replace_range` gives the same headings and
//!   preamble as a fresh parse of the spliced text;
//! - one edit (keyword, priority, tags, dates, done, properties, logbook,
//!   checkbox, move, promote, demote, delete, refile, insert, append) keeps
//!   that true, and leaves the text alone when it is refused;
//! - rendering and highlighting never panic, and highlight runs are
//!   ordered, inside the line and on char boundaries.
//!
//! Input layout: an edit byte, a heading byte, two little-endian u16
//! offsets, then the document, a NUL byte, and the replacement text.
#![no_main]

use blossom_core::{Document, InsertAt, Keywords, NewHeading, Timestamp, highlight, render};
use chrono::NaiveDate;
use libfuzzer_sys::fuzz_target;

fn snap(s: &str, i: usize) -> usize {
    let mut i = i % (s.len() + 1);
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The parse the incremental document must equal.
fn assert_fresh(d: &Document, what: &str) {
    let fresh = Document::parse(d.text(), d.keywords().clone());
    assert!(d.headings() == fresh.headings(), "{what}: headings differ from a fresh parse");
    assert!(d.preamble() == fresh.preamble(), "{what}: preamble differs from a fresh parse");
}

/// Applies edit number `op` to heading `h` (with `text` as a free-form
/// argument); `true` when it was accepted.
fn edit(d: &mut Document, op: u8, h: usize, text: &str, at: usize) -> bool {
    let now = NaiveDate::from_ymd_opt(2026, 11, 23)
        .and_then(|d| d.and_hms_opt(10, 30, 0))
        .unwrap_or_default();
    let line = text.lines().next().unwrap_or("");
    let stamp = Timestamp::parse(line).or_else(|| Timestamp::parse("<2026-11-24 Tue +1w>"));
    let n = d.headings().len().max(1);
    let other = (h + 1 + usize::from(op)) % n;
    match op % 20 {
        0 => d.set_keyword(h, Some("TODO")).is_ok(),
        1 => d.set_keyword(h, None).is_ok(),
        2 => d.set_priority(h, Some("A")).is_ok(),
        3 => d.set_tags(h, &line.split_whitespace().map(String::from).collect::<Vec<_>>()).is_ok(),
        4 => d.set_scheduled(h, stamp.as_ref()).is_ok(),
        5 => d.set_deadline(h, None).is_ok(),
        6 => d.mark_done(h, "DONE", now).is_ok(),
        7 => d.change_keyword(h, Some("NEXT"), now).is_ok(),
        8 => d.upsert_property(h, "KEY", line).is_ok(),
        9 => d.append_logbook_entry(h, line).is_ok(),
        10 => d.toggle_checkbox(at).is_ok(),
        11 => d.move_subtree(h, op & 0x80 != 0).is_ok(),
        12 => d.promote(h).is_ok(),
        13 => d.demote(h).is_ok(),
        14 => d.delete_subtree(h).is_ok(),
        15 => d.refile(h, (op & 0x80 != 0).then_some(other)).is_ok(),
        16 => {
            let new = NewHeading {
                title: line.to_string(),
                keyword: Some("TODO".into()),
                body: Some(text.to_string()),
                ..NewHeading::default()
            };
            let at = match op % 3 {
                0 => InsertAt::LastChild(h),
                1 => InsertAt::Before(h),
                _ => InsertAt::FirstTopLevel,
            };
            d.insert_heading(at, &new).is_ok()
        }
        17 => d.insert_subtree(InsertAt::After(h), text).is_ok(),
        18 => d.append_line((op & 0x80 != 0).then_some(h), line).is_ok(),
        _ => d.reopen(h, Some("TODO")).is_ok(),
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 6 {
        return;
    }
    let (op, h) = (data[0], usize::from(data[1]));
    let a = usize::from(u16::from_le_bytes([data[2], data[3]]));
    let b = usize::from(u16::from_le_bytes([data[4], data[5]]));
    let rest = &data[6..];
    let (doc, with) = match rest.iter().position(|&c| c == 0) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &[][..]),
    };
    let (Ok(doc), Ok(with)) = (std::str::from_utf8(doc), std::str::from_utf8(with)) else {
        return;
    };
    let keywords = Keywords::parse("TODO NEXT | DONE");

    // Rendering and highlighting.
    let d = Document::parse(doc, keywords.clone());
    let _ = render::blocks(doc);
    for i in 0..d.headings().len() {
        let _ = render::blocks(d.body_text(i));
    }
    let mut state = highlight::LineState::default();
    for line in doc.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let mut pos = 0;
        for (r, _) in highlight::highlight_line(line, &mut state, &keywords) {
            assert!(pos <= r.start && r.start <= r.end && r.end <= content.len());
            assert!(content.is_char_boundary(r.start) && content.is_char_boundary(r.end));
            pos = r.end;
        }
    }

    // A splice.
    let (a, b) = (snap(doc, a), snap(doc, b));
    let (start, end) = (a.min(b), a.max(b));
    let mut spliced = d.clone();
    spliced.replace_range(start..end, with);
    assert_eq!(spliced.text(), format!("{}{with}{}", &doc[..start], &doc[end..]));
    assert_fresh(&spliced, "splice");

    // One edit on the original document.
    let mut edited = d.clone();
    let heading = h % d.headings().len().max(1);
    if !edit(&mut edited, op, heading, with, start) {
        assert_eq!(edited.text(), doc, "a refused edit changed the text");
    }
    assert_fresh(&edited, "edit");
});
