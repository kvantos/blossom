//! Semantic search for Blossom: rank headings by meaning, not by words.
//!
//! An [`Embedder`] turns text into vectors: a built-in [`Model`] run with
//! candle (feature `builtin-model`, downloaded once by `download`), an
//! OpenAI-compatible server ([`ServerEmbedder`]), or [`TestEmbedder`] for
//! tests. [`Backend::from_config`] reads the choice from `[semantic]` and
//! [`open_embedder`] makes the embedder. A [`VectorStore`] keeps one
//! vector per distinct heading text, keyed by a hash of that text, in
//! `<index_dir>/semantic/<id>.bin`. Like the index, it is only a cache:
//! delete it and [`VectorStore::sync`] rebuilds it from the notes.
//!
//! Keying by text rather than by position means inserting a heading only
//! embeds that heading, and identical texts share one vector. Vectors are
//! stored normalised, so cosine similarity is a dot product; search is
//! brute force over the candidate notes, which is fast enough for tens of
//! thousands of headings.

#[cfg(feature = "builtin-model")]
mod bert;
#[cfg(feature = "builtin-model")]
mod download;
mod model;
mod server;
mod store;

use std::fmt;
use std::path::Path;

use blossom_index::Note;
use blossom_vault::config::SemanticConfig;

#[cfg(feature = "builtin-model")]
pub use bert::{BertEmbedder, MAX_TOKENS};
#[cfg(feature = "builtin-model")]
pub use download::{DownloadProgress, download};
pub use model::{Model, ModelFile, Pooling};
pub use server::{DEFAULT_MODEL, DEFAULT_URL, ServerEmbedder, ServerSettings};
pub use store::{Hit, Progress, SyncReport, VectorStore};

/// Which embedder to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// A built-in model.
    Builtin(Model),
    /// A server.
    Server(ServerSettings),
}

impl Backend {
    /// The choice in `[semantic]`: `model = "english"` (the default),
    /// `"multilingual"` or `"server"` (with `server`, `server_model` and
    /// `api_key_env`, each optional).
    ///
    /// # Errors
    ///
    /// For an unknown model name, or an `api_key_env` that isn't set.
    pub fn from_config(c: &SemanticConfig) -> Result<Backend, String> {
        let name = c.model.as_deref().unwrap_or("english");
        if let Some(m) = Model::from_name(name) {
            return Ok(Backend::Builtin(m));
        }
        if !name.trim().eq_ignore_ascii_case("server") {
            return Err(format!(
                "[semantic] model = \"{name}\": use \"english\", \"multilingual\" or \"server\""
            ));
        }
        let api_key = match c.api_key_env.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(var) => Some(
                std::env::var(var)
                    .map_err(|_| format!("[semantic] api_key_env: ${var} is not set"))?,
            ),
        };
        let pick = |v: &Option<String>, default: &str| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(default)
                .to_string()
        };
        Ok(Backend::Server(ServerSettings {
            url: pick(&c.server, DEFAULT_URL),
            model: pick(&c.server_model, DEFAULT_MODEL),
            api_key,
        }))
    }
}

/// An embedder that can move to and be shared with a background thread.
pub type SharedEmbedder = Box<dyn Embedder + Send + Sync>;

/// The embedder for `backend`; a built-in model is loaded from under
/// `data_dir` and must be downloaded first.
///
/// # Errors
///
/// When the model isn't downloaded or can't be loaded, the server can't be
/// reached, or this build has no built-in models.
pub fn open_embedder(backend: &Backend, data_dir: &Path) -> Result<SharedEmbedder, SemanticError> {
    match backend {
        #[cfg(feature = "builtin-model")]
        Backend::Builtin(m) => Ok(Box::new(BertEmbedder::load(*m, data_dir)?)),
        #[cfg(not(feature = "builtin-model"))]
        Backend::Builtin(_) => {
            let _ = data_dir;
            Err(SemanticError::Embed(
                "this build has no built-in models; set model = \"server\" in [semantic]".into(),
            ))
        }
        Backend::Server(s) => Ok(Box::new(ServerEmbedder::connect(s.clone())?)),
    }
}

/// Turns text into vectors.
///
/// Models are trained with different wording for queries and for the
/// passages searched (e5 wants `query: ` and `passage: ` prefixes, bge an
/// instruction before queries), so the two are separate calls.
pub trait Embedder {
    /// A stable name for the model and its settings, e.g.
    /// `bge-small-en-v1.5`. It names the store file, so vectors from
    /// different models never mix.
    fn id(&self) -> &str;

    /// Length of every vector.
    fn dims(&self) -> usize;

    /// One vector per passage, in order.
    ///
    /// # Errors
    ///
    /// When the model or server fails.
    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticError>;

    /// The vector for a search query.
    ///
    /// # Errors
    ///
    /// When the model or server fails.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, SemanticError>;
}

