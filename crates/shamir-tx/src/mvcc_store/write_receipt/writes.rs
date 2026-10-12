use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_storage::types::RecordKey;

use super::{VisibilityState, VisibilityWait, WriteOutcome};
use crate::MvccStore;

impl MvccStore {
    /// Finish a guarded write after releasing all writer/barrier guards.
    /// Zero is the internal no-op sentinel, never a reconciliation position.
    /// cancel-safe: yes — only acknowledgement is abandoned.
    #[doc(hidden)]
    pub async fn await_non_tx_ack(&self, version: u64, wait: VisibilityWait) -> VisibilityState {
        self.gate.await_non_tx_ack(version, wait).await
    }

    /// cancel-safe: NO mid-write; only post-known observation is cancel-safe.
    pub async fn set_versioned_with_receipt(
        &self,
        key: RecordKey,
        value: Bytes,
        wait: VisibilityWait,
    ) -> DbResult<WriteOutcome<u64>> {
        let version = self.set_versioned_no_wait(key, value).await?;
        Ok(self.receipt(version, wait).await)
    }

    /// Writes a tombstone, including for an absent key (existing MVCC contract).
    /// cancel-safe: NO mid-write; only post-known observation is cancel-safe.
    pub async fn delete_versioned_with_receipt(
        &self,
        key: RecordKey,
        wait: VisibilityWait,
    ) -> DbResult<WriteOutcome<u64>> {
        let version = self.delete_versioned_no_wait(key).await?;
        Ok(self.receipt(version, wait).await)
    }

    /// cancel-safe: NO mid-write; only post-known observation is cancel-safe.
    pub async fn set_versioned_many_with_receipt(
        &self,
        items: Vec<(RecordKey, Bytes)>,
        wait: VisibilityWait,
    ) -> DbResult<WriteOutcome<u64>> {
        let version = self.set_versioned_many_no_wait(items).await?;
        Ok(self.receipt(version, wait).await)
    }

    /// Same fresh-key contract as set_versioned_many_append_only.
    /// cancel-safe: NO mid-write; only post-known observation is cancel-safe.
    pub async fn set_versioned_many_append_only_with_receipt(
        &self,
        items: Vec<(RecordKey, Bytes)>,
        wait: VisibilityWait,
    ) -> DbResult<WriteOutcome<u64>> {
        let version = self.set_versioned_many_append_only_no_wait(items).await?;
        Ok(self.receipt(version, wait).await)
    }

    /// Guarded callers: immediate observation, never park under writer guards.
    /// cancel-safe: NO mid-write; only post-known observation is cancel-safe.
    pub async fn set_versioned_no_wait_with_receipt(
        &self,
        key: RecordKey,
        value: Bytes,
    ) -> DbResult<WriteOutcome<u64>> {
        self.set_versioned_with_receipt(key, value, VisibilityWait::Bounded { timeout_ms: 0 })
            .await
    }

    /// Guarded callers: immediate observation, never park under writer guards.
    /// cancel-safe: NO mid-write; only post-known observation is cancel-safe.
    pub async fn delete_versioned_no_wait_with_receipt(
        &self,
        key: RecordKey,
    ) -> DbResult<WriteOutcome<u64>> {
        self.delete_versioned_with_receipt(key, VisibilityWait::Bounded { timeout_ms: 0 })
            .await
    }

    /// Guarded callers: immediate observation; empty input has no position.
    /// cancel-safe: NO mid-write; only post-known observation is cancel-safe.
    pub async fn set_versioned_many_no_wait_with_receipt(
        &self,
        items: Vec<(RecordKey, Bytes)>,
    ) -> DbResult<WriteOutcome<u64>> {
        self.set_versioned_many_with_receipt(items, VisibilityWait::Bounded { timeout_ms: 0 })
            .await
    }

    /// Same fresh-key contract as set_versioned_many_append_only.
    /// cancel-safe: NO mid-write; only post-known observation is cancel-safe.
    pub async fn set_versioned_many_append_only_no_wait_with_receipt(
        &self,
        items: Vec<(RecordKey, Bytes)>,
    ) -> DbResult<WriteOutcome<u64>> {
        self.set_versioned_many_append_only_with_receipt(
            items,
            VisibilityWait::Bounded { timeout_ms: 0 },
        )
        .await
    }

    /// cancel-safe: yes — write and vacuum have already completed.
    async fn receipt(&self, version: u64, wait: VisibilityWait) -> WriteOutcome<u64> {
        WriteOutcome {
            value: version,
            commit_version: (version != 0).then_some(version),
            visibility: self.gate.await_non_tx_ack(version, wait).await,
        }
    }

    pub(in crate::mvcc_store) async fn strict_ack(&self, version: u64) -> DbResult<()> {
        self.receipt(version, VisibilityWait::Strict)
            .await
            .into_strict_value()
            .map(|_| ())
    }
}
