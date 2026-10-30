//! Clone-shared posting cache with a bounded accounted resident footprint.
//!
//! The charge includes key bytes, the RecordId slice, and fixed map-entry and
//! Arc allocation metadata. Hash-table capacity, allocator overhead, and result
//! Arcs held by callers after eviction are deliberately excluded.

use std::mem::size_of;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;

pub(in crate::base_index) const POSTING_CACHE_CAP: usize = 512;
pub(in crate::base_index) const POSTING_CACHE_BUDGET: usize = 16 * 1024 * 1024;

struct Entry {
    generation: u64,
    values: Arc<[RecordId]>,
    charge: usize,
    used: Arc<AtomicUsize>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.used.fetch_sub(self.charge, Ordering::AcqRel);
    }
}

pub(in crate::base_index) struct PostingCache {
    entries: DashMap<Bytes, Entry, THasher>,
    used: Arc<AtomicUsize>,
}

impl PostingCache {
    pub(in crate::base_index) fn new() -> Self {
        Self {
            entries: DashMap::with_capacity_and_hasher(POSTING_CACHE_CAP, THasher::default()),
            used: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(in crate::base_index) fn get(&self, key: &Bytes) -> Option<(u64, Arc<[RecordId]>)> {
        self.entries
            .get(key)
            .map(|entry| (entry.generation, Arc::clone(&entry.values)))
    }

    // Eviction never holds an iterator shard guard when acquiring a write guard.
    fn evict_one(&self) -> bool {
        let key = self.entries.iter().next().map(|entry| entry.key().clone());
        key.is_some_and(|key| self.entries.remove(&key).is_some())
    }

    pub(in crate::base_index) fn insert(
        &self,
        key: Bytes,
        generation: u64,
        values: Arc<[RecordId]>,
    ) {
        let Some(charge) = values
            .len()
            .checked_mul(size_of::<RecordId>())
            .and_then(|n| n.checked_add(key.len()))
            .and_then(|n| {
                n.checked_add(size_of::<Bytes>() + size_of::<Entry>() + 2 * size_of::<usize>())
            })
        else {
            return;
        };
        if charge > POSTING_CACHE_BUDGET {
            return;
        }
        // Bounded work even when competing publishers keep consuming the freed
        // space. Reservations include unpublished entries, preventing overshoot.
        for _ in 0..=POSTING_CACHE_CAP {
            if self.entries.len() >= POSTING_CACHE_CAP {
                self.evict_one();
            }
            if self
                .used
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    used.checked_add(charge)
                        .filter(|total| *total <= POSTING_CACHE_BUDGET)
                })
                .is_ok()
            {
                // Entry owns the reservation before publication. Replacement,
                // invalidation, retain and unwinding all release it exactly once.
                let entry = Entry {
                    generation,
                    values,
                    charge,
                    used: Arc::clone(&self.used),
                };
                // Copy the logical key: a Bytes slice can otherwise retain an
                // arbitrarily larger backing allocation than the charged length.
                let key = Bytes::copy_from_slice(&key);
                self.entries.insert(key, entry);
                return;
            }
            if !self.evict_one() {
                return;
            }
        }
    }

    pub(in crate::base_index) fn remove(&self, key: &[u8]) {
        self.entries.remove(key);
    }

    pub(in crate::base_index) fn retain_prefix_excluding(&self, prefix: &[u8]) {
        self.entries.retain(|key, _| !key.starts_with(prefix));
    }

    #[cfg(test)]
    pub(in crate::base_index) fn accounted_bytes(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(in crate::base_index) fn len(&self) -> usize {
        self.entries.len()
    }
}
