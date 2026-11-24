//! Semantic search for Blossom: rank headings by meaning, not by words.
//!
//! An [`Embedder`] turns text into vectors (a built-in model or a server,
//! added later; [`TestEmbedder`] for tests). A [`VectorStore`] keeps one
//! vector per distinct heading text, keyed by a hash of that text, in
//! `<index_dir>/semantic/<id>.bin`. Like the index, it is only a cache:
//! delete it and [`VectorStore::sync`] rebuilds it from the notes.
//!
//! Keying by text rather than by position means inserting a heading only
//! embeds that heading, and identical texts share one vector. Vectors are
//! stored normalised, so cosine similarity is a dot product; search is
//! brute force over the candidate notes, which is fast enough for tens of
//! thousands of headings.

mod store;

use std::fmt;

use blossom_index::Note;

pub use store::{Hit, Progress, SyncReport, VectorStore};

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
