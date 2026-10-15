//! PR6b-2 receipt resume: re-own already-allocated versions without
//! allocating replacements.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::repo_tx_gate::RepoTxGate;
use crate::version_guard::VersionGuard;
use crate::TxId;

impl RepoTxGate {
    /// Allocate a unique tx-id. Lock-free.
    pub fn fresh_tx_id(&self) -> TxId {
        TxId::new(self.next_tx_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Construct a guard for an ALREADY-allocated `version` — the follower
    /// receipt resume path (S5c-X PR6b-2) re-owns the versions recorded in a
    /// durable receipt instead of allocating replacements. Obligation wiring is
    /// identical to [`Self::assign_next_version_guarded`]; only the allocation
    /// step is skipped.
    pub fn resume_version_guard(&self, version: u64) -> VersionGuard {
        VersionGuard::new(
            version,
            Arc::clone(&self.completion),
            Arc::clone(&self.durable_completion),
            Arc::clone(&self.last_committed_version),
            Arc::clone(&self.orphaned_post_wal),
            Arc::clone(&self.orphan_max),
            Arc::clone(&self.active_post_wal),
            Arc::clone(&self.orphan_entries),
            Arc::clone(&self.wal_unresolved),
            Arc::clone(&self.requires_reopen),
        )
    }
}
