//! The tantivy index: one document per file (path and revision) and one per
//! heading (its [`NoteRecord`] as JSON, plus its text as lower-cased
//! trigrams for narrowing). It is a cache: deleting it loses nothing.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use blossom_vault::{Conflict, Revision, Vault};
use tantivy::collector::DocSetCollector;
use tantivy::query::{AllQuery, BooleanQuery, Occur, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{NgramTokenizer, TextAnalyzer};
use tantivy::{
    Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term as TantivyTerm, doc,
};

use crate::matcher::{self, MatchContext};
use crate::query::{Condition, Expr, Query};
use crate::record::{Note, NoteRecord, records};

/// Bump when the schema or the record format changes; an index with another
/// version (or other settings) is deleted and rebuilt.
const SCHEMA_VERSION: u32 = 1;
const META_FILE: &str = "blossom-index.meta";
const GRAMS: &str = "grams3";
const WRITER_MEMORY: usize = 50_000_000;

/// An index error.
#[derive(Debug)]
pub enum IndexError {
    /// From tantivy.
    Tantivy(tantivy::TantivyError),
    /// From the filesystem.
    Io(io::Error),
    /// A stored record could not be decoded.
    Corrupt(String),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexError::Tantivy(e) => write!(f, "index: {e}"),
            IndexError::Io(e) => write!(f, "index: {e}"),
            IndexError::Corrupt(e) => {
                write!(f, "index is corrupt ({e}); run `blsm index --rebuild`")
            }
        }
    }
}

impl std::error::Error for IndexError {}

impl From<tantivy::TantivyError> for IndexError {
    fn from(e: tantivy::TantivyError) -> Self {
        IndexError::Tantivy(e)
    }
}

impl From<io::Error> for IndexError {
    fn from(e: io::Error) -> Self {
        IndexError::Io(e)
    }
}

impl From<tantivy::directory::error::OpenDirectoryError> for IndexError {
    fn from(e: tantivy::directory::error::OpenDirectoryError) -> Self {
        IndexError::Io(io::Error::other(e.to_string()))
    }
}

/// What a [`NoteIndex::sync`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Files indexed for the first time.
    pub added: Vec<String>,
    /// Files re-indexed because their revision changed.
    pub updated: Vec<String>,
    /// Files no longer in the vault.
    pub removed: Vec<String>,
    /// Files that could not be read, with the reason (left as they were).
    pub failed: Vec<(String, String)>,
    /// Syncthing conflict copies found.
    pub conflicts: Vec<Conflict>,
}

struct Fields {
    kind: Field,
    path: Field,
    revision: Field,
    record: Field,
    grams: Field,
}

/// The search index over one vault.
pub struct NoteIndex {
    index: Index,
    reader: IndexReader,
    fields: Fields,
}

impl NoteIndex {
    /// Opens the index in folder `dir`, creating it, or recreating it when it
    /// was built by another schema version or with other `settings` (a
    /// fingerprint of whatever changes the records, such as the keywords).
    ///
    /// # Errors
    ///
    /// On I/O or tantivy errors.
    pub fn open(dir: &Path, settings: &str) -> Result<NoteIndex, IndexError> {
        let meta = format!("{SCHEMA_VERSION}\n{settings}\n");
        let meta_path = dir.join(META_FILE);
        if fs::read_to_string(&meta_path).ok().as_deref() != Some(meta.as_str()) && dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        fs::create_dir_all(dir)?;
        let (schema, fields) = schema();
        let mmap = tantivy::directory::MmapDirectory::open(dir)?;
        let index = Index::open_or_create(mmap, schema)?;
        fs::write(&meta_path, meta)?;
        NoteIndex::with_index(index, fields)
    }

    /// An index in memory, for tests and one-off use.
    ///
    /// # Errors
    ///
    /// On tantivy errors.
    pub fn in_memory() -> Result<NoteIndex, IndexError> {
        let (schema, fields) = schema();
        NoteIndex::with_index(Index::create_in_ram(schema), fields)
    }

    fn with_index(index: Index, fields: Fields) -> Result<NoteIndex, IndexError> {
        let grams = NgramTokenizer::new(3, 3, false)?;
        index
            .tokenizers()
            .register(GRAMS, TextAnalyzer::from(grams));
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(NoteIndex {
            index,
            reader,
            fields,
        })
    }

