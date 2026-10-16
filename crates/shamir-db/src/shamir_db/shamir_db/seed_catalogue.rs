use futures::StreamExt;

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{DbError, DbResult, ShamirDb};

/// S5c-X PR8b-2 (evidence row 6): test-only fault seam — when set,
/// `flush_seed_catalogue` fails so terminal quarantine persistence cannot
/// reach its final durability barrier.
#[cfg(test)]
static SEED_CATALOGUE_FLUSH_FAULT: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
impl ShamirDb {
    /// Arm the injected `flush_seed_catalogue` failure (evidence row 6).
    pub(crate) fn arm_seed_catalogue_flush_fault_for_test(&self) {
        SEED_CATALOGUE_FLUSH_FAULT.store(true, Ordering::SeqCst);
    }

    /// Disarm the injected `flush_seed_catalogue` failure.
    pub(crate) fn disarm_seed_catalogue_flush_fault_for_test(&self) {
        SEED_CATALOGUE_FLUSH_FAULT.store(false, Ordering::SeqCst);
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
        #[cfg(test)]
        if SEED_CATALOGUE_FLUSH_FAULT.load(Ordering::SeqCst) {
            return Err(DbError::Storage(
                "injected seed-catalogue flush failure (test)".into(),
            ));
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
