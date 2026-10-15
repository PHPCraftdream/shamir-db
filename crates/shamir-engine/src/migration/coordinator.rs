use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
use shamir_types::time::unix_nanos;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::shadow_log::{MigrationShadowLog, ShadowEntry, ShadowOp};
use crate::repo::{RepoWriteDomain, RepoWritePermit};
use crate::table::MigrationFreezeGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationPhase {
    ShadowStarted,
    Snapshotting,
    Draining,
    CutoverReady,
    Committed,
    RolledBack,
}

impl std::fmt::Display for MigrationPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShadowStarted => write!(f, "shadow_started"),
            Self::Snapshotting => write!(f, "snapshotting"),
            Self::Draining => write!(f, "draining"),
            Self::CutoverReady => write!(f, "cutover_ready"),
            Self::Committed => write!(f, "committed"),
            Self::RolledBack => write!(f, "rolled_back"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationState {
    pub id: String,
    pub phase: MigrationPhase,
    pub table_name: String,
    pub src_repo: String,
    pub dst_repo: String,
    pub dst_engine: String,
    pub dst_path: Option<String>,
    pub snapshot_lsn: u64,
    pub last_lsn_applied: u64,
    pub started_at_ns: u64,
    pub records_copied: u64,
}

impl MigrationState {
    pub fn new(
        id: String,
        table_name: String,
        src_repo: String,
        dst_repo: String,
        dst_engine: String,
        dst_path: Option<String>,
    ) -> Self {
        let started_at_ns = unix_nanos();
        Self {
            id,
            phase: MigrationPhase::ShadowStarted,
            table_name,
            src_repo,
            dst_repo,
            dst_engine,
            dst_path,
            snapshot_lsn: 0,
            last_lsn_applied: 0,
            started_at_ns,
            records_copied: 0,
        }
    }
}

/// Outcome of a bounded [`MigrationCoordinator::drain_until_caught_up`] run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainCatchUpResult {
    /// Total shadow-log entries applied across every pass.
    pub total_applied: u64,
    /// Lag remaining when the call returned. `<= max_lag` means the
    /// caller's target was reached; a value above `max_lag` once `passes
    /// == DRAIN_PASS_CAP` means sustained writes outran drain throughput
    /// — the caller should retry, back off, or surface "still catching up".
    pub residual_lag: u64,
    /// Number of drain passes actually performed (always <= `DRAIN_PASS_CAP`).
    pub passes: usize,
}

pub struct MigrationCoordinator {
    state: Mutex<MigrationState>,
    // Rare admin finalisation only; never taken by record reads/writes.
    finalisation: Mutex<()>,
    // Rare admin ownership transfers only; held for Option::take, never across
    // await. Poison recovery still permits synchronous freeze cleanup.
    source_freeze: std::sync::Mutex<Option<MigrationFreezeGuard>>,
    shadow_log: Arc<MigrationShadowLog>,
    src_data: Arc<dyn Store>,
    dst_data: Arc<dyn Store>,
    /// Optional handle to the source table's MvccStore (version-log seam).
    /// When present, `run_snapshot` reads the snapshot via `current_stream`
    /// (the log seam) instead of the raw `src_data` store.  This ensures the
    /// snapshot remains correct after `main`-store dual-writes are removed.
    /// `None` preserves the raw-store fallback path used by unit tests that
    /// construct the coordinator with plain in-memory stores.
    src_mvcc: Option<Arc<shamir_tx::MvccStore>>,
    /// S5c-X F3 (Round 6): the source/destination repositories' write
    /// domains, captured by the admitted factories. Every admitted twin
    /// (`*_admitted`) debug-checks the presented permits against THESE
    /// domains, so a coordinator handed out by the shamir-db facade can
    /// never be driven through a state-changing call without repo admission
    /// — even though the owned `Arc` escapes the factory. `None` (direct
    /// `MigrationCoordinator::new` in cross-crate tests with raw stores)
    /// skips the check.
    src_domain: Option<Arc<RepoWriteDomain>>,
    dst_domain: Option<Arc<RepoWriteDomain>>,
}

