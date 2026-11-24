//! The vector store: one normalised vector per distinct passage, keyed by
//! [`text_hash`], saved as a small binary file.
//!
//! File layout, little-endian: `BLSV`, format version (u32), dims (u32),
//! id length (u32) and id bytes, count (u64), then per vector its hash (u64)
//! and `dims` f32s. A file that is missing, damaged, of another version or
//! for another embedder is ignored: the store starts empty and the next
//! sync rebuilds it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use blossom_index::Note;

use crate::{Embedder, SemanticError, passage, text_hash};

const MAGIC: &[u8; 4] = b"BLSV";
const VERSION: u32 = 1;

/// Vectors for an embedder, optionally backed by a file.
#[derive(Clone, Debug)]
pub struct VectorStore {
    id: String,
    dims: usize,
    file: Option<PathBuf>,
    vectors: HashMap<u64, Vec<f32>>,
}

/// How far a sync has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Passages embedded so far in this sync.
    pub done: usize,
    /// Passages this sync has to embed.
    pub total: usize,
}

/// What a sync did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Passages embedded.
    pub added: usize,
    /// Vectors dropped because no heading has that text any more.
    pub removed: usize,
    /// Passages still without a vector (the sync was stopped early).
    pub left: usize,
}

/// A search result: a note (by its position in the slice searched) and its
/// cosine similarity to the query, from -1 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Index into the notes passed to [`VectorStore::search`].
    pub index: usize,
    /// Similarity; higher is closer.
    pub score: f32,
}

impl VectorStore {
    /// Where the store for embedder `id` lives under `index_dir`.
    #[must_use]
    pub fn file_for(index_dir: &Path, id: &str) -> PathBuf {
        let name: String = id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        index_dir.join("semantic").join(format!("{name}.bin"))
    }

    /// The store for `embedder` under `index_dir`, with whatever vectors
    /// its file holds; empty when the file is missing or unusable.
    #[must_use]
    pub fn open(index_dir: &Path, embedder: &dyn Embedder) -> VectorStore {
        let file = VectorStore::file_for(index_dir, embedder.id());
        let mut store = VectorStore::in_memory(embedder);
        if let Ok(bytes) = fs::read(&file)
            && let Some(vectors) = decode(&bytes, embedder.id(), embedder.dims())
        {
            store.vectors = vectors;
        }
        store.file = Some(file);
        store
    }

    /// An empty store that [`VectorStore::save`] doesn't write.
    #[must_use]
    pub fn in_memory(embedder: &dyn Embedder) -> VectorStore {
        VectorStore {
            id: embedder.id().to_string(),
            dims: embedder.dims(),
            file: None,
            vectors: HashMap::new(),
        }
    }

    /// Number of vectors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Whether there are no vectors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// How many distinct passages of `notes` have no vector yet.
    #[must_use]
    pub fn pending(&self, notes: &[Note]) -> usize {
        missing(&self.vectors, notes).len()
    }

    /// Embeds the passages of `notes` that have no vector, `batch` at a
    /// time, and drops vectors no note uses any more. `progress` hears
    /// after every batch; returning `Break` stops early, keeping the
    /// vectors made so far. Doesn't save: call [`VectorStore::save`].
    ///
    /// # Errors
    ///
    /// When the embedder fails or returns the wrong shape; vectors from
    /// earlier batches are kept.
    pub fn sync(
        &mut self,
        embedder: &dyn Embedder,
        notes: &[Note],
        batch: usize,
        progress: &mut dyn FnMut(Progress) -> ControlFlow<()>,
    ) -> Result<SyncReport, SemanticError> {
        let wanted: HashSet<u64> = notes.iter().map(|n| text_hash(passage(n))).collect();
        let before = self.vectors.len();
        self.vectors.retain(|h, _| wanted.contains(h));
        let mut report = SyncReport {
            removed: before - self.vectors.len(),
            ..SyncReport::default()
        };
        let todo = missing(&self.vectors, notes);
        let total = todo.len();
        for chunk in todo.chunks(batch.max(1)) {
            let texts: Vec<&str> = chunk.iter().map(|(_, t)| *t).collect();
            let vectors = embedder.embed_passages(&texts)?;
            if vectors.len() != chunk.len() {
                return Err(SemanticError::Shape {
                    expected: chunk.len(),
                    got: vectors.len(),
                });
            }
            for ((hash, _), v) in chunk.iter().zip(vectors) {
                self.vectors.insert(*hash, self.normalised(v)?);
            }
            report.added += chunk.len();
            let step = Progress {
                done: report.added,
                total,
            };
            if progress(step).is_break() {
                break;
            }
        }
        report.left = total - report.added;
        Ok(report)
    }

