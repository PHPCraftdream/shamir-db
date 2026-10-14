use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(in crate::tx::drainer) struct Metrics {
    repaired: AtomicU64,
    gaps: AtomicU64,
}

impl Metrics {
    pub(in crate::tx::drainer) fn repaired(&self) {
        self.repaired.fetch_add(1, Ordering::Relaxed);
    }

    pub(in crate::tx::drainer) fn gap(&self) {
        self.gaps.fetch_add(1, Ordering::Relaxed);
    }

    pub(in crate::tx::drainer) fn snapshot(&self) -> (u64, u64) {
        (
            self.repaired.load(Ordering::Relaxed),
            self.gaps.load(Ordering::Relaxed),
        )
    }
}
