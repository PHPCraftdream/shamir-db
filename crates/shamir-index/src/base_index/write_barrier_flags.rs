//! F-69 (#896, P0) — single packed atomic backing `needs_write_barrier()`.
//!
//! # The bug this closes
//!
//! Before this module existed, `TableManager::needs_write_barrier()`
//! (`shamir-engine`) OR'd together SIX independent loads: five `AtomicBool`s
//! owned by `TableManager` itself (all `SeqCst`) plus
//! `IndexManager::has_unique_indexes()` (`shamir-index`), whose backing
//! `AtomicBool` was loaded `Relaxed`. Six separate loads are NOT a single
//! atomic snapshot — the compound condition could observe a torn read across
//! a genuine state transition, and the one `Relaxed` operand additionally
//! broke the `SeqCst` total-order argument F-56 (#882) proved for the
//! `WriterDrainBarrier` protocol (see `writer_drain_barrier.rs` in
//! `shamir-engine`, which this module's doc extends rather than
//! contradicts). A writer racing a concurrent `create_unique_index_locked`
//! could read the whole predicate as `false` on the fast path while the
//! unique index was in the middle of going live, letting a duplicate value
//! slip past the constraint.
//!
//! # The fix
//!
//! `WriteBarrierFlags` is ONE `Arc<AtomicU16>` — seven gate bits packed into a
//! single word. `needs_write_barrier()` becomes exactly one `SeqCst` load
//! (`flags.load(Ordering::SeqCst) != 0`) instead of six. A single atomic load
//! can never be torn regardless of memory ordering, so the whole compound
//! condition is now a genuine point-in-time snapshot, and F-56's `SeqCst`
//! total-order proof (see `writer_drain_barrier.rs`) now covers the FULL
//! seven-condition predicate instead of five-sixths of it.
//!
//! # Ownership — why this type lives in `shamir-index`, not `shamir-engine`
//!
//! Bit 0 ([`UNIQUE_INDEX_EXISTS`]) is [`IndexManager`](super::index_manager::IndexManager)'s
//! own responsibility: it is flipped by `create_unique_index_from_records`
//! and `drop_unique_index` (both `shamir-index`-internal), and it is ALSO
//! consulted as a hot-path fast-skip by roughly a dozen call sites inside
//! `shamir-index` itself (`validate_unique_for_create`, `unique_keys_for`,
//! `on_record_created_unique`, the planner variants, …) — all of which must
//! keep working with zero knowledge of `shamir-engine`'s five DDL-intent
//! bits. `shamir-engine` depends on `shamir-index` (never the reverse), so
//! moving bit 0's source of truth OUT of `IndexManager` and into
//! `TableManager` would force every one of those `shamir-index`-internal
//! call sites to reach across a crate boundary that does not exist in that
//! direction. Instead, `IndexManager` owns and constructs the packed word
//! (bit 0 is its exclusive write authority, flipped via this type's own
//! [`set`](WriteBarrierFlags::set)/[`set_to`](WriteBarrierFlags::set_to)),
//! and exposes an `Arc<AtomicU16>` clone via
//! [`IndexManager::write_barrier_flags`](super::index_manager::IndexManager::write_barrier_flags).
//! `TableManager` takes that SAME `Arc` at construction time and uses bits
//! 1-5 for its own five DDL-intent conditions (via the SAME
//! [`set`](WriteBarrierFlags::set)/[`clear`](WriteBarrierFlags::clear)/
//! [`set_to`](WriteBarrierFlags::set_to) methods on its own bit constants) —
//! each struct still owns exactly the bits it is responsible for; neither
//! reaches into the other's private fields, and both read the identical
//! word for the compound predicate.
//!
//! # Ordering
//!
//! Every bit-set/bit-clear/full-word-load in this module is `SeqCst`,
//! preserving the ordering every individual flag already had before this
//! merge (none was weakened). `SeqCst` bitwise RMW
//! (`fetch_or`/`fetch_and`) on a shared byte is a single locked instruction,
//! same cost class as the `AtomicBool::store` each bit used to get — merging
//! six bits into one byte does not introduce a CAS loop (contrast with
//! folding a counter in, discussed below).
//!
//! # Why the `active` writer counter is NOT folded into this word
//!
//! [`WriterDrainBarrier::active`](super::super::table::writer_drain_barrier::WriterDrainBarrier)
//! (in `shamir-engine`) is a *separate* `AtomicUsize`, deliberately kept out
//! of this packed word:
//!
//! - `enter_writer` is a plain `SeqCst fetch_add` — a single locked
//!   instruction. Packing the counter as a sub-field of this byte would
//!   turn every `enter_writer`/guard-drop into a `fetch_update` CAS loop
//!   (a raw `fetch_add` on a shared word corrupts neighboring bits when it
//!   overflows into them), changing `enter_writer`'s cost profile from "one
//!   locked instruction" to "CAS loop, unbounded retries under contention"
//!   — exactly the degradation `writer_drain_barrier.rs`'s "Cost" section
//!   documents `enter_writer` as NOT having today. The counter is the
//!   HOTTEST path in this whole barrier (every fast-path writer bumps it
//!   twice, enter + drop); the six gate bits are DDL-only (rare). Keeping
//!   them apart avoids taxing the hot path to shrink an already-rare one.
//! - Realistic concurrent-writer ceilings on one table are unbounded in
//!   principle (no connection/session cap enforces a hard ceiling on
//!   in-flight fast-path writers per table in this codebase today), so a
//!   packed sub-field would need a saturating/overflow policy that doesn't
//!   exist for `AtomicUsize::fetch_add` — another reason not to force it
//!   into a fixed-width shared word.
//! - Folding buys ONE fewer atomic per `needs_write_barrier()` call only in
//!   the sense of merging the counter's OWN read into the same word — but
//!   `needs_write_barrier()` never reads `active` in the first place (only
//!   `enter_writer`/`drain` do); there is no shared call site that would
//!   collapse from 2 loads to 1 by folding. The single-atomic win this task
//!   targets is fully realized by packing the six gate bits alone.
//!
//! This is an explicit, scoped decision: seven gate bits in one atomic (now
//! expanded to `AtomicU16` to accommodate the seventh bit), the writer-drain
//! counter stays a distinct atomic with its existing `fetch_add`/`fetch_sub`
//! cost profile, unchanged.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

