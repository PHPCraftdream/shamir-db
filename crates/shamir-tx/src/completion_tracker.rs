//! Completion tracker — lock-free watermark over versioned state transitions.
//!
//! Tracks whether each allocated MVCC version has been fully materialized or
//! aborted, and maintains a contiguous watermark: the highest V where all
//! versions ≤ V are complete (Materialized or Aborted).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::sync::{futures::Notified, Notify};

use shamir_collections::THasher;

/// Terminal state of a version slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Materialized,
    Aborted,
}

/// Lock-free completion tracker with a contiguous watermark.
pub struct CompletionTracker {
    /// Highest watermark already published — version V where ∀k≤V:
    /// state(k) ∈ {Materialized, Aborted}.
    watermark: AtomicU64,
    /// Sparse map of version → state for versions above the watermark.
    /// Versions ≤ watermark are forgotten (compaction).
    states: scc::HashMap<u64, State, THasher>,
    waiters: AtomicUsize,
    progress: Notify,
    visible_wait_slow_count: AtomicU64,
    #[cfg(test)]
    waiter_registrations: AtomicUsize,
}

/// Owns one registration, including while the caller is parked or cancelled.
pub(crate) struct CompletionWaitGuard<'a> {
    tracker: &'a CompletionTracker,
}

impl Drop for CompletionWaitGuard<'_> {
    fn drop(&mut self) {
        self.tracker.waiters.fetch_sub(1, Ordering::SeqCst);
    }
}

impl CompletionTracker {
    /// Create a new tracker with watermark at 0.
    pub fn new() -> Self {
        Self {
            watermark: AtomicU64::new(0),
            states: scc::HashMap::with_hasher(THasher::default()),
            waiters: AtomicUsize::new(0),
            progress: Notify::new(),
            visible_wait_slow_count: AtomicU64::new(0),
            #[cfg(test)]
            waiter_registrations: AtomicUsize::new(0),
        }
    }

    /// Create a tracker with the watermark pre-seeded to `initial`.
    /// Used on repo open when `last_committed` is recovered from durable state.
    pub fn with_watermark(initial: u64) -> Self {
        Self {
            watermark: AtomicU64::new(initial),
            states: scc::HashMap::with_hasher(THasher::default()),
            waiters: AtomicUsize::new(0),
            progress: Notify::new(),
            visible_wait_slow_count: AtomicU64::new(0),
            #[cfg(test)]
            waiter_registrations: AtomicUsize::new(0),
        }
    }

    /// Mark version V as Materialized or Aborted, then try to advance the
    /// watermark. Marking a version ≤ current watermark is a no-op.
    pub fn mark(&self, version: u64, state: State) {
        let current_wm = self.watermark.load(Ordering::Acquire);
        if version <= current_wm {
            return; // already compacted
        }
        let _ = self.states.insert_sync(version, state);
        self.try_advance();
    }

    /// Fixture-only settling. Preserve terminal states; pending slots become
    /// Materialized and missing slots Aborted. Normal advancement wakes waiters
    /// and compacts the prefix. Call only after fixture data is installed.
    pub(crate) fn settle_through(&self, version: u64) {
        let current = self.watermark();
        if version <= current {
            return;
        }
        for next in current + 1..=version {
            {
                match self.states.entry_sync(next) {
                    scc::hash_map::Entry::Occupied(mut entry) => {
                        if *entry.get() == State::Pending {
                            *entry.get_mut() = State::Materialized;
                        }
                    }
                    scc::hash_map::Entry::Vacant(entry) => {
                        entry.insert_entry(State::Aborted);
                    }
                }
            }
            self.try_advance();
        }
    }

    /// Current watermark value.
    pub fn watermark(&self) -> u64 {
        self.watermark.load(Ordering::Acquire)
    }

    pub(crate) fn register_waiter(&self) -> CompletionWaitGuard<'_> {
        self.waiters.fetch_add(1, Ordering::SeqCst);
        #[cfg(test)]
        self.waiter_registrations.fetch_add(1, Ordering::Relaxed);
        CompletionWaitGuard { tracker: self }
    }

    pub(crate) fn notified(&self) -> Notified<'_> {
        self.progress.notified()
    }

    /// The post-registration recheck participates in the same total order as
    /// registration, the advancing CAS, and the publisher's waiter load.
    pub(crate) fn registered_watermark(&self) -> u64 {
        self.watermark.load(Ordering::SeqCst)
    }

    pub(crate) fn record_slow_visible_wait(&self) {
        self.visible_wait_slow_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Number of visibility waits that reached the slow-warning threshold.
    pub fn visible_wait_slow_count(&self) -> u64 {
        self.visible_wait_slow_count.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn waiter_registrations(&self) -> usize {
        self.waiter_registrations.load(Ordering::Relaxed)
    }

    /// Current registered visibility waits, including cancellation-safe RAII owners.
    pub fn waiter_count(&self) -> usize {
        self.waiters.load(Ordering::SeqCst)
    }

    /// Best-effort counts in `(watermark, high_water]`: unsettled, explicit
    /// Pending, and missing slots. Scans only sparse entries, never H-W slots.
    /// Concurrent settlement/compaction can skew these telemetry-only counts.
    pub(crate) fn unsettled_counts(&self, watermark: u64, high_water: u64) -> (u64, u64, u64) {
        let span = high_water.saturating_sub(watermark);
        let mut terminal = 0_u64;
        let mut pending = 0_u64;
        self.states.iter_sync(|version, state| {
            if *version > watermark && *version <= high_water {
                match state {
                    State::Pending => pending = pending.saturating_add(1),
                    State::Materialized | State::Aborted => {
                        terminal = terminal.saturating_add(1);
                    }
                }
            }
            true
        });
        let unsettled = span.saturating_sub(terminal);
        let pending = pending.min(unsettled);
        (unsettled, pending, unsettled.saturating_sub(pending))
    }

    /// Try to advance the watermark by walking contiguous completed versions.
    fn try_advance(&self) {
        loop {
            let current = self.watermark.load(Ordering::Acquire);
            let next = current + 1;

            // Check if next version is completed.
            let completed = self.states.read_sync(&next, |_, s| {
                matches!(s, State::Materialized | State::Aborted)
            });

            match completed {
                Some(true) => {
                    // Try to CAS the watermark forward.
                    match self.watermark.compare_exchange_weak(
                        current,
                        next,
                        Ordering::SeqCst,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            // If registration preceded this load, broadcast to
                            // enabled waiters. Otherwise their SeqCst recheck
                            // observes this CAS; neither side can miss the other.
                            if self.waiters.load(Ordering::SeqCst) > 0 {
                                self.progress.notify_waiters();
                            }
                            // Successfully advanced — remove the entry.
                            let _ = self.states.remove_sync(&next);
                        }
                        Err(_) => {
                            // Another thread advanced; retry from top.
                            continue;
                        }
                    }
                }
                _ => break, // Pending or missing — stop.
            }
        }
    }
}

impl Default for CompletionTracker {
    fn default() -> Self {
        Self::new()
    }
}
