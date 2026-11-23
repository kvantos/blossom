//! Syncthing conflict copies (Bloom's `SyncConflicts`). When two devices
//! change a file, Syncthing keeps one version in place and writes the other as
//! `name.sync-conflict-YYYYMMDD-HHMMSS-DEVICEID.org` next to it.

/// The file a conflict copy belongs to (`notes/a.org` for
/// `notes/a.sync-conflict-20261121-143200-ABCDEF.org`), or `None` when `path`
/// is not a conflict copy.
#[must_use]
pub fn conflict_base(path: &str) -> Option<String> {
    const MARK: &str = ".sync-conflict-";
    let at = path.rfind(MARK)?;
    let (head, rest) = (&path[..at], &path[at + MARK.len()..]);
    if head.is_empty() || head.ends_with('/') {
        return None;
    }
    let b = rest.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    if !(digits(0..8) && b.get(8) == Some(&b'-') && digits(9..15) && b.get(15) == Some(&b'-')) {
        return None;
    }
    let tail = &rest[16..];
    let (device, ext) = tail
        .find('.')
        .map_or((tail, ""), |i| (&tail[..i], &tail[i..]));
    let ok = !device.is_empty()
        && device.bytes().all(|c| c.is_ascii_alphanumeric())
        && !ext[ext.len().min(1)..].contains('.');
    ok.then(|| format!("{head}{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_copies() {
        assert_eq!(
            conflict_base("a.sync-conflict-20261121-143200-ABC123.org").as_deref(),
            Some("a.org")
        );
        assert_eq!(
            conflict_base("dir/my.notes.sync-conflict-20261121-143200-X.org").as_deref(),
            Some("dir/my.notes.org")
        );
        assert_eq!(conflict_base("a.sync-conflict-2026112-143200-X.org"), None);
        assert_eq!(conflict_base("a.sync-conflict-20261121-143200-.org"), None);
        assert_eq!(
            conflict_base("a.sync-conflict-20261121-143200-X.org.bak"),
            None
        );
        assert_eq!(
            conflict_base("dir/.sync-conflict-20261121-143200-X.org"),
            None
        );
        assert_eq!(conflict_base("plain.org"), None);
    }
}
