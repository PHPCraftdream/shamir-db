//! Admin (DDL) operation DTOs.

pub mod access;
pub mod activity;
pub mod shape_stats;
pub mod types;
pub use types::schema_migration;

#[cfg(test)]
mod tests;

pub use access::{
    AccessTreeOp, AddGroupMemberOp, ChgrpOp, ChmodOp, ChownOp, CreateGroupOp, DropGroupOp,
    GroupRef, RemoveGroupMemberOp, RenameGroupOp, ResourceRef,
};
pub use activity::{
    ActivityKind, ActivityPage, ActivityRequestState, ActivityRow, ActivityScope, ActivityTxState,
    CancelOutcome, ListActivityReq, RequestExecId,
};
pub use shape_stats::{
    ListShapeStatsReq, ResetShapeStatsReq, ShapeStatsPage, ShapeStatsRow, ShapeStatsTotals,
};
pub use types::{
    AddSchemaRuleOp, AlterBufferConfigOp, AlterFunctionOp, AlterRepoOp, AlterSubscriptionOp,
    BackupStatusOp, BindValidatorOp, BufferConfigDto, BufferConfigPatch, CancelBackupOp,
    CancelExportOp, ChangesSinceEvent, ChangesSinceOp, ChangesSinceRecordChange,
    ChangesSinceResponse, CheckFunctionOp, CheckFunctionSpec, CommitMigrationOp, CompareDto,
    ConstraintsDto, CreateDbOp, CreateFunctionFolderOp, CreateFunctionOp, CreateIndexOp,
    CreatePublicationOp, CreateReplicationProfileOp, CreateRepoOp, CreateSubscriptionOp,
    CreateTableOp, CreateValidatorOp, DescribeFunctionOp, DescribeRepoOp, DescribeTableOp,
    DropDbOp, DropFunctionOp, DropIndexOp, DropPublicationOp, DropReplicationProfileOp, DropRepoOp,
    DropSubscriptionOp, DropTableOp, DropValidatorOp, ExportCancelReceipt, ExportFailure,
    ExportFunctionOp, ExportPhase, ExportStartReceipt, ExportStatusOp, ExportStatusReceipt,
    FieldRuleDto, FkAction, ForeignKeyDto, GetBufferConfigOp, GetTableSchemaOp, InternerDumpOp,
    InternerTouchOp, ListOp, ListPublicationsOp, ListSubscriptionsOp, ListValidatorsOp,
    MigrationStatusOp, NumDto, PurgeHistoryOp, PurgeScope, RemoveSchemaRuleOp, RenameDbOp,
    RenameFunctionFolderOp, RenameFunctionOp, RenameIndexOp, RenameRepoOp, RenameTableOp,
    RenameValidatorOp, ReplDirection, ReplMode, ReplScope, ReplStream, ReplicationStatusOp,
    Retention, RollbackMigrationOp, SetBufferConfigOp, SetRetentionOp, SetTableSchemaOp,
    StartBackupOp, StartExportOp, StartMigrationOp, SubAction, UnbindValidatorOp,
};
