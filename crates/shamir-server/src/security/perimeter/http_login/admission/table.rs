use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{PrefixKey, Source};

const BUCKETS: usize = 4096;
const CHILD_QUOTA: usize = 32;
const SCALE: u64 = 1_000_000;
/// Maximum unique buckets one table may pin at once (3072 of 4096).
const PIN_BUDGET: usize = 3_072;

#[derive(Clone, Copy)]
struct Bucket {
    key: PrefixKey,
    parent: PrefixKey,
    parent_slot: u16,
    tokens: u64,
    updated: Instant,
    touched: u64,
}
struct Slot {
    bucket: Option<Bucket>,
    pins: Arc<AtomicUsize>,
}

/// Bounded hierarchical login token buckets.
pub struct LoginBudgetTable {
    slots: Box<[Slot]>,
    pinned_buckets: Arc<AtomicUsize>,
    period_ns: u128,
    tick: u64,
    #[cfg(test)]
    slot_visits: AtomicUsize,
}

/// Reason a new source could not obtain a resident bucket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetDropReason {
    /// All resident buckets were pinned or had outstanding descendants.
    NoEvictableBucket,
}
/// Result of a successful debit or bounded-accounting drop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebitOutcome {
    /// Every applicable distinct tier was debited.
    Charged,
    /// This source was omitted rather than sharing another source's bucket.
    Dropped(BudgetDropReason),
}
/// Rate-limit rejection with a bounded retry delay in seconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BudgetDenied {
    /// Retry delay, clamped to the inclusive range 1 through 3600 seconds.
    pub retry_after_seconds: u32,
}
/// Pins a source hierarchy against safe eviction until dropped.
pub struct BudgetPin {
    markers: [Option<Arc<AtomicUsize>>; 3],
    budget: Arc<AtomicUsize>,
}
impl Clone for BudgetPin {
    fn clone(&self) -> Self {
        let markers = std::array::from_fn(|i| self.markers[i].clone());
        // A clone never changes the 0/1 bucket state: the original keeps every
        // marker above zero, so no budget unit is reserved or released.
        for marker in markers.iter().flatten() {
            marker.fetch_add(1, Ordering::AcqRel);
        }
        Self {
            markers,
            budget: Arc::clone(&self.budget),
        }
    }
}
impl Drop for BudgetPin {
    fn drop(&mut self) {
        for marker in self.markers.iter().flatten() {
            unpin_marker(marker, &self.budget);
        }
    }
}