/// Bit 0 — at least one base_index unique index is registered on this table.
/// Exclusive write authority: [`IndexManager`](super::index_manager::IndexManager)
/// (`create_unique_index_from_records`, `drop_unique_index`, `sync_flags`).
pub const UNIQUE_INDEX_EXISTS: u8 = 1 << 0;

/// Bit 1 — an index2 (`fts`/`functional`/`vector`) `create_index_v2`
/// backfill→register sequence is in flight. Exclusive write authority:
/// `TableManager` (`shamir-engine`), formerly `index2_create_barrier`.
pub const INDEX2_CREATE: u8 = 1 << 1;

/// Bit 2 — a schema-activation DDL sequence
/// (`set_table_schema`/`add_schema_rule`) is inside its `keyset_safe`
/// count-proof → persist → activate window. Exclusive write authority:
/// `TableManager`, formerly `schema_activation_barrier`.
pub const SCHEMA_ACTIVATION: u8 = 1 << 2;

/// Bit 3 — a regular (non-unique) hash-index `create_index`
/// snapshot→backfill→register sequence is in flight. Exclusive write
/// authority: `TableManager`, formerly `regular_index_create_barrier`.
pub const REGULAR_INDEX_CREATE: u8 = 1 << 3;

/// Bit 4 — a unique-index `create_unique_index` snapshot→backfill→register
/// sequence is in flight (covers the FIRST unique index on a table, where
/// bit 0 is still unset). Exclusive write authority: `TableManager`,
/// formerly `unique_index_create_barrier`.
pub const UNIQUE_INDEX_CREATE: u8 = 1 << 4;

/// Bit 5 — a sorted-index `create_sorted_index_with_include`
/// register→backfill sequence is in flight. Exclusive write authority:
/// `TableManager`, formerly `sorted_index_create_barrier`.
pub const SORTED_INDEX_CREATE: u8 = 1 << 5;

/// Bit 6 — #1102: at least one base_index regular (non-unique) hash index
/// is registered on this table. Exclusive write authority:
/// [`IndexManager`](super::index_manager::IndexManager)
/// (the four `create_index*` paths and `drop_index`). Replaces the former
/// standalone `has_indexes: Arc<AtomicBool>` (written `Release`, read
/// `Relaxed` — no happens-before edge; see #1102 brief). Folding into this
/// packed word gives it the same `SeqCst` ordering as bit 0, closing the
/// ordering gap that let a row commit with no posting under a concurrently-
/// created regular index.
pub const REGULAR_INDEX_EXISTS: u8 = 1 << 6;

