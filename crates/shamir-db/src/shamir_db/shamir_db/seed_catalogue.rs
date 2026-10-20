use futures::StreamExt;

#[cfg(any(test, feature = "test-util"))]
use shamir_types::types::value::QueryValue;
#[cfg(any(test, feature = "test-util"))]
use std::sync::atomic::Ordering;

use crate::{DbError, DbResult, ShamirDb};

/// S5c-X PR8b-2 (evidence row 6; round 3 P2): test-only PER-INSTANCE fault
/// seam — while armed on an instance, `flush_seed_catalogue` fails so
/// terminal quarantine persistence cannot reach its final durability
/// barrier. The round-2 GLOBAL atomic let concurrent tests on different
/// instances interfere; arming is now an instance field.
/// `SeedCatalogueFlushFaultForTest` is the RAII guard.
///
/// Gated by `#[cfg(any(test, feature = "test-util"))]` (round 5): only test
/// builds of THIS crate and of downstream crates that opt into the opt-in
/// `test-util` feature (shamir-server's dev-dependency enables it) see the
/// seam; production default features are untouched.
#[cfg(any(test, feature = "test-util"))]
pub struct SeedCatalogueFlushFaultForTest(ShamirDb);

#[cfg(any(test, feature = "test-util"))]
impl SeedCatalogueFlushFaultForTest {
    /// Arm the injected `flush_seed_catalogue` failure for this guard's
    /// lifetime (evidence row 6): it fires immediately.
    pub fn arm(shamir: &ShamirDb) -> Self {
        Self::arm_after_passing(shamir, 0)
    }

    /// Arm the fault so the first `n` `flush_seed_catalogue` calls succeed
    /// and every later one fails — for tests that must observe a success
    /// before the injected failure.
    pub fn arm_after_passing(shamir: &ShamirDb, n: usize) -> Self {
        shamir.flush_fault_armed.store(true, Ordering::SeqCst);
        shamir.flush_fault_fail_first.store(0, Ordering::SeqCst);
        shamir.flush_fault_pass_first.store(n, Ordering::SeqCst);
        Self::reset_counters(shamir);
        Self(shamir.clone())
    }

    /// Round-5 B3 fail-first mode: the first `n` `flush_seed_catalogue`
    /// calls FAIL, and every LATER call passes again. For tests that need a
    /// bounded prefix of real failures (barrier deferrals plus terminal-mark
    /// retry attempts) followed by a succeeding tail — e.g. `mark_with_retry`
    /// must exhaust its first attempts on the seam and then succeed.
    pub fn arm_fail_first(shamir: &ShamirDb, n: usize) -> Self {
        shamir.flush_fault_armed.store(true, Ordering::SeqCst);
        shamir.flush_fault_fail_first.store(n, Ordering::SeqCst);
        shamir
            .flush_fault_pass_first
            .store(usize::MAX, Ordering::SeqCst);
        Self::reset_counters(shamir);
        Self(shamir.clone())
    }

    /// Round 7 seam counters: how many `flush_seed_catalogue` ATTEMPTS were
    /// made while this guard was armed, and how many were injected
    /// FAILURES. Reset by every `arm*` constructor and by `Drop`.
    pub fn attempts(&self) -> usize {
        self.0.flush_fault_attempts.load(Ordering::SeqCst)
    }
    pub fn failures(&self) -> usize {
        self.0.flush_fault_failures.load(Ordering::SeqCst)
    }
    /// Number of fully completed real flushes during this armed interval.
    pub fn successes(&self) -> usize {
        self.0.flush_fault_successes.load(Ordering::SeqCst)
    }

