//! Admin (DDL) operation DTOs.

pub mod access;
pub mod types;

pub use access::{
    AccessTreeOp, AddGroupMemberOp, ChgrpOp, ChmodOp, ChownOp, CreateGroupOp, DropGroupOp,
    GroupRef, RemoveGroupMemberOp, ResourceRef,
};
pub use types::{
    AlterBufferConfigOp, BindValidatorOp, BufferConfigDto, BufferConfigPatch, CommitMigrationOp,
    CreateDbOp, CreateFunctionFolderOp, CreateFunctionOp, CreateIndexOp, CreateRepoOp,
    CreateTableOp, CreateValidatorOp, DropDbOp, DropFunctionOp, DropIndexOp, DropRepoOp,
    DropTableOp, DropValidatorOp, GetBufferConfigOp, ListOp, ListValidatorsOp, MigrationStatusOp,
    PurgeHistoryOp, PurgeScope, RenameFunctionOp, RenameValidatorOp, Retention,
    RollbackMigrationOp, SetBufferConfigOp, SetRetentionOp, StartMigrationOp, UnbindValidatorOp,
};
