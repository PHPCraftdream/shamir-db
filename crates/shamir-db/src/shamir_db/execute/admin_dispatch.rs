//! `AdminExecutor` trait impl + thin `execute_admin` dispatcher.

use crate::access::{Action, Actor, ResourcePath};
use crate::query::batch::{AdminExecutor, BatchError, BatchOp};
use crate::query::read::QueryResult;

use super::super::shamir_db::ShamirDb;

/// AdminExecutor that operates on ShamirDb.
pub(super) struct ShamirAdminExecutor {
    pub(super) shamir: ShamirDb,
    pub(super) db_name: String,
    pub(super) actor: Actor,
}

#[async_trait::async_trait]
impl AdminExecutor for ShamirAdminExecutor {
    async fn execute_admin(&self, op: &BatchOp) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };

        match op {
            // ── DB / Repo ──────────────────────────────────────────────
            BatchOp::CreateDb(op) => self.handle_create_db(op).await,
            BatchOp::DropDb(op) => self.handle_drop_db(op).await,
            BatchOp::CreateRepo(op) => self.handle_create_repo(op).await,
            BatchOp::DropRepo(op) => self.handle_drop_repo(op).await,
            BatchOp::RenameRepo(op) => self.handle_rename_repo(op).await,

            // ── Table / Index ──────────────────────────────────────────
            BatchOp::CreateTable(op) => self.handle_create_table(op).await,
            BatchOp::DropTable(op) => self.handle_drop_table(op).await,
            op @ BatchOp::RenameTable(_) => self.handle_rename_table(op).await,
            BatchOp::CreateIndex(op) => self.handle_create_index(op).await,
            BatchOp::DropIndex(op) => self.handle_drop_index(op).await,
            op @ BatchOp::RenameIndex(_) => self.handle_rename_index(op).await,

            // ── Buffer ─────────────────────────────────────────────────
            BatchOp::GetBufferConfig(op) => self.handle_get_buffer_config(op).await,
            BatchOp::SetBufferConfig(op) => self.handle_set_buffer_config(op).await,
            BatchOp::AlterBufferConfig(op) => self.handle_alter_buffer_config(op).await,

            // ── List ───────────────────────────────────────────────────
            BatchOp::List(op) => self.handle_list(op).await,

            // ── Users / Roles ──────────────────────────────────────────
            BatchOp::CreateUser(op) => self.handle_create_user(op).await,
            BatchOp::DropUser(op) => self.handle_drop_user(op).await,
            BatchOp::CreateRole(op) => self.handle_create_role(op).await,
            BatchOp::DropRole(op) => self.handle_drop_role(op).await,
            BatchOp::GrantRole(op) => self.handle_grant_role(op).await,
            BatchOp::RevokeRole(op) => self.handle_revoke_role(op).await,

            // ── Migration ──────────────────────────────────────────────
            op @ BatchOp::StartMigration(_) => self.handle_start_migration(op).await,
            op @ BatchOp::CommitMigration(_) => self.handle_commit_migration(op).await,
            op @ BatchOp::RollbackMigration(_) => self.handle_rollback_migration(op).await,
            op @ BatchOp::MigrationStatus(_) => self.handle_migration_status(op).await,

            // ── Access control ─────────────────────────────────────────
            BatchOp::Chmod(op) => self.handle_chmod(op).await,
            BatchOp::Chown(op) => self.handle_chown(op).await,
            BatchOp::Chgrp(op) => self.handle_chgrp(op).await,
            BatchOp::CreateGroup(op) => self.handle_create_group(op).await,
            BatchOp::DropGroup(op) => self.handle_drop_group(op).await,
            BatchOp::AddGroupMember(op) => self.handle_add_group_member(op).await,
            BatchOp::RemoveGroupMember(op) => self.handle_remove_group_member(op).await,
            op @ BatchOp::AccessTree(_) => self.handle_access_tree(op).await,

            // ── Function DDL ───────────────────────────────────────────
            op @ BatchOp::CreateFunction(_) => self.handle_create_function(op).await,
            op @ BatchOp::DropFunction(_) => self.handle_drop_function(op).await,
            op @ BatchOp::RenameFunction(_) => self.handle_rename_function(op).await,
            op @ BatchOp::CreateFunctionFolder(_) => self.handle_create_function_folder(op).await,
            op @ BatchOp::RenameFunctionFolder(_) => self.handle_rename_function_folder(op).await,

            // ── Validator DDL ──────────────────────────────────────────
            op @ BatchOp::CreateValidator(_) => self.handle_create_validator(op).await,
            op @ BatchOp::DropValidator(_) => self.handle_drop_validator(op).await,
            op @ BatchOp::RenameValidator(_) => self.handle_rename_validator(op).await,
            op @ BatchOp::BindValidator(_) => self.handle_bind_validator(op).await,
            op @ BatchOp::UnbindValidator(_) => self.handle_unbind_validator(op).await,
            op @ BatchOp::ListValidators(_) => self.handle_list_validators(op).await,

            // ── Declarative schema DDL (Phase A) ──────────────────────
            BatchOp::SetTableSchema(op) => self.handle_set_table_schema(op).await,
            BatchOp::AddSchemaRule(op) => self.handle_add_schema_rule(op).await,
            BatchOp::RemoveSchemaRule(op) => self.handle_remove_schema_rule(op).await,
            BatchOp::GetTableSchema(op) => self.handle_get_table_schema(op).await,
            BatchOp::DescribeTable(op) => self.handle_describe_table(op).await,

            // ── Retention / History ────────────────────────────────────
            op @ BatchOp::SetRetention(_) => self.handle_set_retention(op).await,
            op @ BatchOp::PurgeHistory(_) => self.handle_purge_history(op).await,
            op @ BatchOp::ChangesSince(_) => self.handle_changes_since(op).await,

            // ── Interner (Stage 5d) ────────────────────────────────────
            op @ BatchOp::InternerDump(_) => self.handle_interner_dump(op).await,
            op @ BatchOp::InternerTouch(_) => self.handle_interner_touch(op).await,

            _ => Err(err("Not an admin operation".to_string())),
        }
    }
}

impl ShamirAdminExecutor {
    /// Authorize a user-lifecycle op (`CreateUser` / `DropUser`) under the
    /// owner-delegation model.
    ///
    /// Two acceptance paths (either suffices):
    ///   1. **Global admin** — `Manage` on [`ResourcePath::Root`]. `System`
    ///      bypasses inside [`authorize_access`]. A global admin may manage
    ///      any user, scoped or not.
    ///   2. **Database owner** — when `scope == Some(db)` and the actor holds
    ///      `Manage` on [`ResourcePath::Database`] for that `db`. Lets a
    ///      database owner manage users bound to *their* database without
    ///      global-admin rights.
    ///
    /// Returns the original root-level [`AccessError`] when neither path
    /// admits, so the denial message reflects the admin domain.
    pub(super) async fn authorize_user_lifecycle(
        &self,
        scope: Option<&str>,
    ) -> Result<(), shamir_types::access::AccessError> {
        // Path 1: global admin (Manage on the root). System bypasses here.
        let root_decision = self
            .shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await;
        if root_decision.is_ok() {
            return Ok(());
        }

        // Path 2: database owner of the user's scope.
        if let Some(db) = scope {
            if self
                .shamir
                .authorize_access(
                    &self.actor,
                    &ResourcePath::Database { db: db.to_string() },
                    Action::Manage,
                )
                .await
                .is_ok()
            {
                return Ok(());
            }
        }

        // Neither path admits — surface the root-level denial.
        root_decision
    }
}
