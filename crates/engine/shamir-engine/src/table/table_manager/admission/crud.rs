use shamir_query_types::batch::{VisibilityPendingReason, VisibilityState, VisibilityWait};
use shamir_storage::error::DbResult;
use shamir_tx::mvcc_store::write_receipt as storage_ack;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::WriteOutcome;
use crate::table::TableManager;

impl TableManager {
    /// Direct best-effort/no-WAL insert; visibility does not imply atomicity.
    /// cancel-safe: NO mid-sequence; only acknowledgement is cancel-safe.
    pub async fn insert_with_receipt(
        &self,
        value: &InnerValue,
        wait: VisibilityWait,
    ) -> DbResult<WriteOutcome<RecordId>> {
        let (value, version) = self.insert_unacknowledged(value).await?;
        Ok(self.write_receipt(value, version, wait).await)
    }

    /// Direct best-effort/no-WAL batch; does not upgrade backend atomicity.
    /// cancel-safe: NO mid-sequence; only acknowledgement is cancel-safe.
    pub async fn insert_many_with_receipt(
        &self,
        values: &[InnerValue],
        wait: VisibilityWait,
    ) -> DbResult<WriteOutcome<Vec<RecordId>>> {
        let (value, version) = self.insert_many_unacknowledged(values).await?;
        Ok(self.write_receipt(value, version, wait).await)
    }

    /// Missing records mint no version and require no timer or registration.
    /// cancel-safe: NO mid-sequence; only acknowledgement is cancel-safe.
    pub async fn delete_with_receipt(
        &self,
        id: RecordId,
        wait: VisibilityWait,
    ) -> DbResult<WriteOutcome<bool>> {
        let (value, version) = self.delete_unacknowledged(id).await?;
        Ok(self.write_receipt(value, version, wait).await)
    }

    /// Returns true for creation, false for replacement, like set.
    /// cancel-safe: NO mid-sequence; only acknowledgement is cancel-safe.
    pub async fn set_with_receipt(
        &self,
        id: RecordId,
        value: &InnerValue,
        wait: VisibilityWait,
    ) -> DbResult<WriteOutcome<bool>> {
        let (value, version) = self.set_unacknowledged(id, value).await?;
        Ok(self.write_receipt(value, version, wait).await)
    }

    /// cancel-safe: yes — all writer guards were released by the inner write.
    async fn write_receipt<T>(
        &self,
        value: T,
        version: u64,
        wait: VisibilityWait,
    ) -> WriteOutcome<T> {
        let visibility = if version == 0 {
            VisibilityState::Visible
        } else if let Some(mvcc) = self.mvcc_store_ref() {
            let policy = match wait {
                VisibilityWait::Strict => storage_ack::VisibilityWait::Strict,
                VisibilityWait::Bounded { timeout_ms } => {
                    storage_ack::VisibilityWait::Bounded { timeout_ms }
                }
            };
            match mvcc.await_non_tx_ack(version, policy).await {
                storage_ack::VisibilityState::Visible => VisibilityState::Visible,
                storage_ack::VisibilityState::Pending {
                    visible_through,
                    blocking_version,
                    reason,
                } => VisibilityState::Pending {
                    visible_through,
                    blocking_version,
                    reason: match reason {
                        storage_ack::VisibilityPendingReason::Timeout => {
                            VisibilityPendingReason::Timeout
                        }
                        storage_ack::VisibilityPendingReason::ReopenRequired => {
                            VisibilityPendingReason::ReopenRequired
                        }
                    },
                },
            }
        } else {
            VisibilityState::Visible
        };
        WriteOutcome {
            value,
            commit_version: (version != 0).then_some(version),
            visibility,
        }
    }

    pub(crate) async fn insert_returning_version(
        &self,
        value: &InnerValue,
    ) -> DbResult<(RecordId, u64)> {
        let receipt = self
            .insert_with_receipt(value, VisibilityWait::Strict)
            .await?;
        let version = receipt.commit_version.unwrap_or(0);
        Ok((receipt.into_strict_value()?, version))
    }

    pub(crate) async fn insert_many_returning_version(
        &self,
        values: &[InnerValue],
    ) -> DbResult<(Vec<RecordId>, u64)> {
        let receipt = self
            .insert_many_with_receipt(values, VisibilityWait::Strict)
            .await?;
        let version = receipt.commit_version.unwrap_or(0);
        Ok((receipt.into_strict_value()?, version))
    }

    pub(crate) async fn delete_returning_version(&self, id: RecordId) -> DbResult<(bool, u64)> {
        let receipt = self.delete_with_receipt(id, VisibilityWait::Strict).await?;
        let version = receipt.commit_version.unwrap_or(0);
        Ok((receipt.into_strict_value()?, version))
    }

    pub(crate) async fn set_returning_version(
        &self,
        id: RecordId,
        value: &InnerValue,
    ) -> DbResult<(bool, u64)> {
        let receipt = self
            .set_with_receipt(id, value, VisibilityWait::Strict)
            .await?;
        let version = receipt.commit_version.unwrap_or(0);
        Ok((receipt.into_strict_value()?, version))
    }
}
