//! #1003 (follow-up to #984), corrected post-`@oh`-review — in-flight
//! `CREATE INDEX` identity set.
//!
//! [`degraded_index_count`](super::table_manager::TableManager::degraded_index_count)
//! counts every index whose `.state != IndexState::Ready` — but
//! `IndexState::Building` is set at the START of a `CREATE INDEX`, before the
//! backfill even begins (F-72, #899), and is only flipped to `Ready` once the
//! backfill fully completes. A large-table backfill can legitimately run for
//! minutes (`docs/guide-docs/KNOWN_LIMITATIONS.md` §3), so a completely
//! healthy, currently-in-progress create made the gauge read non-zero for the
//! ENTIRE build — a real false-positive that would page an operator to
//! "repair" an index that is not stuck at all.
//!
//! **First attempt (scalar counter) was wrong — the review that caught it:**
//! the original fix here subtracted a plain `Arc<AtomicU64>` count of
//! in-flight creates from the raw non-`Ready` tally. An `@oh` adversarial
//! review of the whole batch found this scalar subtraction has NO
//! correspondence between the two sets it combines: the unique family
//! (`create_unique_index_from_records`) registers its definition exactly
//! ONCE, post-backfill, so it NEVER contributes to the raw tally during its
//! own (potentially long) backfill — yet its guard was live the whole time.
//! Same for `index2`'s create path: the descriptor enters the live registry
//! only after the entire backfill, right before it flips `Ready`. So a
//! genuinely-in-flight unique or index2 create could subtract 1 from the
//! tally while contributing 0 to it — silently masking an UNRELATED,
//! genuinely stuck index (e.g. a sorted index left `Building` by a past
//! crash) for as long as the unrelated create ran. That is a false
//! NEGATIVE on a health gauge — worse than the false positive #984 was
//! fixing.
//!
//! **The fix: track in-flight create IDENTITIES (interned index-name ids),
//! not a scalar count.** `degraded_index_count()` now skips a non-`Ready`
//! definition ONLY if that SPECIFIC index's name is currently in this set —
//! never a scalar subtraction that can bleed across unrelated indexes. An
//! index that never appears in the registry during its own backfill (the
//! unique/index2 cases above) is simply never iterated over in the first
//! place, so tracking its identity here is a no-op for it (harmless) and,
//! critically, can never affect any OTHER index whose NAME differs from
//! it. Caveat (a second `@oh` review found this, worth naming precisely):
//! "identity" here means index NAME, which is unique per-name across the
//! interner but NOT enforced unique across the four FAMILIES at create
//! time (only `rename_index`'s destination-name guard checks across
//! families) — so a crash-orphaned sorted index and a healthy in-flight
//! regular-family create that happen to share the exact same name string
//! would still collide. This is a pre-existing naming-collision gap in the
//! index-family model generally, not something this identity-set fix
//! introduces or could fix on its own.
//!
//! Refcounted (`BTreeMap<u64, u32>`, not a plain set) because two
//! CONCURRENT creates racing for the same name (rare, but not structurally
//! impossible before a duplicate-name check lands) must not have the first
//! guard's `Drop` prematurely un-hide the second's still-in-flight identity.
//!
//! `std::sync::Mutex` is the sanctioned low-frequency fallback here
//! (CLAUDE.md): CREATE INDEX is a DDL operation, contention is nil, and the
//! lock is never held across an `.await` point — mirrors this crate's
//! existing `dropping_regular`/`dropping_unique` guard sets
//! (`shamir-index`'s `IndexManager`).
//!
//! Call [`InFlightCreateSet::enter`] with the index's ALREADY-INTERNED name
//! id — i.e. after the name has been resolved/interned but before the
//! definition is published at `Building` state — at the start of each of
//! the four index-create families (`create_index`, `create_unique_index`
//! [via `create_unique_index_body`], `create_sorted_index_with_include`,
//! `create_index_v2`'s non-btree branch). The returned RAII guard removes
//! the identity on drop (panic-safe and early-`?`-return-safe), mirroring
//! [`WriterDrainBarrier`](super::writer_drain_barrier::WriterDrainBarrier)'s
//! `enter_writer`/`WriterDrainGuard` shape.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Process-local, per-`TableManager` set of index-name ids (interned)
/// currently undergoing a `CREATE INDEX` in THIS process. Shared across
/// `TableManager` clones via `Arc`, like the sibling barrier/guard-set
/// fields.
#[derive(Debug)]
pub struct InFlightCreateSet {
    ids: Arc<Mutex<BTreeMap<u64, u32>>>,
}

impl InFlightCreateSet {
    pub fn new() -> Self {
        Self {
            ids: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Enter: record `name_interned` as in-flight, return an RAII guard
    /// that removes it on drop (panic-safe and early-`?`-return-safe — no
    /// hand-rolled decrement is needed at any exit site).
    ///
    /// Call this with the index's ALREADY-INTERNED name id, as early as
    /// possible after that id is known (typically right after the name is
    /// resolved/interned, before the definition is published at
    /// `Building`), so every subsequent return path (including an early
    /// `?`) is covered by the guard's `Drop`.
    #[must_use]
    pub fn enter(&self, name_interned: u64) -> InFlightCreateGuard {
        *self.ids.lock().unwrap().entry(name_interned).or_insert(0) += 1;
        InFlightCreateGuard {
            ids: Arc::clone(&self.ids),
            name_interned,
        }
    }

    /// Whether `name_interned` currently has a `CREATE INDEX` in flight in
    /// THIS process. Read by `degraded_index_count()` per non-`Ready`
    /// definition, to skip counting it as degraded if so — never a scalar
    /// subtraction, so this can only ever affect the SPECIFIC identity
    /// checked, never an unrelated index. Plain mutex lookup — no store I/O.
    pub fn contains(&self, name_interned: u64) -> bool {
        self.ids.lock().unwrap().contains_key(&name_interned)
    }
}

impl Clone for InFlightCreateSet {
    fn clone(&self) -> Self {
        Self {
            ids: Arc::clone(&self.ids),
        }
    }
}

impl Default for InFlightCreateSet {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard returned by [`InFlightCreateSet::enter`]. Removes the
/// identity (or decrements its refcount, for the rare concurrent-same-name
/// race) on drop — covers panics and every early `?` return, mirroring
/// [`WriterDrainGuard`](super::writer_drain_barrier::WriterDrainGuard).
#[derive(Debug)]
pub struct InFlightCreateGuard {
    ids: Arc<Mutex<BTreeMap<u64, u32>>>,
    name_interned: u64,
}

impl Drop for InFlightCreateGuard {
    fn drop(&mut self) {
        let mut ids = self.ids.lock().unwrap();
        if let Some(count) = ids.get_mut(&self.name_interned) {
            *count -= 1;
            if *count == 0 {
                ids.remove(&self.name_interned);
            }
        }
    }
}
