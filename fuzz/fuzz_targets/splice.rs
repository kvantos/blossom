//! `replace_range` gives the same text as a naive splice and the same tree
//! as a fresh parse.
//!
//! Input layout: 4 bytes (two little-endian u16 offsets), then the document,
//! a NUL byte, and the replacement text.
#![no_main]

use libfuzzer_sys::fuzz_target;
use orgize::{Org, TextRange, TextSize};

fn snap(s: &str, i: usize) -> usize {
    let mut i = i % (s.len() + 1);
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let a = usize::from(u16::from_le_bytes([data[0], data[1]]));
    let b = usize::from(u16::from_le_bytes([data[2], data[3]]));
    let rest = &data[4..];
    let (doc, with) = match rest.iter().position(|&c| c == 0) {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, &[][..]),
    };
    let (Ok(doc), Ok(with)) = (std::str::from_utf8(doc), std::str::from_utf8(with)) else {
        return;
    };
    let (a, b) = (snap(doc, a), snap(doc, b));
    let (start, end) = (a.min(b), a.max(b));
    let expected = format!("{}{with}{}", &doc[..start], &doc[end..]);

    let mut org = Org::parse(doc);
    let size = |n: usize| TextSize::from(u32::try_from(n).unwrap());
    org.replace_range(TextRange::new(size(start), size(end)), with);
    assert_eq!(org.to_org(), expected);
    assert!(org.green() == Org::parse(&expected).green(), "tree differs from fresh parse");
});
