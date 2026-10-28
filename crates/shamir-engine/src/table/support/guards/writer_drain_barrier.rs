//! F-48 (#859, P0) — reusable writer-drain barrier.
//!
//! Closes the check-then-act race that `unique_write_lock` + an intent flag
//! alone cannot close. A writer that reads `needs_write_barrier() == false`
//! proceeds lock-free through its ENTIRE validate→write→index sequence with
//! no further check; the DDL/index-create side raises the flag and takes the
//! SAME lock, but never waits for (drains) a writer that already read `false`
//! a moment earlier. This module provides a small, reusable primitive that
//! genuinely drains those in-flight fast-path writers before the drainer
//! proceeds past its snapshot/proof point.
//!
//! F-56 (#882, P0) wired this SAME primitive into `create_index_v2`'s residual
//! (the `drain_writers()` call after `index2_create_barrier` is raised, before
//! the backfill snapshot) — closing the older, candidly-documented instance of
//! the SAME race class that `table_manager_index_mgmt.rs::backfill_index2_backend`'s
//! doc comment previously tracked as an open residual.
//!
//! F-56 ALSO corrected a memory-ordering flaw in this primitive: the original
//! `Relaxed`/`Acquire`/`Release` orderings do NOT actually carry the
//! happens-before edge they claimed (see the proof on [`WriterDrainBarrier`]).
//! All four cross-atomic operations are now `SeqCst`.
//!
//! F-69 (#896, P0) — **scope correction, read this before assuming the proof
//! below covers the whole write-barrier predicate.** F-56's proof was
//! rigorous but its stated scope was too narrow: it covered exactly ONE
//! flag+counter pair, `schema_activation_barrier` and this module's `active`
//! counter. `TableManager::needs_write_barrier()` at the time OR'd together
//! SIX conditions (five `AtomicBool` flags here plus
//! `IndexManager::has_unique_indexes()`, a `shamir-index`-owned flag loaded
//! `Relaxed`, not `SeqCst`), and F-56 never extended its proof to the other
//! four flags or to the `Relaxed` operand — which is exactly how a real bug
//! survived: a writer's `enter_writer` (`SeqCst`) could reorder relative to
//! a `Relaxed` `has_unique_indexes()` read, letting a duplicate slip past a
//! unique index that had just gone live via `create_unique_index_locked`
//! concurrently. F-69 closes this by collapsing all six conditions into ONE
//! packed `Arc<AtomicU8>` word (see
//! `crate::index::write_barrier_flags::WriteBarrierFlags`) — every setter is
//! still `SeqCst`, so the worked proof below (steps 1-5) now applies
//! verbatim to the WHOLE six-condition predicate: `flag.load` in the proof
//! is `WriteBarrierFlags::any_set()` (one `SeqCst` load of the packed word),
//! and `flag.store(true)` is any of `WriteBarrierFlags::set`/`set_to` (the
//! `SeqCst` `fetch_or` any DDL-intent bit, including the
//! `shamir-index`-owned `UNIQUE_INDEX_EXISTS` bit, now performs). The proof's
//! reasoning is unchanged — extended in coverage, not contradicted. The
//! writer-drain counter (`active`, this module) remains a SEPARATE atomic,
//! not folded into `WriteBarrierFlags` — see that module's doc for why
//! (folding would turn `enter_writer`'s single locked `fetch_add` into a CAS
//! loop on the hottest path in this whole barrier).
//!
//! # F-70 (#897, P0) — THE canonical lock-order hierarchy (read this before
//! # adding ANY new caller of `drain_writers()` or `unique_write_lock`)
//!
//! F-57 (#883) wired `unique_write_lock().lock().await` FOLLOWED BY
//! `drain_writers().await` into every DDL create path
//! (`create_index_v2`/`create_index`/`create_unique_index_locked`/sorted-index
//! create) — lock-then-drain. Independently, F-48b (#867) had already wired
//! `pre_commit_prelock` (the tx-commit path, `tx/pre_commit.rs`) to enter this
//! module's drain set for EVERY table in `tx.write_set` and keep the guard
//! alive (fast path) BEFORE acquiring `unique_write_lock` for any OTHER
//! barriered table in the SAME transaction (slow path) — drain-guard-then-lock,
//! the OPPOSITE order, for a DIFFERENT table.
//!
//! This is a genuine lock-order inversion, reachable with three parties on two
//! tables X, Y: a DDL on X holds `unique_write_lock(X)` and blocks in
//! `drain(X)` waiting for committer A's kept-alive drain guard on X (A itself
//! blocked acquiring `unique_write_lock(Y)`, held by committer B, which is
//! itself blocked acquiring `unique_write_lock(X)` — held by the DDL). DDL → A
//! → B → DDL: a real deadlock, not a race, first reachable once F-57 gave a
//! second DDL family the lock-then-drain shape (see
//! `docs/dev-artifacts/prompts/post-alpha/126-f70-lock-order-inversion.md` for
//! the full derivation and `tx::pre_commit::tests::f70_lock_order_inversion_tests`
//! for the deterministic reproduction).
//!
//! ## The fix: drain-then-lock on the DDL side
//!
//! **Every DDL acquisition of a per-table write barrier now follows THIS
//! order, and no other:**
//!
//! 1. Raise the intent bit (`WriteBarrierFlags::set`/`IndexCreateBarrierGuard`
//!    / `set_schema_activation_barrier(true)`) — makes every NEW writer take
//!    the slow (locked) path.
//! 2. [`drain`](WriterDrainBarrier::drain) — wait for every writer that read
//!    the flag as `false` BEFORE step 1 to finish its in-flight
//!    validate→write→index sequence and exit the drain set.
//! 3. **Only THEN** acquire `unique_write_lock`.
//! 4. Run the snapshot/backfill/register/persist body.
//! 5. Release the lock, then clear the intent bit (RAII, reverse order).
//!
//! [`TableManager::begin_write_barrier`](crate::table::table_manager::TableManager::begin_write_barrier)
//! is the ONE canonical entry point that performs steps 1-3 in this order —
//! every DDL call site (in `shamir-engine` AND `shamir-db`) goes through it
//! instead of hand-rolling the lock+flag+drain sequence, so the order cannot
//! silently drift back to lock-then-drain at a new call site.
//!
//! ## Why drain-then-lock closes the cycle (not just "the other" order)
//!
//! The hazard in lock-then-drain is that the DDL is BOTH a lock-holder
//! (`unique_write_lock(X)`) AND a waiter (on the drain count) at the SAME
//! time — and the parties it waits on (A, transitively B) may need OTHER
//! locks the DDL never touches, closing an arbitrarily long cycle back to
//! `unique_write_lock(X)`.
//!
//! Under drain-then-lock, while the DDL is parked in `drain(X)` it holds NO
//! table lock for this operation at all — it cannot be a link in any
//! lock-wait cycle during that wait, because nothing it holds can block
//! anyone else. Once `drain(X)` returns (every fast-path writer that read the
//! flag as `false` has exited), the DDL competes for `unique_write_lock(X)`
//! exactly like a brand-new slow-path writer would — an ordinary two-party
//! wait (DDL vs. whoever currently holds the lock), never a cycle, because
//! that lock-holder does not (and structurally cannot: see below) wait on
//! anything the DDL holds.
//!
//! This also does not reopen the snapshot-safety property F-56/F-57 built
//! `drain_writers()` for: a writer that arrives AFTER the flag is raised
//! reads `needs_write_barrier() == true` and takes the slow path, competing
//! for the SAME `unique_write_lock` the DDL is about to acquire. Either
//! outcome is safe — if the writer wins the race, it completes its write and
//! releases the lock BEFORE the DDL's snapshot begins (so the snapshot sees
//! it); if the DDL wins, the writer simply waits its turn behind the DDL,
//! identical to today's serialization. A writer that read the flag as
//! `false` a moment BEFORE the raise is still caught by `drain()` in step 2,
//! exactly as before — reordering step 2 relative to step 3 changes nothing
//! about what `drain()` itself waits for (see "Why bump BEFORE the flag
//! check" above: the writer's fast/slow fork is decided at ITS OWN flag read,
//! independent of when the DDL happens to acquire, or not yet acquire, the
//! lock).
//!
//! ## `pre_commit_prelock`'s order is UNCHANGED and is the one this mirrors
//!
//! The commit-side order (drain-guard-kept-alive, THEN lock for a different
//! table) was never the bug — F-48b's shape already amounts to
//! "drain-membership before any lock acquisition, per table, for the whole
//! prelock call". The DDL side is what had to change to match it. See
//! `tx::pre_commit::pre_commit_prelock`'s own doc comment for its half of the
//! now-consistent hierarchy.
//!
//! ## Multi-table / rename callers
//!
//! `TableManager::rename_index`'s unique-index branch needs the lock held
//! across an entire drop→create span (not just create), for a DIFFERENT
//! reason (uniqueness-gap atomicity, audit A9) — unrelated to drain ordering.
//! It still goes through `begin_write_barrier` for the initial
//! raise→drain→lock acquisition and then simply keeps the returned lock guard
//! alive across both the drop and the create body — see that method's doc.
//! This does not reintroduce a lock-then-drain window: the drain always
//! completes strictly BEFORE this caller (or any other) holds the lock.

