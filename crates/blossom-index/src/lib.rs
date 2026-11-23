//! The search index and query language for Blossom.
//!
//! [`NoteIndex`] keeps one record per heading in a tantivy index stored in a
//! cache folder, updated per file when its [`Revision`](blossom_vault::Revision)
//! changes and rebuildable from the vault at any time. Queries use Bloom's
//! Orgzly-compatible syntax ([`Query`]). The index only narrows the
//! candidates; [`matcher`] decides every result, so results are the same
//! with or without narrowing.

pub mod agenda;
pub mod matcher;
pub mod query;
mod record;
mod store;

pub use matcher::{AgendaDay, AgendaItem, AgendaReason, MatchContext, agenda};
pub use query::Query;
pub use record::{Note, NoteRecord, records};
pub use store::{IndexError, NoteIndex, SyncReport, filter_and_sort};
