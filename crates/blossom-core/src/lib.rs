//! Org-mode document model and lossless text-splice edits for Blossom.
//!
//! [`Document`] wraps one file's text. Reading it gives headings, planning,
//! properties, logbook, timestamps and file-level data; editing it (see the
//! methods in [`edit`]) splices text ranges, so bytes outside an edit never
//! change. This crate does no file I/O.

mod document;
pub mod edit;
pub mod editing;
pub mod highlight;
mod keywords;
pub mod render;
mod text;
mod timestamp;

pub use document::{Document, Heading, HeadingSpans, Planning, PlanningKind, Preamble};
pub use edit::{EditError, InsertAt, NewHeading};
pub use keywords::Keywords;
pub use timestamp::{Repeater, RepeaterKind, TimeUnit, Timestamp, Warning, add_units};