#[cfg(loom)]
use loom::sync::{Arc, atomic::{AtomicUsize, Ordering, fence}};
#[cfg(not(loom))]
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

/// Reusable drain barrier for a table's non-tx write path.
///
/// One instance lives on each
/// [`TableManager`](crate::table::table_manager::TableManager) (shared across clones
/// via `Arc`, like the sibling barrier flags). Fast-path writers — those that
/// read `needs_write_barrier() == false` — bracket their entire
/// validate→write→index sequence with [`enter_writer`](Self::enter_writer)
/// (RAII bump/decrement of an `active` counter). A drainer — after raising its
/// intent flag AND acquiring `unique_write_lock` — calls
/// [`drain`](Self::drain) to wait for every in-flight fast-path writer to
/// exit before proceeding.
///
/// # Why bump BEFORE the flag check
///
/// The counter increment MUST be sequenced before the
/// `needs_write_barrier()` flag read in the writer. Reversing the order
/// (flag check then bump) reopens the race: the drain could load `0` in the
/// gap between the flag read and the bump, then proceed while the writer is
/// still in flight.
///
/// # Slow-path writers do NOT stay in the drain set
///
/// A writer that reads `needs_write_barrier() == true` takes the slow path
/// (`unique_write_lock`). It MUST exit the drain set (drop the guard) BEFORE
/// blocking on the lock — otherwise the drainer (which holds the lock) would
/// wait forever for a writer that cannot make progress. Slow-path writers are
/// serialized by the lock itself and cannot be in flight when
/// [`drain`](Self::drain) returns.
///
/// # Cost
///
/// `enter_writer`: one `SeqCst fetch_add` in production, relaxed increment and
/// loom-only fence in the model. `WriterDrainGuard::drop`: one SeqCst or Release
/// decrement respectively. `drain`: a SeqCst load per iteration, `yield_now`
/// between iterations (rare DDL path). When
/// no drain is in progress the drainer never touches this primitive — zero
/// contention.
///
/// # Memory model (F-56/F-84; loom proof)
///
/// F-56's original argument for the shared flag/counter pair is that the
/// writer's `fetch_add` precedes its flag load and the drainer's flag store
/// precedes its counter load in the single SeqCst order S. If the writer reads
/// the old flag, its increment precedes the drainer's counter load, which must
/// then see that increment or a later modification. The same total-order
/// argument applies to the packed barrier predicate described above.
///
/// This is the standard cross-atomic SeqCst proof: the counter and intent flag
/// are separate atomics, so acquire/release alone cannot establish this
/// ordering when the writer reads the old flag value. The data store's own
/// synchronization provides write visibility; the counter ensures temporal
/// drain membership through the writer's completed work.
///
/// # Reusability
///
/// This primitive is barrier-agnostic: [`drain`](Self::drain) waits for ALL
/// in-flight fast-path writers, regardless of which intent flag the drainer
/// raised. Every DDL call site (schema-activation, F-37/F-48; `create_index_v2`
/// /`create_index`/`create_unique_index`/sorted-index create, F-56/F-57) now
/// reaches it through the ONE canonical entry point,
/// [`TableManager::begin_write_barrier`](crate::table::table_manager::TableManager::begin_write_barrier)
/// (F-70, #897) — after raising the intent bit, before acquiring
/// `unique_write_lock` and before the snapshot/proof point — identically.
#[derive(Debug)]
pub struct WriterDrainBarrier {
    active: Arc<AtomicUsize>,
}

