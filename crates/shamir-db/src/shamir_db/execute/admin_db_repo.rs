//! Admin handlers: CreateDb, DropDb, CreateRepo, DropRepo.

use crate::access::{Action, ResourcePath};
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::mpack;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, validate_name_component};

impl ShamirAdminExecutor {
    pub(super) async fn handle_create_db(
        &self,
        op: &crate::query::admin::CreateDbOp,
    ) -> Result<QueryResult, BatchError> {
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        validate_name_component(&op.create_db, "db_name")?;

        // TOCTOU close (task #546): hold `db_create_lock` across the WHOLE
        // exists-check -> authorize -> create sequence, so two concurrent
        // `CREATE DATABASE` (or `IF NOT EXISTS`) calls for the SAME name
        // can't both observe "does not exist" and both proceed to create.
        // The audit framed this as narrow (create already happens under an
        // internal per-instance lock, so ACL is respected end-to-end) — this
        // is an idempotency/race-on-the-exists-check hazard, not a rights
        // bypass; the lock is global (not per-name) since db creation is
        // rare, mirroring `group_id_lock`'s established pattern.
        let _create_guard = self.shamir.db_create_lock().lock().await;

        if self.shamir.has_db(&op.create_db) {
            if op.if_not_exists {
                return Ok(admin_result(mpack!({
                    "created": false,
                    "existed": true,
                    "db": @(QueryValue::Str(op.create_db.clone()))
                })));
            }
            return Err(err_code(
                "exists",
                format!("Database '{}' already exists", op.create_db),
            ));
        }
        self.shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Create)
            .await
            .map_err(err_access)?;
        self.shamir
            .create_db_as(&op.create_db, self.actor.clone())
            .await;
        Ok(admin_result(mpack!({
            "created": true,
            "existed": false,
            "db": @(QueryValue::Str(op.create_db.clone()))
        })))
    }

    pub(super) async fn handle_drop_db(
        &self,
        op: &crate::query::admin::DropDbOp,
    ) -> Result<QueryResult, BatchError> {
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // if_exists early-exit: missing db → no-op.
        if op.if_exists && !self.shamir.has_db(&op.drop_db) {
            return Ok(admin_result(mpack!({
                "dropped": @(QueryValue::Str(op.drop_db.clone())),
                "existed": false,
            })));
        }

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::database(op.drop_db.clone()),
                Action::Delete,
            )
            .await
            .map_err(err_access)?;
        // Referential integrity: check for child repositories.
        if let Some(db) = self.shamir.get_db(&op.drop_db) {
            let repos = db.list_repos();
            if !repos.is_empty() {
                if !op.cascade {
                    return Err(err_code(
                        "still_referenced",
                        format!(
                            "cannot drop database '{}': still has repositories: {:?}",
                            op.drop_db, repos
                        ),
                    ));
                }
                // Cascade: remove every repo (and its tables) first.
                for repo_name in &repos {
                    // Remove tables within each repo.
                    if let Ok(tables) = db.list_tables(repo_name) {
                        for table_name in &tables {
                            let _ = self
                                .shamir
                                .drop_table_cleaning_validators(
                                    &self.db_name,
                                    repo_name,
                                    table_name,
                                )
                                .await;
                        }
                    }
                    self.shamir.remove_repo(&op.drop_db, repo_name).await;
                }
            }
        }
        let removed = self.shamir.remove_db(&op.drop_db).await;
        Ok(admin_result(mpack!({
            "dropped": @(QueryValue::Str(op.drop_db.clone())),
            "existed": @(QueryValue::Bool(removed)),
        })))
    }

    pub(super) async fn handle_create_repo(
        &self,
        op: &crate::query::admin::CreateRepoOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        validate_name_component(&self.db_name, "db_name")?;
        validate_name_component(&op.create_repo, "repo_name")?;

        // TOCTOU close (task #546): hold a per-db lock across the WHOLE
        // exists-check -> authorize -> create sequence, so two concurrent
        // `CREATE REPO` (or `IF NOT EXISTS`) calls for the same (db, repo)
        // name can't both observe "does not exist" and both proceed to
        // create. Keyed by `db_name` (not globally) so repo creation in
        // unrelated databases doesn't serialise against each other — mirrors
        // `admin_user_locks`'s per-key get-or-insert-then-lock pattern.
        let repo_lock = self
            .shamir
            .repo_create_locks()
            .entry(self.db_name.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _create_guard = repo_lock.lock().await;

        // Check existence for if_not_exists / duplicate guard.
        if let Some(db) = self.shamir.get_db(&self.db_name) {
            if db.has_repo(&op.create_repo) {
                if op.if_not_exists {
                    return Ok(admin_result(mpack!({
                        "created": false,
                        "existed": true,
                        "repo": @(QueryValue::Str(op.create_repo.clone()))
                    })));
                }
                return Err(err_code(
                    "exists",
                    format!(
                        "Repository '{}' already exists in database '{}'",
                        op.create_repo, self.db_name
                    ),
                ));
            }
        }

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::Database {
                    db: self.db_name.clone(),
                },
                Action::Create,
            )
            .await
            .map_err(err_access)?;

        let factory = match op.engine.as_deref() {
            Some("in_memory") => BoxRepoFactory::in_memory(),
            Some("fjall") | None => {
                // Durable default: if the home has a data_root,
                // use a fjall directory under data_root/<db>/<repo>.
                // In-memory home (tests) falls back to in_memory.
                match self.shamir.data_root() {
                    Some(root) => {
                        let db_dir = root.join(&self.db_name);
                        tokio::fs::create_dir_all(&db_dir).await.map_err(|e| {
                            err(format!(
                                "failed to create repo directory '{}': {}",
                                db_dir.display(),
                                e
                            ))
                        })?;
                        let path = db_dir.join(&op.create_repo);
                        BoxRepoFactory::fjall_raw(path)
                    }
                    None => BoxRepoFactory::in_memory(),
                }
            }
            Some(other) => {
                return Err(err(format!(
                    "Unsupported engine '{}'. Supported: in_memory, fjall.",
                    other
                )));
            }
        };

        let mut config = RepoConfig::new(&op.create_repo, factory);
        for table_name in &op.tables {
            config = config.add_table(TableConfig::new(table_name));
        }

        self.shamir
            .add_repo_as(&self.db_name, config, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_repo": @(QueryValue::Str(op.create_repo.clone())),
            "created": true,
            "existed": false,
        })))
    }

    pub(super) async fn handle_drop_repo(
        &self,
        op: &crate::query::admin::DropRepoOp,
    ) -> Result<QueryResult, BatchError> {
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // if_exists early-exit: missing db or repo → no-op.
        if op.if_exists {
            let exists = self
                .shamir
                .get_db(&self.db_name)
                .is_some_and(|db| db.has_repo(&op.drop_repo));
            if !exists {
                return Ok(admin_result(mpack!({
                    "dropped_repo": @(QueryValue::Str(op.drop_repo.clone())),
                    "existed": false,
                })));
            }
        }

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::store(self.db_name.clone(), op.drop_repo.clone()),
                Action::Delete,
            )
            .await
            .map_err(err_access)?;
        // Referential integrity: check for child tables.
        if let Some(db) = self.shamir.get_db(&self.db_name) {
            if let Ok(tables) = db.list_tables(&op.drop_repo) {
                if !tables.is_empty() {
                    if !op.cascade {
                        return Err(err_code(
                            "still_referenced",
                            format!(
                                "cannot drop repository '{}': still has tables: {:?}",
                                op.drop_repo, tables
                            ),
                        ));
                    }
                    // Cascade: remove every table first.
                    for table_name in &tables {
                        let _ = self
                            .shamir
                            .drop_table_cleaning_validators(
                                &self.db_name,
                                &op.drop_repo,
                                table_name,
                            )
                            .await;
                    }
                }
            }
        }
        // Route through ShamirDb so the repo's catalogue record is
        // removed from the system store and the repo does not
        // resurrect on the next open (symmetry with CreateRepo).
        let removed = self.shamir.remove_repo(&self.db_name, &op.drop_repo).await;
        Ok(admin_result(mpack!({
            "dropped_repo": @(QueryValue::Str(op.drop_repo.clone())),
            "existed": @(QueryValue::Bool(removed)),
        })))
    }

    pub(super) async fn handle_rename_repo(
        &self,
        op: &crate::query::admin::RenameRepoOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        validate_name_component(&op.rename_repo, "repo_name")?;
        validate_name_component(&op.to, "repo_name")?;

        // Auth: Write on the source repo (rename mutates the repo's
        // identity). Mirrors rename_table's auth path.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::store(self.db_name.clone(), op.rename_repo.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        self.shamir
            .rename_repo_as(&self.db_name, &op.rename_repo, &op.to, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;

        Ok(admin_result(mpack!({
            "renamed_repo": @(QueryValue::Str(op.rename_repo.clone())),
            "to": @(QueryValue::Str(op.to.clone())),
        })))
    }

    pub(super) async fn handle_rename_db(
        &self,
        op: &crate::query::admin::RenameDbOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        validate_name_component(&op.rename_db, "db_name")?;
        validate_name_component(&op.to, "db_name")?;

        // Auth: Write on the source database (rename mutates the db's
        // identity). Mirrors handle_drop_db's auth path (db-level), but
        // uses Write as rename is a mutation, not a delete.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::database(op.rename_db.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        self.shamir
            .rename_db_as(&op.rename_db, &op.to, self.actor.clone())
            .await
            .map_err(|e| err(e.to_string()))?;

        Ok(admin_result(mpack!({
            "renamed_db": @(QueryValue::Str(op.rename_db.clone())),
            "to": @(QueryValue::Str(op.to.clone())),
        })))
    }
}
