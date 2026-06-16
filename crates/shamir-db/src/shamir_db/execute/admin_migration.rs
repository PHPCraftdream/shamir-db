//! Admin handlers: StartMigration, CommitMigration, RollbackMigration, MigrationStatus.

use std::sync::Arc;

use serde_json::json;

use crate::access::{Action, ResourcePath};
use crate::engine::migration::{MigrationCoordinator, MigrationShadowLog, MigrationState};
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

impl ShamirAdminExecutor {
    pub(super) async fn handle_start_migration(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::StartMigration(op) = batch_op else {
            unreachable!("handle_start_migration called with non-StartMigration op");
        };

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

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.start_migration.clone(),
                ),
                Action::Manage,
            )
            .await
            .map_err(err_access)?;
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;

        let table_name = &op.start_migration;
        // Atomic counter + ns timestamp + random suffix — collision-free
        // even under concurrent start_migration on same table within
        // the same nanosecond.
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let rand_suffix: u32 = rand::random();
        let migration_id = format!("mig_{}_{}_{:08x}", table_name, now_ns, rand_suffix);

        // Reject if any active migration already targets this table.
        let table_already_migrating = self
            .shamir
            .active_migrations()
            .iter()
            .any(|e| e.value().targets_table(&op.repo, table_name));
        if table_already_migrating {
            return Err(err(format!(
                "migration already in progress for table '{}/{}'",
                op.repo, table_name
            )));
        }

        // Get source table's data_store + info_store
        let src_table = db
            .get_table(&op.repo, table_name)
            .await
            .map_err(|e| err(e.to_string()))?;
        let src_data = Arc::clone(src_table.table().data_store());
        let info_store = Arc::clone(src_table.info_store());

        // Resolve dst engine factory
        let dst_factory = match op.dst_engine.as_str() {
            "in_memory" => BoxRepoFactory::in_memory(),
            engine => {
                return Err(err(format!(
                    "Migration dst_engine '{}' not yet supported. Supported: in_memory",
                    engine
                )))
            }
        };
        let dst_repo_name = &op.dst_repo;
        let dst_config =
            RepoConfig::new(dst_repo_name, dst_factory).add_table(TableConfig::new(table_name));
        db.add_repo(dst_config)
            .await
            .map_err(|e| err(e.to_string()))?;

        // From here on, any error must clean up dst repo
        // (rollback-on-failure). We pull dst_data + run snapshot/drain;
        // a `?` aborts the whole batch, but the dst repo would leak.
        // So unwind explicitly on failure.
        let run = async {
            let dst_table = db.get_table(dst_repo_name, table_name).await?;
            let dst_data = Arc::clone(dst_table.table().data_store());

            // Step 1: carry src's repo-interner `(id → name)` mappings
            // into dst's repo interner BEFORE any data lands. The
            // migration coordinator copies raw `data_store` bytes, which
            // embed src's `InternerKey(u64)` ids for field names; under
            // per-repo interners dst starts with its own empty interner,
            // so without this step those ids miss on dst and the index2
            // backfill (`bulk_populate_index2`) reads empty field values
            // → the dst index is built empty. `replicate_interner_from`
            // replays each (name, id) via `touch_with_id` preserving the
            // SAME ids, so the copied bytes decode unchanged (no
            // re-encode). Must run before `replicate_index2_descriptors_from`
            // (which re-interns index path segments through dst's interner)
            // and before the snapshot/drain that populates dst's data_store.
            dst_table.replicate_interner_from(&src_table).await?;

            // Step 2: replicate index2 descriptors (FTS / Functional
            // / Vector) from src → dst. Creates empty backends on
            // dst so that bulk_populate_index2 (called later in
            // CommitMigration) can fill them. Must happen before
            // any data lands on dst.
            dst_table
                .replicate_index2_descriptors_from(&src_table)
                .await?;

            let shadow = Arc::new(MigrationShadowLog::new(migration_id.clone(), info_store));
            let state = MigrationState::new(
                migration_id.clone(),
                table_name.to_string(),
                op.repo.clone(),
                op.dst_repo.clone(),
                op.dst_engine.clone(),
                op.dst_path.clone(),
            );
            // Q1: capture the source's MvccStore handle so
            // run_snapshot reads through the log seam
            // (current_stream) instead of the raw data_store.
            let src_mvcc = src_table.mvcc_store();
            let coord = Arc::new(MigrationCoordinator::new(
                state, shadow, src_data, dst_data, src_mvcc,
            ));

            coord.run_snapshot().await?;
            coord.drain_until_caught_up(0).await?;
            coord.mark_cutover_ready().await?;
            Ok::<_, shamir_storage::error::DbError>(coord)
        }
        .await;

        let coord = match run {
            Ok(c) => c,
            Err(e) => {
                // Roll back: remove the orphan dst repo.
                db.remove_repo(dst_repo_name).await;
                return Err(err(e.to_string()));
            }
        };

        self.shamir
            .active_migrations()
            .insert(migration_id.clone(), coord);

        Ok(admin_result(json!({
            "migration_id": migration_id,
            "phase": "cutover_ready",
            "table": table_name,
            "src_repo": op.repo,
            "dst_repo": op.dst_repo,
            "dst_engine": op.dst_engine,
        })))
    }

    pub(super) async fn handle_commit_migration(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::CommitMigration(op) = batch_op else {
            unreachable!("handle_commit_migration called with non-CommitMigration op");
        };

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

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::database(self.db_name.clone()),
                Action::Manage,
            )
            .await
            .map_err(err_access)?;
        let coord = self
            .shamir
            .active_migrations()
            .get(&op.commit_migration)
            .ok_or_else(|| {
                err_code(
                    "not_found",
                    format!("migration '{}' not found", op.commit_migration),
                )
            })?
            .clone();
        let tail = coord
            .final_drain_and_commit()
            .await
            .map_err(|e| err(e.to_string()))?;
        let (src_count, dst_count) = coord
            .verify_record_count()
            .await
            .map_err(|e| err(e.to_string()))?;
        let state = coord.state().await;

        // Bulk-populate index2 backends on dst.
        //
        // Order of operations:
        //   1. replicate_index2_descriptors_from (StartMigration) — empty backends
        //   2. run_snapshot + drain_until_caught_up — data_store only
        //   3. final_drain_and_commit — drains remaining shadow log
        //      entries into dst data_store (NO index2 hooks)
        //   4. bulk_populate_index2 ← we are here — streams ALL
        //      dst data_store records into on_batch_insert, creating
        //      postings in info_store + in-memory state.
        //
        // After this point the migration is committed. New writes
        // go through `insert()` → `index2_on_insert` automatically.
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let dst_table = db
            .get_table(&state.dst_repo, &state.table_name)
            .await
            .map_err(|e| err(e.to_string()))?;

        // C2 (collapse-main bridge): the migration coordinator copies
        // records straight into the dst `data_store` (raw, bypassing the
        // version log). Reads now resolve from the log, so seed the dst
        // log from `data_store` here so migrated records become current
        // versions in the log (visible to reads AND to the log-backed
        // `bulk_populate_index2` seam). The coordinator will write the log
        // directly in a later slice (C5); this bridges it at cutover.
        dst_table
            .seed_log_from_data_store()
            .await
            .map_err(|e| err(e.to_string()))?;

        dst_table
            .bulk_populate_index2()
            .await
            .map_err(|e| err(e.to_string()))?;

        // Remove from active map — committed migrations are
        // terminal, no further state changes possible. Status
        // queries on a committed id will now return 404, which
        // is the correct semantics (migration is done; query the
        // dst table directly).
        self.shamir.active_migrations().remove(&op.commit_migration);

        Ok(admin_result(json!({
            "migration_id": op.commit_migration,
            "phase": "committed",
            "tail_drained": tail,
            "src_records": src_count,
            "dst_records": dst_count,
            "records_copied": state.records_copied,
        })))
    }

    pub(super) async fn handle_rollback_migration(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::RollbackMigration(op) = batch_op else {
            unreachable!("handle_rollback_migration called with non-RollbackMigration op");
        };

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

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::database(self.db_name.clone()),
                Action::Manage,
            )
            .await
            .map_err(err_access)?;
        let coord = self
            .shamir
            .active_migrations()
            .get(&op.rollback_migration)
            .ok_or_else(|| {
                err_code(
                    "not_found",
                    format!("migration '{}' not found", op.rollback_migration),
                )
            })?
            .clone();
        coord.rollback().await.map_err(|e| err(e.to_string()))?;
        self.shamir
            .active_migrations()
            .remove(&op.rollback_migration);

        Ok(admin_result(json!({
            "migration_id": op.rollback_migration,
            "phase": "rolled_back",
        })))
    }

    pub(super) async fn handle_migration_status(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::MigrationStatus(op) = batch_op else {
            unreachable!("handle_migration_status called with non-MigrationStatus op");
        };

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
                &ResourcePath::database(self.db_name.clone()),
                Action::Read,
            )
            .await
            .map_err(err_access)?;
        let coord = self
            .shamir
            .active_migrations()
            .get(&op.migration_status)
            .ok_or_else(|| {
                err_code(
                    "not_found",
                    format!("migration '{}' not found", op.migration_status),
                )
            })?
            .clone();
        let state = coord.state().await;
        let shadow_lag = coord.shadow_lag().await;

        Ok(admin_result(json!({
            "migration_id": state.id,
            "phase": state.phase.to_string(),
            "table": state.table_name,
            "src_repo": state.src_repo,
            "dst_repo": state.dst_repo,
            "dst_engine": state.dst_engine,
            "snapshot_lsn": state.snapshot_lsn,
            "last_lsn_applied": state.last_lsn_applied,
            "records_copied": state.records_copied,
            "shadow_lag": shadow_lag,
        })))
    }
}