impl WriterDrainBarrier {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Enter the drain set: bump the active-writer counter, return an RAII
    /// guard that decrements on drop.
    ///
    /// Call this on the fast path BEFORE reading `needs_write_barrier()`,
    /// bracketing the entire validate→write→index sequence. If the flag check
    /// then returns `true` (slow path), drop the returned guard BEFORE taking
    /// `unique_write_lock` — the lock serializes the slow path, the counter
    /// must not.
    #[must_use]
    pub fn enter_writer(&self) -> WriterDrainGuard {
        #[cfg(not(loom))]
        self.active.fetch_add(1, Ordering::SeqCst);
        #[cfg(loom)]
        {
            self.active.fetch_add(1, Ordering::Relaxed);
            fence(Ordering::SeqCst);
        }
        WriterDrainGuard {
            active: Arc::clone(&self.active),
        }
    }

    /// Starts the drain observation phase. The fence is modeling-only: the
    /// production SeqCst loads themselves participate in the global order.
    pub fn begin_drain(&self) {
        #[cfg(loom)]
        fence(Ordering::SeqCst);
    }

    #[cfg(not(loom))]
    pub fn is_drained(&self) -> bool {
        self.active.load(Ordering::SeqCst) == 0
    }

    #[cfg(loom)]
    pub fn is_drained(&self) -> bool {
        self.active.load(Ordering::Acquire) == 0
    }

