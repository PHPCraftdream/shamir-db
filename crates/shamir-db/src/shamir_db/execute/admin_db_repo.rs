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

        validate_name_component(&op.create_db, "db_name")?;
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

        validate_name_component(&self.db_name, "db_name")?;
        validate_name_component(&op.create_repo, "repo_name")?;

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

        let factory = match op.engine.as_deref() {
            Some("in_memory") => BoxRepoFactory::in_memory(),
            Some("redb") | None => {
                // Durable default: if the home has a data_root,
                // use a redb file under data_root/<db>/<repo>.redb.
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
                        let path = db_dir.join(format!("{}.redb", op.create_repo));
                        BoxRepoFactory::redb_raw(path)
                    }
                    None => BoxRepoFactory::in_memory(),
                }
            }
            Some(other) => {
                return Err(err(format!(
                    "Unsupported engine '{}'. Supported: in_memory, redb.",
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
}
