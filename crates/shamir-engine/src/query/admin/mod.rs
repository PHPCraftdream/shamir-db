//! Admin (DDL) operations module.
//!
//! Pure DTOs live in `shamir-query-types::admin`. Re-exported here so
//! existing `crate::query::admin::*` paths in the engine resolve.

pub use shamir_query_types::admin::{
    AlterBufferConfigOp, BufferConfigDto, BufferConfigPatch, CreateDbOp, CreateIndexOp,
    CreateRepoOp, CreateTableOp, DropDbOp, DropIndexOp, DropRepoOp, DropTableOp, GetBufferConfigOp,
    ListOp, SetBufferConfigOp,
};
