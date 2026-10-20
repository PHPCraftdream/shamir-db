//! Group-commit scaffolding — `PendingCommit` queued for batch materialisation.

use bytes::Bytes;
use shamir_collections::TFxSet;
use shamir_storage::error::DbError;
use tokio::sync::oneshot;

use crate::TxContext;

/// A transaction waiting to be committed as part of a group-commit batch.
///
/// The leader drains these from `RepoTxGate::pending_commits`, checks for
/// write-set conflicts, and materialises the entire batch under a single
/// WAL fsync.
pub struct PendingCommit {
    pub tx: TxContext,
    pub write_set_keys: TFxSet<(u64, Bytes)>,
    /// Per-table unique-write-lock guards acquired in the pre-lock phase.
    pub uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    pub result_tx: oneshot::Sender<Result<u64, DbError>>,
}

impl PendingCommit {
    pub fn new(
        tx: TxContext,
        write_set_keys: TFxSet<(u64, Bytes)>,
        uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
        result_tx: oneshot::Sender<Result<u64, DbError>>,
    ) -> Self {
        Self {
            tx,
            write_set_keys,
            uwl_guards,
            result_tx,
        }
    }
}
