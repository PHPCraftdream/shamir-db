//! ShamirDB — top-level facade over the typed value model
//! (`shamir-types`), the storage / engine / query layers, and the
//! convenience public API.
//!
//! Re-exports `types`, `codecs`, and `core` from `shamir-types` so
//! callers that imported `shamir_db::types::value::Value` etc. before
//! the crate split keep working unchanged.

pub use shamir_types::codecs;
pub use shamir_types::core;
pub use shamir_types::types;

pub mod api;
pub mod db;

// Top-level convenience re-exports — `shamir_db::ShamirDb` etc. resolve
// without forcing callers to type the full `db::shamir_db::` path.
pub use crate::db::shamir_db::{ShamirDb, SystemStoreConfig};