    /// The revision each indexed file was indexed at.
    ///
    /// # Errors
    ///
    /// On tantivy errors.
    pub fn revisions(&self) -> Result<HashMap<String, Revision>, IndexError> {
        let mut out = HashMap::new();
        for doc in self.docs(&TermQuery::new(
            TantivyTerm::from_field_text(self.fields.kind, "file"),
            IndexRecordOption::Basic,
        ))? {
            let path = Self::text(&doc, self.fields.path);
            if let (Some(p), Some(r)) = (
                path,
                Self::text(&doc, self.fields.revision).and_then(Revision::parse),
            ) {
                out.insert(p.to_string(), r);
            }
        }
        Ok(out)
    }

    /// Brings the index up to date with `vault`: re-indexes files whose
    /// revision changed and drops files that are gone.
    ///
    /// # Errors
    ///
    /// When the vault can't be listed, or on tantivy errors. A file that
    /// can't be read is reported in [`SyncReport::failed`] instead.
    pub fn sync(&mut self, vault: &Vault) -> Result<SyncReport, IndexError> {
        let listing = vault.list()?;
        let known = self.revisions()?;
        let mut report = SyncReport {
            conflicts: listing.conflicts,
            ..SyncReport::default()
        };
        let mut writer: Option<IndexWriter> = None;
        for file in &listing.files {
            let was = known.get(&file.path);
            if was == Some(&file.revision) {
                continue;
            }
            let (doc, rev) = match vault.document(&file.path) {
                Ok(found) => found,
                Err(e) => {
                    report.failed.push((file.path.clone(), e.to_string()));
                    continue;
                }
            };
            let recs = records(&file.path, &doc, vault.keywords(), rev.mtime_ns);
            let w = self.writer(&mut writer)?;
            self.put_file(w, &file.path, rev, &recs)?;
            if was.is_some() {
                report.updated.push(file.path.clone());
            } else {
                report.added.push(file.path.clone());
            }
        }
        let present: std::collections::HashSet<&str> =
            listing.files.iter().map(|f| f.path.as_str()).collect();
        let mut gone: Vec<&String> = known
            .keys()
            .filter(|p| !present.contains(String::as_str(p)))
            .collect();
        gone.sort();
        for path in gone {
            let w = self.writer(&mut writer)?;
            w.delete_term(TantivyTerm::from_field_text(self.fields.path, path));
            report.removed.push(path.clone());
        }
        if let Some(mut w) = writer {
            w.commit()?;
            self.reader.reload()?;
        }
        Ok(report)
    }

    /// Indexes one file's records now (e.g. right after Blossom wrote it).
    ///
    /// # Errors
    ///
    /// On tantivy errors.
    pub fn update_file(
        &mut self,
        path: &str,
        revision: Revision,
        recs: &[NoteRecord],
    ) -> Result<(), IndexError> {
        let mut writer = None;
        let w = self.writer(&mut writer)?;
        self.put_file(w, path, revision, recs)?;
        if let Some(mut w) = writer {
            w.commit()?;
        }
        self.reader.reload()?;
        Ok(())
    }

    /// Deletes everything, so the next [`NoteIndex::sync`] re-reads every file.
    ///
    /// # Errors
    ///
    /// On tantivy errors.
    pub fn clear(&mut self) -> Result<(), IndexError> {
        let mut writer = None;
        let w = self.writer(&mut writer)?;
        w.delete_all_documents()?;
        if let Some(mut w) = writer {
            w.commit()?;
        }
        self.reader.reload()?;
        Ok(())
    }

