//! Engine-owned sparse coverage populated by the detached sync worker.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use dashmap::DashMap;
use shamir_collections::THasher;
use shamir_wal::wal_group_commit::WalCoverage;

/// Sparse slots above R. Zero generation means individually settled/covered;
/// positive generation means written, awaiting successful sync of that batch.
/// Allocation is proportional to outstanding versions (not hard bounded).
/// Scans occur at sync, never at the commit policy seam.
///
/// A sharded `DashMap` (point lookups only): an `scc::TreeIndex` here
/// livelocked a tokio worker in a leaf-lock spin after remove-during-iterate
/// compaction (CI thread dump, 100% CPU, same frame in every sample). No
/// `Ref` is ever held across another map call or an `.await`.
pub(crate) struct RecoverabilityCoverage {
    memory: AtomicBool,
    prefix: AtomicU64,
    generation: AtomicU64,
    synced_generation: AtomicU64,
    slots: DashMap<u64, u64, THasher>,
}

impl RecoverabilityCoverage {
    /// Initial is a recovery-certified floor, never a live allocation maximum.
    pub(crate) fn new(initial: u64) -> Self {
        Self {
            memory: AtomicBool::new(false),
            prefix: AtomicU64::new(initial),
            generation: AtomicU64::new(0),
            synced_generation: AtomicU64::new(0),
            slots: DashMap::with_hasher(THasher::default()),
        }
    }

    fn advance(&self) {
        loop {
            let current = self.prefix.load(Ordering::Acquire);
            let Some(next) = current.checked_add(1) else {
                return;
            };
            // Copy the generation out: the `Ref` (shard read lock) dies here.
            let slot = self.slots.get(&next).map(|slot| *slot);
            let covered = slot.is_some_and(|generation| {
                generation == 0 || generation <= self.synced_generation.load(Ordering::Acquire)
            });
            if !covered {
                return;
            }
            if self
                .prefix
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.slots.remove(&next);
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
                // Keep an existing slot (insert-if-absent semantics).
                self.slots
                    .entry(version)
                    .or_insert(if memory { 0 } else { generation });
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
        let prefix = self.prefix.load(Ordering::Acquire);
        self.slots.retain(|version, _| *version > prefix);
    }
    fn settle(&self, version: u64) {
        if version <= self.prefix.load(Ordering::Acquire) {
            return;
        }
        // Existing written slot must become settled as well (idempotent).
        self.slots.insert(version, 0);
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