/// Why semantic search failed.
#[derive(Debug)]
pub enum SemanticError {
    /// Reading or writing the store.
    Io(std::io::Error),
    /// The embedder failed.
    Embed(String),
    /// The built-in model has to be downloaded first.
    NotDownloaded(Model),
    /// The embedder returned the wrong number of vectors or a vector of the
    /// wrong length.
    Shape {
        /// What was expected.
        expected: usize,
        /// What came back.
        got: usize,
    },
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticError::Io(e) => write!(f, "semantic store: {e}"),
            SemanticError::Embed(e) => write!(f, "embedding: {e}"),
            SemanticError::NotDownloaded(m) => {
                write!(f, "the {} model is not downloaded yet", m.name())
            }
            SemanticError::Shape { expected, got } => {
                write!(f, "embedding: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for SemanticError {}

impl From<std::io::Error> for SemanticError {
    fn from(e: std::io::Error) -> SemanticError {
        SemanticError::Io(e)
    }
}

/// The text embedded for a heading: its title and body.
#[must_use]
pub fn passage(note: &Note) -> &str {
    note.record.text.trim()
}

/// The store key for a passage.
#[must_use]
pub fn text_hash(text: &str) -> u64 {
    fnv1a(text.as_bytes())
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher` (the same
/// function names index folders in `blossom_vault::config`).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A deterministic stand-in for a model: each lowercase word adds 1 to the
/// slot its hash picks. Texts sharing words score as similar. For tests,
/// here and in the crates that use this one.
#[derive(Clone, Debug)]
pub struct TestEmbedder {
    dims: usize,
    id: String,
}

impl TestEmbedder {
    /// An embedder with vectors of length `dims` (at least 1).
    #[must_use]
    pub fn new(dims: usize) -> TestEmbedder {
        let dims = dims.max(1);
        TestEmbedder {
            dims,
            id: format!("test-{dims}"),
        }
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0; self.dims];
        let words = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty());
        for w in words {
            let slot = fnv1a(w.to_lowercase().as_bytes()) % self.dims as u64;
            // In range: `slot` < `dims`, a usize.
            if let Some(x) = usize::try_from(slot).ok().and_then(|i| v.get_mut(i)) {
                *x += 1.0;
            }
        }
        v
    }
}

impl Embedder for TestEmbedder {
    fn id(&self) -> &str {
        &self.id
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticError> {
        Ok(texts.iter().map(|t| self.embed(t)).collect())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, SemanticError> {
        Ok(self.embed(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_from_config() {
        let c = |toml: &[(&str, &str)]| {
            let mut c = SemanticConfig::default();
            for (k, v) in toml {
                let v = Some((*v).to_string());
                match *k {
                    "model" => c.model = v,
                    "server" => c.server = v,
                    "server_model" => c.server_model = v,
                    _ => c.api_key_env = v,
                }
            }
            Backend::from_config(&c)
        };
        assert_eq!(c(&[]), Ok(Backend::Builtin(Model::English)));
        assert_eq!(
            c(&[("model", "Multilingual")]),
            Ok(Backend::Builtin(Model::Multilingual))
        );
        assert_eq!(
            c(&[("model", "server")]),
            Ok(Backend::Server(ServerSettings {
                url: DEFAULT_URL.into(),
                model: DEFAULT_MODEL.into(),
                api_key: None,
            }))
        );
        let s = c(&[
            ("model", "server"),
            ("server", "http://x/v1"),
            ("server_model", "e"),
        ]);
        assert!(
            matches!(s, Ok(Backend::Server(ServerSettings { url, model, .. })) if url == "http://x/v1" && model == "e")
        );
        // PATH is always set; a made-up name isn't.
        assert!(matches!(
            c(&[("model", "server"), ("api_key_env", "PATH")]),
            Ok(Backend::Server(ServerSettings {
                api_key: Some(_),
                ..
            }))
        ));
        let e = c(&[
            ("model", "server"),
            ("api_key_env", "BLOSSOM_NO_SUCH_VAR_42"),
        ]);
        assert!(e.unwrap_err().contains("BLOSSOM_NO_SUCH_VAR_42"));
        assert!(c(&[("model", "gpt")]).unwrap_err().contains("\"gpt\""));
    }

    #[test]
    fn stable_hash() {
        // Same values as `blossom_vault::config`'s copy.
        assert_eq!(text_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(text_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn test_embedder_counts_words() {
        let e = TestEmbedder::new(16);
        assert_eq!(e.id(), "test-16");
        let v = e.embed_query("Milk, milk and BREAD").unwrap();
        assert_eq!(v.len(), 16);
        assert!((v.iter().sum::<f32>() - 4.0).abs() < f32::EPSILON);
        assert_eq!(
            e.embed_passages(&["milk"]).unwrap()[0],
            e.embed_query("MILK").unwrap()
        );
    }
}
