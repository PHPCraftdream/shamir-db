use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Default)]
pub struct RetentionGate {
    refuse: AtomicBool,
    /// Refusal enforcement passes, not individual admissions.
    refusals_total: AtomicU64,
    declare_gaps_total: AtomicU64,
    retention_overflow_events_total: AtomicU64,
    checkpoint_failures_total: AtomicU64,
    checkpoints_total: AtomicU64,
}

impl RetentionGate {
    #[inline]
    pub fn refuse_writes(&self) -> bool {
        self.refuse.load(Ordering::Acquire)
    }

    pub(crate) fn record_refusal(&self) {
        self.refuse.store(true, Ordering::Release);
        self.refusals_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn clear_refuse_if_set(&self) {
        if self.refuse_writes() {
            self.refuse.store(false, Ordering::Release);
        }
    }

    pub(crate) fn record_overflow_event(&self) {
        self.retention_overflow_events_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_declare_gap(&self) {
        self.declare_gaps_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_checkpoint_success(&self) {
        self.checkpoints_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_checkpoint_failure(&self) {
        self.checkpoint_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn checkpoint_successes_total(&self) -> u64 {
        self.checkpoints_total.load(Ordering::Relaxed)
    }

    pub fn refusal_passes_total(&self) -> u64 {
        self.refusals_total.load(Ordering::Relaxed)
    }

    pub fn declare_gaps_total(&self) -> u64 {
        self.declare_gaps_total.load(Ordering::Relaxed)
    }

    pub fn retention_overflow_events_total(&self) -> u64 {
        self.retention_overflow_events_total.load(Ordering::Relaxed)
    }

    pub fn checkpoint_failures_total(&self) -> u64 {
        self.checkpoint_failures_total.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> [u64; 5] {
        [
            self.checkpoint_failures_total(),
            self.checkpoint_successes_total(),
            self.retention_overflow_events_total(),
            self.refusal_passes_total(),
            self.declare_gaps_total(),
        ]
    }
}
