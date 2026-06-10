//! Typed constructors for DDL / admin `BatchOp` variants.
//!
//! Every public function or builder in this module returns a
//! [`BatchOp`](shamir_query_types::batch::BatchOp) that can be fed
//! straight into `Batch::op(alias, ddl::create_db("mydb"))`.
//!
//! Where an operation has many or optional fields a builder struct is
//! returned instead; call `.build()` to finalize it into a `BatchOp`.
//!
//! Re-exports [`ResourceRef`] and [`GroupRef`] from `shamir-query-types`
//! so callers do not need an extra import. The [`res`] sub-module provides
//! tiny helpers to construct a `ResourceRef` without spelling out enum
//! variants.

// Re-export wire types that callers need to assemble resource / group
// references and buffer configs.
pub use shamir_query_types::admin::Retention;
pub use shamir_query_types::admin::{BufferConfigDto as BufConfig, BufferConfigPatch as BufPatch};
pub use shamir_query_types::admin::{GroupRef, ResourceRef};
pub use shamir_query_types::WriteOp;

// ============================================================================
// Sub-modules
// ============================================================================

/// Ergonomic helpers to build a [`ResourceRef`] without spelling out enum variants.
pub mod res;

mod access_control;
mod auth;
mod buffer_config;
mod create_db;
mod create_index;
mod create_repo;
mod create_table;
mod drop_db;
mod drop_index;
mod drop_repo;
mod drop_table;
mod function;
mod list;
mod migration;
mod retention;
mod validator;

pub use access_control::*;
pub use auth::*;
pub use buffer_config::*;
pub use create_db::*;
pub use create_index::*;
pub use create_repo::*;
pub use create_table::*;
pub use drop_db::*;
pub use drop_index::*;
pub use drop_repo::*;
pub use drop_table::*;
pub use function::*;
pub use list::*;
pub use migration::*;
pub use retention::*;
pub use validator::*;

#[cfg(test)]
mod tests;
