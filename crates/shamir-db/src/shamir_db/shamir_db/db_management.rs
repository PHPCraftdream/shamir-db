use crate::access::{Actor, ResourceMeta};
use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::repo::RepoConfig;
use crate::{DbError, DbResult};

use super::ShamirDb;
use super::SYSTEM_DB_NAME;

impl ShamirDb {
    pub async fn create_db(&self, name: &str) -> DbInstance {
        self.create_db_as(name, Actor::System).await
    }

    /// Like [`create_db`] but stamps the new database's owner as `actor`
    /// instead of `System`. Mode stays `0o777` (open).
    pub async fn create_db_as(&self, name: &str, actor: Actor) -> DbInstance {
        let db = DbInstance::new();
        self.dbs.insert(name.to_string(), db.clone());

        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Persist to system store
        {
            let mut m = shamir_types::types::common::new_map();
            m.insert(
                "name".to_string(),
                shamir_types::types::value::QueryValue::Str(name.to_string()),
            );
            m.insert(
                "created_at".to_string(),
                shamir_types::types::value::QueryValue::Int(created_at as i64),
            );
            let record = shamir_types::types::value::QueryValue::Map(m);
            if let Err(e) = self
                .system_store
                .save_database(name, &record, &ResourceMeta::owned_by(actor))
                .await
            {
                log::warn!("shamir_db::create_db: failed to persist '{}': {}", name, e);
            }
        }

        db
    }

    pub async fn remove_db(&self, name: &str) -> bool {
        if name == SYSTEM_DB_NAME {
            return false;
        }

        let removed = self.dbs.remove(name).is_some();

        if removed {
            if let Err(e) = self.system_store.remove_database(name).await {
                log::warn!(
                    "shamir_db::remove_db: failed to remove '{}' from system store: {}",
                    name,
                    e
                );
            }
        }

        removed
    }

    pub async fn add_repo(&self, db_name: &str, config: RepoConfig) -> DbResult<()> {
        self.add_repo_as(db_name, config, Actor::System).await
    }

    /// Like [`add_repo`] but stamps the repo (and its inline tables) with
    /// the given actor as owner.
    pub async fn add_repo_as(
        &self,
        db_name: &str,
        config: RepoConfig,
        actor: Actor,
    ) -> DbResult<()> {
        // Owned clone (cheap Arc) — never hold the DashMap shard guard
        // across the `add_repo` / recovery awaits below.
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;

        let repo_name = config.name.clone();
        let storage_type = Self::extract_storage_type(&config.factory);
        let path = Self::extract_path(&config.factory);
        // Capture the inline table list before `config` is moved into
        // `db.add_repo`, so the per-repo table catalogue can be persisted
        // alongside the repo record (I.2).
        let inline_tables: Vec<(String, bool)> = config
            .tables
            .iter()
            .map(|t| (t.name.clone(), t.enable_indexes))
            .collect();

        db.add_repo(config).await?;

        // CRIT-A: run V2 WAL crash recovery before the repo is reachable
        // by callers. For a freshly created repo `list_inflight` is empty
        // so this is a cheap no-op; for a *re-attached* on-disk repo it
        // replays any inflight tx left by a prior crash. Recovery failure
        // is propagated — a repo that cannot recover must not be served.
        if let Some(repo) = db.get_repo(&repo_name) {
            let recovered = repo.recover_v2_inflight().await?;
            if recovered > 0 {
                log::info!(
                    "recovered {} inflight transactions for repo '{}/{}'",
                    recovered,
                    db_name,
                    repo_name
                );
            }
        }

        let meta = ResourceMeta::owned_by(actor.clone());

        // Persist to system store
        if let Err(e) = self
            .system_store
            .save_repository(db_name, &repo_name, &storage_type, path.as_deref(), &meta)
            .await
        {
            log::warn!(
                "shamir_db::add_repo: failed to persist '{}/{}': {}",
                db_name,
                repo_name,
                e
            );
        }

        // Persist the inline table catalogue so these tables are re-created
        // on the next open (I.2). Best-effort per table, matching the
        // repo-record persistence above.
        for (table_name, enable_indexes) in &inline_tables {
            if let Err(e) = self
                .system_store
                .save_table(db_name, &repo_name, table_name, *enable_indexes, &meta)
                .await
            {
                log::warn!(
                    "shamir_db::add_repo: failed to persist table catalogue '{}/{}/{}': {}",
                    db_name,
                    repo_name,
                    table_name,
                    e
                );
            }
        }

        Ok(())
    }

    pub async fn remove_repo(&self, db_name: &str, repo_name: &str) -> bool {
        if let Some(db) = self.get_db(db_name) {
            let removed = db.remove_repo(repo_name).await;
            if removed {
                if let Err(e) = self
                    .system_store
                    .remove_repository(db_name, repo_name)
                    .await
                {
                    log::warn!(
                        "shamir_db::remove_repo: failed to remove '{}/{}' from system store: {}",
                        db_name,
                        repo_name,
                        e
                    );
                }
            }
            removed
        } else {
            false
        }
    }

    /// Drain every repo's in-memory MemBuffers to their durable backing.
    ///
    /// Called on graceful shutdown to close the ~500 ms buffered-commit
    /// loss window. For each repo the tx-info store and every table's
    /// data + info stores are flushed. In-memory stores are no-ops.
    /// Best-effort: individual errors are logged and skipped; returns the
    /// first error encountered (if any) after attempting all repos/tables.
    pub async fn flush_all(&self) -> DbResult<()> {
        let mut first_err: Option<DbError> = None;
        let db_names = self.list_dbs();
        for db_name in &db_names {
            let Some(db) = self.get_db(db_name) else {
                continue;
            };
            for repo_name in db.list_repos() {
                let Some(repo) = db.get_repo(&repo_name) else {
                    continue;
                };

                if let Err(e) = repo.flush_buffers().await {
                    log::warn!("flush_all: {}/{}: {}", db_name, repo_name, e);
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
