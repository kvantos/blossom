//! `parse(x).to_org() == x` for any UTF-8 input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use orgize::Org;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    assert_eq!(Org::parse(text).to_org(), text);
});
