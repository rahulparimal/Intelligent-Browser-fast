//! Local document search.
//!
//! * [`LiteStore`] - SQLite FTS5 (BM25) index. Same schema, hashing and settings format as the Python implementation, so an
//!   existing index and settings file are read as they are.
//! * [`extract`] - text extraction for txt, md, html, docx and pdf.
//! * [`text`] - chunking, stopwords and joined-word forms (`e-mail` also matches `email`).
//! * [`settings`] - validated, atomically written settings with secrets that never leave the process.

pub mod acl;
pub mod extract;
pub mod paths;
pub mod settings;
mod store;
pub mod text;

pub use store::{DocInfo, Hit, IndexOptions, IndexStats, LiteStore, SearchError, StoreStats};
