//! Engine-owned sparse coverage populated by the detached sync worker.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use scc::{Guard, TreeIndex};
use shamir_wal::wal_group_commit::WalCoverage;

/// Sparse slots above R. Zero generation means individually settled/covered;
/// positive generation means written, awaiting successful sync of that batch.
/// Allocation is proportional to outstanding versions (not hard bounded).
/// Scans occur at sync, never at the commit policy seam. No guard crosses await.
pub(crate) struct RecoverabilityCoverage {
    memory: AtomicBool,
    prefix: AtomicU64,
    generation: AtomicU64,
    synced_generation: AtomicU64,
    slots: TreeIndex<u64, AtomicU64>,
}

impl RecoverabilityCoverage {
    /// Initial is a recovery-certified floor, never a live allocation maximum.
    pub(crate) fn new(initial: u64) -> Self {
        Self {
            memory: AtomicBool::new(false),
            prefix: AtomicU64::new(initial),
            generation: AtomicU64::new(0),
            synced_generation: AtomicU64::new(0),
            slots: TreeIndex::new(),
        }
    }

    fn advance(&self) {
        loop {
            let current = self.prefix.load(Ordering::Acquire);
            let Some(next) = current.checked_add(1) else {
                return;
            };
            let covered = self
                .slots
                .read_sync(&next, |_, slot| {
                    let generation = slot.load(Ordering::Acquire);
                    generation == 0 || generation <= self.synced_generation.load(Ordering::Acquire)
                })
                .unwrap_or(false);
            if !covered {
                return;
            }
            if self
                .prefix
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.slots.remove_sync(&next);
            }
        }
    }
}

impl WalCoverage for RecoverabilityCoverage {
    fn enable_memory(&self) {
        self.memory.store(true, Ordering::Release);
    }
    fn memory_completed(&self, through: u64) {
        if self.memory.load(Ordering::Acquire) {
            // RAM completion proves all predecessors appended/materialized or
            // aborted; replicated application lives in the same RAM lifetime.
            self.prefix.fetch_max(through, Ordering::Release);
        }
    }
    fn written(&self, versions: &[u64], memory: bool) {
        // Only the sync worker invokes this. Publish generation AFTER all
        // exact slots: capture acquiring it cannot include a partial batch.
        let generation = self.generation.load(Ordering::Acquire) + 1;
        for &version in versions {
            if version > self.prefix.load(Ordering::Acquire) {
                let _ = self
                    .slots
                    .insert_sync(version, AtomicU64::new(if memory { 0 } else { generation }));
            }
        }
        self.generation.store(generation, Ordering::Release);
        self.advance();
    }
    fn capture(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    fn synced(&self, generation: u64) {
        self.synced_generation
            .fetch_max(generation, Ordering::AcqRel);
        // Successful sync covers generations, NOT all versions below max(W).
        self.advance();
        // Compact stale slots left by a concurrent duplicate publication.
        let guard = Guard::new();
        for (version, _) in self.slots.iter(&guard) {
            if *version <= self.prefix.load(Ordering::Acquire) {
                self.slots.remove_sync(version);
            }
        }
    }
    fn settle(&self, version: u64) {
        if version <= self.prefix.load(Ordering::Acquire) {
            return;
        }
        let _ = self.slots.insert_sync(version, AtomicU64::new(0));
        // Existing written slot must become settled as well (idempotent).
        self.slots.read_sync(&version, |_, slot| {
            slot.store(0, Ordering::Release);
        });
        self.advance();
    }
    fn certify_exclusive_open(&self, through: u64) {
        // No admission is allowed here. Recovery has durably absorbed surviving
        // data and certified unknown/aborted/truncated history as journal gaps.
        self.prefix.fetch_max(through, Ordering::AcqRel);
        self.advance();
    }
    fn recoverable_through(&self) -> u64 {
        self.prefix.load(Ordering::Acquire)
    }
}
