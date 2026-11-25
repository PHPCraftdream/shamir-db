//! [`BatchOp::activity_name`] — the fixed op-kind summary string rows of
//! the activity view carry in their `op_kind` field (R4b step 1, §5.1).
//!
//! Vocabulary (design §2): `read` / `write` / `tx_open` / `admin:<op>`.
//! Names are fixed strings derived from the parsed operation discriminant
//! only — never from arguments, filter values or literals (privacy, D3).
//!
//! This inherent impl lives here (not in `ops/batch/ops/batch_op.rs`)
//! because both folders are at the 7-entry layout limit and
//! `batch_op.rs` sits at the 1000-line file limit; an inherent `impl`
//! block may live in any module of the defining crate.

use crate::admin::types::schema_migration::SchemaMigrationAdminOp;
use crate::batch::BatchOp;

impl BatchOp {
    /// Fixed op-kind summary for activity-view rows: data reads are
    /// `read`, every data mutation (and conservatively `Call`,
    /// `Subscribe`/`Unsubscribe`, and the recursive `Batch`/`ForEach`
    /// containers, whose bodies may mutate) is `write`; admin/DDL,
    /// auth, replication, backup, schema-migration and activity ops are
    /// `admin:` plus their snake_case op name (equal to the wire
    /// discriminator, with the schema-migration sub-operation named
    /// after the `operation` tag). Interactive transactions are not
    /// `BatchOp`s, so `tx_open` is minted by the server's tx path, not
    /// here. Exhaustive `match` with no wildcard arm: a new variant
    /// fails to compile until it is consciously named.
    pub fn activity_name(&self) -> &'static str {
        match self {
            // ----- data ops ------------------------------------------------
            BatchOp::Read(_) => "read",
            BatchOp::Insert(_) | BatchOp::Update(_) | BatchOp::Set(_) | BatchOp::Delete(_) => {
                "write"
            }

            // ----- conservatively mutating non-admin ops -------------------
            BatchOp::Call(_)
            | BatchOp::Subscribe(_)
            | BatchOp::Unsubscribe(_)
            | BatchOp::Batch(_)
            | BatchOp::ForEach(_) => "write",

            // ----- schema-migration sub-operations (named subkind) ---------
            BatchOp::ApplicationSchema(SchemaMigrationAdminOp::ApplicationSchemaStatus(_)) => {
                "admin:application_schema_status"
            }
            BatchOp::ApplicationSchema(SchemaMigrationAdminOp::ApplicationSchemaPlan(_)) => {
                "admin:application_schema_plan"
            }
            BatchOp::ApplicationSchema(
                SchemaMigrationAdminOp::ApplyApplicationSchemaMigration(_),
            ) => "admin:apply_application_schema_migration",

            // ----- admin / DDL / auth / replication / backup / activity ----
            BatchOp::CreateDb(_) => "admin:create_db",
            BatchOp::DropDb(_) => "admin:drop_db",
            BatchOp::CreateRepo(_) => "admin:create_repo",
            BatchOp::DropRepo(_) => "admin:drop_repo",
            BatchOp::RenameRepo(_) => "admin:rename_repo",
            BatchOp::AlterRepo(_) => "admin:alter_repo",
            BatchOp::RenameDb(_) => "admin:rename_db",
            BatchOp::CreateTable(_) => "admin:create_table",
            BatchOp::DropTable(_) => "admin:drop_table",
            BatchOp::RenameTable(_) => "admin:rename_table",
            BatchOp::CreateIndex(_) => "admin:create_index",
            BatchOp::DropIndex(_) => "admin:drop_index",
            BatchOp::RenameIndex(_) => "admin:rename_index",
            BatchOp::SetBufferConfig(_) => "admin:set_buffer_config",
            BatchOp::GetBufferConfig(_) => "admin:get_buffer_config",
            BatchOp::AlterBufferConfig(_) => "admin:alter_buffer_config",
            BatchOp::List(_) => "admin:list",
            BatchOp::StartMigration(_) => "admin:start_migration",
            BatchOp::CommitMigration(_) => "admin:commit_migration",
            BatchOp::RollbackMigration(_) => "admin:rollback_migration",
            BatchOp::MigrationStatus(_) => "admin:migration_status",
            BatchOp::CreateUser(_) => "admin:create_user",
            BatchOp::DropUser(_) => "admin:drop_user",
            BatchOp::GrantRole(_) => "admin:grant_role",
            BatchOp::RevokeRole(_) => "admin:revoke_role",
            BatchOp::Chmod(_) => "admin:chmod",
            BatchOp::Chown(_) => "admin:chown",
            BatchOp::Chgrp(_) => "admin:chgrp",
            BatchOp::CreateGroup(_) => "admin:create_group",
            BatchOp::DropGroup(_) => "admin:drop_group",
            BatchOp::RenameGroup(_) => "admin:rename_group",
            BatchOp::AddGroupMember(_) => "admin:add_group_member",
            BatchOp::RemoveGroupMember(_) => "admin:remove_group_member",
            BatchOp::AccessTree(_) => "admin:access_tree",
            BatchOp::CreateFunction(_) => "admin:create_function",
            BatchOp::DropFunction(_) => "admin:drop_function",
            BatchOp::RenameFunction(_) => "admin:rename_function",
            BatchOp::DescribeFunction(_) => "admin:describe_function",
            BatchOp::ExportFunction(_) => "admin:export_function",
            BatchOp::CheckFunction(_) => "admin:check_function",
            BatchOp::AlterFunction(_) => "admin:alter_function",
            BatchOp::CreateValidator(_) => "admin:create_validator",
            BatchOp::DropValidator(_) => "admin:drop_validator",
            BatchOp::RenameValidator(_) => "admin:rename_validator",
            BatchOp::BindValidator(_) => "admin:bind_validator",
            BatchOp::UnbindValidator(_) => "admin:unbind_validator",
            BatchOp::ListValidators(_) => "admin:list_validators",
            BatchOp::SetTableSchema(_) => "admin:set_table_schema",
            BatchOp::AddSchemaRule(_) => "admin:add_schema_rule",
            BatchOp::RemoveSchemaRule(_) => "admin:remove_schema_rule",
            BatchOp::GetTableSchema(_) => "admin:get_table_schema",
            BatchOp::DescribeTable(_) => "admin:describe_table",
            BatchOp::DescribeRepo(_) => "admin:describe_repo",
            BatchOp::CreateFunctionFolder(_) => "admin:create_function_folder",
            BatchOp::RenameFunctionFolder(_) => "admin:rename_function_folder",
            BatchOp::InternerDump(_) => "admin:interner_dump",
            BatchOp::InternerTouch(_) => "admin:interner_touch",
            BatchOp::PurgeHistory(_) => "admin:purge_history",
            BatchOp::SetRetention(_) => "admin:set_retention",
            BatchOp::ChangesSince(_) => "admin:changes_since",
            BatchOp::CreateReplicationProfile(_) => "admin:create_replication_profile",
            BatchOp::DropReplicationProfile(_) => "admin:drop_replication_profile",
            BatchOp::CreatePublication(_) => "admin:create_publication",
            BatchOp::DropPublication(_) => "admin:drop_publication",
            BatchOp::CreateSubscription(_) => "admin:create_subscription",
            BatchOp::DropSubscription(_) => "admin:drop_subscription",
            BatchOp::AlterSubscription(_) => "admin:alter_subscription",
            BatchOp::ListPublications(_) => "admin:list_publications",
            BatchOp::ListSubscriptions(_) => "admin:list_subscriptions",
            BatchOp::ReplicationStatus(_) => "admin:replication_status",
            BatchOp::StartBackup(_) => "admin:start_backup",
            BatchOp::BackupStatus(_) => "admin:backup_status",
            BatchOp::CancelBackup(_) => "admin:cancel_backup",
            BatchOp::ListActivity(_) => "admin:list_activity",
            BatchOp::StartExport(_) => "admin:start_export",
            BatchOp::ExportStatus(_) => "admin:export_status",
            BatchOp::CancelExport(_) => "admin:cancel_export",
            BatchOp::ListShapeStats(_) => "admin:list_shape_stats",
            BatchOp::ResetShapeStats(_) => "admin:reset_shape_stats",
            BatchOp::CancelRequest(_) => "admin:cancel_request",
            BatchOp::CancelTx(_) => "admin:cancel_tx",
            BatchOp::CancelCursorAdmin(_) => "admin:cancel_cursor_admin",
        }
    }
}