    fn writer<'a>(
        &self,
        slot: &'a mut Option<IndexWriter>,
    ) -> Result<&'a mut IndexWriter, IndexError> {
        if slot.is_none() {
            *slot = Some(self.index.writer(WRITER_MEMORY)?);
        }
        slot.as_mut()
            .ok_or_else(|| IndexError::Corrupt("no writer".into()))
    }

    fn put_file(
        &self,
        w: &IndexWriter,
        path: &str,
        rev: Revision,
        recs: &[NoteRecord],
    ) -> Result<(), IndexError> {
        let f = &self.fields;
        w.delete_term(TantivyTerm::from_field_text(f.path, path));
        w.add_document(doc!(f.kind => "file", f.path => path, f.revision => rev.to_string()))?;
        for r in recs {
            let json = serde_json::to_string(r).map_err(|e| IndexError::Corrupt(e.to_string()))?;
            let grams = format!("{}\n{}", r.text, r.tags.join(" ")).to_lowercase();
            w.add_document(
                doc!(f.kind => "note", f.path => path, f.record => json, f.grams => grams),
            )?;
        }
        Ok(())
    }

    /// Every indexed heading.
    ///
    /// # Errors
    ///
    /// On tantivy errors or undecodable records.
    pub fn all(&self) -> Result<Vec<Note>, IndexError> {
        self.notes(&self.note_query(None))
    }

    /// Runs `query`: the index narrows to candidates when the query has a
    /// required text term of three or more characters, then the matcher
    /// decides each one and the results are sorted.
    ///
    /// # Errors
    ///
    /// On tantivy errors or undecodable records.
    pub fn search(&self, query: &Query, ctx: &MatchContext) -> Result<Vec<Note>, IndexError> {
        let candidates = self.notes(&self.note_query(narrowing_term(query).as_deref()))?;
        Ok(filter_and_sort(candidates, query, ctx))
    }

    /// `kind:note`, plus every trigram of `term` when given.
    fn note_query(&self, term: Option<&str>) -> BooleanQuery {
        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = vec![(
            Occur::Must,
            Box::new(TermQuery::new(
                TantivyTerm::from_field_text(self.fields.kind, "note"),
                IndexRecordOption::Basic,
            )),
        )];
        if let Some(term) = term {
            for gram in trigrams(&term.to_lowercase()) {
                clauses.push((
                    Occur::Must,
                    Box::new(TermQuery::new(
                        TantivyTerm::from_field_text(self.fields.grams, &gram),
                        IndexRecordOption::Basic,
                    )),
                ));
            }
        }
        BooleanQuery::new(clauses)
    }

    fn notes(&self, query: &dyn tantivy::query::Query) -> Result<Vec<Note>, IndexError> {
        let mut out = Vec::new();
        for doc in self.docs(query)? {
            let json = Self::text(&doc, self.fields.record)
                .ok_or_else(|| IndexError::Corrupt("record missing".into()))?;
            let record: NoteRecord =
                serde_json::from_str(json).map_err(|e| IndexError::Corrupt(e.to_string()))?;
            out.push(Note::from(record));
        }
        // Collection order is arbitrary; make it deterministic.
        out.sort_by(|a, b| {
            (&a.record.path, a.record.heading).cmp(&(&b.record.path, b.record.heading))
        });
        Ok(out)
    }

    fn docs(&self, query: &dyn tantivy::query::Query) -> Result<Vec<TantivyDocument>, IndexError> {
        let searcher = self.reader.searcher();
        let addresses = searcher.search(query, &DocSetCollector)?;
        addresses
            .into_iter()
            .map(|a| searcher.doc(a).map_err(IndexError::from))
            .collect()
    }

    fn text(doc: &TantivyDocument, field: Field) -> Option<&str> {
        doc.get_first(field).and_then(|v| v.as_str())
    }

    /// Every document, for checks: all notes regardless of kind filters.
    #[doc(hidden)]
    pub fn count_all(&self) -> Result<usize, IndexError> {
        Ok(self.docs(&AllQuery)?.len())
    }
}

/// Filters `notes` with the matcher and sorts them.
#[must_use]
pub fn filter_and_sort(notes: Vec<Note>, query: &Query, ctx: &MatchContext) -> Vec<Note> {
    let mut hits: Vec<Note> = notes
        .into_iter()
        .filter(|n| matcher::matches(n, query, ctx))
        .collect();
    matcher::sort(&mut hits, query, ctx);
    hits
}

fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let grams = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(GRAMS)
            .set_index_option(IndexRecordOption::Basic),
    );
    let fields = Fields {
        kind: b.add_text_field("kind", STRING),
        path: b.add_text_field("path", STRING | STORED),
        revision: b.add_text_field("revision", STORED),
        record: b.add_text_field("record", STORED),
        grams: b.add_text_field("grams", grams),
    };
    (b.build(), fields)
}

/// The longest positive text term that every match must contain: only when
/// the query is one term or an AND at the top. Terms under three characters
/// can't narrow (they have no trigram).
fn narrowing_term(query: &Query) -> Option<String> {
    let leaves: Vec<_> = match query.expr.as_ref()? {
        Expr::Leaf(t) => vec![t],
        Expr::And(children) => children
            .iter()
            .filter_map(|c| match c {
                Expr::Leaf(t) => Some(t),
                _ => None,
            })
            .collect(),
        Expr::Or(_) => return None,
    };
    leaves
        .into_iter()
        .filter(|t| !t.negated)
        .filter_map(|t| match &t.condition {
            Condition::Text(s) => Some(s.to_lowercase()),
            _ => None,
        })
        .filter(|s| s.chars().count() >= 3)
        .max_by_key(|s| s.chars().count())
}

/// Every run of three characters in `s`.
fn trigrams(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut grams: Vec<String> = chars.windows(3).map(|w| w.iter().collect()).collect();
    grams.sort();
    grams.dedup();
    grams
}
