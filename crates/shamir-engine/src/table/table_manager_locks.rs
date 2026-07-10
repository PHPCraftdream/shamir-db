use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::RecordKey;

use super::table_manager::TableManager;

impl TableManager {
    /// Acquire a Level-3 `Shared` (read) lock on `key` for a Pessimistic tx.
    ///
    /// No-op for non-Pessimistic isolation. Records the locked key on the tx
    /// (via interior mutability) so it is released on commit / abort. A
    /// wound-wait abort surfaces as `DbError::Conflict`.
    pub async fn acquire_pessimistic_read_lock(
        &self,
        key: RecordKey,
        tx: &shamir_tx::TxContext,
    ) -> DbResult<()> {
        if tx.isolation != shamir_tx::IsolationLevel::Pessimistic {
            return Ok(());
        }
        let mvcc = self.mvcc_store.as_ref().ok_or_else(|| {
            DbError::Conflict(format!(
                "Level-3 (Pessimistic) op on non-MVCC table '{}': no mvcc_store",
                self.name
            ))
        })?;
        mvcc.lock_key(
            key.clone(),
            tx.tx_id.0,
            tx.wounded_flag(),
            tx.wound_notify(),
            shamir_tx::LockMode::Shared,
        )
        .await?;
        tx.record_locked_key(self.table_token(), key);
        Ok(())
    }

    /// Acquire a Level-3 `Exclusive` (write) lock on `key` for a Pessimistic
    /// tx. Same contract as [`acquire_pessimistic_read_lock`] but exclusive.
    pub async fn acquire_pessimistic_write_lock(
        &self,
        key: RecordKey,
        tx: &shamir_tx::TxContext,
    ) -> DbResult<()> {
        if tx.isolation != shamir_tx::IsolationLevel::Pessimistic {
            return Ok(());
        }
        let mvcc = self.mvcc_store.as_ref().ok_or_else(|| {
            DbError::Conflict(format!(
                "Level-3 (Pessimistic) op on non-MVCC table '{}': no mvcc_store",
                self.name
            ))
        })?;
        mvcc.lock_key(
            key.clone(),
            tx.tx_id.0,
            tx.wounded_flag(),
            tx.wound_notify(),
            shamir_tx::LockMode::Exclusive,
        )
        .await?;
        tx.record_locked_key(self.table_token(), key);
        Ok(())
    }
}
