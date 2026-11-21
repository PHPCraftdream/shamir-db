//! `AdminExecutor` trait impl + thin `execute_admin` dispatcher.

use std::future::Future;
use std::pin::Pin;

use crate::access::{Action, Actor, ResourcePath};
use crate::query::batch::{AdminExecutor, BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::shamir_db::ports::RepoAdminObserver;
use shamir_query_types::admin::{AlterRepoOp, ResourceRef};

use super::super::shamir_db::{ShamirDb, SYSTEM_DB_NAME};
use super::admin_db_schema_migration::SchemaMigrationLifecycleGuard;
use super::helpers::admin_result;

/// Names an explicit database mutation (including the namespace slot for
/// CREATE), not a read, global object's reference or rename destination.
pub(super) fn explicit_migration_target(op: &BatchOp) -> Option<&str> {
    let resource = match op {
        BatchOp::CreateDb(op) => return Some(&op.create_db),
        BatchOp::DropDb(op) => return Some(&op.drop_db),
        BatchOp::RenameDb(op) => return Some(&op.rename_db),
        BatchOp::BindValidator(op) => return Some(&op.db),
        BatchOp::UnbindValidator(op) => return Some(&op.db),
        BatchOp::Chmod(op) => &op.chmod,
        BatchOp::Chown(op) => &op.chown,
        BatchOp::Chgrp(op) => &op.chgrp,
        _ => return None,
    };
    resource_migration_target(resource)
}

pub(super) fn resource_migration_target(resource: &ResourceRef) -> Option<&str> {
    match resource {
        ResourceRef::Database { database: db }
        | ResourceRef::Store { store: [db, _] }
        | ResourceRef::Table { table: [db, ..] } => Some(db),
        _ => None,
    }
}

/// AdminExecutor that operates on ShamirDb.
pub(super) struct ShamirAdminExecutor {
    pub(super) shamir: ShamirDb,
    pub(super) db_name: String,
    pub(super) actor: Actor,
    pub(super) repo_admin_observer: Option<std::sync::Arc<dyn RepoAdminObserver>>,
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
            BatchOp::CreateDb(_)
            | BatchOp::DropDb(_)
            | BatchOp::RenameDb(_)
            | BatchOp::BindValidator(_)
            | BatchOp::UnbindValidator(_)
            | BatchOp::ListValidators(_)
            | BatchOp::Chmod(_)
            | BatchOp::Chown(_)
            | BatchOp::Chgrp(_)
            | BatchOp::AccessTree(_) => self.dispatch_target_admin(op).await,
            // ── DB / Repo ──────────────────────────────────────────────
            BatchOp::CreateRepo(op) => self.handle_create_repo(op).await,
            BatchOp::DropRepo(op) => self.handle_drop_repo(op).await,
            BatchOp::RenameRepo(op) => self.handle_rename_repo(op).await,
            // S5c-X Round-11 (item 5): routed through `dispatch_alter_repo`
            // so the durability-swap arm's await state stays out of
            // `execute_admin`'s own future (nested-Batch Windows stack
            // overflow; same mechanism as the Round-14 replication fix).
            BatchOp::AlterRepo(op) => Self::dispatch_alter_repo(self, op).await,
            BatchOp::DescribeRepo(op) => self.handle_describe_repo(op).await,

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
            BatchOp::GrantRole(op) => self.handle_grant_role(op).await,
            BatchOp::RevokeRole(op) => self.handle_revoke_role(op).await,

            // ── Migration ──────────────────────────────────────────────
            op @ BatchOp::StartMigration(_) => self.handle_start_migration(op).await,
            op @ BatchOp::CommitMigration(_) => self.handle_commit_migration(op).await,
            op @ BatchOp::RollbackMigration(_) => self.handle_rollback_migration(op).await,
            op @ BatchOp::MigrationStatus(_) => self.handle_migration_status(op).await,

            // ── Access control ─────────────────────────────────────────
            BatchOp::CreateGroup(op) => self.handle_create_group(op).await,
            BatchOp::DropGroup(op) => self.handle_drop_group(op).await,
            BatchOp::RenameGroup(op) => self.handle_rename_group(op).await,
            BatchOp::AddGroupMember(op) => self.handle_add_group_member(op).await,
            BatchOp::RemoveGroupMember(op) => self.handle_remove_group_member(op).await,

            // ── Function DDL ───────────────────────────────────────────
            op @ BatchOp::CreateFunction(_) => self.handle_create_function(op).await,
            op @ BatchOp::DropFunction(_) => self.handle_drop_function(op).await,
            op @ BatchOp::RenameFunction(_) => self.handle_rename_function(op).await,
            op @ BatchOp::DescribeFunction(_) => self.handle_describe_function(op).await,
            op @ BatchOp::ExportFunction(_) => self.handle_export_function(op).await,
            op @ BatchOp::CheckFunction(_) => self.handle_check_function(op).await,
            op @ BatchOp::AlterFunction(_) => self.handle_alter_function(op).await,
            op @ BatchOp::CreateFunctionFolder(_) => self.handle_create_function_folder(op).await,
            op @ BatchOp::RenameFunctionFolder(_) => self.handle_rename_function_folder(op).await,

            // ── Validator DDL ──────────────────────────────────────────
            op @ BatchOp::CreateValidator(_) => self.handle_create_validator(op).await,
            op @ BatchOp::DropValidator(_) => self.handle_drop_validator(op).await,
            op @ BatchOp::RenameValidator(_) => self.handle_rename_validator(op).await,

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

            // ── Application schema migrations (R1c 2b-1) ───────────────
            // Routed through a boxed dispatcher so the status arm's await
            // state stays out of `execute_admin`'s own future (Windows
            // nested-Batch stack overflow; same mechanism as dispatch_repl).
            op @ BatchOp::ApplicationSchema(_) => Self::dispatch_schema_migration(self, op).await,

            // ── Node-local online backup (R2a step 3a, design D6) ─────
            // Boxed dispatcher for the same Windows nested-Batch stack
            // reason as above; all three are node-local, NOT
            // database-scoped, and run through the injected
            // `OnlineBackupAdminPort` (server registry).
            op if matches!(
                op,
                BatchOp::StartBackup(_) | BatchOp::BackupStatus(_) | BatchOp::CancelBackup(_)
            ) =>
            {
                Self::dispatch_online_backup(self, op).await
            }

            // ── Replication DDL (386-a) ────────────────────────────────
            // Round-14: routed through `dispatch_repl` so the replication
            // arms' await state stays out of `execute_admin`'s own future
            // (Round-12/14 Windows stack overflow).
            op if matches!(
                op,
                BatchOp::CreateReplicationProfile(_)
                    | BatchOp::DropReplicationProfile(_)
                    | BatchOp::CreatePublication(_)
                    | BatchOp::DropPublication(_)
                    | BatchOp::CreateSubscription(_)
                    | BatchOp::DropSubscription(_)
                    | BatchOp::AlterSubscription(_)
                    | BatchOp::ListPublications(_)
                    | BatchOp::ListSubscriptions(_)
                    | BatchOp::ReplicationStatus(_)
            ) =>
            {
                Self::dispatch_repl(self, op).await
            }

            // ── Interner (Stage 5d) ────────────────────────────────────
            op @ BatchOp::InternerDump(_) => self.handle_interner_dump(op).await,
            op @ BatchOp::InternerTouch(_) => self.handle_interner_touch(op).await,

            _ => Err(err("Not an admin operation".to_string())),
        }
    }
}