    /// Notes ranked by similarity to `query`, best first, at most `limit`.
    /// Notes without a vector (not synced yet) are left out.
    ///
    /// # Errors
    ///
    /// When the embedder fails or returns the wrong shape.
    pub fn search(
        &self,
        embedder: &dyn Embedder,
        query: &str,
        notes: &[Note],
        limit: usize,
    ) -> Result<Vec<Hit>, SemanticError> {
        let q = self.normalised(embedder.embed_query(query)?)?;
        let mut hits: Vec<Hit> = notes
            .iter()
            .enumerate()
            .filter_map(|(index, n)| {
                let v = self.vectors.get(&text_hash(passage(n)))?;
                let score = v.iter().zip(&q).map(|(a, b)| a * b).sum();
                Some(Hit { index, score })
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.index.cmp(&b.index)));
        hits.truncate(limit);
        Ok(hits)
    }

    /// Writes the file (through a temporary file, so a crash never leaves
    /// half of one). Does nothing for an in-memory store.
    ///
    /// # Errors
    ///
    /// On I/O errors.
    pub fn save(&self) -> Result<(), SemanticError> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension("bin.tmp");
        let mut out = fs::File::create(&tmp)?;
        out.write_all(&encode(&self.id, self.dims, &self.vectors))?;
        out.sync_all()?;
        drop(out);
        fs::rename(&tmp, file)?;
        Ok(())
    }

    /// `v` scaled to length 1 (a zero vector stays zero), after checking
    /// its length.
    fn normalised(&self, mut v: Vec<f32>) -> Result<Vec<f32>, SemanticError> {
        if v.len() != self.dims {
            return Err(SemanticError::Shape {
                expected: self.dims,
                got: v.len(),
            });
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        Ok(v)
    }
}

/// Distinct passages of `notes` without a vector, in note order.
fn missing<'a>(vectors: &HashMap<u64, Vec<f32>>, notes: &'a [Note]) -> Vec<(u64, &'a str)> {
    let mut seen = HashSet::new();
    notes
        .iter()
        .map(|n| {
            let text = passage(n);
            (text_hash(text), text)
        })
        .filter(|(h, _)| !vectors.contains_key(h) && seen.insert(*h))
        .collect()
}

