use crate::types::common::{new_dash_map_wc, TDashMap};
use arc_swap::ArcSwap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use super::{InternerKey, TouchInd, UserKey};

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
/// count.
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
    /// population from `touch_ind`/`touch_with_id`, including growth).
    /// See the struct doc for why a single-writer critical section
    /// replaced an earlier fully-lock-free attempt that had a data-loss
    /// race. Never taken by any read path.
    reverse_write_lock: std::sync::Mutex<()>,
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
            // index 0 reserved (sentinel) — a single unset OnceLock.
            reverse: ArcSwap::from_pointee(vec![OnceLock::new()]),
            reverse_write_lock: std::sync::Mutex::new(()),
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
        // the highest id assigned), plus the sentinel at 0. Unassigned
        // ids stay as unset `OnceLock`s (read as `None` — a gap).
        let mut reverse: Vec<OnceLock<Arc<str>>> = Vec::with_capacity((max_id as usize) + 1);
        reverse.resize_with((max_id as usize) + 1, OnceLock::new);

        for (interned_key, user_key) in initial_data {
            let id = interned_key.id();
            let arc: Arc<str> = Arc::from(user_key.as_str());
            map_user_to_interned.insert(user_key, interned_key);
            // Fresh vec, exclusive `&mut` — set is guaranteed to succeed.
            let _ = reverse[id as usize].set(arc);
        }

        Interner {
            map_user_to_interned,
            reverse: ArcSwap::from_pointee(reverse),
            reverse_write_lock: std::sync::Mutex::new(()),
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
        // Op B: build Arc<str> once for the reverse slot — no extra alloc
        // vs the String we'd make for UserKey anyway.
        let arc: Arc<str> = Arc::from(s);

        // Reserve a fresh ID lock-free. If the forward-map CAS below
        // loses the race (Occupied branch), this slot is silently leaked
        // — the interner is monotonic and small leaks are harmless.
        let new_id = self.current_id.fetch_add(1, Ordering::Relaxed) + 1;
        let new_key = InternerKey::new(new_id);

        // CAS into forward map — another thread may have raced us.
        use dashmap::mapref::entry::Entry;
        match self.map_user_to_interned.entry(key) {
            Entry::Occupied(existing) => {
                // Race: another thread inserted between our get() and entry().
                // `new_id` is wasted (small leaked slot, harmless).
                Ok(TouchInd::Exists(existing.get().clone()))
            }
            Entry::Vacant(vacant) => {
                vacant.insert(new_key.clone());
                // #501: two-phase reverse-spine population — ensure
                // capacity by doubling (rare, O(N) total across N
                // touches) then `OnceLock::set` the slot in place
                // (common, no vec clone). Because ids are strictly
                // monotonic, `new_id` is owned exclusively by this call.
                self.set_reverse_slot(new_id as usize, arc);
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

    /// #501: populate the reverse spine at `idx` with `arc` for
    /// `touch_ind`'s fresh-insert path. `idx` comes from strictly
    /// monotonic `current_id` allocation, so it's uniquely owned by
    /// this call **with respect to other `touch_ind` callers** — the
    /// `debug_assert!` below only holds under that assumption. It is
    /// NOT exclusive with respect to `touch_with_id` (WAL-recovery path,
    /// which assigns caller-supplied ids and does its own collision
    /// detection inline rather than calling this method) — a `touch_ind`
    /// racing a concurrent `touch_with_id` for the same id is a
    /// pre-existing, out-of-scope hazard (WAL recovery is not expected
    /// to run concurrently with live `touch_ind` traffic).
    ///
    /// Holds `reverse_write_lock` for the whole ensure-capacity-then-set
    /// step — see the struct doc for why this replaced an earlier
    /// fully-lock-free two-phase attempt that had a data-loss race.
    /// Growth (rare — only when `idx >= cur.len()`) doubles capacity, so
    /// total clone work across N monotonic touches is a geometric
    /// series ≈ 2N — O(N), not the old O(N²). The common case (capacity
    /// already present) is a single `OnceLock::set`, no clone.
    fn set_reverse_slot(&self, idx: usize, arc: Arc<str>) {
        let _guard = self
            .reverse_write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cur = self.reverse.load_full();
        if idx < cur.len() {
            let set_ok = cur[idx].set(arc).is_ok();
            debug_assert!(
                set_ok,
                "touch_ind: slot {idx} already set — a concurrent touch_with_id \
                 raced this id, which the recovery model assumes cannot happen"
            );
            return;
        }
        self.reverse
            .store(Arc::new(Self::grown_reverse(&cur, idx, arc)));
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

    /// Monotonic interning generation: incremented on every successful
    /// `touch_ind` / `touch_with_id` and only ever grows (the map is
    /// append-only — there is NO `remove`/`clear`).
    ///
    /// This is the O(1) lock-free growth signal for caches that compile a
    /// derived structure against the interner and need to detect staleness
    /// (e.g. subscription filter compilation). An unchanged generation
    /// means no new field was interned, so a filter compiled at that
    /// generation is still complete.
    #[inline]
    pub fn generation(&self) -> u64 {
        self.current_id.load(Ordering::Relaxed)
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
    ///
    /// UserKey is reconstructed from the `Arc<str>` slot — this is a
    /// cold path (persistence / diagnostics), so the String allocation
    /// is acceptable.
    pub fn all_entries(&self) -> Vec<(InternerKey, UserKey)> {
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
    pub fn touch_with_id(&self, name: &str, id: u64) -> Result<(), String> {
        use dashmap::mapref::entry::Entry;

        if id == 0 {
            return Err("touch_with_id: id 0 is reserved (sentinel)".into());
        }

        let key = UserKey::from_str(name);

        // Check if name already exists in the forward map.
        if let Some(existing) = self.map_user_to_interned.get(name) {
            let existing_id = existing.id();
            return if existing_id == id {
                Ok(()) // idempotent
            } else {
                Err(format!(
                    "touch_with_id: name '{}' already mapped to id {}, cannot remap to {}",
                    name, existing_id, id
                ))
            };
        }

        // Check reverse map for id collision before inserting.
        {
            let rev = self.reverse.load();
            if let Some(existing_arc) = rev.get(id as usize).and_then(|c| c.get()) {
                if &**existing_arc != name {
                    return Err(format!(
                        "touch_with_id: id {} already used by '{}', cannot assign to '{}'",
                        id, &**existing_arc, name
                    ));
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
                    Err(format!(
                        "touch_with_id: name '{}' raced to id {}, cannot assign {}",
                        name, existing_id, id
                    ))
                }
            }
            Entry::Vacant(vacant) => {
                let new_key = InternerKey::new(id);
                vacant.insert(new_key);

                // #501: ensure capacity (doubling) then `OnceLock::set`
                // the slot in place, under `reverse_write_lock`'s
                // single-writer critical section — no whole-vec clone
                // per touch, and no CAS-retry/confirm race (see the
                // struct doc). Unlike `touch_ind`, `id` here is
                // arbitrary (WAL-supplied), so a *different* name may
                // race us for the SAME id: with the lock held, the
                // slot's state at the moment we look is authoritative —
                // if a different name already occupies it, roll back
                // our forward-map entry and report the collision.
                let idx = id as usize;
                let collision = {
                    let _guard = self
                        .reverse_write_lock
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let cur = self.reverse.load_full();
                    if idx < cur.len() {
                        match cur[idx].get() {
                            None => {
                                let _ = cur[idx].set(arc.clone());
                                None
                            }
                            Some(existing) if &**existing == name => None, // idempotent replay
                            Some(existing) => Some(existing.clone()),
                        }
                    } else {
                        self.reverse
                            .store(Arc::new(Self::grown_reverse(&cur, idx, arc.clone())));
                        None
                    }
                };
                if let Some(existing) = collision {
                    // A different name won this id. Roll back our
                    // forward-map insert and report the collision.
                    let msg = format!(
                        "touch_with_id: id {} raced to '{}', cannot assign '{}'",
                        id, &*existing, name
                    );
                    self.map_user_to_interned.remove(name);
                    return Err(msg);
                }

                // Bump current_id so subsequent touch_ind won't reuse this id.
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
                    // scanning so populated higher ids are still captured, but
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
