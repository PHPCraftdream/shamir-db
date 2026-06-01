//! Admin (DDL) operations module.
//!
//! Pure DTOs live in `shamir-query-types::admin`. Re-exported here so
//! existing `crate::query::admin::*` paths in the engine resolve.

pub use shamir_query_types::admin::{
    AddGroupMemberOp, AlterBufferConfigOp, BufferConfigDto, BufferConfigPatch, ChgrpOp, ChmodOp,
    ChownOp, CreateDbOp, CreateGroupOp, CreateIndexOp, CreateRepoOp, CreateTableOp, DropDbOp,
    DropGroupOp, DropIndexOp, DropRepoOp, DropTableOp, GetBufferConfigOp, GroupRef, ListOp,
    RemoveGroupMemberOp, ResourceRef, SetBufferConfigOp,
};