/// Every bit this word packs — used only for `debug_assert!`-style sanity
/// checks (no production code should need to mask against this directly).
#[cfg(test)]
pub const ALL_BITS: u16 = (UNIQUE_INDEX_EXISTS
    | INDEX2_CREATE
    | SCHEMA_ACTIVATION
    | REGULAR_INDEX_CREATE
    | UNIQUE_INDEX_CREATE
    | SORTED_INDEX_CREATE
    | REGULAR_INDEX_EXISTS) as u16;

/// The single packed atomic backing the whole write-barrier predicate.
///
/// Cheap to clone (`Arc<AtomicU16>` clone — one refcount bump); every clone
/// observes the SAME underlying byte, mirroring how the six flags it
/// replaces were each independently `Arc`-shared across `TableManager`
/// clones before this merge.
#[derive(Debug, Clone)]
pub struct WriteBarrierFlags {
    bits: Arc<AtomicU16>,
}

impl WriteBarrierFlags {
    /// New word with every bit clear (no unique index, no DDL in flight).
    pub fn new() -> Self {
        Self {
            bits: Arc::new(AtomicU16::new(0)),
        }
    }

    /// Construct with [`UNIQUE_INDEX_EXISTS`] pre-set to `initial_unique`
    /// (used by `IndexManager::new` when loading a table that already has a
    /// registered unique index — mirrors the old `sync_flags` initial
    /// store).
    pub fn with_unique_index_exists(initial_unique: bool) -> Self {
        let word = Self::new();
        if initial_unique {
            word.set(UNIQUE_INDEX_EXISTS);
        }
        word
    }

    /// #1102: construct with [`REGULAR_INDEX_EXISTS`] pre-set to `initial_regular`
    /// and [`UNIQUE_INDEX_EXISTS`] pre-set to `initial_unique` (used by
    /// `IndexManager::new` when loading a table that already has registered
    /// indexes).
    pub fn with_regular_and_unique_index_exists(
        initial_regular: bool,
        initial_unique: bool,
    ) -> Self {
        let word = Self::new();
        if initial_regular {
            word.set(REGULAR_INDEX_EXISTS);
        }
        if initial_unique {
            word.set(UNIQUE_INDEX_EXISTS);
        }
        word
    }

    /// The whole compound write-barrier predicate: `true` iff ANY bit is
    /// set. Exactly ONE atomic load — see the module doc for why this
    /// closes the F-69 torn-read bug. `SeqCst`: preserves F-56's total-order
    /// argument (see `writer_drain_barrier.rs`), which every individual bit
    /// already required before this merge.
    #[inline]
    pub fn any_set(&self) -> bool {
        self.bits.load(Ordering::SeqCst) != 0
    }

    /// Set `bit` (one of the module-level `*_INDEX_*`/`*_CREATE`/
    /// `*_ACTIVATION` constants). `SeqCst` `fetch_or` — a single locked
    /// instruction, same cost class as the `AtomicBool::store(true)` this
    /// replaces.
    #[inline]
    pub fn set(&self, bit: u8) {
        self.bits.fetch_or(bit as u16, Ordering::SeqCst);
    }

    /// Clear `bit`. `SeqCst` `fetch_and` with the bit's complement — a
    /// single locked instruction, same cost class as the
    /// `AtomicBool::store(false)` this replaces.
    #[inline]
    pub fn clear(&self, bit: u8) {
        self.bits.fetch_and(!(bit as u16), Ordering::SeqCst);
    }

    /// Set `bit` to `on` (true → set, false → clear). Convenience for
    /// call sites that mirror the old `AtomicBool::store(on, ..)` shape
    /// (e.g. `set_schema_activation_barrier`).
    #[inline]
    pub fn set_to(&self, bit: u8, on: bool) {
        if on {
            self.set(bit);
        } else {
            self.clear(bit);
        }
    }

    /// Whether `bit` alone is currently set. `SeqCst` — used by tests and by
    /// `IndexManager::has_unique_indexes()`'s public accessor (previously a
    /// `Relaxed` load on its own `AtomicBool`; now `SeqCst` on this shared
    /// word, closing the ordering gap the F-69 bug report identified).
    #[inline]
    pub fn is_set(&self, bit: u8) -> bool {
        (self.bits.load(Ordering::SeqCst) & (bit as u16)) != 0
    }

    /// Raw snapshot of the whole byte — test-only introspection.
    #[cfg(test)]
    pub fn raw(&self) -> u16 {
        self.bits.load(Ordering::SeqCst)
    }
}

impl Default for WriteBarrierFlags {
    fn default() -> Self {
        Self::new()
    }
}