/// Mirror of `NestedTxBodyFuture` (batch_execute.rs): keeps the boxed
/// dispatcher futures' type off `dispatch_repl` / `dispatch_alter_repo`
/// signatures so `clippy::type_complexity` stays quiet under `-D warnings`.
type AdminDispatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<QueryResult, BatchError>> + Send + 'a>>;

impl ShamirAdminExecutor {
    #[inline(never)]
    fn dispatch_target_admin<'a>(&'a self, op: &'a BatchOp) -> AdminDispatchFuture<'a> {
        Box::pin(async move {
            match op {
                BatchOp::CreateDb(op) => Box::pin(self.handle_create_db(op)).await,
                BatchOp::DropDb(op) => Box::pin(self.handle_drop_db(op)).await,
                BatchOp::RenameDb(op) => Box::pin(self.handle_rename_db(op)).await,
                BatchOp::Chmod(op) => Box::pin(self.handle_chmod(op)).await,
                BatchOp::Chown(op) => Box::pin(self.handle_chown(op)).await,
                BatchOp::Chgrp(op) => Box::pin(self.handle_chgrp(op)).await,
                BatchOp::AccessTree(_) => Box::pin(self.handle_access_tree(op)).await,
                BatchOp::ListValidators(_) => Box::pin(self.handle_list_validators(op)).await,
                BatchOp::BindValidator(_) | BatchOp::UnbindValidator(_) => {
                    Box::pin(self.dispatch_validator_binding(op)).await
                }
                _ => unreachable!("not a target admin operation"),
            }
        })
    }

