#![deny(clippy::unwrap_used, clippy::expect_used)]

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
pub mod index {
    pub use shamir_index::base_index::index_definition;
    pub use shamir_index::base_index::index_info;
    pub use shamir_index::base_index::index_info_item;
    pub use shamir_index::base_index::index_keys;
    pub use shamir_index::base_index::index_manager;
    pub use shamir_index::base_index::index_manager_unique;
    pub use shamir_index::base_index::index_record_key;
    pub use shamir_index::base_index::index_status;
    pub use shamir_index::base_index::sorted_index_definition;
    pub use shamir_index::base_index::sorted_index_manager;
    pub use shamir_index::base_index::write_barrier_flags;
}
// Compatibility paths: `shamir_engine::migration` moved to `db_instance::migration`.
pub use db_instance::migration;
// Compatibility path: `shamir_engine::meta` moved to `repo::meta`.
pub use repo::meta;
pub use shamir_index as index2;
pub mod query;
pub mod repo;
pub mod table;
pub mod tx;
pub mod validator;

// Phase 3b — surface the changefeed event types at the crate root so the
// `shamir-db` facade can name `shamir_engine::ChangelogEvent` directly.
pub use repo::BoundedJournalRead;
pub use shamir_tx::repo_tx_gate::VisibleOutcome;
pub use tx::{ChangeOp, ChangelogEvent, JournalRead, RecordChange};
