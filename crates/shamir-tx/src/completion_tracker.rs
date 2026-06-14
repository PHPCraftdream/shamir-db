//! Completion tracker — lock-free watermark over versioned state transitions.
//!
//! Tracks whether each allocated MVCC version has been fully materialized or
//! aborted, and maintains a contiguous watermark: the highest V where all
//! versions ≤ V are complete (Materialized or Aborted).

use std::sync::atomic::{AtomicU64, Ordering};

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
}

impl CompletionTracker {
    /// Create a new tracker with watermark at 0.
    pub fn new() -> Self {
        Self {
            watermark: AtomicU64::new(0),
            states: scc::HashMap::with_hasher(THasher::default()),
        }
    }

    /// Create a tracker with the watermark pre-seeded to `initial`.
    /// Used on repo open when `last_committed` is recovered from durable state.
    pub fn with_watermark(initial: u64) -> Self {
        Self {
            watermark: AtomicU64::new(initial),
            states: scc::HashMap::with_hasher(THasher::default()),
        }
    }

    /// Mark version V as Materialized or Aborted, then try to advance the
    /// watermark. Marking a version ≤ current watermark is a no-op.
    pub fn mark(&self, version: u64, state: State) {
        let current_wm = self.watermark.load(Ordering::Acquire);
        if version <= current_wm {
            return; // already compacted
        }
        let _ = self.states.insert(version, state);
        self.try_advance();
    }

    /// Current watermark value.
    pub fn watermark(&self) -> u64 {
        self.watermark.load(Ordering::Acquire)
    }

    /// Try to advance the watermark by walking contiguous completed versions.
    fn try_advance(&self) {
        loop {
            let current = self.watermark.load(Ordering::Acquire);
            let next = current + 1;

            // Check if next version is completed.
            let completed = self.states.read(&next, |_, s| {
                matches!(s, State::Materialized | State::Aborted)
            });

            match completed {
                Some(true) => {
                    // Try to CAS the watermark forward.
                    match self.watermark.compare_exchange_weak(
                        current,
                        next,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            // Successfully advanced — remove the entry.
                            let _ = self.states.remove(&next);
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
