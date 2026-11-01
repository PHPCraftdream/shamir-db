//! Identity-migration protocol (design P34-12 sections (b), (c), (e) S2) —
//! the automatic, blocking, open-time base-family migration.
//!
//! `#[path]`-wired impl-block file (a CHILD of the `table_manager` module,
//! declared at the top of `table_manager.rs`), same pattern as
//! `write_domain_barrier.rs`.
//!
//! Base-family protocol (replacing the S9b `lfv` gate), run inside
//! `TableManager::create` before the table is returned:
//!
//! 1. One atomic `info_store.transact([Remove lfv, Remove idv])`. From here
//!    every old binary will rebuild (lfv absent) and every new binary will
//!    migrate (idv absent).
//! 2. For each regular/unique definition, sweep both namespaces in place and
//!    backfill through the canonical creation path without removing definitions.
//! 3. `set idv = 2` (u32 LE under `system:_m.idx.idv`). Commit point.
//!
//! Crash safety: a crash after 1 or during 2 leaves `idv` absent, so the
//! next open repeats 2 from scratch (the drops are idempotent and cover
//! both namespaces). A crash after 3 is complete. An old binary between 1
//! and 3 rebuilds in v3 and stamps `lfv`, which forces the next new open to
//! migrate again (the need rule: `!(idv == 2 && lfv absent)`).
//!
//! The re-run path (`rerun_identity_migration_admitted`) is called by the
//! post-replay recovery hook for tables that migrated THIS open AND
//! received a replayed pre-upgrade `IndexPut`/`IndexDel`: it forces steps
//! 1-3 again regardless of the stored markers, so literal v1/v3 keys
//! written by a crashed old binary are folded back into the v2 index by a
//! full rebuild from the (already replayed) data store.

use std::sync::atomic::Ordering;

use futures::StreamExt;
use shamir_storage::error::DbResult;

use super::TableManager;
use crate::repo::RepoWritePermit;

impl TableManager {
    /// Ensure the base index family is on identity v2 for THIS table.
    /// Returns `true` when the migration ran (either because the stored
    /// markers said `need`, or because `force` is set by the post-replay
    /// re-run).
    ///
    /// Failure gate (fail-closed): `idv > 2` is a typed open error — the
    /// table was written by a newer binary.
    pub(crate) async fn ensure_base_identity_current(
        &self,
        permit: &RepoWritePermit,
    ) -> DbResult<bool> {
        // Forward gate, checked ALWAYS (not only when need is true).
        shamir_index::persistence::check_identity_version(&self.info_store).await?;

        let need = shamir_index::persistence::identity_migration_needed(&self.info_store).await?;
        if !need {
            return Ok(false);
        }
        self.run_base_identity_migration(permit, false).await?;
        Ok(true)
    }

    /// Post-replay re-run entry (design (c), "Pre-upgrade WAL entries"):
    /// force steps 1-3 regardless of the stored markers. Only meaningful
    /// for a table that migrated THIS open; the caller checks the flag.
    pub(crate) async fn rerun_identity_migration_admitted(
        &self,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        shamir_index::persistence::check_identity_version(&self.info_store).await?;
        self.run_base_identity_migration(permit, true).await
    }

