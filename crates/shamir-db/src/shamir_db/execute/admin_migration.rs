//! Admin handlers: StartMigration, CommitMigration, RollbackMigration, MigrationStatus.

use std::sync::Arc;

use crate::access::{Action, ResourcePath};
use crate::engine::migration::{MigrationPhase, MigrationState};
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoWriteDomain;
use crate::engine::table::TableConfig;
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::types::value::QueryValue;
use shamir_types::mpack;

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

        // Experimental-feature gate (#795 / F-5). The online migration API
        // is disabled by default; a disabled feature is not even an authz
        // question, so check the flag FIRST — before the authorize call —
        // and reject with a structured error that names the opt-in method
        // and the unfixed correctness gaps. This is the only entry point
        // gated (Commit/Rollback/Status become unreachable for any caller
        // who never opted in, so they don't re-check).
        if !self.shamir.experimental_migration_enabled() {
            return Err(err_code(
                "experimental_feature_disabled",
                "StartMigration is experimental and disabled by default. \
                 The source is frozen for writes during migration; rejected \
                 writes return table_migrating. Only the in_memory dst_engine \
                 is supported, so destination data does not survive restart. \
                 Coordinator state is persisted in the source info store, but \
                 the active coordinator and freeze guard are process-local; \
                 persisted state alone does not provide crash-safe resumption. \
                 Replication followers are unsupported: replicated apply \
                 bypasses the freeze. To opt in for internal testing, call \
                 ShamirDb::enable_experimental_migration_api()."
                    .to_string(),
            ));
        }

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
        let dst_repo_name = &op.dst_repo;
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
        // Resolve the registered source and reject a destination name collision
        // before constructing its private instance (including factory recovery).
        let src_repo = db
            .get_repo(&op.repo)
            .ok_or_else(|| err(format!("Repository '{}' not found", op.repo)))?;
        if db.has_repo(dst_repo_name) {
            return Err(err(format!(
                "Repository '{}' already exists",
                dst_repo_name
            )));
        }
        let dst_repo = crate::engine::repo::RepoInstance::from_factory(
            dst_repo_name.clone(),
            dst_factory,
            vec![TableConfig::new(table_name)],
        )
        .await
        .map_err(|e| err(e.to_string()))?;

        // Both permits cover table initialization, snapshot/drain and publication.
        let run = async {
            let (src_permit, dst_permit) =
                RepoWriteDomain::admit_pair(src_repo.write_domain(), dst_repo.write_domain())
                    .await
                    .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            let src_table = src_repo.get_table_admitted(table_name, &src_permit).await?;
            let dst_table = dst_repo.get_table_admitted(table_name, &dst_permit).await?;

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
            dst_table
                .replicate_interner_from_admitted(&src_table, &dst_permit)
                .await?;

            // Step 2: replicate index2 descriptors (FTS / Functional
            // / Vector) from src → dst. Creates empty backends on
            // dst so that bulk_populate_index2 (called later in
            // CommitMigration) can fill them. Must happen before
            // any data lands on dst.
            dst_table
                .replicate_index2_descriptors_from_admitted(&src_table, &dst_permit)
                .await?;

            // S5c-X: the shadow log, raw data-store handles, MVCC log seam
            // and source migration freeze are assembled engine-side by the
            // admitted coordinator factory (the facade only supplies state
            // and both permits — no raw store handles cross the boundary).
            // Q1: the factory captures the source's MvccStore handle so
            // run_snapshot reads through the log seam (current_stream)
            // instead of the raw data_store.
            let state = MigrationState::new(
                migration_id.clone(),
                table_name.to_string(),
                op.repo.clone(),
                op.dst_repo.clone(),
                op.dst_engine.clone(),
                op.dst_path.clone(),
            );
            let coord = src_table
                .migration_coordinator_admitted(
                    state,
                    migration_id.clone(),
                    &dst_table,
                    &src_permit,
                    &dst_permit,
                )
                .await
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;

            coord.run_snapshot().await?;
            let caught_up = coord.drain_until_caught_up(0).await?;
            if caught_up.residual_lag != 0 {
                return Err(shamir_storage::error::DbError::Conflict(
                    "frozen migration has residual shadow lag; start again".to_string(),
                ));
            }
            coord.mark_cutover_ready().await?;
            coord.persist_state().await?;
            Ok::<_, shamir_storage::error::DbError>((coord, src_permit, dst_permit))
        }
        .await;

        let (coord, src_permit, dst_permit) = match run {
            Ok(result) => result,
            Err(e) => {
                // The private destination is dropped on failure; it was never published.
                return Err(err_code(
                    e.code().unwrap_or("migration_start_error"),
                    e.to_string(),
                ));
            }
        };
        db.publish_repo_admitted(dst_repo, &dst_permit)
            .map_err(|e| err(e.to_string()))?;
        drop(src_permit);
        drop(dst_permit);

        let phase = coord.phase().await.to_string();
        self.shamir
            .active_migrations()
            .insert(migration_id.clone(), coord);

        Ok(admin_result(mpack!({
            "migration_id": @(QueryValue::Str(migration_id)),
            "phase": @(QueryValue::Str(phase)),
            "table": @(QueryValue::Str(table_name.clone())),
            "src_repo": @(QueryValue::Str(op.repo.clone())),
            "dst_repo": @(QueryValue::Str(op.dst_repo.clone())),
            "dst_engine": @(QueryValue::Str(op.dst_engine.clone())),
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
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let active = self
            .shamir
            .active_migrations()
            .get(&op.commit_migration)
            .map(|entry| Arc::clone(entry.value()));
        let coord = if let Some(coord) = active {
            coord
        } else {
            // Retry after restart: state keys live in the source table's info store.
            let mut recovered = None;
            'repos: for repo in db.list_repos() {
                for table in db.list_tables(&repo).map_err(|e| err(e.to_string()))? {
                    let src = db
                        .get_table(&repo, &table)
                        .await
                        .map_err(|e| err(e.to_string()))?;
                    if let Some(state) = src
                        .info_store_read_view()
                        .load_migration_state(&op.commit_migration)
                        .await
                        .map_err(|e| err(e.to_string()))?
                    {
                        // A process-local freeze cannot be reconstructed from
                        // metadata. Refreezing now would miss intervening writes.
                        if state.phase != MigrationPhase::Committed {
                            return Err(err_code(
                                "migration_restart_required",
                                "Uncommitted migration has no source freeze; cannot resume its snapshot"
                                    .to_string(),
                            ));
                        }
                        let src = db
                            .get_table(&state.src_repo, &state.table_name)
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        let dst = db
                            .get_table(&state.dst_repo, &state.table_name)
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        // S5c-X: rebuild engine-side under repo admission —
                        // the admitted recovery factory rebinds the shadow
                        // log and the raw source/destination data-store
                        // handles without exposing them to the facade.
                        let src_repo_instance = db.get_repo(&state.src_repo).ok_or_else(|| {
                            err(format!("Repository '{}' not found", state.src_repo))
                        })?;
                        let dst_repo_instance = db.get_repo(&state.dst_repo).ok_or_else(|| {
                            err(format!("Repository '{}' not found", state.dst_repo))
                        })?;
                        let (rec_src_permit, rec_dst_permit) = RepoWriteDomain::admit_pair(
                            src_repo_instance.write_domain(),
                            dst_repo_instance.write_domain(),
                        )
                        .await
                        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
                        recovered = Some(
                            src.migration_coordinator_recovered_admitted(
                                state,
                                &dst,
                                &rec_src_permit,
                                &rec_dst_permit,
                            )
                            .await
                            .map_err(|e| err(e.to_string()))?,
                        );
                        break 'repos;
                    }
                }
            }
            let recovered = recovered.ok_or_else(|| {
                err_code(
                    "not_found",
                    format!("migration '{}' not found", op.commit_migration),
                )
            })?;
            // Concurrent recovery attempts must share the same finalisation lock.
            Arc::clone(
                self.shamir
                    .active_migrations()
                    .entry(op.commit_migration.clone())
                    .or_insert(recovered)
                    .value(),
            )
        };
        // Resolve coordinator state before admission; then acquire repository
        // permits before finalisation, consistently with rollback.
        let initial_state = coord.state().await;
        let src_repo = db
            .get_repo(&initial_state.src_repo)
            .ok_or_else(|| err(format!("Repository '{}' not found", initial_state.src_repo)))?;
        let dst_repo = db
            .get_repo(&initial_state.dst_repo)
            .ok_or_else(|| err(format!("Repository '{}' not found", initial_state.dst_repo)))?;
        let (src_permit, dst_permit) =
            RepoWriteDomain::admit_pair(src_repo.write_domain(), dst_repo.write_domain())
                .await
                .map_err(|e| err(e.to_string()))?;
        let _finalisation = coord.lock_finalisation().await;
        let locked_state = coord.state().await;
        if locked_state.id != initial_state.id
            || locked_state.src_repo != initial_state.src_repo
            || locked_state.dst_repo != initial_state.dst_repo
            || locked_state.table_name != initial_state.table_name
        {
            return Err(err_code(
                "migration_state_changed",
                "Migration target changed while acquiring admission".to_string(),
            ));
        }
        let source_freeze = coord.take_migration_freeze();

        if source_freeze.is_none() && coord.phase().await != MigrationPhase::Committed {
            return Err(err_code(
                "migration_restart_required",
                "Uncommitted migration has no source freeze; cannot resume its snapshot"
                    .to_string(),
            ));
        }
        if coord.phase().await == MigrationPhase::Draining {
            let result = coord
                .drain_until_caught_up(0)
                .await
                .map_err(|e| err(e.to_string()))?;
            if result.residual_lag != 0 {
                coord
                    .persist_state()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                return Err(err_code(
                    "migration_residual_lag",
                    format!(
                        "{} shadow entries remain; retry CommitMigration",
                        result.residual_lag,
                    ),
                ));
            }
            coord.mark_cutover_ready().await.map_err(|e| {
                err_code(e.code().unwrap_or("migration_cutover_error"), e.to_string())
            })?;
        }
        let cutover_state = coord.state().await;
        if cutover_state.id != initial_state.id
            || cutover_state.src_repo != initial_state.src_repo
            || cutover_state.dst_repo != initial_state.dst_repo
            || cutover_state.table_name != initial_state.table_name
        {
            return Err(err_code(
                "migration_state_changed",
                "Migration target changed while acquiring admission".to_string(),
            ));
        }
        let src_table = src_repo
            .get_table_admitted(&initial_state.table_name, &src_permit)
            .await
            .map_err(|e| err(e.to_string()))?;
        let dst_table = dst_repo
            .get_table_admitted(&initial_state.table_name, &dst_permit)
            .await
            .map_err(|e| err(e.to_string()))?;
        let (src_barrier, src_write_lock) = src_table
            .begin_write_barrier_admitted(
                crate::engine::index::write_barrier_flags::INDEX2_CREATE,
                &src_permit,
            )
            .await;

        let tail = coord
            .final_drain_and_commit()
            .await
            .map_err(|e| err(e.to_string()))?;
        drop(src_write_lock);
        drop(src_barrier);
        let (src_count, dst_count) = if source_freeze.is_some() {
            coord.verify_record_count().await
        } else {
            // Committed retries operate on the fixed destination, never on
            // source writes admitted after a prior exit released the freeze.
            coord.verify_committed_record_count().await
        }
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
        let dst_table = dst_repo
            .get_table_admitted(&state.table_name, &dst_permit)
            .await
            .map_err(|e| err(e.to_string()))?;
        let (barrier, write_lock) = dst_table
            .begin_write_barrier_admitted(
                crate::engine::index::write_barrier_flags::INDEX2_CREATE,
                &dst_permit,
            )
            .await;
        let progress = coord
            .finalisation_progress()
            .await
            .map_err(|e| err(e.to_string()))?;
        if progress < 1 {
            // Seed only absent current versions. A crash between a row write and
            // the stage marker must not allocate a second version for that row.
            let seeded = dst_table
                .seed_current_versions_admitted(&dst_permit)
                .await
                .map_err(|e| err(e.to_string()))?;
            if !seeded {
                dst_table
                    .seed_log_from_data_store_admitted(&dst_permit)
                    .await
                    .map_err(|e| err(e.to_string()))?;
            }
            coord
                .complete_finalisation_step(1)
                .await
                .map_err(|e| err(e.to_string()))?;
        }
        if progress < 2 {
            // plan_insert includes non-idempotent BM25 bumps. Reset derived
            // in-memory backends on EVERY incomplete attempt, not just restart.
            // Posting writes are overwrite-by-key; the migrated data is fixed.
            // Construct fresh backends; FtsRankedBackend::rebuild adds counts
            // and would NOT reset an already populated instance. Constructors
            // do not restore persisted state, so counts start at zero even
            // when table-open recovery populated the previous instance.
            let resolver =
                shamir_funclib::scalar_resolver::ScalarResolver::new(Arc::clone(db.scalars()));
            dst_table
                .reset_and_populate_index2_admitted(&dst_permit, &resolver)
                .await
                .map_err(|e| err(e.to_string()))?;
            dst_table
                .info_store_read_view()
                .flush()
                .await
                .map_err(|e| err(e.to_string()))?;
            coord
                .complete_finalisation_step(2)
                .await
                .map_err(|e| err(e.to_string()))?;
        }
        drop(write_lock);
        drop(barrier);
        drop(src_permit);
        drop(dst_permit);

        // Keep the durable completed marker as an idempotent retry receipt;
        // only the transient active-map entry is cleaned up.
        self.shamir.active_migrations().remove(&op.commit_migration);

        Ok(admin_result(mpack!({
            "migration_id": @(QueryValue::Str(op.commit_migration.clone())),
            "phase": "committed",
            "tail_drained": @(QueryValue::Int(tail as i64)),
            "src_records": @(QueryValue::Int(src_count as i64)),
            "dst_records": @(QueryValue::Int(dst_count as i64)),
            "records_copied": @(QueryValue::Int(state.records_copied as i64)),
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
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let state = coord.state().await;
        let src_repo = db
            .get_repo(&state.src_repo)
            .ok_or_else(|| err(format!("Repository '{}' not found", state.src_repo)))?;
        let dst_repo = db.get_repo(&state.dst_repo);
        let src_permit;
        let dst_permit: Option<crate::engine::repo::RepoWritePermit>;
        if let Some(dst_repo) = dst_repo {
            let (src, dst) =
                RepoWriteDomain::admit_pair(src_repo.write_domain(), dst_repo.write_domain())
                    .await
                    .map_err(|e| err(e.to_string()))?;
            src_permit = src;
            let _src_table = src_repo
                .get_table_admitted(&state.table_name, &src_permit)
                .await
                .map_err(|e| err(e.to_string()))?;
            let _dst_table = dst_repo
                .get_table_admitted(&state.table_name, &dst)
                .await
                .map_err(|e| err(e.to_string()))?;
            dst_permit = Some(dst);
        } else {
            src_permit = src_repo
                .write_domain()
                .admit()
                .await
                .map_err(|e| err(e.to_string()))?;
            let _src_table = src_repo
                .get_table_admitted(&state.table_name, &src_permit)
                .await
                .map_err(|e| err(e.to_string()))?;
            dst_permit = None;
        }
        // Commit and rollback use the same repository-permit -> finalisation-lock order.
        let _finalisation = coord.lock_finalisation().await;
        let locked_state = coord.state().await;
        if locked_state.id != state.id
            || locked_state.src_repo != state.src_repo
            || locked_state.dst_repo != state.dst_repo
            || locked_state.table_name != state.table_name
        {
            return Err(err_code(
                "migration_state_changed",
                "Migration target changed while acquiring admission".to_string(),
            ));
        }
        let _source_freeze = coord.take_migration_freeze();
        coord.rollback().await.map_err(|e| err(e.to_string()))?;
        drop(dst_permit);
        drop(src_permit);
        self.shamir
            .active_migrations()
            .remove(&op.rollback_migration);

        Ok(admin_result(mpack!({
            "migration_id": @(QueryValue::Str(op.rollback_migration.clone())),
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

        Ok(admin_result(mpack!({
            "migration_id": @(QueryValue::Str(state.id.clone())),
            "phase": @(QueryValue::Str(state.phase.to_string())),
            "table": @(QueryValue::Str(state.table_name.clone())),
            "src_repo": @(QueryValue::Str(state.src_repo.clone())),
            "dst_repo": @(QueryValue::Str(state.dst_repo.clone())),
            "dst_engine": @(QueryValue::Str(state.dst_engine.clone())),
            "snapshot_lsn": @(QueryValue::Int(state.snapshot_lsn as i64)),
            "last_lsn_applied": @(QueryValue::Int(state.last_lsn_applied as i64)),
            "records_copied": @(QueryValue::Int(state.records_copied as i64)),
            "shadow_lag": @(QueryValue::Int(shadow_lag as i64)),
        })))
    }
}
