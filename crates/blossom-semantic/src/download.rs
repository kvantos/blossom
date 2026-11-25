//! Downloading a built-in model's pinned files from Hugging Face.
//!
//! Each file is streamed to `<name>.part` while it is hashed, and renamed
//! into place only when its size and SHA-256 match, so a broken or stopped
//! download never looks finished. Files already in place are kept.

use std::fs;
use std::io::{Read, Write};
use std::ops::ControlFlow;
use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::SemanticError;
use crate::model::{Model, ModelFile};

/// Bytes downloaded so far, of the model's total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadProgress {
    /// Bytes in place or received.
    pub done: u64,
    /// The model's total size.
    pub total: u64,
}

/// Downloads `model` into `model.dir(data_dir)`. `progress` hears now and
/// then; returning `Break` stops, and a later call carries on from the
/// files already finished.
///
/// # Errors
///
/// On network or disk errors, a checksum mismatch, or when stopped
/// (`SemanticError::Embed("download stopped")`).
pub fn download(
    model: Model,
    data_dir: &Path,
    progress: &mut dyn FnMut(DownloadProgress) -> ControlFlow<()>,
) -> Result<(), SemanticError> {
    let base = format!(
        "https://huggingface.co/{}/resolve/{}",
        model.repo(),
        model.revision()
    );
    download_files(&base, model.files(), &model.dir(data_dir), progress)
}

/// Downloads `files` from `base/<name>` into `dir`.
fn download_files(
    base: &str,
    files: &[ModelFile],
    dir: &Path,
    progress: &mut dyn FnMut(DownloadProgress) -> ControlFlow<()>,
) -> Result<(), SemanticError> {
    fs::create_dir_all(dir)?;
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let total = files.iter().map(|f| f.size).sum();
    let mut done = 0;
    for f in files {
        let path = dir.join(f.name);
        if fs::metadata(&path).is_ok_and(|m| m.len() == f.size) {
            done += f.size;
            continue;
        }
        let url = format!("{base}/{}", f.name);
        let fail = |what: String| SemanticError::Embed(format!("downloading {url}: {what}"));
        let resp = agent.get(&url).call().map_err(|e| fail(e.to_string()))?;
        let mut body = resp.into_body().into_reader();
        let part = dir.join(format!("{}.part", f.name));
        let mut out = fs::File::create(&part)?;
        let mut hash = Sha256::new();
        let mut got: u64 = 0;
        let mut buf = vec![0; 256 * 1024];
        let result = loop {
            let n = match body.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => n,
                Err(e) => break Err(fail(e.to_string())),
            };
            got += n as u64;
            if got > f.size {
                break Err(fail(format!("larger than the expected {} bytes", f.size)));
            }
            hash.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            let step = DownloadProgress {
                done: done + got,
                total,
            };
            if progress(step).is_break() {
                break Err(SemanticError::Embed("download stopped".into()));
            }
        };
        let result = result.and_then(|()| {
            let sha = hex(&hash.finalize());
            if got != f.size || sha != f.sha256 {
                return Err(fail(format!(
                    "got {got} bytes with SHA-256 {sha}, expected {} bytes with {}",
                    f.size, f.sha256
                )));
            }
            out.sync_all()?;
            Ok(())
        });
        drop(out);
        if let Err(e) = result {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
        fs::rename(&part, &path)?;
        done += f.size;
    }
    let _ = progress(DownloadProgress { done, total });
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    /// Serves `files` (name → bytes) over HTTP until the test ends.
    fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/m", listener.local_addr().unwrap());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                let path = first.split(' ').nth(1).unwrap_or("");
                let found = files.iter().find(|(n, _)| path == format!("/m/{n}"));
                let (status, body) = found.map_or((404, &b""[..]), |(_, b)| (200, &b[..]));
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
        });
        base
    }

    fn spec(name: &'static str, data: &[u8]) -> ModelFile {
        let sha: &'static str = Box::leak(hex(&Sha256::digest(data)).into_boxed_str());
        ModelFile {
            name,
            size: data.len() as u64,
            sha256: sha,
        }
    }

    fn go(base: &str, files: &[ModelFile], dir: &Path) -> Result<(), SemanticError> {
        download_files(base, files, dir, &mut |_| ControlFlow::Continue(()))
    }

    /// Downloads the real English model into `BLOSSOM_TEST_DOWNLOAD_DIR`
    /// (HTTPS, Hugging Face's redirects, the pinned checksums):
    /// `cargo test -p blossom-semantic --features builtin-model -- --ignored`.
    #[test]
    #[ignore = "downloads 134 MB"]
    fn downloads_the_english_model() {
        let dir = std::env::var("BLOSSOM_TEST_DOWNLOAD_DIR").expect("BLOSSOM_TEST_DOWNLOAD_DIR");
        let dir = Path::new(&dir);
        download(Model::English, dir, &mut |_| ControlFlow::Continue(())).unwrap();
        assert!(Model::English.is_downloaded(dir));
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(
            hex(&Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn downloads_checks_and_resumes() {
        let a = b"alpha".to_vec();
        let b = vec![7u8; 600_000];
        let base = serve(vec![("a.json", a.clone()), ("b.bin", b.clone())]);
        let dir = tempfile::tempdir().unwrap();
        let files = [spec("a.json", &a), spec("b.bin", &b)];
        // Stopped part-way: the first file is done, the second isn't.
        let mut calls = 0;
        let r = download_files(&base, &files, dir.path(), &mut |p| {
            assert_eq!(p.total, 600_005);
            calls += 1;
            if p.done > 5 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert!(matches!(r, Err(SemanticError::Embed(m)) if m == "download stopped"));
        assert_eq!(fs::read(dir.path().join("a.json")).unwrap(), a);
        assert!(!dir.path().join("b.bin").exists());
        assert!(!dir.path().join("b.bin.part").exists());
        // Carrying on fetches the rest.
        let mut last = None;
        download_files(&base, &files, dir.path(), &mut |p| {
            last = Some(p);
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(
            last,
            Some(DownloadProgress {
                done: 600_005,
                total: 600_005
            })
        );
        assert_eq!(fs::read(dir.path().join("b.bin")).unwrap(), b);
        assert!(calls >= 2);
    }

    #[test]
    fn rejects_wrong_content_and_missing_files() {
        let base = serve(vec![("a.json", b"tampered".to_vec())]);
        let dir = tempfile::tempdir().unwrap();
        let mut want = spec("a.json", b"original");
        want.size = 8;
        let err = go(&base, &[want], dir.path()).unwrap_err().to_string();
        assert!(err.contains("SHA-256"), "{err}");
        assert!(!dir.path().join("a.json").exists());
        assert!(!dir.path().join("a.json.part").exists());
        let err = go(&base, &[spec("gone.bin", b"x")], dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("404"), "{err}");
    }
}
