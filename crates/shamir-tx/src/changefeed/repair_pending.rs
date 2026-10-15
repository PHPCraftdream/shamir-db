//! Repair-pending: deferred admission for changefeed journal events that hit
//! a full journal channel.
//!
//! ## Mechanism
//!
//! When [`RepoChangefeed`]'s bounded journal channel (`try_send`) returns
//! `Full`, the event is NOT dropped and NOT gapped: its `commit_version` is
//! recorded in a lock-free [`RepairPending`] set (`scc::HashMap` + atomic
//! depth/min mirrors), the event object itself stays retained by the engine
//! (the WAL entry it derives from is retained, so the event is re-derivable),
//! and the set's [`Notify`](tokio::sync::Notify) is woken. A background
//! repair drainer (built by the engine, not here) later re-enqueues the
//! event once the channel has room, then calls
//! [`RepairPending::on_admitted`] to retire the version.
//!
//! While any version is pending, the writer clamps every confirmation
//! barrier below the oldest pending version
//! ([`RepairPending::clamp_barrier`]), so no bounded reader can be
//! confirmed past the hole. The engine clamps its J-checkpoint ceiling the
//! same way, so I2 retention holds the WAL segment until the repaired event
//! is durably checkpointed.
//!
//! A GAP is still declared immediately only when admission is impossible in
//! principle: the writer is closed (`Closed`), the entry is not derivable
//! (corruption), or retention forces abandonment.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_collections::THasher;
use tokio::sync::mpsc;
use tokio::sync::Notify;

use super::{ChangelogEvent, JournalMsg, RepoChangefeed};

/// Versions whose changefeed events await channel re-admission, plus the
/// counters and wakeup signal the engine drainer coordinates through.
///
/// Lock-free throughout: `scc::HashMap` guarded only by synchronous scc calls
/// (no guard is ever held across an `.await`), with O(1)
/// atomic mirrors for depth and the oldest pending version.
#[derive(Debug)]
pub struct RepairPending {
    /// Pending commit versions. Insert-unique; duplicates are no-ops.
    map: scc::HashMap<u64, (), THasher>,
    /// O(1) mirror of `map` (scc `len()` is banned by clippy.toml).
    depth: AtomicUsize,
    /// Oldest pending version; `0` sentinel = empty set.
    min_version: AtomicU64,
    /// Total channel-full admissions recorded (deferred, not dropped).
    channel_full_total: AtomicU64,
    /// Total events successfully (re-)enqueued after deferral.
    repaired_total: AtomicU64,
    /// Total gap marks caused by a closed journal writer.
    gaps_writer_closed_total: AtomicU64,
    /// Bumped by the ENGINE drainer via [`RepairPending::bump_repair_mismatch`]
    /// when a repaired re-read of the retained WAL entry does not match the
    /// originally projected event (derivation mismatch — the retained entry
    /// was overwritten/evicted underneath the drainer).
    repair_mismatch_total: AtomicU64,
    /// Repair drainers wait here; the Full arm wakes them.
    wake: Notify,
    /// Test-only negative-control seam (SHAMIR tests elsewhere use env/hook
    /// seams): when set, `clamp_barrier` returns `w` unchanged AND the
    /// engine drainer's repair-pending truncation-ceiling clamp
    /// ([`RepoChangefeed::repair_ceiling_clamp`]) reports `None`. The engine
    /// drainer's tests set this via [`RepairPending::test_set_clamp_disabled`]
    /// to prove both clamps are load-bearing. Always compiled; zero cost until
    /// set.
    clamp_disabled: AtomicBool,
}

impl RepairPending {
    pub fn new() -> Self {
        Self {
            map: scc::HashMap::with_hasher(THasher::default()),
            depth: AtomicUsize::new(0),
            min_version: AtomicU64::new(0),
            channel_full_total: AtomicU64::new(0),
            repaired_total: AtomicU64::new(0),
            gaps_writer_closed_total: AtomicU64::new(0),
            repair_mismatch_total: AtomicU64::new(0),
            wake: Notify::new(),
            clamp_disabled: AtomicBool::new(false),
        }
    }

    /// Record a version whose event hit a full journal channel. Duplicate
    /// versions are ignored (the first pending record wins). Wakes the
    /// repair drainer.
    pub fn record_channel_full(&self, version: u64) {
        if self.map.insert_sync(version, ()).is_ok() {
            self.depth.fetch_add(1, Ordering::AcqRel);
            let mut cur = self.min_version.load(Ordering::Relaxed);
            while cur == 0 || version < cur {
                match self.min_version.compare_exchange_weak(
                    cur,
                    version,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(actual) => cur = actual,
                }
            }
        }
        self.channel_full_total.fetch_add(1, Ordering::Relaxed);
        self.wake.notify_one();
    }

