//! Post-WAL orphan bookkeeping, unresolved-transfer handover, and write fencing.
use super::RepoTxGate;
use super::Unresolved;
use shamir_wal::WalEntryV2;
use std::sync::atomic::Ordering;
use std::sync::Arc;

impl RepoTxGate {
    pub fn orphaned_post_wal_versions(&self) -> Vec<u64> {
        let mut versions = Vec::new();
        self.orphaned_post_wal.iter_sync(|version, _| {
            versions.push(*version);
            true
        });
        versions.sort_unstable();
        versions
    }

    /// Retained replay entry, including frames outside the drainer's window.
    pub fn orphan_entry(&self, version: u64) -> Option<Arc<WalEntryV2>> {
        self.orphan_entries
            .read_sync(&version, |_, entry| Arc::clone(entry))
    }

    /// Ascending snapshot; concurrent transfers may appear on the next scan.
    pub fn unresolved_wal_versions(&self) -> Vec<u64> {
        let mut versions = Vec::new();
        self.wal_unresolved.iter_sync(|version, _| {
            versions.push(*version);
            true
        });
        versions.sort_unstable();
        versions
    }

    pub fn unresolved_wal(&self, version: u64) -> Option<Unresolved> {
        self.wal_unresolved
            .read_sync(&version, |_, value| value.clone())
    }

    pub fn register_written_orphan(&self, version: u64, entry: Arc<WalEntryV2>) {
        let _ = self.orphan_entries.insert_sync(version, entry);
        let _ = self.orphaned_post_wal.insert_sync(version, ());
        self.orphan_max.fetch_max(version, Ordering::AcqRel);
    }

    /// Transfer resolution ownership without settling either tracker.
    /// The caller must retain the ticket/entry until its physical fate is known.
    pub fn take_unresolved(&self, version: u64) -> Option<Unresolved> {
        self.wal_unresolved
            .remove_sync(&version)
            .map(|(_, value)| value)
    }

    /// Settle a version proven unwritten. Never use for PENDING or UNKNOWN.
    /// The resolver must first take ownership with `take_unresolved`.
    pub fn abort_unwritten(&self, version: u64) {
        self.completion
            .mark(version, crate::completion_tracker::State::Aborted);
        self.durable_completion
            .mark(version, crate::completion_tracker::State::Aborted);
        self.sync_last_committed_from_watermark();
    }

    /// Fail-stop signal only; admission wiring belongs to the later engine stage.
    pub fn set_requires_reopen(&self) {
        self.requires_reopen.store(true, Ordering::Release);
    }

    pub fn requires_reopen(&self) -> bool {
        self.requires_reopen.load(Ordering::Acquire)
    }

    /// Fail-stop: refuse new writes and require reopen.
    pub fn fence_writes(&self) {
        self.writes_fenced.store(true, Ordering::Release);
        self.set_requires_reopen();
    }

    pub fn writes_fenced(&self) -> bool {
        self.writes_fenced.load(Ordering::Acquire)
    }

    /// Highest version ever registered as a post-WAL orphan (zero if none).
    /// Monotone scan bound; forgetting an orphan does not lower it.
    pub fn orphan_max(&self) -> u64 {
        self.orphan_max.load(Ordering::Acquire)
    }

    /// Whether a live commit owns this WAL-accepted version's materialization.
    /// The drainer must stop at it rather than replaying concurrently.
    pub fn is_active_post_wal(&self, version: u64) -> bool {
        self.active_post_wal.contains_sync(&version)
    }

    /// Whether a dropped post-WAL guard transferred this version to replay.
    pub fn is_orphaned_post_wal(&self, version: u64) -> bool {
        self.orphaned_post_wal.contains_sync(&version)
    }

    /// Mark a registered orphan Materialized and advance visibility after
    /// replay succeeds. Does not mark durability or forget membership.
    pub fn complete_orphaned_post_wal(&self, version: u64) {
        if self.is_orphaned_post_wal(version) {
            self.completion
                .mark(version, crate::completion_tracker::State::Materialized);
            self.sync_last_committed_from_watermark();
        }
    }

    /// Remove settled orphan membership after replay and visibility completion.
    /// The drainer separately marks durability; orphan_max remains monotone.
    pub fn forget_orphaned_post_wal(&self, version: u64) {
        let _ = self.orphaned_post_wal.remove_sync(&version);
        let _ = self.orphan_entries.remove_sync(&version);
    }
}
