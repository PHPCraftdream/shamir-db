use crate::access::{Actor, ResourceMeta};
use crate::engine::table::{TableConfig, TableManager};
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
                &ResourceMeta::owned_by(actor),
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
