//! Admin (DDL) operation DTOs.

pub mod access;
pub mod types;

#[cfg(test)]
mod tests;

pub use access::{
    AccessTreeOp, AddGroupMemberOp, ChgrpOp, ChmodOp, ChownOp, CreateGroupOp, DropGroupOp,
    GroupRef, RemoveGroupMemberOp, ResourceRef,
};
pub use types::{
    AddSchemaRuleOp, AlterBufferConfigOp, BindValidatorOp, BufferConfigDto, BufferConfigPatch,
    ChangesSinceOp, CommitMigrationOp, ConstraintsDto, CreateDbOp, CreateFunctionFolderOp,
    CreateFunctionOp, CreateIndexOp, CreateRepoOp, CreateTableOp, CreateValidatorOp, DropDbOp,
    DropFunctionOp, DropIndexOp, DropRepoOp, DropTableOp, DropValidatorOp, FieldRuleDto,
    GetBufferConfigOp, GetTableSchemaOp, InternerDumpOp, InternerTouchOp, ListOp, ListValidatorsOp,
    MigrationStatusOp, NumDto, PurgeHistoryOp, PurgeScope, RemoveSchemaRuleOp, RenameFunctionOp,
    RenameValidatorOp, Retention, RollbackMigrationOp, SetBufferConfigOp, SetRetentionOp,
    SetTableSchemaOp, StartMigrationOp, UnbindValidatorOp,
};
