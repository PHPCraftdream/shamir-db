//! ShamirDB — top-level facade over the typed value model
//! (`shamir-types`), the storage / engine / query layers, and the
//! convenience public API.
//!
//! After the per-layer split (engine/query → `shamir-engine`,
//! storage → `shamir-storage`) the in-tree `db/` wrapper became a
//! single-level passthrough, so it was lifted: `shamir-db` now exposes
//! `engine`, `query`, `storage`, `net`, and `shamir_db` directly at the
//! crate root instead of under a redundant `db::` prefix.

// Re-export foundation crates so callers that imported them through
// `shamir_db::*` keep working unchanged.
pub use shamir_types::access;
pub use shamir_types::codecs;
pub use shamir_types::core;
pub use shamir_types::record_view;
pub use shamir_types::types;

// Engine + query are in `shamir-engine`; storage is in `shamir-storage`.
// Re-exported at the crate root.
pub use shamir_engine as engine;
pub use shamir_engine::query;
pub use shamir_storage as storage;
pub use shamir_storage::error::{DbError, DbResult};

pub mod api;
pub mod shamir_db;

// Top-level convenience re-exports — `shamir_db::ShamirDb` etc. resolve
// without forcing callers to type the full `shamir_db::*` path.
pub use crate::shamir_db::{PortError, PrincipalInfo, PrincipalResolver, UserAdminPort};
pub use crate::shamir_db::{ShamirDb, SystemStoreConfig};
