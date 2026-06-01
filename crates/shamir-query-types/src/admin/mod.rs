//! Admin (DDL) operation DTOs.

pub mod access;
pub mod types;

pub use access::{
    AddGroupMemberOp, ChgrpOp, ChmodOp, ChownOp, CreateGroupOp, DropGroupOp, GroupRef,
    RemoveGroupMemberOp, ResourceRef,
};
pub use types::{
    AlterBufferConfigOp, BufferConfigDto, BufferConfigPatch, CommitMigrationOp, CreateDbOp,
    CreateIndexOp, CreateRepoOp, CreateTableOp, DropDbOp, DropIndexOp, DropRepoOp, DropTableOp,
    GetBufferConfigOp, ListOp, MigrationStatusOp, RollbackMigrationOp, SetBufferConfigOp,
    StartMigrationOp,
};
