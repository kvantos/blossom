//! Developer hook: `BLOSSOM_SCREENSHOT=out.png blossom VAULT` loads the first
//! file (or `BLOSSOM_SCREENSHOT_FILE`), selects heading
//! `BLOSSOM_SCREENSHOT_HEADING` if given (opening its editor when
//! `BLOSSOM_SCREENSHOT_EDIT` is set), saves a screenshot of the window as
//! PNG and exits. `BLOSSOM_THEME=dark|light` overrides the system mode. Used
//! to check the UI without a screenshot tool; not part of normal use.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// What the hook should capture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Where to write the PNG.
    pub out: PathBuf,
    /// The file to open.
    pub file: Option<String>,
    /// The heading to select.
    pub heading: Option<usize>,
    /// A forced light/dark mode.
    pub mode: Option<iced::theme::Mode>,
    /// Open the section editor before capturing.
    pub edit: bool,
}

impl Plan {
    /// The plan from the environment, if the hook is on.
    #[must_use]
    pub fn from_env() -> Option<Plan> {
        let out = std::env::var_os("BLOSSOM_SCREENSHOT")?;
        Some(Plan {
            out: PathBuf::from(out),
            file: std::env::var("BLOSSOM_SCREENSHOT_FILE").ok(),
            heading: std::env::var("BLOSSOM_SCREENSHOT_HEADING")
                .ok()
                .and_then(|h| h.parse().ok()),
            mode: match std::env::var("BLOSSOM_THEME").ok().as_deref() {
                Some("dark") => Some(iced::theme::Mode::Dark),
                Some("light") => Some(iced::theme::Mode::Light),
                _ => None,
            },
            edit: std::env::var_os("BLOSSOM_SCREENSHOT_EDIT").is_some(),
        })
    }
}

/// Writes RGBA pixels as a PNG (uncompressed deflate: simple, big, fine for a debug hook).
///
/// # Errors
///
/// On I/O errors or a size that doesn't match the pixels.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    let row = width as usize * 4;
    if rgba.len() != row * height as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pixel count does not match size",
        ));
    }
    // Scanlines, each with filter byte 0.
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgba.chunks(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    // zlib stream of stored deflate blocks (at most 65535 bytes each).
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(0xFFFF).peekable();
    if blocks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(b) = blocks.next() {
        let len = u16::try_from(b.len()).unwrap_or(u16::MAX);
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut f = io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"\x89PNG\r\n\x1a\n")?;
    chunk(&mut f, *b"IHDR", &ihdr)?;
    chunk(&mut f, *b"IDAT", &z)?;
    chunk(&mut f, *b"IEND", &[])?;
    f.flush()
}

fn chunk(w: &mut impl Write, kind: [u8; 4], data: &[u8]) -> io::Result<()> {
    let len = u32::try_from(data.len()).map_err(|_| io::Error::other("chunk too big"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&kind)?;
    w.write_all(data)?;
    let mut crc = crc32_update(0xFFFF_FFFF, &kind);
    crc = crc32_update(crc, data);
    w.write_all(&(!crc).to_be_bytes())
}

fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in bytes {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums() {
        assert_eq!(!crc32_update(0xFFFF_FFFF, b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn writes_a_png() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.png");
        write_png(&p, 2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(write_png(&p, 3, 3, &[0; 4]).is_err());
    }
}