fn encode(id: &str, dims: usize, vectors: &HashMap<u64, Vec<f32>>) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + id.len() + vectors.len() * (8 + dims * 4));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&u32::try_from(dims).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&u32::try_from(id.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(id.as_bytes());
    out.extend_from_slice(&(vectors.len() as u64).to_le_bytes());
    // Sorted, so the same vectors always make the same file.
    let mut keys: Vec<&u64> = vectors.keys().collect();
    keys.sort_unstable();
    for k in keys {
        out.extend_from_slice(&k.to_le_bytes());
        for x in &vectors[k] {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    out
}

/// The vectors in `bytes`, or `None` unless it is a whole, current-version
/// file for embedder `id` with `dims`-long vectors.
fn decode(bytes: &[u8], id: &str, dims: usize) -> Option<HashMap<u64, Vec<f32>>> {
    let mut r = Reader(bytes);
    if r.take(4)? != MAGIC || r.u32()? != VERSION || usize::try_from(r.u32()?).ok()? != dims {
        return None;
    }
    let id_len = usize::try_from(r.u32()?).ok()?;
    if r.take(id_len)? != id.as_bytes() {
        return None;
    }
    let count = usize::try_from(r.u64()?).ok()?;
    // Checked before allocating, so a damaged count can't ask for too much.
    if r.0.len() != count.checked_mul(8 + dims * 4)? {
        return None;
    }
    let mut vectors = HashMap::with_capacity(count);
    for _ in 0..count {
        let hash = r.u64()?;
        let v = (0..dims).map(|_| r.f32()).collect::<Option<Vec<f32>>>()?;
        vectors.insert(hash, v);
    }
    Some(vectors)
}

/// Reads little-endian values off the front of a byte slice.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use blossom_core::{Document, Keywords};
    use blossom_index::records;

    use super::*;
    use crate::TestEmbedder;

    fn notes(text: &str) -> Vec<Note> {
        let doc = Document::parse(text, Keywords::default());
        records("a.org", &doc, &Keywords::default(), 0)
            .into_iter()
            .map(Note::from)
            .collect()
    }

    fn sync(store: &mut VectorStore, e: &dyn Embedder, notes: &[Note]) -> SyncReport {
        store
            .sync(e, notes, 2, &mut |_| ControlFlow::Continue(()))
            .unwrap()
    }

    /// Counts the passages it is asked to embed.
    struct Counting {
        inner: TestEmbedder,
        passages: Cell<usize>,
    }

    impl Embedder for Counting {
        fn id(&self) -> &str {
            self.inner.id()
        }
        fn dims(&self) -> usize {
            self.inner.dims()
        }
        fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticError> {
            self.passages.set(self.passages.get() + texts.len());
            self.inner.embed_passages(texts)
        }
        fn embed_query(&self, text: &str) -> Result<Vec<f32>, SemanticError> {
            self.inner.embed_query(text)
        }
    }

    #[test]
    fn ranks_by_meaning_of_shared_words() {
        let e = TestEmbedder::new(64);
        let n = notes("* Buy milk\nand bread\n* Fix the bike\n* Call the vet about the cat\n");
        let mut s = VectorStore::in_memory(&e);
        assert_eq!(s.pending(&n), 3);
        assert_eq!(sync(&mut s, &e, &n).added, 3);
        let hits = s.search(&e, "bread and milk", &n, 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].index, 0);
        assert!(hits[0].score > hits[1].score);
        assert!(hits[0].score <= 1.0 + 1e-6);
    }

    #[test]
    fn sync_embeds_only_new_text_and_drops_unused() {
        let e = Counting {
            inner: TestEmbedder::new(32),
            passages: Cell::new(0),
        };
        let mut s = VectorStore::in_memory(&e);
        // Two headings with the same text share one vector.
        let n = notes("* one\n* two\n* two\n");
        assert_eq!(sync(&mut s, &e, &n).added, 2);
        assert_eq!(e.passages.get(), 2);
        // Inserting a heading before the others embeds only it.
        let n = notes("* zero\n* one\n* two\n* two\n");
        let r = sync(&mut s, &e, &n);
        assert_eq!((r.added, r.removed), (1, 0));
        assert_eq!(e.passages.get(), 3);
        // A removed heading's vector goes.
        let n = notes("* zero\n* two\n");
        let r = sync(&mut s, &e, &n);
        assert_eq!((r.added, r.removed), (0, 1));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn sync_stops_when_asked_and_resumes() {
        let e = TestEmbedder::new(8);
        let n = notes("* a\n* b\n* c\n* d\n* e\n");
        let mut s = VectorStore::in_memory(&e);
        let mut seen = Vec::new();
        let r = s
            .sync(&e, &n, 2, &mut |p| {
                seen.push(p);
                ControlFlow::Break(())
            })
            .unwrap();
        assert_eq!(seen, [Progress { done: 2, total: 5 }]);
        assert_eq!((r.added, r.left), (2, 3));
        assert_eq!(s.pending(&n), 3);
        // Unsynced notes are left out of results.
        assert_eq!(s.search(&e, "c", &n, 10).unwrap().len(), 2);
        assert_eq!(sync(&mut s, &e, &n).added, 3);
    }

    #[test]
    fn saves_and_reopens_and_ignores_unusable_files() {
        let dir = tempfile::tempdir().unwrap();
        let e = TestEmbedder::new(16);
        let n = notes("* Buy milk\n* Fix the bike\n");
        let mut s = VectorStore::open(dir.path(), &e);
        assert!(s.is_empty());
        sync(&mut s, &e, &n);
        s.save().unwrap();
        let file = VectorStore::file_for(dir.path(), "test-16");
        assert_eq!(file, dir.path().join("semantic/test-16.bin"));
        let again = VectorStore::open(dir.path(), &e);
        assert_eq!(again.pending(&n), 0);
        assert_eq!(
            again.search(&e, "milk", &n, 1).unwrap(),
            s.search(&e, "milk", &n, 1).unwrap()
        );
        // Another embedder has its own file.
        assert!(VectorStore::open(dir.path(), &TestEmbedder::new(8)).is_empty());
        // A truncated or foreign file starts empty, and is replaced on save.
        let bytes = fs::read(&file).unwrap();
        fs::write(&file, &bytes[..bytes.len() - 1]).unwrap();
        assert!(VectorStore::open(dir.path(), &e).is_empty());
        fs::write(&file, b"junk").unwrap();
        let mut s = VectorStore::open(dir.path(), &e);
        assert!(s.is_empty());
        sync(&mut s, &e, &n);
        s.save().unwrap();
        assert_eq!(fs::read(&file).unwrap(), bytes);
    }

    #[test]
    fn file_names_are_safe() {
        let f = VectorStore::file_for(Path::new("/i"), "server/nomic embed:v1.5");
        assert_eq!(f, Path::new("/i/semantic/server_nomic_embed_v1.5.bin"));
    }

    #[test]
    fn wrong_shapes_are_errors() {
        struct Bad;
        impl Embedder for Bad {
            fn id(&self) -> &'static str {
                "bad"
            }
            fn dims(&self) -> usize {
                4
            }
            fn embed_passages(&self, _: &[&str]) -> Result<Vec<Vec<f32>>, SemanticError> {
                Ok(vec![vec![1.0; 3]])
            }
            fn embed_query(&self, _: &str) -> Result<Vec<f32>, SemanticError> {
                Ok(Vec::new())
            }
        }
        let mut s = VectorStore::in_memory(&Bad);
        let n = notes("* a\n");
        let err = s.sync(&Bad, &n, 8, &mut |_| ControlFlow::Continue(()));
        assert!(matches!(
            err,
            Err(SemanticError::Shape {
                expected: 4,
                got: 3
            })
        ));
        assert!(s.search(&Bad, "a", &n, 1).is_err());
    }
}
