//! Admin (DDL) operation DTOs.

pub mod access;
pub mod types;

#[cfg(test)]
mod tests;

pub use access::{
    AccessTreeOp, AddGroupMemberOp, ChgrpOp, ChmodOp, ChownOp, CreateGroupOp, DropGroupOp,
    GroupRef, RemoveGroupMemberOp, RenameGroupOp, ResourceRef,
};
pub use types::{
    AddSchemaRuleOp, AlterBufferConfigOp, BindValidatorOp, BufferConfigDto, BufferConfigPatch,
    ChangesSinceOp, CommitMigrationOp, CompareDto, ConstraintsDto, CreateDbOp,
    CreateFunctionFolderOp, CreateFunctionOp, CreateIndexOp, CreateRepoOp, CreateTableOp,
    CreateValidatorOp, DescribeTableOp, DropDbOp, DropFunctionOp, DropIndexOp, DropRepoOp,
    DropTableOp, DropValidatorOp, FieldRuleDto, FkAction, ForeignKeyDto, GetBufferConfigOp,
    GetTableSchemaOp, InternerDumpOp, InternerTouchOp, ListOp, ListValidatorsOp, MigrationStatusOp,
    NumDto, PurgeHistoryOp, PurgeScope, RemoveSchemaRuleOp, RenameFunctionFolderOp,
    RenameFunctionOp, RenameIndexOp, RenameRepoOp, RenameTableOp, RenameValidatorOp, Retention,
    RollbackMigrationOp, SetBufferConfigOp, SetRetentionOp, SetTableSchemaOp, StartMigrationOp,
    UnbindValidatorOp,
};
