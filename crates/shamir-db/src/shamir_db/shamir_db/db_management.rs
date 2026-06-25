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
                .save_database(name, &record, &ResourceMeta::owned_enforced(actor))
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

        let meta = ResourceMeta::owned_enforced(actor.clone());

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

    /// Rename a repository, preserving its tables, data, indexes, and
    /// catalogue metadata (Phase F.3 — RENAME REPO).
    ///
    /// Contract:
    /// - **Logical re-key only** — the in-memory `RepoInstance` is moved
    ///   to the new key in `DbInstance::repos` and its `name` field is
    ///   updated. The repo's physical table stores
    ///   (`__data__<table>` / `__info__<table>` / `__history__<table>`)
    ///   are keyed only by table name *inside* the repo, so they travel
    ///   with the repo under the new logical key at zero cost. No
    ///   `rename_table_stores` is invoked and no drain is needed.
    /// - **Catalogue re-key** — the old `(db, from)` repositories row is
    ///   removed and a new `(db, to)` row is written preserving
    ///   `engine` / `path` / `ResourceMeta`. Every child table's catalogue
    ///   row `(db, from, table)` is likewise re-keyed to `(db, to, table)`.
    ///
    /// Guards (refuse with a typed [`DbError`] instead of leaving
    /// dangling state):
    /// - The source repo must exist; the destination must not.
    pub async fn rename_repo_as(
        &self,
        db_name: &str,
        from: &str,
        to: &str,
        _actor: Actor,
    ) -> DbResult<()> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;

        // Existence guards.
        if !db.has_repo(from) {
            return Err(DbError::NotFound(format!(
                "Repository '{}/{}' not found",
                db_name, from
            )));
        }
        if db.has_repo(to) {
            return Err(DbError::Validation(format!(
                "cannot rename repo '{}/{}' to '{}': destination repository already exists",
                db_name, from, to
            )));
        }

        // Snapshot the table list BEFORE the re-key so the catalogue rows
        // can be rewritten once the in-memory re-key has succeeded.
        let table_names = db.list_tables(from).unwrap_or_default();

        // Load the persisted repo record to preserve engine/path/meta
        // across the re-key.
        let old_repo_record = self
            .system_store
            .load_repository_record(db_name, from)
            .await?
            .ok_or_else(|| {
                DbError::NotFound(format!(
                    "repository catalogue record for '{}/{}' not found",
                    db_name, from
                ))
            })?;

        let engine = old_repo_record
            .get("engine")
            .and_then(|v| v.as_str())
            .unwrap_or("in_memory")
            .to_string();
        let path = old_repo_record
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let existing_meta = ResourceMeta::from_record(&old_repo_record);

        // 1. In-memory re-key (no per-table store copy).
        let renamed = db.rename_repo(from, to);
        debug_assert!(renamed, "rename_repo returned false despite has_repo guard");

        // 2. Persist the new repo catalogue row FIRST so a crash between
        //    the two writes leaves the new repo resolvable on reboot; a
        //    stale (db, from) row resurrects nothing because the live
        //    registration under `from` is already gone.
        if let Err(e) = self
            .system_store
            .save_repository(db_name, to, &engine, path.as_deref(), &existing_meta)
            .await
        {
            log::warn!(
                "shamir_db::rename_repo: failed to persist new catalogue row '{}/{}': {}",
                db_name,
                to,
                e
            );
        }
        if let Err(e) = self.system_store.remove_repository(db_name, from).await {
            log::warn!(
                "shamir_db::rename_repo: failed to remove old catalogue row '{}/{}': {}",
                db_name,
                from,
                e
            );
        }

        // 3. Re-key every child table's catalogue row: load the old row,
        //    write a new one under `(db, to, table)` preserving meta +
        //    enable_indexes, then remove the old `(db, from, table)` row.
        //    Write-before-remove so a crash leaves the new row resolvable.
        for table_name in &table_names {
            match self
                .system_store
                .load_table_record(db_name, from, table_name)
                .await
            {
                Ok(Some(rec)) => {
                    let enable_indexes = rec
                        .get("enable_indexes")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let table_meta = ResourceMeta::from_record(&rec);
                    if let Err(e) = self
                        .system_store
                        .save_table(db_name, to, table_name, enable_indexes, &table_meta)
                        .await
                    {
                        log::warn!(
                            "shamir_db::rename_repo: failed to persist new table catalogue \
                             row '{}/{}/{}': {}",
                            db_name,
                            to,
                            table_name,
                            e
                        );
                    }
                    if let Err(e) = self
                        .system_store
                        .remove_table(db_name, from, table_name)
                        .await
                    {
                        log::warn!(
                            "shamir_db::rename_repo: failed to remove old table catalogue \
                             row '{}/{}/{}': {}",
                            db_name,
                            from,
                            table_name,
                            e
                        );
                    }
                }
                Ok(None) => {
                    log::warn!(
                        "shamir_db::rename_repo: no catalogue row for '{}/{}/{}' \
                         (continuing)",
                        db_name,
                        from,
                        table_name
                    );
                }
                Err(e) => {
                    log::warn!(
                        "shamir_db::rename_repo: failed to load table catalogue \
                         '{}/{}/{}': {}",
                        db_name,
                        from,
                        table_name,
                        e
                    );
                }
            }
        }

        Ok(())
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