impl LoginBudgetTable {
    /// Allocates all bucket and pin-marker storage up front.
    pub fn new(period: Duration) -> Self {
        let slots = (0..BUCKETS)
            .map(|_| Slot {
                bucket: None,
                pins: Arc::new(AtomicUsize::new(0)),
            })
            .collect();
        Self {
            slots,
            pinned_buckets: Arc::new(AtomicUsize::new(0)),
            period_ns: period.as_nanos().max(1),
            tick: 0,
            #[cfg(test)]
            slot_visits: AtomicUsize::new(0),
        }
    }
    /// Atomically debits the distinct source tiers at the supplied instant.
    pub fn debit(&mut self, source: Source, now: Instant) -> Result<DebitOutcome, BudgetDenied> {
        let (tiers, count) = distinct_tiers(source);
        let mut indices = [usize::MAX; 3];
        for n in (0..count).rev() {
            let key = tiers[n];
            if let Some(idx) = self.find(key) {
                indices[n] = idx;
                continue;
            }
            // An over-quota child charges its existing principal bucket.
            let root = source.principal();
            let child_prefix = match (key.family(), key.prefix()) {
                (4, 24) => 24,
                (6, 56) => 56,
                (6, 64) => 64,
                _ => 0,
            };
            let quota = child_prefix != 0 && self.child_count(root, child_prefix) >= CHILD_QUOTA;
            if quota {
                if let Some(idx) = self.find(root) {
                    indices[n] = idx;
                }
                continue;
            }
            let parent = if key == root { key } else { root };
            match self.obtain(
                key,
                parent,
                now,
                &indices,
                if key == root {
                    None
                } else {
                    Some(indices[count - 1])
                },
            ) {
                Some(idx) => indices[n] = idx,
                None => return Ok(DebitOutcome::Dropped(BudgetDropReason::NoEvictableBucket)),
            }
        }
        let mut denied = false;
        let mut retry_ns = 0u128;
        let mut unique = [usize::MAX; 3];
        let mut unique_count = 0;
        for idx in indices {
            if idx != usize::MAX && !unique[..unique_count].contains(&idx) {
                unique[unique_count] = idx;
                unique_count += 1;
            }
        }
        for idx in unique.into_iter().take(unique_count) {
            let key = self.slots[idx].bucket.expect("resident bucket").key;
            self.refill(idx, now, capacity(key));
            let bucket = self.slots[idx].bucket.as_ref().expect("resident bucket");
            if bucket.tokens < SCALE {
                denied = true;
                let missing = u128::from(SCALE - bucket.tokens);
                let wait = missing
                    .saturating_mul(self.period_ns)
                    .div_ceil(u128::from(capacity(key)) * u128::from(SCALE));
                retry_ns = retry_ns.max(wait);
            }
        }
        if denied {
            return Err(BudgetDenied {
                retry_after_seconds: retry_ns.div_ceil(1_000_000_000).clamp(1, 3600) as u32,
            });
        }
        for idx in unique.into_iter().take(unique_count) {
            if let Some(bucket) = self.slots[idx].bucket.as_mut() {
                bucket.tokens -= SCALE;
            }
            self.touch(idx);
        }
        Ok(DebitOutcome::Charged)
    }
    /// Pins all currently resident levels for a source hierarchy, or fails closed
    /// when the table's unique pinned-bucket budget is exhausted.
    pub fn pin(&self, source: Source) -> Option<BudgetPin> {
        let (tiers, count) = distinct_tiers(source);
        let mut pin = BudgetPin {
            markers: std::array::from_fn(|_| None),
            budget: Arc::clone(&self.pinned_buckets),
        };
        for (level, tier) in tiers.iter().enumerate().take(count) {
            if let Some(idx) = self.find(*tier) {
                let marker = Arc::clone(&self.slots[idx].pins);
                if !try_pin_marker(&marker, &self.pinned_buckets) {
                    // Refused halfway: Drop releases every marker raised above.
                    return None;
                }
                pin.markers[level] = Some(marker);
            }
        }
        Some(pin)
    }
    #[cfg(test)]
    pub(super) fn bucket_size() -> usize {
        std::mem::size_of::<Slot>() + 24
    }
    #[cfg(test)]
    pub(super) fn resident_pin_count(&self, source: Source) -> [usize; 3] {
        let (tiers, count) = distinct_tiers(source);
        let mut counts = [0; 3];
        for (index, key) in tiers.into_iter().take(count).enumerate() {
            if let Some(slot) = self
                .slots
                .iter()
                .find(|slot| slot.bucket.is_some_and(|bucket| bucket.key == key))
            {
                counts[index] = slot.pins.load(Ordering::Acquire);
            }
        }
        counts
    }
    #[cfg(test)]
    pub(super) fn reset_slot_visits(&self) {
        self.slot_visits.store(0, Ordering::Release);
    }
    #[cfg(test)]
    pub(super) fn slot_visits(&self) -> usize {
        self.slot_visits.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub(super) fn occupied_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.bucket.is_some()).count()
    }
    #[cfg(test)]
    pub(super) fn pinned_bucket_count(&self) -> usize {
        self.pinned_buckets.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub(super) fn pin_root_for_test(&self, source: Source) -> bool {
        let (tiers, _) = distinct_tiers(source);
        self.find(tiers[0])
            .is_some_and(|idx| try_pin_marker(&self.slots[idx].pins, &self.pinned_buckets))
    }
    #[inline]
    fn slot_visit(&self) {
        #[cfg(test)]
        self.slot_visits.fetch_add(1, Ordering::Relaxed);
    }
    fn find(&self, key: PrefixKey) -> Option<usize> {
        self.slots
            .iter()
            .inspect(|_| self.slot_visit())
            .position(|s| s.bucket.is_some_and(|b| b.key == key))
    }
    fn child_count(&self, root: PrefixKey, prefix: u8) -> usize {
        self.slots
            .iter()
            .inspect(|_| self.slot_visit())
            .filter(|s| {
                s.bucket
                    .is_some_and(|b| b.parent == root && b.key.prefix() == prefix && b.key != root)
            })
            .count()
    }
    fn obtain(
        &mut self,
        key: PrefixKey,
        parent: PrefixKey,
        now: Instant,
        protected: &[usize; 3],
        parent_slot: Option<usize>,
    ) -> Option<usize> {
        if let Some(idx) = self
            .slots
            .iter()
            .inspect(|_| self.slot_visit())
            .position(|s| s.bucket.is_none())
        {
            self.tick = self.tick.wrapping_add(1);
            self.slots[idx].bucket = Some(Bucket {
                key,
                parent,
                parent_slot: parent_slot.unwrap_or(idx) as u16,
                tokens: u64::from(capacity(key)) * SCALE,
                updated: now,
                touched: self.tick,
            });
            return Some(idx);
        }
        let mut has_child = [false; BUCKETS];
        let mut held_child = [false; BUCKETS];
        for (child_idx, slot) in self.slots.iter().enumerate() {
            self.slot_visit();
            if let Some(child) = slot.bucket {
                if child.parent_slot as usize != child_idx {
                    let root_idx = child.parent_slot as usize;
                    has_child[root_idx] = true;
                    if protected.contains(&child_idx) || slot.pins.load(Ordering::Acquire) != 0 {
                        held_child[root_idx] = true;
                    }
                }
            }
        }
        let mut last_candidate: Option<usize> = None;
        for i in 0..self.slots.len() {
            self.slot_visit();
            let Some(bucket) = self.slots[i].bucket else {
                continue;
            };
            if protected.contains(&i) || self.slots[i].pins.load(Ordering::Acquire) != 0 {
                continue;
            }
            self.refill(i, now, capacity(bucket.key));
            if self.slots[i].pins.load(Ordering::Acquire) != 0 {
                continue;
            }
            if bucket.key == bucket.parent && has_child[i] && held_child[i] {
                continue;
            }
            let fullness = self.slots[i].bucket.expect("resident bucket").tokens;
            let cap = u128::from(capacity(bucket.key)) * u128::from(SCALE);
            let full = fullness == cap as u64;
            let better = match last_candidate {
                None => true,
                Some(best) => {
                    let old = self.slots[best].bucket.expect("resident bucket");
                    let old_cap = u128::from(capacity(old.key)) * u128::from(SCALE);
                    let old_full = u128::from(old.tokens) == old_cap;
                    if full != old_full {
                        full
                    } else if old_full {
                        self.slots[i].bucket.unwrap().touched < old.touched
                    } else {
                        let left = u128::from(fullness) * old_cap;
                        let right = u128::from(old.tokens) * cap;
                        left > right
                            || (left == right
                                && self.slots[i].bucket.unwrap().touched < old.touched)
                    }
                }
            };
            if better {
                last_candidate = Some(i);
            }
        }
        let idx = last_candidate?;
        let old = self.slots[idx].bucket.expect("resident bucket");
        if self.slots[idx].pins.load(Ordering::Acquire) != 0
            || (old.key == old.parent && has_child[idx] && held_child[idx])
        {
            return None;
        }
        if old.key != old.parent {
            let parent_idx = old.parent_slot as usize;
            self.refill(parent_idx, now, capacity(old.parent));
            let debt = u64::from(capacity(old.key)) * SCALE - old.tokens;
            let parent_bucket = self.slots[parent_idx]
                .bucket
                .as_mut()
                .expect("resident parent");
            parent_bucket.tokens = parent_bucket.tokens.saturating_sub(debt);
        } else {
            #[cfg(test)]
            let slot_visits = &self.slot_visits;
            for slot in self.slots.iter_mut() {
                #[cfg(test)]
                slot_visits.fetch_add(1, Ordering::Relaxed);
                if slot
                    .bucket
                    .is_some_and(|b| b.parent_slot as usize == idx && b.key != b.parent)
                {
                    slot.bucket = None;
                }
            }
        }
        self.tick = self.tick.wrapping_add(1);
        self.slots[idx].bucket = Some(Bucket {
            key,
            parent,
            parent_slot: parent_slot.unwrap_or(idx) as u16,
            tokens: u64::from(capacity(key)) * SCALE,
            updated: now,
            touched: self.tick,
        });
        Some(idx)
    }
    fn refill(&mut self, idx: usize, now: Instant, cap: u32) {
        let Some(b) = self.slots[idx].bucket.as_mut() else {
            return;
        };
        let elapsed = now.saturating_duration_since(b.updated).as_nanos();
        let added = elapsed
            .saturating_mul(u128::from(cap))
            .saturating_mul(u128::from(SCALE))
            .checked_div(self.period_ns)
            .unwrap_or(u128::MAX);
        b.tokens = b
            .tokens
            .saturating_add(added.min(u128::from(u64::MAX)) as u64)
            .min(u64::from(cap) * SCALE);
        b.updated = now;
    }
    fn touch(&mut self, idx: usize) {
        self.tick = self.tick.wrapping_add(1);
        if let Some(bucket) = self.slots[idx].bucket.as_mut() {
            bucket.touched = self.tick;
        }
    }
}