impl MigrationCoordinator {
    /// Non-blocking check: does this coordinator target the given
    /// `(src_repo, table_name)`? Skips on lock contention (caller
    /// retries via the normal phase check in start_migration). Used
    /// to detect concurrent migrations on the same table.
    pub fn targets_table(&self, src_repo: &str, table_name: &str) -> bool {
        match self.state.try_lock() {
            Ok(s) => s.table_name == table_name && s.src_repo == src_repo,
            Err(_) => false,
        }
    }
}

impl MigrationCoordinator {
    /// Max drain passes attempted per `drain_until_caught_up` call. Guards
    /// against livelock: under sustained source writes that outrun drain
    /// throughput, `shadow_lag()` may never fall to `max_lag` and looping
    /// on `applied > 0` alone would spin forever. Once the budget is
    /// spent, the caller gets back the residual lag instead. Mirrors
    /// `TableManager::CATCHUP_ITERATION_CAP`'s local-const pattern — no
    /// `shamir-tunables` precedent for this cap yet.
    pub(crate) const DRAIN_PASS_CAP: usize = 32;

    pub fn new(
        state: MigrationState,
        shadow_log: Arc<MigrationShadowLog>,
        src_data: Arc<dyn Store>,
        dst_data: Arc<dyn Store>,
        src_mvcc: Option<Arc<shamir_tx::MvccStore>>,
    ) -> Self {
        Self {
            state: Mutex::new(state),
            finalisation: Mutex::new(()),
            source_freeze: std::sync::Mutex::new(None),
            shadow_log,
            src_data,
            dst_data,
            src_mvcc,
            src_domain: None,
            dst_domain: None,
        }
    }

    /// Capture the source/destination write domains (admitted factories
    /// only). Chain after `with_migration_freeze`.
    pub fn with_domains(
        mut self,
        src_domain: Arc<RepoWriteDomain>,
        dst_domain: Arc<RepoWriteDomain>,
    ) -> Self {
        self.src_domain = Some(src_domain);
        self.dst_domain = Some(dst_domain);
        self
    }

    /// S5c-X F3: identity-check the presented permits against the captured
    /// domains. Debug-only (mirrors `RepoWriteDomain::debug_check_permit`).
    /// The destination check is skipped when no destination permit is
    /// presented (rollback after the destination repository was removed).
    fn debug_check_admission(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: Option<&RepoWritePermit>,
    ) {
        if let Some(domain) = &self.src_domain {
            domain.debug_check_permit(src_permit);
        }
        if let (Some(domain), Some(permit)) = (&self.dst_domain, dst_permit) {
            domain.debug_check_permit(permit);
        }
    }

    /// Transfer Start's guard without an await or an unowned freeze window.
    pub fn with_migration_freeze(mut self, guard: MigrationFreezeGuard) -> Self {
        *self
            .source_freeze
            .get_mut()
            .unwrap_or_else(|e| e.into_inner()) = Some(guard);
        self
    }

