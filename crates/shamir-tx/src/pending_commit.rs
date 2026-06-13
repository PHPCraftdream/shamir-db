//! Group-commit scaffolding — `PendingCommit` queued for batch materialisation.

use std::collections::HashSet;

use bytes::Bytes;
use shamir_collections::THasher;
use shamir_storage::error::DbError;
use tokio::sync::oneshot;

use crate::TxContext;

/// A transaction waiting to be committed as part of a group-commit batch.
///
/// The leader drains these from `RepoTxGate::pending_commits`, checks for
/// write-set conflicts, and materialises the entire batch under a single
/// WAL fsync.
#[allow(dead_code)]
pub struct PendingCommit {
    pub tx: TxContext,
    pub write_set_keys: HashSet<(u64, Bytes), THasher>,
    pub result_tx: oneshot::Sender<Result<u64, DbError>>,
}

#[allow(dead_code)]
impl PendingCommit {
    pub fn new(
        tx: TxContext,
        write_set_keys: HashSet<(u64, Bytes), THasher>,
        result_tx: oneshot::Sender<Result<u64, DbError>>,
    ) -> Self {
        Self {
            tx,
            write_set_keys,
            result_tx,
        }
    }
}
