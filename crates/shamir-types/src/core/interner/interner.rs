use crate::types::common::{new_dash_map_wc, TDashMap};
use arc_swap::ArcSwap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::{InternerKey, TouchInd, UserKey};

/// A thread-safe, two-way map for interning strings into compact binary IDs.
///
/// **Reverse-lookup layout (Opt G):** the `id → UserKey` direction is
/// an `ArcSwap<Vec<Option<UserKey>>>`. Readers do a single atomic
/// load (no shared-lock acquire/release atomic-counter bouncing
/// across cores). The growing-vec semantics are preserved: on
/// insert we clone the current vec, append, and `store` the new
/// Arc. Writes are rare relative to reads (one insert per first
/// touch of a fresh string vs. many reads from filter/projection
/// hot paths), so the clone-and-swap cost is amortised heavily.
///
/// The forward direction (`UserKey → id`) stays a `TDashMap` —
/// it's sharded and already scales nearly linearly with thread
/// count.
#[derive(Debug)]
pub struct Interner {
    map_user_to_interned: TDashMap<UserKey, InternerKey>,
    /// Reverse direction — `vec[id as usize] = Some(UserKey)`. Indexed
    /// by raw `id`; entry `0` is always `None` (sentinel, ids start at 1).
    reverse: ArcSwap<Vec<Option<UserKey>>>,
    current_id: AtomicU64,
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

impl Interner {
    /// Creates a new, empty Interner.
    pub fn new() -> Interner {
        Interner {
            map_user_to_interned: new_dash_map_wc(64),
            reverse: ArcSwap::from_pointee(vec![None]), // index 0 reserved
            current_id: AtomicU64::new(0),
        }
    }

    /// Creates a new Interner from a pre-existing state.
    /// This is used to "hydrate" interner from a persistent store.
    pub fn with_state(initial_data: Vec<(InternerKey, UserKey)>) -> Self {
        if initial_data.is_empty() {
            return Self::new();
        }

        let map_user_to_interned = new_dash_map_wc(initial_data.len());
        let mut max_id: u64 = 0;
        for (interned_key, _) in &initial_data {
            let id = interned_key.id();
            if id > max_id {
                max_id = id;
            }
        }
        // +1 because vec is sized to hold index `max_id` (which is
        // the highest id assigned), plus the sentinel at 0.
        let mut reverse: Vec<Option<UserKey>> = vec![None; (max_id as usize) + 1];

        for (interned_key, user_key) in initial_data {
            let id = interned_key.id();
            map_user_to_interned.insert(user_key.clone(), interned_key);
            reverse[id as usize] = Some(user_key);
        }

        Interner {
            map_user_to_interned,
            reverse: ArcSwap::from_pointee(reverse),
            current_id: AtomicU64::new(max_id),
        }
    }

    /// Gets an ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, &'static str> {
        let s = str.as_ref();

        // Fast path: existing entry. `UserKey: Borrow<str>` lets the
        // DashMap lookup take a `&str` directly — no `String` alloc
        // on cache hits (the 99% case once the codec/query has warmed
        // up). Only the cold "first touch" path below allocates.
        if let Some(existing) = self.map_user_to_interned.get(s) {
            return Ok(TouchInd::Exists(existing.clone()));
        }

        let key = UserKey::from_str(s);

        // Reserve a fresh ID lock-free. If the forward-map CAS below
        // loses the race (Occupied branch), this slot is silently leaked
        // — the interner is monotonic and small leaks are harmless.
        let new_id = self.current_id.fetch_add(1, Ordering::Relaxed) + 1;
        let new_key = InternerKey::new(new_id);

        // CAS into forward map — another thread may have raced us.
        use dashmap::mapref::entry::Entry;
        match self.map_user_to_interned.entry(key.clone()) {
            Entry::Occupied(existing) => {
                // Race: another thread inserted between our get() and entry().
                // `new_id` is wasted (small leaked slot, harmless).
                Ok(TouchInd::Exists(existing.get().clone()))
            }
            Entry::Vacant(vacant) => {
                vacant.insert(new_key.clone());
                // CAS-loop: grow and populate the reverse vec without a mutex.
                // Multiple concurrent insertions each retry until their slot
                // lands, so no writer's update is lost.
                loop {
                    let cur = self.reverse.load_full();
                    let mut new_rev = (*cur).clone();
                    if (new_id as usize) >= new_rev.len() {
                        new_rev.resize((new_id as usize) + 1, None);
                    }
                    new_rev[new_id as usize] = Some(key.clone());
                    let prev = self.reverse.compare_and_swap(&cur, Arc::new(new_rev));
                    if Arc::ptr_eq(&prev, &cur) {
                        break;
                    }
                    // Another writer's swap landed first — reload and retry.
                }
                Ok(TouchInd::New(new_key))
            }
        }
    }

    /// Gets the user key corresponding to an interned key.
    ///
    /// **Hot path (Opt G):** one `ArcSwap::load` (single atomic
    /// load, no read-lock acquire/release) + bounds-check + clone.
    /// Scales linearly across cores under read-heavy load.
    #[inline]
    pub fn get_str(&self, id: &InternerKey) -> Option<UserKey> {
        let rev = self.reverse.load();
        let idx = id.id() as usize;
        rev.get(idx).and_then(|slot| slot.clone())
    }