    /// Retire a version after the engine drainer successfully (re-)enqueued
    /// its event. Bumps `repaired_total`; a no-op for unknown versions.
    pub fn on_admitted(&self, version: u64) {
        if self.map.remove_sync(&version).is_some() {
            self.depth.fetch_sub(1, Ordering::AcqRel);
            self.recompute_min();
        }
        self.repaired_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Retire a version whose gap was already declared because the journal
    /// writer is closed. No `repaired_total` bump — the event was not
    /// repaired, only bookkept.
    pub fn on_writer_closed(&self, version: u64) {
        self.gaps_writer_closed_total
            .fetch_add(1, Ordering::Relaxed);
        if self.map.remove_sync(&version).is_some() {
            self.depth.fetch_sub(1, Ordering::AcqRel);
            self.recompute_min();
        }
    }

    /// Oldest pending version, or `None` when the set is empty. O(1) atomic
    /// load; `0` sentinel maps to `None`.
    pub fn min_pending(&self) -> Option<u64> {
        match self.min_version.load(Ordering::Acquire) {
            0 => None,
            v => Some(v),
        }
    }

    /// Number of pending versions. O(1) atomic mirror load.
    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::Acquire)
    }

    /// All pending versions, unordered. Synchronous collect; callers are
    /// synchronous contexts only.
    pub fn snapshot(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.map.iter_sync(|k, _| {
            out.push(*k);
            true
        });
        out
    }

    /// Wake signal the engine repair drainer waits on.
    pub fn repair_notify(&self) -> &Notify {
        &self.wake
    }

    /// Whether the negative-control seam disabled the barrier clamp AND the
    /// engine's repair-pending truncation-ceiling clamp; test-only, zero cost
    /// until set.
    pub fn clamp_disabled(&self) -> bool {
        self.clamp_disabled.load(Ordering::Relaxed)
    }

    /// Clamp a confirmation barrier below the oldest pending version so no
    /// reader is confirmed across the pending hole.
    pub fn clamp_barrier(&self, w: u64) -> u64 {
        if self.clamp_disabled.load(Ordering::Relaxed) {
            return w;
        }
        self.min_pending().map_or(w, |m| w.min(m.saturating_sub(1)))
    }

    /// Total channel-full admissions recorded.
    pub fn channel_full_total(&self) -> u64 {
        self.channel_full_total.load(Ordering::Relaxed)
    }

    /// Total events repaired (successfully re-enqueued after deferral).
    pub fn repaired_total(&self) -> u64 {
        self.repaired_total.load(Ordering::Relaxed)
    }

    /// Total gap marks caused by a closed journal writer.
    pub fn gaps_writer_closed_total(&self) -> u64 {
        self.gaps_writer_closed_total.load(Ordering::Relaxed)
    }

    /// Total repair-derivation mismatches reported by the engine drainer.
    pub fn repair_mismatch_total(&self) -> u64 {
        self.repair_mismatch_total.load(Ordering::Relaxed)
    }

    /// Engine-side hook: the drainer bumps this when a repaired re-read of
    /// the retained WAL entry does not match the originally projected event.
    pub fn bump_repair_mismatch(&self) {
        self.repair_mismatch_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Recompute `min_version` from the map. Synchronous scc iteration.
    fn recompute_min(&self) {
        let mut m = 0u64;
        self.map.iter_sync(|k, _| {
            m = if m == 0 { *k } else { m.min(*k) };
            true
        });
        self.min_version.store(m, Ordering::Release);
    }

    /// TEST-ONLY negative-control seam: disables the barrier clamp
    /// ([`RepairPending::clamp_barrier`]) AND the engine drainer's
    /// truncation-ceiling clamp ([`RepoChangefeed::repair_ceiling_clamp`]) so
    /// engine tests can prove both clamps are load-bearing. Never call in
    /// production code paths.
    pub fn test_set_clamp_disabled(&self) {
        self.clamp_disabled.store(true, Ordering::Relaxed);
    }

    /// Retire a version whose repair is impossible — the retained WAL entry
    /// was overwritten/evicted or the entry is legacy not-derivable. Removes
    /// the version from the set WITHOUT bumping any counter: the engine
    /// drainer records the reason itself via its own metrics and declares
    /// the gap (`mark_gap`).
    ///
    /// The three retirement paths for a pending version are:
    /// - [`RepairPending::on_admitted`] — event successfully re-enqueued;
    /// - [`RepairPending::on_writer_closed`] — journal writer closed, gap
    ///   already declared at admission time;
    /// - [`RepairPending::on_unrepairable`] — WAL entry no longer derivable,
    ///   engine declares the gap and counts the reason itself.
    pub fn on_unrepairable(&self, version: u64) {
        if self.map.remove_sync(&version).is_some() {
            self.depth.fetch_sub(1, Ordering::AcqRel);
            self.recompute_min();
        }
    }
}

