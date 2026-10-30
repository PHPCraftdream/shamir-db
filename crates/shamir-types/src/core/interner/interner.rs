use crate::types::common::{new_dash_map_wc, TDashMap};
use arc_swap::ArcSwap;
use dashmap::mapref::entry::Entry;
#[cfg(test)]
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::TryLockError;
use std::sync::{Arc, OnceLock};

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_REVERSE_SET: Cell<bool> = const { Cell::new(false) };
}

use super::quota::{QuotaCounters, QuotaRejection};
use super::{InternerError, InternerKey, InternerQuota, TouchInd, UserKey};

/// A thread-safe, two-way map for interning strings into compact binary IDs.
///
/// **Reverse-lookup layout (Opt G + Op B + #501):** the `id → str`
/// direction is an `ArcSwap<Vec<OnceLock<Arc<str>>>>`. Readers do a
/// single atomic load (no shared-lock acquire/release atomic-counter
/// bouncing across cores) then read the leaf `OnceLock` — a flat,
/// single-index (`rev[id]`) lookup, unchanged from the previous
/// `Option<Arc<str>>` shape. **The read path never takes a lock of any
/// kind** — this property is unaffected by the rest of this note.
///
/// **#501 (doubling growth + single-writer critical section):** the
/// reverse spine no longer clones the WHOLE vec on every first-touch.
/// Growth is *doubling* (only when `id >= cur.len()`, cloning the
/// current vec into a bigger one), so total clone work across N
/// first-touches is a geometric series ≈ 2N — **O(N)**, not the old
/// **O(N²)**. Once capacity exists, landing a value is a single
/// `OnceLock::set` with no clone at all.
///
/// A first attempt at this made growth-and-set fully lock-free via a
/// two-phase "ensure capacity, then set, then confirm-and-retry-if-
/// orphaned" CAS protocol — it had a genuine, reproducible data-loss
/// race: a grower's clone-forward reads each cell one at a time (a
/// non-atomic window over N cells), so a concurrent writer's in-place
/// `set` on the SAME still-live vec can land, pass its own "am I still
/// live" confirmation check (because the grow's swap hadn't happened
/// *yet*), and then be silently dropped moments later when the grow's
/// swap lands using a clone that was read *before* that `set`. Making
/// this airtight lock-free needs a seqlock-style generation counter
/// around every grow; instead, `reverse_write_lock` serializes ALL
/// reverse-spine WRITES (first-touch population, both from `touch_ind`
/// and `touch_with_id`) behind a single, rarely-contended
/// `std::sync::Mutex` — a low-frequency/setup-only exception, per this
/// repo's own concurrency ideology (writes here happen once per
/// distinct field NAME ever seen, never on the read-hot decode path).
/// The single-writer invariant eliminates the race class outright: with
/// no concurrent mutation of `reverse` possible while the lock is held,
/// ensure-capacity + set is one atomic step, no CAS-retry/confirm dance
/// needed. Every read (`get_str`/`with_str`/`reverse_snapshot`/
/// `all_entries`/`entries_after`/`entries_in_id_range`) never touches
/// this lock — reads stay 100% lock-free via `ArcSwap::load`,
/// unaffected by write contention.
///
/// **Op B (Arc<str> spine):** each slot holds `Arc<str>` rather than
/// an owned `UserKey(String)`; capacity-ensure clones bump the Arc
/// refcount — O(1) per slot — rather than copying owned bytes.
///
/// The forward direction (`UserKey → id`) stays a `TDashMap` —
/// it's sharded and already scales nearly linearly with thread
/// count. Every forward write occurs only after the reverse slot is set,
/// under the same writer mutex used by automatic and explicit IDs.
#[derive(Debug)]
pub struct Interner {
    map_user_to_interned: TDashMap<UserKey, InternerKey>,
    /// Reverse direction — `vec[id as usize]` holds an `Arc<str>` once
    /// set. Indexed by raw `id`; entry `0` is always an unset
    /// `OnceLock` (sentinel, ids start at 1). An unset slot reads as
    /// `None` via `.get()` — this covers both "reserved-but-unswapped"
    /// and "grown-but-not-yet-filled" ids with the same collapsed
    /// semantics the previous `Option<Arc<str>>` design had (a
    /// resized-but-unset slot was plain `None` there too).
    ///
    /// #501: doubling-growth capacity-ensure + `OnceLock::set` so a
    /// first-touch that finds capacity already present clones nothing.
    /// All MUTATION of this field is serialized by `reverse_write_lock`;
    /// reads never touch that lock.
    reverse: ArcSwap<Vec<OnceLock<Arc<str>>>>,
    /// #501: serializes reverse-spine WRITES only (first-touch
    /// `touch_ind` and `touch_with_id`, including growth and forward-map
    /// insertion so readers cannot observe a partially published pair.
    /// See the struct doc for why a single-writer critical section
    /// replaced an earlier fully-lock-free attempt that had a data-loss
    /// race. Never taken by any read path.
    reverse_write_lock: std::sync::Mutex<()>,
    current_id: AtomicU64,
    published_generation: AtomicU64,
    quota: Option<InternerQuota>,
    /// Repo label threaded into `InternerError::QuotaExceeded` so the
    /// message names the repository (empty for scratch/new interners).
    quota_label: String,
    counters: QuotaCounters,
    #[cfg(test)]
    all_entries_calls: std::sync::atomic::AtomicUsize,
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
            // index 0 reserved (sentinel) — a single unset OnceLock.
            reverse: ArcSwap::from_pointee(vec![OnceLock::new()]),
            reverse_write_lock: std::sync::Mutex::new(()),
            current_id: AtomicU64::new(0),
            published_generation: AtomicU64::new(0),
            quota: None,
            quota_label: String::new(),
            counters: QuotaCounters::new(0, 0),
            #[cfg(test)]
            all_entries_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Creates a new Interner from a pre-existing state.
    /// This is used to "hydrate" interner from a persistent store.
    pub fn with_state(initial_data: Vec<(InternerKey, UserKey)>) -> Result<Self, InternerError> {
        let mut ids = shamir_collections::TFxMap::default();
        let mut names = shamir_collections::TFxMap::default();
        for (key, name) in &initial_data {
            let id = key.id();
            if id == 0 {
                return Err(InternerError::ReservedId);
            }
            if let Some(existing) = ids.insert(id, name.as_str()) {
                if existing != name.as_str() {
                    return Err(InternerError::IdConflict {
                        id,
                        existing: existing.to_owned(),
                        requested: name.as_str().to_owned(),
                    });
                }
            }
            if let Some(existing) = names.insert(name.as_str(), id) {
                if existing != id {
                    return Err(InternerError::NameConflict {
                        name: name.as_str().to_owned(),
                        existing,
                        requested: id,
                    });
                }
            }
        }
        // Fx-hashed distinct-name view of the hydrated data — the quota
        // counters must reflect distinct names, not duplicate tuples.
        let names: shamir_collections::TFxSet<&str> =
            initial_data.iter().map(|(_, name)| name.as_str()).collect();
        let count = u64::try_from(names.len()).unwrap_or(u64::MAX);
        let published_generation = count;
        let bytes = names.iter().fold(0u64, |sum, name| {
            sum.saturating_add(u64::try_from(name.len()).unwrap_or(u64::MAX))
        });
        let map_user_to_interned = new_dash_map_wc(initial_data.len());
        let mut max_id: u64 = 0;
        for (interned_key, _) in &initial_data {
            let id = interned_key.id();
            if id > max_id {
                max_id = id;
            }
        }
        // +1 because vec is sized to hold index `max_id` (which is
        // the highest id assigned), plus the sentinel at 0. Unassigned
        // ids stay as unset `OnceLock`s (read as `None` — a gap).
        let size = usize::try_from(max_id)
            .ok()
            .and_then(|id| id.checked_add(1))
            .ok_or(InternerError::IdCollision { id: max_id })?;
        let mut reverse: Vec<OnceLock<Arc<str>>> = Vec::with_capacity(size);
        reverse.resize_with(size, OnceLock::new);

        for (interned_key, user_key) in initial_data {
            let id = interned_key.id();
            let arc: Arc<str> = Arc::from(user_key.as_str());
            map_user_to_interned.insert(user_key, interned_key);
            // Identical recaptures may already have populated this slot.
            let _ = reverse[id as usize].set(arc);
        }

        Ok(Interner {
            map_user_to_interned,
            reverse: ArcSwap::from_pointee(reverse),
            reverse_write_lock: std::sync::Mutex::new(()),
            current_id: AtomicU64::new(max_id),
            published_generation: AtomicU64::new(published_generation),
            quota: None,
            quota_label: String::new(),
            counters: QuotaCounters::new(count, bytes),
            #[cfg(test)]
            all_entries_calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Creates an interner with cumulative admission limits and hydrated counters.
    pub fn with_quota(
        initial_data: Vec<(InternerKey, UserKey)>,
        quota: InternerQuota,
    ) -> Result<Self, InternerError> {
        Self::with_labeled_quota(initial_data, quota, String::new())
    }

    /// [`Interner::with_quota`] with a repo label embedded in quota errors.
    pub fn with_labeled_quota(
        initial_data: Vec<(InternerKey, UserKey)>,
        quota: InternerQuota,
        label: String,
    ) -> Result<Self, InternerError> {
        let mut interner = Self::with_state(initial_data)?;
        interner.quota = Some(quota);
        interner.quota_label = label;
        Ok(interner)
    }

    /// Returns the admitted distinct-name and UTF-8 byte counters.
    pub fn quota_usage(&self) -> (u64, u64) {
        self.counters.snapshot()
    }

    /// Gets an ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, InternerError> {
        self.touch_ind_impl(str.as_ref(), true)
    }

    #[cfg(test)]
    pub(crate) fn touch_ind_with_test_hooks(
        &self,
        s: &str,
        after_reservation: impl FnOnce(),
        after_id_allocation: impl FnOnce(u64),
        between_publications: impl FnOnce(u64),
    ) -> Result<TouchInd, InternerError> {
        self.touch_ind_impl_with(
            s,
            true,
            after_reservation,
            after_id_allocation,
            between_publications,
        )
    }

    #[cfg(test)]
    pub(crate) fn touch_ind_with_test_hook(
        &self,
        s: &str,
        after_reservation: impl FnOnce(),
    ) -> Result<TouchInd, InternerError> {
        self.touch_ind_with_test_hooks(s, after_reservation, |_| {}, |_| {})
    }

    pub fn touch_ind_unmetered<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, InternerError> {
        self.touch_ind_impl(str.as_ref(), false)
    }

    fn touch_ind_impl(&self, s: &str, enforce_quota: bool) -> Result<TouchInd, InternerError> {
        self.touch_ind_impl_with(s, enforce_quota, || {}, |_| {}, |_| {})
    }

    fn touch_ind_impl_with(
        &self,
        s: &str,
        enforce_quota: bool,
        after_reservation: impl FnOnce(),
        after_id_allocation: impl FnOnce(u64),
        between_publications: impl FnOnce(u64),
    ) -> Result<TouchInd, InternerError> {
        // Fast path: existing entry. `UserKey: Borrow<str>` lets the
        // DashMap lookup take a `&str` directly — no `String` alloc
        // on cache hits (the 99% case once the codec/query has warmed
        // up). Only the cold "first touch" path below allocates.
        if let Some(existing) = self.map_user_to_interned.get(s) {
            return Ok(TouchInd::Exists(existing.clone()));
        }

        let bytes = u64::try_from(s.len()).unwrap_or(u64::MAX);
        // Reserve quota before allocating the key/value, and roll back that
        // reservation on every failure before the forward entry is inserted.
        if enforce_quota {
            if let Some(quota) = self.quota {
                self.counters
                    .reserve(quota, bytes)
                    .map_err(
                        |QuotaRejection { kind, limit }| InternerError::QuotaExceeded {
                            label: self.quota_label.clone(),
                            kind,
                            limit,
                        },
                    )?;
            } else {
                self.counters.count_unmetered(bytes);
            }
        } else {
            self.counters.count_unmetered(bytes);
        }

        // Test seam sits after admission but before ID allocation/map insertion.
        after_reservation();

        let _publication_guard = self
            .reverse_write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = UserKey::from_str(s);
        let arc: Arc<str> = Arc::from(s);
        // Automatic IDs and explicit IDs share the reverse-writer lock; publish
        // reverse first so forward readers cannot observe an unresolved ID.
        let Some(new_id) = self.current_id.load(Ordering::Relaxed).checked_add(1) else {
            self.counters.refund(bytes);
            return Err(InternerError::IdCollision { id: u64::MAX });
        };
        after_id_allocation(new_id);
        let new_key = InternerKey::new(new_id);
        match self.map_user_to_interned.entry(key) {
            Entry::Occupied(existing) => {
                self.counters.refund(bytes);
                Ok(TouchInd::Exists(existing.get().clone()))
            }
            Entry::Vacant(vacant) => {
                let Some(idx) = usize::try_from(new_id).ok() else {
                    self.counters.refund(bytes);
                    return Err(InternerError::IdCollision { id: new_id });
                };
                if !self.set_reverse_slot_locked(idx, arc) {
                    self.counters.refund(bytes);
                    return Err(InternerError::IdCollision { id: new_id });
                }
                between_publications(new_id);
                vacant.insert(new_key.clone());
                self.current_id.store(new_id, Ordering::Relaxed);
                self.published_generation.fetch_add(1, Ordering::Release);
                Ok(TouchInd::New(new_key))
            }
        }
    }

    /// Gets the user key corresponding to an interned key.
    ///
    /// **Hot path (Opt G):** one `ArcSwap::load` (single atomic
    /// load, no read-lock acquire/release) + bounds-check + clone.
    /// Scales linearly across cores under read-heavy load.
    ///
    /// Returns `Option<Arc<str>>` — callers that previously used
    /// `.as_str()` can dereference the Arc directly (`&*arc` or
    /// via `Deref`).
    #[inline]
    pub fn get_str(&self, id: &InternerKey) -> Option<Arc<str>> {
        let rev = self.reverse.load();
        let idx = id.id() as usize;
        rev.get(idx).and_then(|slot| slot.get().cloned())
    }

    #[cfg(test)]
    pub(crate) fn publication_lock_held_for_test(&self) -> bool {
        matches!(
            self.reverse_write_lock.try_lock(),
            Err(TryLockError::WouldBlock)
        )
    }

    #[cfg(test)]
    pub(crate) fn set_current_id_for_test(&self, value: u64) {
        self.current_id.store(value, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn fail_next_reverse_set_for_test() {
        FAIL_NEXT_REVERSE_SET.with(|fail| fail.set(true));
    }

    /// #501: populate the reverse spine. Both automatic and explicit ID
    /// publication are serialized by `reverse_write_lock`; conflicting IDs
    /// are rejected before the forward map is exposed.
    ///
    /// Holds `reverse_write_lock` for the whole ensure-capacity-then-set
    /// step — see the struct doc for why this replaced an earlier
    /// fully-lock-free two-phase attempt that had a data-loss race.
    /// Growth (rare — only when `idx >= cur.len()`) doubles capacity, so
    /// total clone work across N monotonic touches is a geometric
    /// series ≈ 2N — O(N), not the old O(N²). The common case (capacity
    /// already present) is a single `OnceLock::set`, no clone.
    fn set_reverse_slot_locked(&self, idx: usize, arc: Arc<str>) -> bool {
        #[cfg(test)]
        if FAIL_NEXT_REVERSE_SET.with(|fail| fail.replace(false)) {
            return false;
        }
        let cur = self.reverse.load_full();
        if idx < cur.len() {
            return cur[idx].set(arc).is_ok();
        }
        self.reverse
            .store(Arc::new(Self::grown_reverse(&cur, idx, arc)));
        true
    }

    /// Build a new reverse vec doubled in capacity to cover `idx`,
    /// carrying every already-set cell of `cur` forward, then setting
    /// `idx` to `arc`. Cells are built explicitly (not via
    /// `OnceLock: Clone`, stabilized late) so this doesn't depend on a
    /// specific MSRV.
    ///
    /// MUST be called only while holding `reverse_write_lock` — no
    /// concurrent mutation of `cur` is possible while iterating it here.
    fn grown_reverse(
        cur: &[OnceLock<Arc<str>>],
        idx: usize,
        arc: Arc<str>,
    ) -> Vec<OnceLock<Arc<str>>> {
        let new_len = (cur.len() * 2).max(idx + 1);
        let mut new_rev: Vec<OnceLock<Arc<str>>> = Vec::with_capacity(new_len);
        for old_cell in cur.iter() {
            let cell = OnceLock::new();
            if let Some(v) = old_cell.get() {
                let _ = cell.set(v.clone());
            }
            new_rev.push(cell);
        }
        new_rev.resize_with(new_len, OnceLock::new);
        let _ = new_rev[idx].set(arc);
        new_rev
    }

    /// Snapshots the reverse-vec via a single `ArcSwap` load and
    /// returns the owning `Arc` so callers can do many lookups
    /// against the same slice without re-loading. Used by codecs
    /// that walk a value tree and resolve many keys against the
    /// interner in tight succession.
    ///
    /// Op B: slot type changed to `Arc<str>` — no semantic change
    /// for presence checks; callers that read the string dereference
    /// the Arc directly. #501: slot cell is now `OnceLock<Arc<str>>` —
    /// callers read via `slot.get()` (an unset slot = an unresolved id).
    pub fn reverse_snapshot(&self) -> Arc<Vec<OnceLock<Arc<str>>>> {
        self.reverse.load_full()
    }

    #[inline]
    pub fn with_str<R>(&self, id: &InternerKey, f: impl FnOnce(&str) -> R) -> Option<R> {
        let rev = self.reverse.load();
        let idx = id.id() as usize;
        rev.get(idx).and_then(|slot| slot.get()).map(|arc| f(arc))
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

    /// Returns the published generation for cache invalidation. It advances
    /// exactly once after each successful new automatic or explicit ID is
    /// visible in both maps; rejected operations and existing-key lookups do
    /// not advance it. The generation is a change counter, not an entry count.
    #[inline]
    pub fn generation(&self) -> u64 {
        self.published_generation.load(Ordering::Acquire)
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

    #[cfg(test)]
    pub(crate) fn all_entries_call_count(&self) -> usize {
        self.all_entries_calls.load(Ordering::Relaxed)
    }

    /// Highest currently published reverse ID, or zero when empty.
    pub fn max_published_id(&self) -> u64 {
        self.reverse
            .load()
            .iter()
            .rposition(|slot| slot.get().is_some())
            .unwrap_or(0) as u64
    }

    /// Returns all interned entries as (InternerKey, UserKey) pairs.
    ///
    /// UserKey is reconstructed from the `Arc<str>` slot — this is a
    /// cold path (persistence / diagnostics), so the String allocation
    /// is acceptable.
    pub fn all_entries(&self) -> Vec<(InternerKey, UserKey)> {
        #[cfg(test)]
        self.all_entries_calls.fetch_add(1, Ordering::Relaxed);
        let rev = self.reverse.load();
        rev.iter()
            .enumerate()
            .filter_map(|(idx, slot)| {
                slot.get()
                    .map(|arc| (InternerKey::new(idx as u64), UserKey::from_str(&**arc)))
            })
            .collect()
    }

    /// Idempotently associate `name` with the exact `id`. Used by WAL recovery
    /// to replay interner deltas: each delta entry calls this so recovery
    /// rebuilds the same intern-id assignments durably present in past WAL
    /// records, even if the interner persist file was older.
    ///
    /// - If `name` is already mapped to `id`: no-op, Ok(()).
    /// - If `name` is mapped to a different id: Err (corrupt state).
    /// - If `id` is already used by a different name: Err (id collision).
    /// - Otherwise: insert atomically.
    pub fn touch_with_id(&self, name: &str, id: u64) -> Result<(), InternerError> {
        self.touch_with_id_impl(name, id)
    }

    fn touch_with_id_impl(&self, name: &str, id: u64) -> Result<(), InternerError> {
        self.touch_with_id_impl_with(name, id, |_| {})
    }

    #[cfg(test)]
    pub(crate) fn touch_with_id_with_test_hook(
        &self,
        name: &str,
        id: u64,
        hook: impl FnOnce(u64),
    ) -> Result<(), InternerError> {
        self.touch_with_id_impl_with(name, id, hook)
    }

    fn touch_with_id_impl_with(
        &self,
        name: &str,
        id: u64,
        between_publications: impl FnOnce(u64),
    ) -> Result<(), InternerError> {
        let _publication_guard = self
            .reverse_write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if id == 0 {
            return Err(InternerError::ReservedId);
        }

        let key = UserKey::from_str(name);

        if let Some(existing) = self.map_user_to_interned.get(name) {
            let existing_id = existing.id();
            return if existing_id == id {
                Ok(()) // idempotent
            } else {
                return Err(InternerError::NameConflict {
                    name: name.to_owned(),
                    existing: existing_id,
                    requested: id,
                });
            };
        }

        let Some(idx) = usize::try_from(id).ok() else {
            return Err(InternerError::IdCollision { id });
        };
        // Check reverse map for id collision before inserting.
        {
            let rev = self.reverse.load();
            if let Some(existing_arc) = rev.get(idx).and_then(|c| c.get()) {
                if &**existing_arc != name {
                    return Err(InternerError::IdConflict {
                        id,
                        existing: existing_arc.to_string(),
                        requested: name.to_owned(),
                    });
                }
                // Same name at same id — idempotent (shouldn't normally reach
                // here since forward map check above would catch it, but
                // defensive).
                return Ok(());
            }
        }

        let arc: Arc<str> = Arc::from(name);
        // CAS into forward map — another thread may have raced us.
        match self.map_user_to_interned.entry(key) {
            Entry::Occupied(existing) => {
                let existing_id = existing.get().id();
                if existing_id == id {
                    Ok(())
                } else {
                    Err(InternerError::NameConflict {
                        name: name.to_owned(),
                        existing: existing_id,
                        requested: id,
                    })
                }
            }
            Entry::Vacant(vacant) => {
                let name_bytes = u64::try_from(name.len()).unwrap_or(u64::MAX);
                let new_key = InternerKey::new(id);
                if !self.set_reverse_slot_locked(idx, arc) {
                    return Err(InternerError::IdCollision { id });
                }
                between_publications(id);
                vacant.insert(new_key);
                self.counters.count_unmetered(name_bytes);
                self.published_generation.fetch_add(1, Ordering::Release);
                loop {
                    let cur = self.current_id.load(Ordering::Relaxed);
                    if cur >= id {
                        break;
                    }
                    if self
                        .current_id
                        .compare_exchange_weak(cur, id, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                    {
                        break;
                    }
                }

                Ok(())
            }
        }
    }

    /// Returns the slice of interned entries whose ids fall in
    /// `(start_exclusive .. end_inclusive]`. Used by the persistence
    /// layer to capture only the delta added since the last persist
    /// without cloning the whole reverse vec.
    ///
    /// Both bounds are interpreted as raw ids (1-based — slot 0 is the
    /// sentinel). `end_inclusive` is clamped to the current reverse-vec
    /// length so a stale `end` from a concurrent reader is safe.
    ///
    /// UserKey is reconstructed from `Arc<str>` — cold path.
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
            if let Some(arc) = rev.get(idx).and_then(|c| c.get()) {
                out.push((InternerKey::new(idx as u64), UserKey::from_str(&**arc)));
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
    /// Gaps: an unset slot — `Some(cell)` where `cell.get()` is `None`
    /// (reserved-but-unswapped id mid-`touch_ind`, a permanently leaked
    /// id, OR trailing doubling-growth capacity that no id has filled
    /// yet) — does **not** stop the scan; populated entries above it are
    /// still captured so they are not lost on restart. However, the
    /// high-water mark is frozen at the id just before the first gap, so
    /// the next `entries_after` call re-captures the gap slot once (if)
    /// it fills. #501: with doubling growth the vec length now exceeds
    /// the highest touched id, so the tail is a run of unset cells — the
    /// freeze logic already handles them exactly like any other gap
    /// (they contribute no entries and never advance new_high).
    ///
    /// UserKey is reconstructed from `Arc<str>` — cold path.
    pub fn entries_after(&self, start_exclusive: usize) -> (Vec<(InternerKey, UserKey)>, usize) {
        let rev = self.reverse.load();
        // `rev.len() - 1` is the highest id that has a slot. Some
        // slots in the captured range may still be unset if we're
        // reading mid-insert from another thread (or they are trailing
        // capacity) — but those will be picked up by the NEXT persist,
        // since we don't advance `last_persisted_len` past them.
        let hi_full = rev.len().saturating_sub(1);
        let lo = start_exclusive.saturating_add(1);
        if lo > hi_full {
            return (Vec::new(), start_exclusive);
        }
        let mut out = Vec::with_capacity(hi_full + 1 - lo);
        let mut new_high = start_exclusive;
        let mut gapped = false;
        for idx in lo..=hi_full {
            match rev.get(idx).map(|c| c.get()) {
                Some(Some(arc)) => {
                    out.push((InternerKey::new(idx as u64), UserKey::from_str(&**arc)));
                    // Only advance the high-water mark while the range is still
                    // gap-free; once a gap is seen we still capture present
                    // entries but must not claim to have persisted past the hole.
                    if !gapped {
                        new_high = idx;
                    }
                }
                Some(None) => {
                    // Unset slot: reserved-but-unswapped (concurrent touch_ind),
                    // a leaked id, or trailing doubling-growth capacity. Keep
                    // scanning so populated entries above it are still captured, but
                    // freeze new_high so the next persist re-captures this slot
                    // once (if) it fills.
                    gapped = true;
                }
                None => break, // past the end of the reverse vec
            }
        }
        (out, new_high)
    }
}
