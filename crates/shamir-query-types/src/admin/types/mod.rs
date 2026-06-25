//! Admin (DDL) operation types.

pub mod buffer_config;
pub mod db_ops;
pub mod fk_action;
pub mod function_ops;
pub mod index_ops;
pub mod interner_ops;
pub mod list_ops;
pub mod migration_ops;
pub mod repo_ops;
pub mod retention;
pub mod schema_ops;
pub mod table_ops;
pub mod validator_ops;

pub use buffer_config::{
    AlterBufferConfigOp, BufferConfigDto, BufferConfigPatch, GetBufferConfigOp, SetBufferConfigOp,
};
pub use db_ops::{CreateDbOp, DropDbOp};
pub use fk_action::FkAction;
pub use function_ops::{
    CreateFunctionFolderOp, CreateFunctionOp, DropFunctionOp, RenameFunctionOp,
};
pub use index_ops::{CreateIndexOp, DropIndexOp};
pub use interner_ops::{InternerDumpOp, InternerTouchOp};
pub use list_ops::ListOp;
pub use migration_ops::{
    CommitMigrationOp, MigrationStatusOp, RollbackMigrationOp, StartMigrationOp,
};
pub use repo_ops::{CreateRepoOp, DropRepoOp};
pub use retention::{ChangesSinceOp, PurgeHistoryOp, PurgeScope, Retention, SetRetentionOp};
pub use schema_ops::{
    AddSchemaRuleOp, CompareDto, ConstraintsDto, FieldRuleDto, ForeignKeyDto, GetTableSchemaOp,
    NumDto, RemoveSchemaRuleOp, SetTableSchemaOp,
};
pub use table_ops::{CreateTableOp, DropTableOp, RenameTableOp};
pub use validator_ops::{
    BindValidatorOp, CreateValidatorOp, DropValidatorOp, ListValidatorsOp, RenameValidatorOp,
    UnbindValidatorOp,
};

#[cfg(test)]
mod tests;