    fn reset_counters(shamir: &ShamirDb) {
        shamir.flush_fault_attempts.store(0, Ordering::SeqCst);
        shamir.flush_fault_failures.store(0, Ordering::SeqCst);
        shamir.flush_fault_successes.store(0, Ordering::SeqCst);
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Drop for SeedCatalogueFlushFaultForTest {
    fn drop(&mut self) {
        self.0.flush_fault_armed.store(false, Ordering::SeqCst);
        self.0.flush_fault_fail_first.store(0, Ordering::SeqCst);
        self.0.flush_fault_pass_first.store(0, Ordering::SeqCst);
        Self::reset_counters(&self.0);
    }
}

/// Round-6 item 5 test seam: while armed on an instance,
/// `mark_subscription_terminal_generation` fails its row SET BEFORE the row
/// changes — so the terminal mark prepublishes its quarantine keys but the
/// durable row is untouched. The pending keys must keep reads and the
/// drop/rename guards closed until an explicit confirmation
/// (`TerminalRowSetFaultForTest` is the RAII guard; same
/// `#[cfg(any(test, feature = "test-util"))]` gating as the flush-fault
/// seams).
#[cfg(any(test, feature = "test-util"))]
pub struct TerminalRowSetFaultForTest(ShamirDb);

#[cfg(any(test, feature = "test-util"))]
impl TerminalRowSetFaultForTest {
    /// Arm the injected row-SET failure for this guard's lifetime.
    pub fn arm(shamir: &ShamirDb) -> Self {
        shamir
            .row_set_fault_armed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Self(shamir.clone())
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Drop for TerminalRowSetFaultForTest {
    fn drop(&mut self) {
        self.0
            .row_set_fault_armed
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Round-5 T7 (cross-crate) handle on the final-write seam: arms
/// `final_write_pause` and hands out the arrival Notify so a downstream
/// test (shamir-server via `test-util`) can drop a CREATE future exactly on
/// the final-write boundary. RAII `Drop` disarms.
#[cfg(any(test, feature = "test-util"))]
pub struct FinalWriteBarrierForTest(ShamirDb);

#[cfg(any(test, feature = "test-util"))]
impl FinalWriteBarrierForTest {
    /// Arm the final-write pause on `shamir`.
    pub fn arm(shamir: &ShamirDb) -> Self {
        shamir
            .final_write_pause
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Self(shamir.clone())
    }

    /// The arrival Notify: fires when the CREATE future reaches the
    /// final-write boundary.
    pub fn arrived(&self) -> std::sync::Arc<tokio::sync::Notify> {
        self.0.final_write_arrived.clone()
    }

    /// Disarm before the guard drops (e.g. to let a retry run for real).
    pub fn disarm(&self) {
        self.0
            .final_write_pause
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Drop for FinalWriteBarrierForTest {
    fn drop(&mut self) {
        self.disarm();
    }
}

impl ShamirDb {
    /// Test-only subscription-catalogue injection seam (S5c-X round 9): upsert
    /// one subscription row through the SAME handle the real admin writers use
    /// — `system_store().subscriptions_table()` + `set_via_implicit_tx`, no db
    /// name (the catalogue lives on the SYSTEM repo, not a user db; a batch
    /// `Insert` addressed to db `"system"` fails with "Database 'system' not
    /// found"). `record` must carry the `name` field (the row key).
    #[cfg(any(test, feature = "test-util"))]
    pub async fn inject_subscription_row_for_test(&self, record: QueryValue) -> DbResult<()> {
        let name = record["name"]
            .as_str()
            .ok_or_else(|| DbError::Validation("subscription record needs a name".into()))?
            .to_owned();
        let store = self.system_store();
        let table = store.subscriptions_table().await?;
        let mut key_fields = crate::types::common::new_map();
        key_fields.insert("name".to_string(), QueryValue::Str(name));
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new("subscriptions"),
            key: QueryValue::Map(key_fields),
            value: record,
        };
        store.set_via_implicit_tx(&table, &op).await?;
        store.persist_table_interner(&table, None).await?;
        self.flush_seed_catalogue().await
    }

    /// Offline-only live-record cap across all system tables.
    pub const SEED_CATALOGUE_MAX_RECORDS: usize = 4096;
    /// Offline-only live-byte cap across all system tables.
    pub const SEED_CATALOGUE_MAX_BYTES: usize = 8 * 1024 * 1024;

    /// Before the normal init loaders allocate their complete catalogues, scan
    /// live MVCC payloads one at a time (the raw __data__ store is inert).
    /// Caps: `SEED_CATALOGUE_MAX_RECORDS` records and `SEED_CATALOGUE_MAX_BYTES`
    /// across all system tables, including functions/validators loaded at boot.
    pub async fn check_seed_catalogue(&self) -> DbResult<()> {
        self.check_seed_catalogue_with_limits(
            Self::SEED_CATALOGUE_MAX_RECORDS,
            Self::SEED_CATALOGUE_MAX_BYTES,
        )
        .await
    }

    /// Same scan with explicit caps, so boundaries are testable without
    /// writing thousands of catalogue rows.
    pub async fn check_seed_catalogue_with_limits(
        &self,
        max_records: usize,
        max_bytes: usize,
    ) -> DbResult<()> {
        let repo = self.system_store.system_repo()?;
        let version = repo.consistent_version().await?;
        let mut bytes = 0usize;
        let mut records = 0usize;
        for name in repo.list_table_names() {
            let table = repo.get_table(&name).await?;
            let stream = table
                .mvcc_snapshot_stream(1, version)
                .ok_or_else(|| DbError::Storage("seed catalogue requires MVCC".into()))?;
            futures::pin_mut!(stream);
            while let Some(batch) = stream.next().await {
                for (key, value) in batch? {
                    bytes = bytes
                        .checked_add(key.as_ref().len())
                        .and_then(|n| n.checked_add(value.len()))
                        .ok_or_else(|| DbError::Storage("seed catalogue overflow".into()))?;
                    records += 1;
                    if bytes > max_bytes || records > max_records {
                        return Err(DbError::Storage("seed catalogue budget exceeded".into()));
                    }
                }
            }
        }
        if repo.consistent_version().await? != version {
            return Err(DbError::Storage(
                "seed catalogue changed during scan".into(),
            ));
        }
        Ok(())
    }

    /// Strict metadata durability for an offline seed; no clean checkpoint is
    /// required merely to flush the authoritative table/repository catalogue.
    pub async fn flush_seed_catalogue(&self) -> DbResult<()> {
        // Test-only per-instance fault seam (production untouched; visible
        // to downstream test builds via the opt-in `test-util` feature).
        #[cfg(any(test, feature = "test-util"))]
        if self.flush_fault_armed.load(Ordering::SeqCst) {
            // Round 7 seam counters: count every attempt while armed; the
            // injected-failure branches count their failures below.
            self.flush_fault_attempts.fetch_add(1, Ordering::SeqCst);
            if self.flush_fault_fail_first.load(Ordering::SeqCst) > 0 {
                self.flush_fault_fail_first.fetch_sub(1, Ordering::SeqCst);
                self.flush_fault_failures.fetch_add(1, Ordering::SeqCst);
                return Err(DbError::Storage(
                    "injected seed-catalogue flush failure (test)".into(),
                ));
            } else if self.flush_fault_pass_first.load(Ordering::SeqCst) > 0 {
                self.flush_fault_pass_first.fetch_sub(1, Ordering::SeqCst);
            } else {
                self.flush_fault_failures.fetch_add(1, Ordering::SeqCst);
                return Err(DbError::Storage(
                    "injected seed-catalogue flush failure (test)".into(),
                ));
            }
        }
        let repo = self.system_store.system_repo()?;
        for name in repo.list_table_names() {
            repo.get_table(&name).await?;
        }
        repo.flush_buffers().await?;
        repo.flush_all_history().await?;
        repo.flush_seed_metadata().await?;
        #[cfg(any(test, feature = "test-util"))]
        if self.flush_fault_armed.load(Ordering::SeqCst) {
            self.flush_fault_successes.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}