    pub(crate) async fn rerun_index2_identity_migration(&self) -> DbResult<()> {
        let Some((_, mut persisted)) =
            crate::index2::persistence::load_index2_metadata(&self.info_store).await?
        else {
            return Ok(());
        };
        let ids: Vec<_> = persisted
            .descriptors
            .iter()
            .filter(|desc| {
                matches!(
                    desc.kind,
                    crate::index2::kind::IndexKind::Fts { .. }
                        | crate::index2::kind::IndexKind::Functional { .. }
                )
            })
            .map(|desc| desc.id)
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        for desc in &mut persisted.descriptors {
            if ids.contains(&desc.id) {
                desc.state = crate::index2::state::IndexState::Building;
            }
        }
        for &id in &ids {
            self.index2_registry
                .set_state(id, crate::index2::state::IndexState::Building)
                .await;
        }
        crate::index2::persistence::save_index2_metadata(&self.index2_registry, &self.info_store)
            .await?;
        self.identity_migrated_this_open
            .store(true, Ordering::Release);
        for id in ids {
            if let Some(backend) = self.index2_registry.get_by_id(id).await {
                backend.drop_all().await?;
                if let Err(error) = self.backfill_index2_backend(backend.as_ref()).await {
                    if matches!(
                        error,
                        crate::index2::IndexError::Storage(_) | crate::index2::IndexError::Db(_)
                    ) {
                        return Err(error.into());
                    }
                    let reason = format!("identity migration for index '{}' (id={id}) could not backfill: {error}; register its scalar with set_scalar_resolver(), then call TableManager::repair()", backend.descriptor().name);
                    self.index2_registry.set_failed(id, reason).await;
                    crate::index2::persistence::save_index2_metadata(
                        &self.index2_registry,
                        &self.info_store,
                    )
                    .await?;
                    continue;
                }
                self.index2_registry
                    .set_state(id, crate::index2::state::IndexState::Ready)
                    .await;
                crate::index2::persistence::save_index2_metadata(
                    &self.index2_registry,
                    &self.info_store,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// The three-step protocol itself. `force` skips the cheap `need` check
    /// (the re-run path); the non-forced entry points gate on it.
    async fn run_base_identity_migration(
        &self,
        permit: &RepoWritePermit,
        force: bool,
    ) -> DbResult<()> {
        if !force && !shamir_index::persistence::identity_migration_needed(&self.info_store).await?
        {
            return Ok(());
        }
        log::info!(
            "identity migration: rebuilding base index postings in identity v2 for table '{}'",
            self.name
        );
        // Step 1: one atomic transaction removing BOTH markers. Commit
        // point for "every binary must rebuild/migrate from here".
        shamir_index::persistence::clear_identity_markers(&self.info_store).await?;

        let (_barrier, _unique_lock) = self
            .begin_write_barrier_admitted(
                crate::index::write_barrier_flags::REGULAR_INDEX_CREATE,
                permit,
            )
            .await;
        let regular_defs: Vec<_> = self.index_manager_ref().iter_indexes().collect();
        let unique_defs: Vec<_> = self.index_manager_ref().iter_unique_indexes().collect();
        // Durable definitions are replaced in place by each backfill path.
        for def in &regular_defs {
            self.index_manager_ref()
                .sweep_postings_for_rebuild(false, def.name_interned)
                .await?;
            let stream = self
                .list_stream(shamir_tunables::store_defaults::FULL_SCAN_BATCH)
                .map(|batch| {
                    batch.and_then(|rows| {
                        rows.into_iter()
                            .map(|(id, cow)| cow.into_inner().map(|value| (id, value)))
                            .collect()
                    })
                });
            self.index_manager_ref()
                .create_index_from_stream(def.clone(), stream)
                .await?;
        }
        if !unique_defs.is_empty() {
            let records = self.collect_all_current_records().await?;
            for def in &unique_defs {
                self.index_manager_ref()
                    .sweep_postings_for_rebuild(true, def.name_interned)
                    .await?;
                self.index_manager_ref()
                    .create_unique_index_from_records_borrowed(def.clone(), &records)
                    .await?;
            }
        }
        drop(_barrier);
        drop(_unique_lock);
        // Step 3: set idv = 2. Commit point: the table is identity-current.
        shamir_index::persistence::save_index_identity_version(&self.info_store).await?;
        self.identity_migrated_this_open
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Test/diagnostic accessor for the per-open migration flag.
    pub(crate) fn identity_migrated_this_open(&self) -> bool {
        self.identity_migrated_this_open.load(Ordering::Acquire)
    }
}