fn distinct_tiers(source: Source) -> ([PrefixKey; 3], usize) {
    let tiers = source.tiers();
    if tiers[1] == tiers[2] {
        ([tiers[0], tiers[1], tiers[1]], 2)
    } else {
        (tiers, 3)
    }
}
/// Raises a bucket marker, reserving one budget unit on its 0-to-1 transition.
fn try_pin_marker(marker: &AtomicUsize, budget: &AtomicUsize) -> bool {
    loop {
        let current = marker.load(Ordering::Acquire);
        if current > 0 {
            // The unit of this bucket is already reserved by an existing pin.
            if marker
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
            continue;
        }
        // 0 -> 1: reserve the unit BEFORE publishing the marker.
        if budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                (u < PIN_BUDGET).then_some(u + 1)
            })
            .is_err()
        {
            return false;
        }
        if marker
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
        // Lost the race: give the unit back and retry.
        budget.fetch_sub(1, Ordering::AcqRel);
    }
}
/// Lowers a bucket marker, releasing the reserved unit on its 1-to-0 transition.
fn unpin_marker(marker: &AtomicUsize, budget: &AtomicUsize) {
    if marker.fetch_sub(1, Ordering::AcqRel) == 1 {
        budget.fetch_sub(1, Ordering::AcqRel);
    }
}
fn capacity(key: PrefixKey) -> u32 {
    match (key.family(), key.prefix()) {
        (4, 24) | (6, 64) => 10,
        (4, 16) => 160,
        (6, 56) => 40,
        (6, 48) => 160,
        _ => 10,
    }
}
