//! Admin (DDL) operation types.

pub mod changes_since_response;
pub mod repo_ops;

mod db;
mod ddl;
mod repl;

pub use db::buffer_config;
pub use db::db_ops;
pub use db::fk_action;
pub use db::retention;
pub use db::schema_ops;
pub use db::table_ops;
pub use ddl::function_ops;
pub use ddl::index_ops;
pub use ddl::interner_ops;
pub use ddl::list_ops;
pub use ddl::migration_ops;
pub use ddl::validator_ops;
pub use repl::repl_ops;

pub use buffer_config::{
    AlterBufferConfigOp, BufferConfigDto, BufferConfigPatch, GetBufferConfigOp, SetBufferConfigOp,
};
pub use changes_since_response::{
    ChangeOpKind, ChangesSinceEvent, ChangesSinceRecordChange, ChangesSinceResponse,
};
pub use db_ops::{CreateDbOp, DropDbOp};
pub use fk_action::FkAction;
pub use function_ops::{
    CreateFunctionFolderOp, CreateFunctionOp, DropFunctionOp, RenameFunctionFolderOp,
    RenameFunctionOp,
};
pub use index_ops::{CreateIndexOp, DropIndexOp, RenameIndexOp};
pub use interner_ops::{InternerDumpOp, InternerTouchOp};
pub use list_ops::ListOp;
pub use migration_ops::{
    CommitMigrationOp, MigrationStatusOp, RollbackMigrationOp, StartMigrationOp,
};
pub use repl_ops::{
    AlterSubscriptionOp, CreatePublicationOp, CreateReplicationProfileOp, CreateSubscriptionOp,
    DropPublicationOp, DropReplicationProfileOp, DropSubscriptionOp, ListPublicationsOp,
    ListSubscriptionsOp, ReplDirection, ReplMode, ReplScope, ReplStream, ReplicationStatusOp,
    SubAction,
};
pub use repo_ops::{
    AlterRepoOp, CreateRepoOp, DescribeRepoOp, DropRepoOp, RenameDbOp, RenameRepoOp,
};
pub use retention::{ChangesSinceOp, PurgeHistoryOp, PurgeScope, Retention, SetRetentionOp};
pub use schema_ops::{
    AddSchemaRuleOp, CompareDto, ConstraintsDto, FieldRuleDto, ForeignKeyDto, GetTableSchemaOp,
    NumDto, RemoveSchemaRuleOp, SetTableSchemaOp,
};
pub use table_ops::{CreateTableOp, DescribeTableOp, DropTableOp, RenameTableOp};
pub use validator_ops::{
    BindValidatorOp, CreateValidatorOp, DropValidatorOp, ListValidatorsOp, RenameValidatorOp,
    UnbindValidatorOp,
};

#[cfg(test)]
mod tests;
