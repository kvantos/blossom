//! Developer hook: `BLOSSOM_SCREENSHOT=out.png blossom VAULT` loads the first
//! file (or `BLOSSOM_SCREENSHOT_FILE`), selects heading
//! `BLOSSOM_SCREENSHOT_HEADING` if given (opening its editor when
//! `BLOSSOM_SCREENSHOT_EDIT` is set) or shows the search screen for
//! `BLOSSOM_SCREENSHOT_SEARCH` (a query, may be empty; by meaning with
//! `BLOSSOM_SCREENSHOT_MEANING`), opens the Appearance panel with
//! `BLOSSOM_SCREENSHOT_APPEARANCE`, saves a screenshot of the window as
//! PNG and exits. `BLOSSOM_THEME` (`light`, `dark`, or
//! `solarized-`/`gray-` + `light`/`dark`) overrides the saved theme. Used
//! to check the UI without a screenshot tool; not part of normal use.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// What the hook should capture.
#[derive(Clone, Debug, Default, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // independent options
pub struct Plan {
    /// Where to write the PNG.
    pub out: PathBuf,
    /// The file to open.
    pub file: Option<String>,
    /// The heading to select.
    pub heading: Option<usize>,
    /// A forced light/dark mode.
    pub mode: Option<iced::theme::Mode>,
    /// Forced colours (with `mode`).
    pub colours: Option<crate::style::Colours>,
    /// Open the section editor before capturing.
    pub edit: bool,
    /// Show the agenda instead of a file.
    pub agenda: bool,
    /// Show the search screen with this query instead of a file.
    pub search: Option<String>,
    /// With the "By meaning" switch on.
    pub meaning: bool,
    /// Open the capture panel before capturing.
    pub capture: bool,
    /// Open the Appearance panel before capturing.
    pub appearance: bool,
    /// Open this diagram (a link path in the file) before capturing.
    pub diagram: Option<String>,
}

/// `BLOSSOM_THEME`'s value: `light`, `dark`, `solarized-dark`, `gray-light`…
fn parse_theme(value: &str) -> Option<(crate::style::Colours, iced::theme::Mode)> {
    use crate::style::Colours;
    let (colours, mode) = match value.rsplit_once('-') {
        Some(("solarized", m)) => (Colours::Solarized, m),
        Some(("gray", m)) => (Colours::Gray, m),
        Some(("blossom", m)) => (Colours::Blossom, m),
        None => (Colours::Blossom, value),
        Some(_) => return None,
    };
    let mode = match mode {
        "light" => iced::theme::Mode::Light,
        "dark" => iced::theme::Mode::Dark,
        _ => return None,
    };
    Some((colours, mode))
}

impl Plan {
    /// The plan from the environment, if the hook is on.
    #[must_use]
    pub fn from_env() -> Option<Plan> {
        let out = std::env::var_os("BLOSSOM_SCREENSHOT")?;
        let theme = std::env::var("BLOSSOM_THEME")
            .ok()
            .and_then(|t| parse_theme(&t));
        Some(Plan {
            out: PathBuf::from(out),
            file: std::env::var("BLOSSOM_SCREENSHOT_FILE").ok(),
            heading: std::env::var("BLOSSOM_SCREENSHOT_HEADING")
                .ok()
                .and_then(|h| h.parse().ok()),
            mode: theme.map(|t| t.1),
            colours: theme.map(|t| t.0),
            edit: std::env::var_os("BLOSSOM_SCREENSHOT_EDIT").is_some(),
            agenda: std::env::var_os("BLOSSOM_SCREENSHOT_AGENDA").is_some(),
            search: std::env::var("BLOSSOM_SCREENSHOT_SEARCH").ok(),
            meaning: std::env::var_os("BLOSSOM_SCREENSHOT_MEANING").is_some(),
            capture: std::env::var_os("BLOSSOM_SCREENSHOT_CAPTURE").is_some(),
            appearance: std::env::var_os("BLOSSOM_SCREENSHOT_APPEARANCE").is_some(),
            diagram: std::env::var("BLOSSOM_SCREENSHOT_DIAGRAM").ok(),
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
    fn theme_names() {
        use crate::style::Colours;
        use iced::theme::Mode;
        assert_eq!(parse_theme("dark"), Some((Colours::Blossom, Mode::Dark)));
        assert_eq!(
            parse_theme("gray-light"),
            Some((Colours::Gray, Mode::Light))
        );
        assert_eq!(
            parse_theme("solarized-dark"),
            Some((Colours::Solarized, Mode::Dark))
        );
        assert_eq!(parse_theme("pink-dark"), None);
        assert_eq!(parse_theme("gray"), None);
    }

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
