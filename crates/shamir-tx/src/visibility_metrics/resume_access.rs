//! PR6b-2 receipt resume: re-own already-allocated versions without
//! allocating replacements.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::repo_tx_gate::RepoTxGate;
use crate::version_guard::VersionGuard;
use crate::TxId;

impl RepoTxGate {
    /// Opt this gate into receipt-native visibility bounds. Call before a
    /// receipt is materialized so cold reads cannot expose it before publish.
    pub fn enable_receipt_native_visibility(&self) {
        self.receipt_native.store(true, Ordering::Release);
    }

    /// Whether receipt-native visibility bounds are enabled for this gate.
    pub fn receipt_native_visibility_enabled(&self) -> bool {
        self.receipt_native.load(Ordering::Acquire)
    }

    /// Allocate a unique tx-id. Lock-free.
    pub fn fresh_tx_id(&self) -> TxId {
        TxId::new(self.next_tx_id.fetch_add(1, Ordering::Relaxed))
    }

    /// S5c-X PR6b-2 (cold-start restore): record that `version` was
    /// materialized into history WITHOUT going through
    /// [`assign_next_version`](Self::assign_next_version) (the follower
    /// receipt's direct `materialize_committed_to_history` call). Advances
    /// [`version_allocation_high_water_mark`](Self::version_allocation_high_water_mark)
    /// to at least `version` (monotonic `fetch_max` — never regresses a
    /// counter advanced by a concurrent allocator). Read paths use
    /// `watermark > floor` to detect the materialize→publish window and
    /// bound their cold history seeks at the floor; without this bump the
    /// window is undetectable when a store drains receipts directly on a
    /// gate whose allocator never armed a version guard.
    pub fn observe_materialized_version(&self, version: u64) {
        if self.receipt_native_visibility_enabled() {
            // AcqRel matches the allocator pair in `assign_next_version`: a
            // reader whose Acquire load observes the bumped watermark must also
            // observe the durable history writes sequenced before the bump.
            self.version_counter.fetch_max(version, Ordering::AcqRel);
        }
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