    /// Drain until no writer remains. Production retains the diagnostic wait
    /// reporting; loom's model uses its thread scheduler in the test module.
    #[cfg(not(loom))]
    pub async fn drain(&self) {
        self.begin_drain();
        // F-68 (#895) cluster D / task #124 diagnostic instrumentation.
        // NOT a behavior change: this is timestamped logging only, no new
        // await point, no retry/timeout.
        //
        // F-70 (#897, P0): this WAS the DDL side of a genuine lock-order
        // inversion — every DDL caller used to acquire `unique_write_lock`
        // FIRST and call `drain()` SECOND, while `pre_commit_prelock`
        // (crates/shamir-engine/src/tx/pre_commit.rs) enters this SAME drain
        // set first and may later acquire `unique_write_lock` for a
        // DIFFERENT table while still holding this table's drain-set
        // membership — DDL → committer A → committer B → DDL, a reachable
        // 3-party cycle. Fixed by reordering every DDL caller to
        // drain-then-lock (`TableManager::begin_write_barrier`); this
        // diagnostic logging is kept as a regression tripwire — if a
        // `drain()` call ever again blocks past 1s, that is the signature of
        // this same deadlock class re-opening (e.g. a new call site that
        // hand-rolls lock-then-drain instead of going through
        // `begin_write_barrier`).
        let drain_started = std::time::Instant::now();
        let mut last_report = drain_started;
        log::debug!(
            "WriterDrainBarrier::drain: enter, active={}",
            self.active.load(Ordering::SeqCst)
        );
        while !self.is_drained() {
            tokio::task::yield_now().await;
            // Threshold-gated so the common (fast, uncontended) path never
            // pays a Instant::now() + log call per yield — only once the
            // wait has already run past 1s do we start periodically
            // reporting, and then only once per additional second.
            if last_report.elapsed() >= std::time::Duration::from_secs(1) {
                log::warn!(
                    "WriterDrainBarrier::drain: still waiting after {:?}, active={} \
                     (possible lock-order-inversion deadlock, see task #897)",
                    drain_started.elapsed(),
                    self.active.load(Ordering::SeqCst)
                );
                last_report = std::time::Instant::now();
            }
        }
        log::debug!(
            "WriterDrainBarrier::drain: exit after {:?}",
            drain_started.elapsed()
        );
    }

    #[cfg(loom)]
    pub async fn drain(&self) {
        self.begin_drain();
        while !self.is_drained() {
            tokio::task::yield_now().await;
        }
    }

    /// Test-only: current active-writer count (for assertions). Not part of
    /// the drain protocol; a plain `Acquire` load suffices for assertions.
    #[cfg(test)]
    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

impl Clone for WriterDrainBarrier {
    fn clone(&self) -> Self {
        Self {
            active: Arc::clone(&self.active),
        }
    }
}

impl Default for WriterDrainBarrier {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard returned by [`WriterDrainBarrier::enter_writer`]. Decrements
/// the counter on drop (`SeqCst`), releasing drain-set membership. The
/// `SeqCst` decrement is what lets the drainer's `SeqCst` load (in
/// [`WriterDrainBarrier::drain`]) conclude the writer's pre-decrement work is
/// done when it observes the resulting `0` — see the struct-level memory-model
/// proof.
#[derive(Debug)]
pub struct WriterDrainGuard {
    active: Arc<AtomicUsize>,
}

impl Drop for WriterDrainGuard {
    fn drop(&mut self) {
        #[cfg(not(loom))]
        self.active.fetch_sub(1, Ordering::SeqCst);
        #[cfg(loom)]
        self.active.fetch_sub(1, Ordering::Release);
    }
}

// F-56/F-84: loom 0.7's SeqCst access behavior does not provide the SC-order
// guarantee, so explicit fences model it. Production uses the real SeqCst
// accesses and does not need fences.