    /// Snapshots the reverse-vec via a single `ArcSwap` load and
    /// returns the owning `Arc` so callers can do many lookups
    /// against the same slice without re-loading. Used by codecs
    /// that walk a value tree and resolve many keys against the
    /// interner in tight succession.
    pub fn reverse_snapshot(&self) -> Arc<Vec<Option<UserKey>>> {
        self.reverse.load_full()
    }

    #[inline]
    pub fn with_str<R>(&self, id: &InternerKey, f: impl FnOnce(&str) -> R) -> Option<R> {
        let rev = self.reverse.load();
        let idx = id.id() as usize;
        rev.get(idx)
            .and_then(|slot| slot.as_ref())
            .map(|key| f(key.as_str()))
    }

    /// Gets the interned key corresponding to a user key.
    /// Same Borrow<str> trick as `touch_ind` — no `String` alloc on
    /// the lookup; only the cache miss path would (and we just
    /// return None on miss anyway).
    pub fn get_ind<S: AsRef<str>>(&self, str: S) -> Option<InternerKey> {
        self.map_user_to_interned
            .get(str.as_ref())
            .map(|id| id.clone())
    }

    /// Returns the current number of interned keys.
    pub fn len(&self) -> usize {
        self.map_user_to_interned.len()
    }

    /// Returns true if the interner is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Create an InternedKey from a numeric ID.
    #[inline]
    pub fn make_key(&self, id: u64) -> InternerKey {
        InternerKey::new(id)
    }

    /// Returns all interned entries as (InternerKey, UserKey) pairs.
    pub fn all_entries(&self) -> Vec<(InternerKey, UserKey)> {
        let rev = self.reverse.load();
        rev.iter()
            .enumerate()
            .filter_map(|(idx, slot)| {
                slot.as_ref()
                    .map(|key| (InternerKey::new(idx as u64), key.clone()))
            })
            .collect()
    }

    /// Returns the slice of interned entries whose ids fall in
    /// `(start_exclusive .. end_inclusive]`. Used by the persistence
    /// layer to capture only the delta added since the last persist
    /// without cloning the whole reverse vec.
    ///
    /// Both bounds are interpreted as raw ids (1-based — slot 0 is the
    /// sentinel). `end_inclusive` is clamped to the current reverse-vec
    /// length so a stale `end` from a concurrent reader is safe.
    pub fn entries_in_id_range(
        &self,
        start_exclusive: usize,
        end_inclusive: usize,
    ) -> Vec<(InternerKey, UserKey)> {
        let rev = self.reverse.load();
        let lo = start_exclusive.saturating_add(1);
        let hi = end_inclusive.min(rev.len().saturating_sub(1));
        if lo > hi {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(hi + 1 - lo);
        for idx in lo..=hi {
            if let Some(Some(key)) = rev.get(idx) {
                out.push((InternerKey::new(idx as u64), key.clone()));
            }
        }
        out
    }

    /// Captures the delta of entries with id > `start_exclusive`,
    /// reading the reverse vec atomically. Returns `(entries,
    /// new_high_water)` where `new_high_water` is the highest
    /// *gap-free* contiguous id present in the reverse vec at capture
    /// time — the persistence layer uses this (NOT `len()`) as the
    /// new `last_persisted_len`, because under concurrent `touch_ind`
    /// the forward map's `len()` can outrun the reverse vec by a
    /// window. Using the reverse-vec high-water mark guarantees we
    /// never advance past unwritten entries.
    ///
    /// Gaps: a `Some(None)` slot (reserved-but-unswapped id, or a
    /// permanently leaked id) does **not** stop the scan — populated
    /// entries above the gap are still captured so they are not lost
    /// on restart. However, the high-water mark is frozen at the id
    /// just before the first gap, so the next `entries_after` call
    /// re-captures the gap slot once (if) it fills.
    pub fn entries_after(&self, start_exclusive: usize) -> (Vec<(InternerKey, UserKey)>, usize) {
        let rev = self.reverse.load();
        // `rev.len() - 1` is the highest id that has a slot. Some
        // slots in the captured range may still be `None` if we're
        // reading mid-insert from another thread — but those will
        // be picked up by the NEXT persist, since we don't advance
        // `last_persisted_len` past them.
        let hi_full = rev.len().saturating_sub(1);
        let lo = start_exclusive.saturating_add(1);
        if lo > hi_full {
            return (Vec::new(), start_exclusive);
        }
        let mut out = Vec::with_capacity(hi_full + 1 - lo);
        let mut new_high = start_exclusive;
        let mut gapped = false;
        for idx in lo..=hi_full {
            match rev.get(idx) {
                Some(Some(key)) => {
                    out.push((InternerKey::new(idx as u64), key.clone()));
                    // Only advance the high-water mark while the range is still
                    // gap-free; once a gap is seen we still capture present
                    // entries but must not claim to have persisted past the hole.
                    if !gapped {
                        new_high = idx;
                    }
                }
                Some(None) => {
                    // Reserved-but-unswapped (concurrent touch_ind) or a leaked
                    // id. Keep scanning so populated higher ids are still
                    // captured, but freeze new_high so the next persist
                    // re-captures this slot once (if) it fills.
                    gapped = true;
                }
                None => break, // past the end of the reverse vec
            }
        }
        (out, new_high)
    }
}
