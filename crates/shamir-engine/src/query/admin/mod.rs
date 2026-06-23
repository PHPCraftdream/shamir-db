//! Admin (DDL) operations module.
//!
//! Pure DTOs live in `shamir-query-types::admin`. Re-exported here so
//! existing `crate::query::admin::*` paths in the engine resolve.

pub use shamir_query_types::admin::{
    AddGroupMemberOp, AddSchemaRuleOp, AlterBufferConfigOp, BufferConfigDto, BufferConfigPatch,
    ChgrpOp, ChmodOp, ChownOp, ConstraintsDto, CreateDbOp, CreateGroupOp, CreateIndexOp,
    CreateRepoOp, CreateTableOp, DropDbOp, DropGroupOp, DropIndexOp, DropRepoOp, DropTableOp,
    FieldRuleDto, GetBufferConfigOp, GetTableSchemaOp, GroupRef, ListOp, NumDto, PurgeScope,
    RemoveGroupMemberOp, RemoveSchemaRuleOp, ResourceRef, SetBufferConfigOp, SetTableSchemaOp,
};
