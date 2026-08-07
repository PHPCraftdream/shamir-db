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
pub use shamir_query_types::admin::{ReplDirection, ReplMode, ReplScope, ReplStream, SubAction};
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
mod create_index_build_error;
mod create_repo;
mod create_table;
mod describe_table;
mod drop_db;
mod drop_index;
mod drop_repo;
mod drop_table;
mod function;
mod index_spec;
mod interner;
mod interner_resolve;
mod list;
mod metric;
mod migration;
mod quantization;
mod rename_db;
mod rename_index;
mod rename_repo;
mod rename_table;
mod replication;
mod retention;
mod schema;
mod tokenizer;
mod validator;

/// Internal typed index-kind IR (NOT a wire type) that `CreateIndex::try_build`
/// and the typed `CreateIndex` constructors route through to make
/// mutually-exclusive index combinations unrepresentable at the type level.
pub(crate) use index_spec::IndexSpec;

pub use access_control::*;
pub use auth::*;
pub use buffer_config::*;
pub use create_db::*;
pub use create_index::*;
pub use create_index_build_error::*;
pub use create_repo::*;
pub use create_table::*;
pub use describe_table::*;
pub use drop_db::*;
pub use drop_index::*;
pub use drop_repo::*;
pub use drop_table::*;
pub use function::*;
pub use interner::*;
pub use interner_resolve::*;
pub use list::*;
pub use metric::*;
pub use migration::*;
pub use quantization::*;
pub use rename_db::*;
pub use rename_index::*;
pub use rename_repo::*;
pub use rename_table::*;
pub use replication::*;
pub use retention::*;
pub use schema::*;
pub use tokenizer::*;
pub use validator::*;

#[cfg(test)]
mod tests;