impl Default for RepairPending {
    fn default() -> Self {
        Self::new()
    }
}

/// `RepoChangefeed` repair-pending surface. Lives in this child module so the
/// repair plumbing can touch `RepoChangefeed`'s private fields without
/// growing the over-ratchet `changefeed.rs`.
impl RepoChangefeed {
    /// Number of live subscribers right now. Used by the commit-path to
    /// decide whether the live broadcast is worth attempting (the journal
    /// is fed unconditionally).
    pub fn subscriber_count(&self) -> usize {
        self.live.receiver_count()
    }

    /// Total confirmation timeouts, including channel admission timeouts.
    pub fn confirm_timeouts(&self) -> u64 {
        self.confirm_timeouts.load(Ordering::Relaxed)
    }

    pub fn clean_horizon(&self) -> u64 {
        self.clean_horizon.load(Ordering::Acquire)
    }

    /// Persisted/certified interval only; both endpoints from one RCU snapshot.
    pub fn certified_gap_interval(&self) -> Option<(u64, u64)> {
        *self.gap.persisted.load_full()
    }

    /// Wake the writer and ask it to drain + stop. Used at shutdown so the
    /// in-flight journal tail is flushed.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    /// Wake the writer to flush promptly (used by tests / explicit flush).
    pub fn flush_hint(&self) {
        self.notify.notify_one();
    }

    /// Handle one journal admission outcome. Returns `true` when the event
    /// was admitted to the channel; `false` on `Full` (recorded for repair)
    /// or `Closed` (gap marked).
    pub(super) fn journal_enqueue_impl(&self, event: &Arc<ChangelogEvent>) -> bool {
        match self
            .journal_tx
            .try_send(JournalMsg::Event(Arc::clone(event)))
        {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(JournalMsg::Event(ev))) => {
                let n = self.journal_dropped.fetch_add(1, Ordering::Relaxed) + 1;
                self.repair_pending.record_channel_full(ev.commit_version);
                log::warn!(
                    "changefeed journal channel full; deferring commit_version {} \
                     (total deferred: {n})",
                    ev.commit_version
                );
                false
            }
            Err(mpsc::error::TrySendError::Closed(JournalMsg::Event(ev))) => {
                self.mark_gap(ev.commit_version);
                self.repair_pending.on_writer_closed(ev.commit_version);
                false
            }
            Err(_) => unreachable!("journal_enqueue only sends events"),
        }
    }

    /// Enqueue without awaiting and report admission. Returns `true` when the
    /// event entered the journal channel; `false` on channel-full (the event
    /// was recorded in the repair-pending set for deferred re-enqueue) or on
    /// a closed writer (a gap was marked).
    pub fn journal_enqueue_tracked(&self, event: &Arc<ChangelogEvent>) -> bool {
        self.journal_enqueue_impl(event)
    }

    /// Repair-pending coordination surface for the engine drainer.
    pub fn repair_pending(&self) -> &Arc<RepairPending> {
        &self.repair_pending
    }

    /// Oldest version deferred by a full journal channel, if any.
    pub fn repair_pending_min(&self) -> Option<u64> {
        self.repair_pending.min_pending()
    }

    /// Truncation-ceiling clamp source for the drainer: the oldest pending
    /// version, unless the negative-control seam disabled the clamp.
    pub fn repair_ceiling_clamp(&self) -> Option<u64> {
        if self.repair_pending.clamp_disabled() {
            None
        } else {
            self.repair_pending.min_pending()
        }
    }

    /// Number of versions deferred by a full journal channel.
    pub fn repair_pending_depth(&self) -> usize {
        self.repair_pending.depth()
    }

    /// Engine drainer hook: report a repair-derivation mismatch (see
    /// [`RepairPending::bump_repair_mismatch`]).
    pub fn bump_repair_mismatch(&self) {
        self.repair_pending.bump_repair_mismatch();
    }
}
