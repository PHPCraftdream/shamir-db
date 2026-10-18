use futures::StreamExt;

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
        shamir.flush_fault_pass_first.store(usize::MAX, Ordering::SeqCst);
        Self(shamir.clone())
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Drop for SeedCatalogueFlushFaultForTest {
    fn drop(&mut self) {
        self.0.flush_fault_armed.store(false, Ordering::SeqCst);
        self.0.flush_fault_fail_first.store(0, Ordering::SeqCst);
        self.0.flush_fault_pass_first.store(0, Ordering::SeqCst);
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
            if self.flush_fault_fail_first.load(Ordering::SeqCst) > 0 {
                self.flush_fault_fail_first.fetch_sub(1, Ordering::SeqCst);
                return Err(DbError::Storage(
                    "injected seed-catalogue flush failure (test)".into(),
                ));
            } else if self.flush_fault_pass_first.load(Ordering::SeqCst) > 0 {
                self.flush_fault_pass_first.fetch_sub(1, Ordering::SeqCst);
            } else {
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
        Ok(())
    }
}
