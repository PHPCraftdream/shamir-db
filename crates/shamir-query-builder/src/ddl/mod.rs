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
pub use shamir_query_types::durability::DurabilityOverride;
pub use shamir_query_types::WriteOp;

// ============================================================================
// Sub-modules (grouped: db / schema / index / policy / repo)
// ============================================================================

mod db;
mod index;
mod policy;
mod repo;
#[path = "schema/mod.rs"]
mod schema_dsl;

pub use db::{create_db, create_table, drop_db, drop_table, rename_db, rename_table};
pub use index::{
    create_index, create_index_build_error, drop_index, index_spec, rename_index, tokenizer,
};
pub use policy::{access_control, auth, metric, migration, replication, retention};
pub use repo::{alter_repo, create_repo, drop_repo, interner, interner_resolve, rename_repo, res};
pub use schema_dsl::schema;
pub use schema_dsl::{buffer_config, describe_table, function, list, quantization, validator};

/// Internal typed index-kind IR (NOT a wire type) that `CreateIndex::try_build`
/// and the typed `CreateIndex` constructors route through to make
/// mutually-exclusive index combinations unrepresentable at the type level.
pub(crate) use index_spec::IndexSpec;

pub use access_control::*;
pub use alter_repo::*;
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
