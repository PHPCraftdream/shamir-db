use crate::access::{Actor, ResourceMeta};
use crate::engine::table::{TableConfig, TableManager};
use crate::shamir_db::shamir_db::schema_management::SCHEMA_FIELD;
use crate::types::value::QueryValue;
use crate::{DbError, DbResult};

use super::ShamirDb;

impl ShamirDb {
    /// Create a table in a repo and persist it to the table catalogue so it
    /// survives a restart (I.2).
    ///
    /// Delegates to [`DbInstance::create_table`] (the same path that lazily
    /// instantiates the `TableManager` on first access) and then records the
    /// table in the system store. Persistence is best-effort: a failed
    /// catalogue write is logged, not propagated, mirroring `add_repo`.
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
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;

        let mut config = TableConfig::new(table_name);
        if enable_indexes {
            config = config.with_indexes();
        }
        db.get_repo(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?
            .add_table(config);

        if let Err(e) = self
            .system_store
            .save_table(
                db_name,
                repo_name,
                table_name,
                enable_indexes,
                &ResourceMeta::owned_enforced(actor),
            )
            .await
        {
            log::warn!(
                "shamir_db::add_table: failed to persist table catalogue '{}/{}/{}': {}",
                db_name,
                repo_name,
                table_name,
                e
            );
        }

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
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        let removed = db.drop_table(repo_name, table_name)?;

        // Always clear the catalogue entry (idempotent), even if the
        // in-memory table was already gone, so a stale record can't
        // resurrect the table on the next open.
        if let Err(e) = self
            .system_store
            .remove_table(db_name, repo_name, table_name)
            .await
        {
            log::warn!(
                "shamir_db::drop_table: failed to remove table catalogue '{}/{}/{}': {}",
                db_name,
                repo_name,
                table_name,
                e
            );
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
        self.drop_table(db_name, repo_name, table_name).await
    }

    /// Rename a table. Convenience wrapper around [`rename_table_as`]
    /// using [`Actor::System`].
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
        let table_names = db.list_tables(repo_name).unwrap_or_default();
        for name in &table_names {
            if name == from {
                continue;
            }
            let rec = match self
                .system_store
                .load_table_record(db_name, repo_name, name)
                .await
            {
                Ok(Some(r)) => r,
                _ => continue,
            };
            let rules = match rec.get(SCHEMA_FIELD) {
                Some(QueryValue::List(rules)) => rules,
                _ => continue,
            };
            for rule in rules {
                let refs_from = rule
                    .get("foreign_key")
                    .and_then(|fk| fk.get("ref_table"))
                    .and_then(|v| v.as_str())
                    .is_some_and(|rt| rt == from);
                if refs_from {
                    return Err(DbError::Validation(format!(
                        "cannot rename table '{}': still referenced by a foreign key in '{}'",
                        from, name
                    )));
                }
            }
        }

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
        let existed = repo.rename_table_stores(from, to).await?;
        debug_assert!(
            existed,
            "rename_table_stores returned false despite has_table guard"
        );

        // 2. Re-key the catalogue row. Write the new row first so a crash
        //    between the two leaves the new table resolvable on reboot;
        //    a stale `(db, repo, from)` row resurrects nothing because
        //    the live registration under `from` is already gone.
        if let Err(e) = self
            .system_store
            .save_table(db_name, repo_name, to, enable_indexes, &existing_meta)
            .await
        {
            log::warn!(
                "shamir_db::rename_table: failed to persist new catalogue row \
                 '{}/{}/{}': {}",
                db_name,
                repo_name,
                to,
                e
            );
        }
        if let Err(e) = self
            .system_store
            .remove_table(db_name, repo_name, from)
            .await
        {
            log::warn!(
                "shamir_db::rename_table: failed to remove old catalogue row \
                 '{}/{}/{}': {}",
                db_name,
                repo_name,
                from,
                e
            );
        }

        Ok(())
    }

    /// Direct table access shortcut.
    ///
    /// The returned `TableManager` has the global `ValidatorRegistry`
    /// injected (S3) so the write path can resolve validator bindings.
    pub async fn get_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<TableManager> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        let mut table = db.get_table(repo_name, table_name).await?;
        table.set_validator_registry(self.validators().clone());
        Ok(table)
    }
}
