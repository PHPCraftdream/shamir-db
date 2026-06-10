//! ShamirDB engine + query layer — `DbInstance` / `RepoInstance` /
//! `TableManager` / `IndexManager` runtime AND the SDBQL query types
//! (filter / read / write / batch / admin / auth).
//!
//! Engine and query share an internal cycle (the table-manager
//! evaluates filters and builds query results) so they live in one
//! crate. Re-exported from `shamir-db` as `db::engine` and `db::query`
//! so all existing `crate::db::engine::*` / `crate::query::*`
//! paths keep resolving without caller-side changes.

pub mod db_instance;
pub use shamir_wasm_host as function;
pub mod index;
pub use shamir_index as index2;
pub mod meta;
pub mod migration;
pub mod query;
pub mod repo;
pub mod table;
pub mod tx;
pub mod validator;

// Phase 3b — surface the changefeed event types at the crate root so the
// `shamir-db` facade can name `shamir_engine::ChangelogEvent` directly.
pub use tx::{ChangeOp, ChangelogEvent, JournalRead, RecordChange};
