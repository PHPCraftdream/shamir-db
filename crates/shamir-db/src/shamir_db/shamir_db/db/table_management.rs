use crate::access::{Actor, ResourceMeta};
use crate::engine::table::{TableConfig, TableManager};
use crate::shamir_db::shamir_db::schema_management::SCHEMA_FIELD;
use crate::types::value::QueryValue;
use crate::{DbError, DbResult};

use super::super::ShamirDb;

impl ShamirDb {
    /// Create a table in a repo and persist it to the table catalogue so it
    /// survives a restart (I.2).
    ///
    /// Delegates to [`DbInstance::create_table`] (the same path that lazily
    /// instantiates the `TableManager` on first access) and then records the
    /// table in the system store. A failed catalogue write propagates
    /// (RV-DB-6540DC7B).
    pub async fn add_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        enable_indexes: bool,
    ) -> DbResult<()> {
        self.add_table_as(
            db_name,
            repo_name,
            table_name,
            enable_indexes,
            Actor::System,
        )
        .await
    }

    /// Like [`add_table`] but stamps the new table with the given actor as
    /// owner.
    pub async fn add_table_as(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        enable_indexes: bool,
        actor: Actor,
    ) -> DbResult<()> {
        let _namespace_guard = self.lock_replication_control().await;
        self.add_table_as_locked(
            db_name,
            repo_name,
            table_name,
            enable_indexes,
            actor,
            &_namespace_guard,
        )
        .await
    }

    pub(crate) async fn add_table_as_locked(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        enable_indexes: bool,
        actor: Actor,
        _namespace_guard: &crate::shamir_db::shamir_db::ReplicationControlGuard<'_>,
    ) -> DbResult<()> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;

        let mut config = TableConfig::new(table_name);
        if enable_indexes {
            config = config.with_indexes();
        }
        db.create_table_checked_with_config(repo_name, config)
            .await?;

        // Propagation per RV-DB-6540DC7B.
        self.system_store
            .save_table(
                db_name,
                repo_name,
                table_name,
                enable_indexes,
                &ResourceMeta::owned_enforced(actor),
            )
            .await?;

        Ok(())
    }

    /// Drop a table from a repo and remove it from the table catalogue.
    /// Returns whether the table existed in the running instance.
    pub async fn drop_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<bool> {
        let _control = self.lock_replication_control().await;
        self.drop_table_locked(db_name, repo_name, table_name, &_control)
            .await
    }

    pub(crate) async fn drop_table_locked(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        _control: &crate::shamir_db::shamir_db::ReplicationControlGuard<'_>,
    ) -> DbResult<bool> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        let removed = db.drop_table_checked(repo_name, table_name).await?;

        // Always clear the catalogue entry (idempotent), even if the
        // in-memory table was already gone, so a stale record can't
        // resurrect the table on the next open.
        // Catalogue persistence failures propagate; a missing row stays an
        // idempotent no-op (the clear is documented idempotent).
        if let Err(e) = self
            .system_store
            .remove_table(db_name, repo_name, table_name)
            .await
        {
            if !matches!(e, DbError::NotFound(_)) {
                return Err(e);
            }
        }

        Ok(removed)
    }

    /// Drop a table, cleaning up any validator `bound_in` references first.
    ///
    /// This is the canonical "drop table" entry point for the executor:
    /// it removes the table from the repo, clears the table catalogue,
    /// AND unbinds every validator that was bound to this table so that
    /// `is_bound` does not reference a ghost table.
    pub async fn drop_table_cleaning_validators(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<bool> {
        let _control = self.lock_replication_control().await;
        self.drop_table_cleaning_validators_locked(db_name, repo_name, table_name, &_control)
            .await
    }

    pub(crate) async fn drop_table_cleaning_validators_locked(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        control: &crate::shamir_db::shamir_db::ReplicationControlGuard<'_>,
    ) -> DbResult<bool> {
        // 1. Clean validator bound_in references.
        let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
        let affected = self.validators.unbind_all_for_table(&table_ref);
        for (id, name) in &affected {
            self.persist_validator_bound_in(name, id).await;
        }

        // 2. Remove the declarative schema validator (if any) so there is
        //    no id/name leak in the registry after the table is gone.
        self.drop_schema_validator(db_name, repo_name, table_name);

        // 3. Drop the table itself.
        self.drop_table_locked(db_name, repo_name, table_name, control)
            .await
    }

    /// Rename a table. Convenience wrapper around [`rename_table_as`]
    /// using [`Actor::System`].
    ///
    /// // SAFETY (wire-reachability, task #546): NEVER call this from a
    /// // wire-reachable path — it runs the rename as `Actor::System`
    /// // unconditionally, silently bypassing ACL attribution for
    /// // whoever actually issued the request. `handle_rename_table` (the
    /// // real wire-reachable admin handler) already calls
    /// // `rename_table_as(db_name, repo_name, from, to, real_actor)`
    /// // directly, never this bare wrapper. This function exists ONLY
    /// // for offline/CLI tooling and test setup that legitimately runs
    /// // as the system principal — grep every call site before adding a
    /// // new one from request-handling code.
    /// // `#[doc(hidden)]` (not `pub(crate)`): narrowing visibility would
    /// // break 50+ integration test files (a separate compiled crate) —
    /// // no production caller outside `shamir-db/src`/`shamir-db/tests`
    /// // exists today, but the test blast radius alone makes
    /// // `pub(crate)` impractical; hiding from public rustdoc/API
    /// // discovery is the achievable P2 mitigation here (task #606).
    #[doc(hidden)]
    pub async fn rename_table(
        &self,
        db_name: &str,
        repo_name: &str,
        from: &str,
        to: &str,
    ) -> DbResult<()> {
        self.rename_table_as(db_name, repo_name, from, to, Actor::System)
            .await
    }

    /// Rename a table inside a repository, preserving its data,
    /// catalogue metadata, and in-memory registration.
    ///
    /// Contract:
    /// - The physical data stores (`__data__`, `__info__`, `__history__`)
    ///   are **copied** from the old name to the new one (see
    ///   [`RepoInstance::rename_table_stores`]). The old stores are
    ///   orphaned — same disposition as `DROP TABLE`, which orphans
    ///   `__data__` because the catalogue is the source of truth.
    /// - The catalogue record is re-keyed: the old `(db, repo, from)`
    ///   row is removed and a new `(db, repo, to)` row is written with
    ///   the same `enable_indexes` flag and the same `ResourceMeta`
    ///   (owner/group/mode) as the original.
    /// - The reverse-index (`token_names`) entry for the old name is
    ///   cleared and a fresh one for the new name is installed.
    ///
    /// Guards (refuse with a typed [`DbError::Validation`] instead of
    /// leaving dangling references):
    /// - The source table must exist; the destination must not.
    /// - The table must not carry a declarative schema — the schema
    ///   validator is registered under a name that embeds the table
    ///   name (`__schema__/<db>/<repo>/<table>`), so a rename would
    ///   orphan it. Rename of schema-bearing tables is a follow-on.
    /// - The table must not be referenced by a foreign key in any
    ///   other table in the same repo — `ref_table` is stored by name
    ///   in the child's persisted schema, so a rename would dangle it.
    pub async fn rename_table_as(
        &self,
        db_name: &str,
        repo_name: &str,
        from: &str,
        to: &str,
        _actor: Actor,
    ) -> DbResult<()> {
        let _control = self.lock_replication_control().await;
        self.rename_table_as_locked(db_name, repo_name, from, to, _actor, &_control)
            .await
    }

    pub(crate) async fn rename_table_as_locked(
        &self,
        db_name: &str,
        repo_name: &str,
        from: &str,
        to: &str,
        _actor: Actor,
        _control: &crate::shamir_db::shamir_db::ReplicationControlGuard<'_>,
    ) -> DbResult<()> {
        // F-68 (#895) cluster D / task #124 diagnostic instrumentation:
        // (e.g. `rename_populated_survives_cold_restart`, TIMEOUT 600.016s
        // on ubuntu-latest) shows whether the stall is inside this call at
        // all, and if so how far it got before the last log line. NOT a
        // behavior change — logging only.
        let rename_started = std::time::Instant::now();
        log::debug!("rename_table_as: enter db={db_name} repo={repo_name} from={from} to={to}");
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;

        // Existence guards.
        if !db.has_table(repo_name, from) {
            return Err(DbError::NotFound(format!(
                "Table '{}/{}/{}' not found",
                db_name, repo_name, from
            )));
        }
        if db.has_table(repo_name, to) {
            return Err(DbError::Validation(format!(
                "cannot rename '{}/{}' to '{}': destination table already exists",
                repo_name, from, to
            )));
        }

        // Load the persisted catalogue row to (a) guard against schema /
        // FK references and (b) preserve ResourceMeta across the re-key.
        let old_record = self
            .system_store
            .load_table_record(db_name, repo_name, from)
            .await?
            .ok_or_else(|| {
                DbError::NotFound(format!(
                    "table catalogue record for '{}/{}/{}' not found",
                    db_name, repo_name, from
                ))
            })?;

        // Schema guard: a declarative schema registers a validator whose
        // name embeds the table path. Renaming would orphan it.
        if old_record.get(SCHEMA_FIELD).is_some()
            && !matches!(old_record.get(SCHEMA_FIELD), Some(QueryValue::Null))
        {
            return Err(DbError::Validation(format!(
                "cannot rename table '{}': it carries a declarative schema; \
                 rename of schema-bearing tables is not yet supported",
                from
            )));
        }

        // Reverse-FK guard: refuse if another table in this repo
        // references `from` as `ref_table`. The child's persisted schema
        // stores the parent name literally, so renaming would dangle it.
        let catalogue = self.table_catalogue_snapshot(db_name, repo_name).await?;
        Self::check_fk_snapshot(&catalogue, from)?;

        // Preserve ResourceMeta (owner/group/mode) across the re-key.
        let existing_meta = ResourceMeta::from_record(&old_record);
        let enable_indexes = old_record
            .get("enable_indexes")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 1. Copy physical stores + swap the live registration
        //    (configs / tables OnceCell / token_names reverse-index).
        let repo = db
            .get_repo(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        // F-68 (#895) cluster D / task #124: timestamped before/after around
        // `rename_table_stores` — this is the call that force-drains the
        // source table's MVCC overlay (`drain_to_history`) and copies the
        // physical stores; if the hang is inside the engine's rename path
        // rather than in commit-pipeline locking, this pinpoints it.
        let stores_started = std::time::Instant::now();
        log::debug!("rename_table_as: calling rename_table_stores from={from} to={to}");
        let existed = repo.rename_table_stores(from, to).await?;
        log::debug!(
            "rename_table_as: rename_table_stores returned after {:?}",
            stores_started.elapsed()
        );
        if !existed {
            return Err(DbError::NotFound(format!(
                "Table '{}/{}/{}' not found",
                db_name, repo_name, from
            )));
        }

        // 2. Re-key the catalogue row. Write the new row first so a crash
        //    between the two leaves the new table resolvable on reboot;
        //    a stale `(db, repo, from)` row resurrects nothing because
        //    the live registration under `from` is already gone.
        self.system_store
            .save_table(db_name, repo_name, to, enable_indexes, &existing_meta)
            .await?;
        self.system_store
            .remove_table(db_name, repo_name, from)
            .await?;

        // F-68 (#895) cluster D / task #124: exit timestamp, paired with the
        // "enter" log above.
        log::debug!(
            "rename_table_as: exit db={db_name} repo={repo_name} from={from} to={to} \
             elapsed={:?}",
            rename_started.elapsed()
        );

        Ok(())
    }

    /// Direct table access shortcut.
    ///
    /// The returned `TableManager` has the global `ValidatorRegistry`
    /// injected (S3) so the write path can resolve validator bindings.
    ///
    /// S5c-X PR8b: a quarantined (`resync_required`) repo refuses table
    /// acquisition with `DbError::ResyncRequired`. The gate is at ACQUISITION:
    /// a `TableManager` handle already obtained before the quarantine cannot
    /// be revoked retroactively (same model as cursor create — per-read
    /// revocation of a raw handle is not possible without a wrapper type).
    pub async fn get_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<TableManager> {
        if let Some(reason) = self.quarantined_read_reason(db_name, repo_name) {
            return Err(DbError::resync_required(db_name, repo_name, reason));
        }
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        let mut table = db.get_table(repo_name, table_name).await?;
        table.set_validator_registry(self.validators().clone());
        Ok(table)
    }

    pub(crate) async fn get_table_admitted(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        permit: &shamir_engine::repo::RepoWritePermit,
    ) -> DbResult<TableManager> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        let mut table = db
            .get_repo(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?
            .get_table_admitted(table_name, permit)
            .await?;
        table.set_validator_registry(self.validators().clone());
        Ok(table)
    }
}

impl ShamirDb {
    pub(crate) async fn reject_drop_if_fk_reference(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<()> {
        let snapshot = self.table_catalogue_snapshot(db_name, repo_name).await?;
        Self::check_fk_snapshot(&snapshot, table_name)
    }

    pub(crate) async fn table_catalogue_snapshot(
        &self,
        db_name: &str,
        repo_name: &str,
    ) -> DbResult<shamir_collections::TFxMap<String, QueryValue>> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{db_name}' not found")))?;
        db.list_tables(repo_name)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let records = self.system_store.load_tables().await?;
        let mut snapshot = shamir_collections::TFxMap::default();
        for record in records {
            let QueryValue::Map(map) = record else {
                return Err(DbError::Internal(
                    "malformed table catalogue record".to_string(),
                ));
            };
            let (Some(db), Some(repo), Some(table)) = (
                map.get("db_name").and_then(QueryValue::as_str),
                map.get("repo_name").and_then(QueryValue::as_str),
                map.get("table_name").and_then(QueryValue::as_str),
            ) else {
                return Err(DbError::Internal(
                    "malformed table catalogue identity".to_string(),
                ));
            };
            if db == db_name
                && repo == repo_name
                && snapshot
                    .insert(table.to_string(), QueryValue::Map(map.clone()))
                    .is_some()
            {
                return Err(DbError::Internal(format!(
                    "duplicate table catalogue record '{db}/{repo}/{table}'"
                )));
            }
        }
        Ok(snapshot)
    }

    pub(crate) fn check_fk_snapshot(
        snapshot: &shamir_collections::TFxMap<String, QueryValue>,
        target_table: &str,
    ) -> DbResult<()> {
        for (name, record) in snapshot {
            if name == target_table {
                continue;
            }
            let Some(schema) = record.get(SCHEMA_FIELD) else {
                continue;
            };
            if matches!(schema, QueryValue::Null) {
                continue;
            }
            let QueryValue::List(rules) = schema else {
                return Err(DbError::Validation(format!(
                    "unreadable schema in table '{name}'"
                )));
            };
            for rule in rules {
                let QueryValue::Map(rule) = rule else {
                    return Err(DbError::Validation(format!(
                        "malformed schema rule in table '{name}'"
                    )));
                };
                let Some(fk) = rule.get("foreign_key") else {
                    continue;
                };
                let QueryValue::Map(fk) = fk else {
                    return Err(DbError::Validation(format!(
                        "malformed foreign key schema in table '{name}'"
                    )));
                };
                let Some(ref_table) = fk.get("ref_table") else {
                    return Err(DbError::Validation(format!(
                        "missing foreign key ref_table in table '{name}'"
                    )));
                };
                let Some(ref_table) = ref_table.as_str() else {
                    return Err(DbError::Validation(format!(
                        "malformed foreign key reference in table '{name}'"
                    )));
                };
                if ref_table == target_table {
                    return Err(DbError::Validation(format!(
                        "table '{target_table}' is referenced by a foreign key in '{name}'"
                    )));
                }
            }
        }
        Ok(())
    }
}