    /// Bind the target identity before ACL under the existing rename/drop interlock.
    /// The returned reservation holds no control lock: table metadata writes reacquire it.
    pub(super) async fn authorize_admin_target_write(
        &self,
        db: Option<&str>,
        path: &ResourcePath,
        action: Action,
    ) -> Result<Option<SchemaMigrationLifecycleGuard<'_>>, BatchError> {
        let db = db.filter(|db| *db != SYSTEM_DB_NAME);
        let _control = if db.is_some() {
            Some(self.shamir.lock_replication_control().await)
        } else {
            None
        };
        // Capture only; ACL still takes precedence over missing-target errors.
        let target = db.map(|db| (db, self.shamir.database_identity_for_fence(db)));
        self.shamir
            .authorize_access(&self.actor, path, action)
            .await
            .map_err(|error| BatchError::query_coded("", "access_denied", error.to_string()))?;
        let Some((db, id)) = target else {
            return Ok(None);
        };
        let missing = || BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{db}' not found"),
            code: None,
        };
        let id = id.ok_or_else(missing)?;
        let guard = self
            .shamir
            .reserve_admin_migration_target(id)
            .map_err(|code| match code {
                Some(code) => BatchError::query_coded(
                    "",
                    code,
                    format!(
                        "an application-schema migration holds the maintenance fence for \
                             database '{db}'; writes, DDL and stored-procedure calls are refused \
                             until it completes"
                    ),
                ),
                None => BatchError::QueryError {
                    alias: String::new(),
                    message: format!("another database operation is in progress for '{db}'"),
                    code: None,
                },
            })?;
        // Once verified, the reservation prevents rename/drop until the write finishes.
        if self.shamir.database_identity_for_fence(db) != Some(id) {
            return Err(missing());
        }
        Ok(Some(guard))
    }

    /// Round-14: replication DDL dispatcher, kept OUT of `execute_admin`'s
    /// own future. `#[inline(never)]` + explicit boxing means the nine
    /// replication arms' await state lives on the heap, not in every
    /// `execute_admin` state machine along the nested-batch recursion
    /// (Round-12/14 Windows stack overflow).
    #[inline(never)]
    fn dispatch_repl<'a>(&'a self, op: &'a BatchOp) -> AdminDispatchFuture<'a> {
        Box::pin(async move {
            match op {
                BatchOp::CreateReplicationProfile(op) => {
                    self.handle_create_replication_profile(op).await
                }
                BatchOp::DropReplicationProfile(op) => {
                    self.handle_drop_replication_profile(op).await
                }
                BatchOp::CreatePublication(op) => self.handle_create_publication(op).await,
                BatchOp::DropPublication(op) => self.handle_drop_publication(op).await,
                BatchOp::CreateSubscription(op) => self.handle_create_subscription(op).await,
                BatchOp::DropSubscription(op) => self.handle_drop_subscription(op).await,
                BatchOp::AlterSubscription(op) => self.handle_alter_subscription(op).await,
                BatchOp::ListPublications(_) => self.handle_list_publications().await,
                BatchOp::ListSubscriptions(_) => self.handle_list_subscriptions().await,
                BatchOp::ReplicationStatus(_) => self.handle_replication_status().await,
                _ => Err(BatchError::QueryError {
                    alias: String::new(),
                    message: "Not a replication operation".to_string(),
                    code: None,
                }),
            }
        })
    }

    /// R1c 2b-1: application-schema migration dispatcher, boxed for the same
    /// reason as `dispatch_repl` — the status arm's durable-journal reads
    /// must not grow `execute_admin`'s own state machine.
    #[inline(never)]
    fn dispatch_schema_migration<'a>(&'a self, op: &'a BatchOp) -> AdminDispatchFuture<'a> {
        Box::pin(async move {
            match op {
                BatchOp::ApplicationSchema(_) => self.handle_application_schema(op).await,
                _ => Err(BatchError::QueryError {
                    alias: String::new(),
                    message: "Not an application-schema operation".to_string(),
                    code: None,
                }),
            }
        })
    }

    /// R2a step 3a (design D6): node-local online-backup dispatcher, boxed
    /// for the same Windows nested-Batch stack reason as
    /// `dispatch_schema_migration`.
    #[inline(never)]
    fn dispatch_online_backup<'a>(&'a self, op: &'a BatchOp) -> AdminDispatchFuture<'a> {
        Box::pin(async move {
            match op {
                BatchOp::StartBackup(_) => self.handle_online_backup(op).await,
                BatchOp::BackupStatus(_) => self.handle_online_backup(op).await,
                BatchOp::CancelBackup(_) => self.handle_online_backup(op).await,
                _ => Err(BatchError::QueryError {
                    alias: String::new(),
                    message: "Not an online-backup operation".to_string(),
                    code: None,
                }),
            }
        })
    }

    /// Node-local online backup (design `online-backup.md` §4.7, D6):
    /// delegate to the injected `OnlineBackupAdminPort` (the server's
    /// `OnlineBackupRegistry`) and wrap the returned record in the standard
    /// single-record admin result. NOT database-scoped — no per-database
    /// authorization here; the superuser gate is the server handler bridge
    /// (`handler.rs` `entry.op.is_admin()`).
    async fn handle_online_backup(&self, op: &BatchOp) -> Result<QueryResult, BatchError> {
        let error = |error: crate::OnlineBackupAdminError| BatchError::QueryError {
            alias: String::new(),
            message: error.to_string(),
            code: Some(error.code().to_string()),
        };
        let port = self
            .shamir
            .online_backup_admin_port()
            .ok_or_else(|| error(crate::OnlineBackupAdminError::NotConfigured))?;
        let value = match op {
            BatchOp::StartBackup(op) => port.start(op.to.clone()).await,
            BatchOp::BackupStatus(_) => port.status().await,
            BatchOp::CancelBackup(_) => port.cancel().await,
            _ => {
                return Err(BatchError::QueryError {
                    alias: String::new(),
                    message: "Not an online-backup operation".to_string(),
                    code: None,
                });
            }
        };
        value.map(admin_result).map_err(error)
    }

    /// S5c-X Round-11 (item 5): ALTER REPO dispatcher, kept OUT of
    /// `execute_admin`'s own future. `#[inline(never)]` + explicit boxing
    /// means the durability path's (admission, catalogue persist, live
    /// policy swap) await state lives on the heap, not in every
    /// `execute_admin` state machine along the nested-Batch/ForEach
    /// recursion (Windows stack overflow in the repo_audit tests).
    #[inline(never)]
    fn dispatch_alter_repo<'a>(&'a self, op: &'a AlterRepoOp) -> AdminDispatchFuture<'a> {
        Box::pin(async move { self.handle_alter_repo(op).await })
    }

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