    /// Move cleanup into the admin future immediately AFTER it acquires the
    /// finalisation lock, with no intervening await. Waiters must not take the
    /// active finaliser's freeze. Body cancellation/errors then drop the guard
    /// even if this coordinator is retained in the map or by another Arc.
    /// An uncommitted retry without a guard cannot safely resume its snapshot.
    ///
    /// S5c-X F3: admitted twin `take_migration_freeze_admitted` is the public
    /// surface; this plain form is crate-internal.
    pub(crate) fn take_migration_freeze(&self) -> Option<MigrationFreezeGuard> {
        self.source_freeze
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Admitted twin of [`Self::take_migration_freeze`] (S5c-X F3): the
    /// caller holds both repositories' write permits; they are
    /// identity-checked against the domains captured at factory time.
    pub fn take_migration_freeze_admitted(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: Option<&RepoWritePermit>,
    ) -> Option<MigrationFreezeGuard> {
        self.debug_check_admission(src_permit, dst_permit);
        self.take_migration_freeze()
    }

    /// Serialize admin retries without holding a map or state guard over I/O.
    pub async fn lock_finalisation(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.finalisation.lock().await
    }

    pub async fn state(&self) -> MigrationState {
        self.state.lock().await.clone()
    }

    pub async fn phase(&self) -> MigrationPhase {
        self.state.lock().await.phase
    }

    /// Persist the unchanged bincode state layout under a separate keyspace.
    /// S5c-X F3: crate-internal; the facade goes through
    /// [`Self::persist_state_admitted`].
    pub(crate) async fn persist_state(&self) -> DbResult<()> {
        let state = self.state().await;
        let bytes = bincode::serialize(&state)
            .map_err(|e| DbError::Codec(format!("migration state: {e}")))?;
        let store = self.shadow_log.store();
        store
            .set(Self::state_key(&state.id), Bytes::from(bytes))
            .await?;
        store.flush().await
    }

    fn state_key(id: &str) -> RecordKey {
        RecordKey::from(format!("__migration_state_{id}").into_bytes())
    }

    pub async fn load_state(store: &Arc<dyn Store>, id: &str) -> DbResult<Option<MigrationState>> {
        match store.get(Self::state_key(id)).await {
            Ok(bytes) => bincode::deserialize(&bytes)
                .map(Some)
                .map_err(|e| DbError::Codec(format!("migration state: {e}"))),
            Err(DbError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Stages are separate one-byte records: no MigrationState layout change.
    pub async fn finalisation_progress(&self) -> DbResult<u8> {
        let id = self.migration_id().await;
        let key = RecordKey::from(format!("__migration_finalise_{id}").into_bytes());
        match self.shadow_log.store().get(key).await {
            Ok(bytes) => match bytes.as_ref() {
                [stage @ 0..=2] => Ok(*stage),
                _ => Err(DbError::Codec(
                    "invalid migration finalisation stage".into(),
                )),
            },
            Err(DbError::NotFound(_)) => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// S5c-X F3: crate-internal; see
    /// [`Self::complete_finalisation_step_admitted`].
    pub(crate) async fn complete_finalisation_step(&self, stage: u8) -> DbResult<()> {
        if self.phase().await != MigrationPhase::Committed || !(1..=2).contains(&stage) {
            return Err(DbError::Validation(
                "invalid migration finalisation step".into(),
            ));
        }
        let current = self.finalisation_progress().await?;
        if stage <= current {
            return Ok(());
        }
        if stage != current + 1 {
            return Err(DbError::Validation(
                "migration finalisation out of order".into(),
            ));
        }
        let id = self.migration_id().await;
        let key = RecordKey::from(format!("__migration_finalise_{id}").into_bytes());
        let store = self.shadow_log.store();
        store.set(key, Bytes::from(vec![stage])).await?;
        store.flush().await
    }

    pub async fn migration_id(&self) -> String {
        self.state.lock().await.id.clone()
    }

    /// S5c-X F3: crate-internal — the log's `append`/`purge` are mutating
    /// and no longer reachable downstream; reads go through
    /// [`Self::shadow_lag`] / [`Self::current_lsn`].
    pub(crate) fn shadow_log(&self) -> &Arc<MigrationShadowLog> {
        &self.shadow_log
    }

    /// Admitted twin of [`Self::run_snapshot`] (S5c-X F3).
    pub async fn run_snapshot_admitted(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: &RepoWritePermit,
    ) -> DbResult<u64> {
        self.debug_check_admission(src_permit, Some(dst_permit));
        self.run_snapshot().await
    }

    /// S5c-X F3: crate-internal; see [`Self::run_snapshot_admitted`].
    pub(crate) async fn run_snapshot(&self) -> DbResult<u64> {
        {
            let mut s = self.state.lock().await;
            if s.phase != MigrationPhase::ShadowStarted {
                return Err(DbError::Internal(format!(
                    "snapshot requires ShadowStarted, got {}",
                    s.phase
                )));
            }
            s.snapshot_lsn = self.shadow_log.current_lsn();
            s.phase = MigrationPhase::Snapshotting;
        }

        let mut copied = 0u64;
        if let Some(mvcc) = &self.src_mvcc {
            // Read the snapshot through the MVCC log seam (current_stream).
            // Suppresses tombstones and yields the current value per key —
            // behaviour-equivalent to reading the source's current data_store
            // while main is still dual-written.
            let mut stream = mvcc.current_stream(MAINT_SCAN_BATCH);
            while let Some(batch) = stream.next().await {
                let records = batch?;
                if records.is_empty() {
                    break;
                }
                // Boundary: the mvcc stream yields `Bytes` keys; `set_many`
                // takes `RecordKey` (byte-identical conversion).
                let items: Vec<(RecordKey, Bytes)> =
                    records.into_iter().map(|(k, v)| (k.into(), v)).collect();
                let count = items.len() as u64;
                self.dst_data.set_many(items).await?;
                copied += count;
            }
        } else {
            // Fallback: read from the raw data_store (used by unit tests that
            // construct the coordinator with plain in-memory stores).
            let mut stream = self.src_data.iter_stream(MAINT_SCAN_BATCH);
            while let Some(batch) = stream.next().await {
                let records = batch?;
                if records.is_empty() {
                    break;
                }
                // `iter_stream` already yields `RecordKey` keys — feed the
                // batch straight to `set_many`.
                let items: Vec<(RecordKey, Bytes)> = records;
                let count = items.len() as u64;
                self.dst_data.set_many(items).await?;
                copied += count;
            }
        }

        {
            let mut s = self.state.lock().await;
            s.records_copied = copied;
            s.phase = MigrationPhase::Draining;
        }
        Ok(copied)
    }

    /// Apply entries in LSN order, batching only adjacent operations of
    /// the same kind. Put runs retain input order (including duplicate keys):
    /// `Store::set_many`'s sequential default and MemBuffer's ordered override
    /// preserve last-writer-wins; other current backends inherit the default.
    /// Returns `entries.len()` (every entry is applied; `set`/`remove`
    /// on a store never surface a per-key "not found" error — a missing
    /// key on `remove` just yields `false`, see `Store::remove`).
    async fn apply_shadow_entries(&self, entries: &[ShadowEntry]) -> DbResult<u64> {
        if entries.is_empty() {
            return Ok(0);
        }
        let mut puts: Vec<(RecordKey, Bytes)> = Vec::new();
        let mut deletes: Vec<RecordKey> = Vec::new();
        for entry in entries {
            match &entry.op {
                ShadowOp::Put { record_id, value } => {
                    if !deletes.is_empty() {
                        self.dst_data
                            .remove_many(std::mem::take(&mut deletes))
                            .await?;
                    }
                    let key = RecordKey::from(record_id.as_bytes().to_vec());
                    puts.push((key, Bytes::from(value.clone())));
                }
                ShadowOp::Delete { record_id } => {
                    if !puts.is_empty() {
                        self.dst_data.set_many(std::mem::take(&mut puts)).await?;
                    }
                    let key = RecordKey::from(record_id.as_bytes().to_vec());
                    deletes.push(key);
                }
            }
        }
        if !puts.is_empty() {
            self.dst_data.set_many(puts).await?;
        }
        if !deletes.is_empty() {
            self.dst_data.remove_many(deletes).await?;
        }
        Ok(entries.len() as u64)
    }

    /// S5c-X F3: crate-internal (called by the drain loops); the facade
    /// goes through [`Self::drain_until_caught_up_admitted`].
    pub(crate) async fn drain_shadow_log(&self) -> DbResult<u64> {
        let start_lsn = {
            let s = self.state.lock().await;
            if s.phase != MigrationPhase::Draining {
                return Err(DbError::Internal(format!(
                    "drain requires Draining, got {}",
                    s.phase
                )));
            }
            s.last_lsn_applied.max(s.snapshot_lsn) + 1
        };

        let entries = self.shadow_log.read_from(start_lsn).await?;
        let applied = self.apply_shadow_entries(&entries).await?;

        if let Some(last) = entries.last() {
            self.state.lock().await.last_lsn_applied = last.lsn;
        }

        Ok(applied)
    }

    pub async fn shadow_lag(&self) -> u64 {
        let (last_applied, snapshot_lsn) = {
            let s = self.state.lock().await;
            (s.last_lsn_applied, s.snapshot_lsn)
        };
        let current = self.shadow_log.current_lsn();
        current.saturating_sub(last_applied.max(snapshot_lsn))
    }

    /// Admitted twin of [`Self::drain_until_caught_up`] (S5c-X F3).
    pub async fn drain_until_caught_up_admitted(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: &RepoWritePermit,
        max_lag: u64,
    ) -> DbResult<DrainCatchUpResult> {
        self.debug_check_admission(src_permit, Some(dst_permit));
        self.drain_until_caught_up(max_lag).await
    }

    /// S5c-X F3: crate-internal; see [`Self::drain_until_caught_up_admitted`].
    pub(crate) async fn drain_until_caught_up(&self, max_lag: u64) -> DbResult<DrainCatchUpResult> {
        let mut total = 0u64;
        let mut passes = 0usize;
        let mut lag = self.shadow_lag().await;
        while lag > max_lag && passes < Self::DRAIN_PASS_CAP {
            let applied = self.drain_shadow_log().await?;
            total += applied;
            passes += 1;
            lag = self.shadow_lag().await;
            if applied == 0 {
                break;
            }
        }
        Ok(DrainCatchUpResult {
            total_applied: total,
            residual_lag: lag,
            passes,
        })
    }

    /// Admitted twin of [`Self::mark_cutover_ready`] (S5c-X F3).
    pub async fn mark_cutover_ready_admitted(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.debug_check_admission(src_permit, Some(dst_permit));
        self.mark_cutover_ready().await
    }

    /// S5c-X F3: crate-internal; see [`Self::mark_cutover_ready_admitted`].
    pub(crate) async fn mark_cutover_ready(&self) -> DbResult<()> {
        let mut s = self.state.lock().await;
        if s.phase != MigrationPhase::Draining {
            return Err(DbError::Internal(format!(
                "cutover_ready requires Draining, got {}",
                s.phase
            )));
        }
        let residual_lag = self
            .shadow_log
            .current_lsn()
            .saturating_sub(s.last_lsn_applied.max(s.snapshot_lsn));
        if residual_lag != 0 {
            return Err(DbError::Conflict(format!(
                "migration cutover has {residual_lag} unapplied entries; retry draining"
            )));
        }
        s.phase = MigrationPhase::CutoverReady;
        Ok(())
    }

    /// Caller must quiesce source writers and serialize coordinator operations.
    /// Failed page application leaves its cursor unchanged and can be replayed.
    ///
    /// S5c-X F3: admitted twin `final_drain_and_commit_admitted` is the
    /// public surface; this plain form is crate-internal.
    pub(crate) async fn final_drain_and_commit(&self) -> DbResult<u64> {
        {
            let s = self.state.lock().await;
            if s.phase == MigrationPhase::Committed {
                drop(s);
                self.persist_state().await?;
                return Ok(0);
            }
            if s.phase != MigrationPhase::CutoverReady {
                return Err(DbError::Internal(format!(
                    "final_drain requires CutoverReady, got {}",
                    s.phase
                )));
            }
        }

        let mut applied = 0u64;
        loop {
            let start_lsn = {
                let s = self.state.lock().await;
                s.last_lsn_applied
                    .max(s.snapshot_lsn)
                    .checked_add(1)
                    .ok_or_else(|| DbError::Internal("migration LSN exhausted".into()))?
            };
            let entries = self.shadow_log.read_from(start_lsn).await?;
            if entries.is_empty() {
                break;
            }
            applied = applied
                .checked_add(self.apply_shadow_entries(&entries).await?)
                .ok_or_else(|| DbError::Internal("migration applied count overflow".into()))?;
            if let Some(last) = entries.last() {
                self.state.lock().await.last_lsn_applied = last.lsn;
            }
        }
        self.dst_data.flush().await?;
        self.persist_state().await?;

        // Purge before flipping to Committed (mirrors `rollback`'s
        // purge-then-flip order): if purge fails, the migration stays in
        // CutoverReady and a retried `final_drain_and_commit` picks up
        // from the already-advanced `last_lsn_applied` (no entries to
        // re-apply) and just retries the purge. A committed migration has
        // no further use for its shadow log — crash recovery replays
        // forward from the committed dst, not from this log — so leaving
        // it on disk forever is pure leaked storage.
        self.shadow_log.purge().await?;
        self.state.lock().await.phase = MigrationPhase::Committed;
        self.persist_state().await?;
        Ok(applied)
    }

    /// S5c-X F3: crate-internal; see [`Self::rollback_admitted`].
    pub(crate) async fn rollback(&self) -> DbResult<()> {
        let phase = self.phase().await;
        if phase == MigrationPhase::Committed {
            return Err(DbError::Internal(
                "cannot rollback a committed migration".into(),
            ));
        }
        if phase == MigrationPhase::RolledBack {
            return Ok(());
        }
        self.shadow_log.purge().await?;
        self.state.lock().await.phase = MigrationPhase::RolledBack;
        self.persist_state().await?;
        Ok(())
    }

    /// A committed retry finalises the fixed destination, not the source's
    /// newly admitted writes. The original snapshot count is its baseline.
    pub async fn verify_committed_record_count(&self) -> DbResult<(u64, u64)> {
        let expected = self.state().await.records_copied;
        let mut count = 0u64;
        let mut stream = self.dst_data.iter_stream(MAINT_SCAN_BATCH);
        while let Some(batch) = stream.next().await {
            count += batch?.len() as u64;
        }
        Ok((expected, count))
    }

    pub async fn verify_record_count(&self) -> DbResult<(u64, u64)> {
        // FINAL-A: when a src_mvcc is available, count via the version-log
        // seam (current_stream) — the raw src_data store is no longer written
        // by the MVCC write paths. Fall back to src_data.iter_stream when no
        // MvccStore is present (e.g. plain unit-test stores).
        let src_count = if let Some(mvcc) = &self.src_mvcc {
            let mut count = 0u64;
            let mut stream = mvcc.current_stream(MAINT_SCAN_BATCH);
            while let Some(batch) = stream.next().await {
                count += batch?.len() as u64;
            }
            count
        } else {
            let mut count = 0u64;
            let mut stream = self.src_data.iter_stream(MAINT_SCAN_BATCH);
            while let Some(batch) = stream.next().await {
                count += batch?.len() as u64;
            }
            count
        };

        let mut dst_count = 0u64;
        let mut stream = self.dst_data.iter_stream(MAINT_SCAN_BATCH);
        while let Some(batch) = stream.next().await {
            dst_count += batch?.len() as u64;
        }

        Ok((src_count, dst_count))
    }

    /// Admitted twin of [`Self::persist_state`] (S5c-X F3).
    pub async fn persist_state_admitted(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.debug_check_admission(src_permit, Some(dst_permit));
        self.persist_state().await
    }

    /// Admitted twin of [`Self::final_drain_and_commit`] (S5c-X F3).
    pub async fn final_drain_and_commit_admitted(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: &RepoWritePermit,
    ) -> DbResult<u64> {
        self.debug_check_admission(src_permit, Some(dst_permit));
        self.final_drain_and_commit().await
    }

    /// Admitted twin of [`Self::rollback`] (S5c-X F3). The destination
    /// permit is `None` when the destination repository was already removed
    /// (crash-recovery rollback); the destination check is skipped then.
    pub async fn rollback_admitted(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: Option<&RepoWritePermit>,
    ) -> DbResult<()> {
        self.debug_check_admission(src_permit, dst_permit);
        self.rollback().await
    }

    /// Admitted twin of [`Self::complete_finalisation_step`] (S5c-X F3).
    pub async fn complete_finalisation_step_admitted(
        &self,
        src_permit: &RepoWritePermit,
        dst_permit: &RepoWritePermit,
        stage: u8,
    ) -> DbResult<()> {
        self.debug_check_admission(src_permit, Some(dst_permit));
        self.complete_finalisation_step(stage).await
    }
}
