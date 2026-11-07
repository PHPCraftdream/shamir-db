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
    AddSchemaRuleOp, AlterBufferConfigOp, AlterFunctionOp, AlterRepoOp, AlterSubscriptionOp,
    BindValidatorOp, BufferConfigDto, BufferConfigPatch, ChangesSinceEvent, ChangesSinceOp,
    ChangesSinceRecordChange, ChangesSinceResponse, CheckFunctionOp, CheckFunctionSpec,
    CommitMigrationOp, CompareDto, ConstraintsDto, CreateDbOp, CreateFunctionFolderOp,
    CreateFunctionOp, CreateIndexOp, CreatePublicationOp, CreateReplicationProfileOp, CreateRepoOp,
    CreateSubscriptionOp, CreateTableOp, CreateValidatorOp, DescribeFunctionOp, DescribeRepoOp,
    DescribeTableOp, DropDbOp, DropFunctionOp, DropIndexOp, DropPublicationOp,
    DropReplicationProfileOp, DropRepoOp, DropSubscriptionOp, DropTableOp, DropValidatorOp,
    ExportFunctionOp, FieldRuleDto, FkAction, ForeignKeyDto, GetBufferConfigOp, GetTableSchemaOp,
    InternerDumpOp, InternerTouchOp, ListOp, ListPublicationsOp, ListSubscriptionsOp,
    ListValidatorsOp, MigrationStatusOp, NumDto, PurgeHistoryOp, PurgeScope, RemoveSchemaRuleOp,
    RenameDbOp, RenameFunctionFolderOp, RenameFunctionOp, RenameIndexOp, RenameRepoOp,
    RenameTableOp, RenameValidatorOp, ReplDirection, ReplMode, ReplScope, ReplStream,
    ReplicationStatusOp, Retention, RollbackMigrationOp, SetBufferConfigOp, SetRetentionOp,
    SetTableSchemaOp, StartMigrationOp, SubAction, UnbindValidatorOp,
};
