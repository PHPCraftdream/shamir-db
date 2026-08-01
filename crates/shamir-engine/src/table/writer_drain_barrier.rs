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
//! [`TableManager::begin_write_barrier`](super::table_manager::TableManager::begin_write_barrier)
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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Reusable drain barrier for a table's non-tx write path.
///
/// One instance lives on each
/// [`TableManager`](super::table_manager::TableManager) (shared across clones
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
/// `enter_writer`: one `SeqCst fetch_add` (uncontended cache-line RMW,
/// ~ns — the `SeqCst` strengthening is a full fence on x86 only at the
/// *subsequent* `flag.load`, the RMW itself is a single locked instruction).
/// `WriterDrainGuard::drop`: one `SeqCst fetch_sub`. `drain`: one `SeqCst`
/// load per iteration, `yield_now` between iterations (rare DDL path). When
/// no drain is in progress the drainer never touches this primitive — zero
/// contention.
///
/// # Memory model (F-56, #882 — corrected; F-69, #896 — scope extended to
/// # the FULL six-condition predicate, not just `schema_activation_barrier`)
///
/// The four cross-atomic operations are ALL `Ordering::SeqCst`:
///   - writer  `active.fetch_add` (`enter_writer`)
///   - writer  `flag.load` (`needs_write_barrier`, now
///     `WriteBarrierFlags::any_set()` — ONE `SeqCst` load of the packed word
///     that replaced the six former independent flags)
///   - drainer `flag.store(true)` (any `WriteBarrierFlags::set`/`set_to` call
///     — `set_schema_activation_barrier` / `WriteBarrierGuard` (F-70, #897;
///     formerly `IndexCreateBarrierGuard`) / `IndexManager`'s
///     `UNIQUE_INDEX_EXISTS` setters all route through the SAME
///     `fetch_or`/`fetch_and` on the SAME `Arc<AtomicU8>`)
///   - drainer `active.load` (`drain`)
///
/// This dependency spans TWO independent atomics (`active` and `flag`), so
/// `Release`/`Acquire` ALONE CANNOT carry it. A `synchronizes-with` edge —
/// the only thing that creates a cross-thread happens-before relationship —
/// requires an `Acquire` load to read the value written by a *specific*
/// `Release` store **on the same atomic**. Here the writer's `flag.load`
/// reads `false` (the OLD value), NOT the `true` the drainer's
/// `flag.store(true)` wrote, so no synchronizes-with edge is created on
/// `flag`; and `active` is an entirely separate atomic whose `Relaxed`
/// increment the drainer's `Acquire` load has no ordering relationship with
/// at all. A legal weak-memory outcome is therefore: the writer increments
/// `active` (real time), reads `flag == false`, and proceeds; the drainer
/// stores `flag = true`, then `active.load` observes `0` and `drain()`
/// returns immediately — precisely the in-flight writer the primitive exists
/// to hold back.
///
/// `SeqCst` closes it. `SeqCst` operations participate in a SINGLE total
/// order `S` over all atomics, with the consistency constraints (C++/Rust
/// memory model, [atomics.order]):
///   (C1) same-thread sequenced-before is respected in `S`;
///   (C2) each atomic's modification order is respected in `S`;
///   (C3) a `SeqCst` load reads the value written by the most recent
///        modification of that atomic that precedes it in `S`.
/// Worked proof that the drainer cannot observe `active == 0`:
///   1. Writer: `fetch_add` is sequenced-before `flag.load` ⟹ by (C1)
///      `fetch_add ≺ flag.load` in `S`.
///   2. Drainer: `flag.store(true)` is sequenced-before `active.load` ⟹ by
///      (C1) `flag.store(true) ≺ active.load` in `S`.
///   3. The writer's `flag.load` read `false`, i.e. the value written by the
///      initial / pre-transition store on `flag` — NOT the drainer's
///      `flag.store(true)`. By (C3), if `flag.store(true)` preceded
///      `flag.load` in `S`, the load would have read `true`. It read `false`,
///      so `flag.load ≺ flag.store(true)` in `S`.
///   4. Chaining 1 → 3 → 2: `fetch_add ≺ flag.load ≺ flag.store(true) ≺
///      active.load`, all in the single total order `S`.
///   5. `fetch_add` and `active.load` are both `SeqCst` ops on the SAME
///      atomic (`active`). By (C3), `active.load` reads the most recent
///      modification of `active` preceding it in `S`. Since
///      `fetch_add ≺ active.load` in `S`, that most-recent modification is
///      `fetch_add` or a later one — so `active.load` observes a value
///      `>= 1` (it cannot observe the pre-increment `0`). The drainer does
///      NOT return; it waits. □
///
/// This is the standard "a third total order carries the cross-atomic
/// happens-before" argument; it works precisely because `SeqCst`, unlike
/// `Acquire`/`Release`, imposes a single global order across DIFFERENT
/// atomics — exactly what this primitive's `flag` + `active` dependency
/// needs. `WriterDrainGuard::drop`'s `fetch_sub` is `SeqCst` too, so the
/// writer's pre-decrement work (data-store write, record-counter bump, index
/// updates) is ordered before `drain()`'s `active.load` observes the
/// resulting `0` (the same chain, step 5, on the decrement side) — i.e. when
/// `drain()` returns, no fast-path writer is still in flight. The data
/// store's own synchronization provides cross-thread write visibility; this
/// counter provides temporal drain, mirroring the role `unique_write_lock`
/// plays on the slow path.
///
/// # Reusability
///
/// This primitive is barrier-agnostic: [`drain`](Self::drain) waits for ALL
/// in-flight fast-path writers, regardless of which intent flag the drainer
/// raised. Every DDL call site (schema-activation, F-37/F-48; `create_index_v2`
/// /`create_index`/`create_unique_index`/sorted-index create, F-56/F-57) now
/// reaches it through the ONE canonical entry point,
/// [`TableManager::begin_write_barrier`](super::table_manager::TableManager::begin_write_barrier)
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
        // SeqCst (not Relaxed): see the struct-level memory-model proof — the
        // cross-atomic dependency on `needs_write_barrier`'s flag load requires
        // a single SeqCst total order, which Relaxed/Acquire/Release cannot
        // provide across two independent atomics.
        self.active.fetch_add(1, Ordering::SeqCst);
        WriterDrainGuard {
            active: Arc::clone(&self.active),
        }
    }

    /// Drain: wait until every in-flight fast-path writer has exited the
    /// drain set.
    ///
    /// The caller MUST have already raised its intent flag/bit (`SeqCst`
    /// store) so NEW writers take the slow (locked) path. Then this catches
    /// any writer that read `false` before the flag went up.
    ///
    /// F-70 (#897, P0): the caller must call this BEFORE acquiring
    /// `unique_write_lock`, NOT after — see
    /// [`TableManager::begin_write_barrier`](super::table_manager::TableManager::begin_write_barrier)
    /// and this module's "F-70 — THE canonical lock-order hierarchy" doc
    /// section for why (the prior lock-then-drain order, F-57, deadlocked
    /// against `pre_commit_prelock`'s drain-guard-then-lock shape). Holding
    /// the lock is not required for `drain()` to be correct: what matters is
    /// that the flag was raised first (so every NEW writer takes the slow
    /// path and cannot re-enter the drain set) — the lock only serializes
    /// those NEW slow-path writers against the caller's own body, a separate
    /// concern from the drain wait itself.
    ///
    /// When no writers are active, returns after a single `SeqCst` load.
    pub async fn drain(&self) {
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
        // SeqCst (not Acquire): pairs with the writer's SeqCst fetch_add via the
        // single total order — see the struct-level memory-model proof.
        while self.active.load(Ordering::SeqCst) != 0 {
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
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_returns_immediately_when_no_writers_active() {
        let b = WriterDrainBarrier::new();
        assert_eq!(b.active_count(), 0);
        // Must not hang — single SeqCst load, immediate return.
        b.drain().await;
    }

    #[tokio::test]
    async fn drain_waits_until_all_writers_exit() {
        let b = WriterDrainBarrier::new();
        let g1 = b.enter_writer();
        let g2 = b.enter_writer();
        assert_eq!(b.active_count(), 2);

        // drain must block while writers are active. Spawn it and confirm it
        // does not finish.
        let b2 = WriterDrainBarrier::clone(&b);
        let drain = tokio::spawn(async move { b2.drain().await });
        tokio::task::yield_now().await;
        assert!(!drain.is_finished(), "drain must wait for active writers");

        drop(g1);
        tokio::task::yield_now().await;
        assert!(
            !drain.is_finished(),
            "drain must wait until ALL writers exit"
        );

        drop(g2);
        drain.await.expect("drain completes once counter hits 0");
    }

    #[tokio::test]
    async fn guard_decrement_returns_counter_to_zero_for_drain() {
        // Structural: enter + drop returns the counter to 0 so a subsequent
        // drain observes it (the SeqCst load reads the SeqCst-stored 0).
        let b = WriterDrainBarrier::new();
        {
            let _g = b.enter_writer();
            assert_eq!(b.active_count(), 1);
        }
        assert_eq!(b.active_count(), 0);
        b.drain().await;
    }

    #[tokio::test]
    async fn clone_shares_the_same_counter() {
        // Clones must observe the same counter (mirrors TableManager's
        // Arc-shared barrier flags) — a writer on one clone must be visible
        // to a drain on another.
        let a = WriterDrainBarrier::new();
        let b = a.clone();
        let _g = a.enter_writer();
        assert_eq!(
            b.active_count(),
            1,
            "clone must share the same Arc<AtomicUsize>"
        );
    }
}

// ============================================================================
// F-56 (#882) — loom model of the writer-drain barrier's interleaving contract.
//
// RUN (NOT part of `./scripts/test.sh` — loom is opt-in via the `loom` cargo
// feature, which the `build.rs` translates into a crate-local `cfg(loom)` so
// the dependency tree is not pulled into its own loom code paths; this module
// is compiled away from every normal build):
//
//   cargo nextest run -p shamir-engine --features loom --lib \
//       -E 'test(loom_model)' --nocapture
//
// (`cargo test` is blocked outright by this workspace's perimeter guard —
// see CLAUDE.md's "Centralised test entry point" section. `cargo nextest`
// is the only invocation that passes.)
//
// ## Honest scope — what this model does and does NOT prove
//
// F-84 (#912) correction: the claim that PRECEDED this one — "loom explores
// all SEQUENTIALLY-CONSISTENT thread interleavings" — is false for loom
// 0.7.x. loom's `Thread::seq_cst()` (the SeqCst-specific behavior of a
// SeqCst-ordered ACCESS, as opposed to an explicit `fence(SeqCst)`) is a
// documented no-op in this version ("the previous implementation ... was
// incorrect ... as a quick fix, just disable it ... may fail to model
// correct code, but will not silently allow bugs" — `loom::rt::thread`).
// Concretely: a plain `store(SeqCst)`/`load(SeqCst)` pair in this loom
// version behaves like `Release`/`Acquire`, NOT full SC — it can still
// exhibit the classic store-buffering (SB/Dekker) outcome that real SeqCst
// forbids. Only an explicit `loom::sync::atomic::fence(Ordering::SeqCst)`
// gets loom's real SC enforcement. The two `fence(SeqCst)` calls below exist
// SOLELY to compensate for this loom limitation — they have NO counterpart
// in the real (non-model) code, which uses plain SeqCst accesses throughout
// and needs no fence, because on a REAL C11/Rust implementation
// `store(SeqCst); load(SeqCst)` already forbids SB. Do not read the
// fences as implying production needs them; they are a modeling-only
// device. (Confirmed via an isolated loom SB litmus-test probe: the same
// two-thread store/load shape run under loom fails without fences and
// passes with one `fence(SeqCst)` per thread between its two accesses.)
//
// Because of this, the fenced model still does NOT red/green the F-56
// weak-memory fix itself (an SC fence forbids SB regardless of what
// ordering the surrounding accesses use, so the model would "pass" whether
// or not the accesses below were SeqCst, Acquire/Release, or even Relaxed)
// — that argument continues to rest entirely on the worked proof in the
// [`WriterDrainBarrier`] doc comment above, NOT on this model.
//
// What this model IS (and always was) good for: guarding the protocol's
// INTERLEAVING/structural contract against future regressions — a broken
// `drain` loop (e.g. one that exits while `active != 0`), a missing guard
// decrement, or a reordering of `enter_writer` AFTER the flag read. Any of
// those shows up here as a violation of the invariant "after `drain()`
// returns, every fast-path writer that entered the drain set has completed
// its write" — PROVIDED the assertion actually samples state at drain-return
// time, not after the caller separately joins the writer thread (see
// `wrote_at_drain_return`'s doc below for why the ORIGINAL assertion here
// was tautological and could not witness any violation, including the SB
// outcome above, until F-84 fixed both the sampling point and added the
// fences needed to make loom actually enforce the ordering the real code
// relies on).
// ============================================================================
#[cfg(loom)]
mod loom_model {
    use loom::sync::atomic::{fence, AtomicBool, AtomicUsize, Ordering};
    use loom::sync::Arc;
    use loom::thread;

    /// Minimal model of the two-atomics that matter: the `active` drain counter
    /// and the intent `flag` (one of `index2_create_barrier` /
    /// `schema_activation_barrier`). `writer_wrote` models the fast-path
    /// writer's data-store write landing — it is the thing the drain exists to
    /// make visible to the drainer's post-drain snapshot.
    ///
    /// `wrote_at_drain_return` is a snapshot of `writer_wrote` taken at the
    /// exact instant the drainer's spin loop observes `active == 0` and
    /// returns — i.e. the moment `drain()` returns in the real code. Asserting
    /// against THIS field (rather than against `writer_wrote` after the caller
    /// later joins the writer thread) is what makes the invariant meaningful:
    /// `join()` always blocks until the writer thread has fully finished and
    /// stored `writer_wrote`, so a post-`join` read of `writer_wrote` is true
    /// by construction and proves nothing about the drain protocol. Sampling
    /// at drain-return time is the only point at which the contract can
    /// actually be violated.
    struct Model {
        active: AtomicUsize,
        flag: AtomicBool,
        writer_wrote: AtomicBool,
        /// Snapshot of `writer_wrote` captured inside `run_drainer` at the
        /// instant its spin loop exits (i.e. when `drain()` returns).
        wrote_at_drain_return: AtomicBool,
    }

    impl Model {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                active: AtomicUsize::new(0),
                flag: AtomicBool::new(false),
                writer_wrote: AtomicBool::new(false),
                wrote_at_drain_return: AtomicBool::new(false),
            })
        }
    }

    /// Writer thread (fast path only — the drain contract concerns fast-path
    /// writers): `enter_writer` (SeqCst fetch_add) → read `flag` (SeqCst) → if
    /// `false`, model the write → drop the guard (SeqCst fetch_sub). Returns
    /// whether it took the fast path.
    fn run_writer(m: Arc<Model>) -> bool {
        m.active.fetch_add(1, Ordering::SeqCst);
        // Modeling-only fence (F-84, #912): compensates for loom 0.7's
        // no-op SeqCst-access enforcement — see this module's top-level doc.
        // No counterpart in the real code.
        fence(Ordering::SeqCst);
        let fast = !m.flag.load(Ordering::SeqCst);
        if fast {
            m.writer_wrote.store(true, Ordering::SeqCst);
        }
        m.active.fetch_sub(1, Ordering::SeqCst);
        fast
    }

    /// Drainer thread: raise `flag` (SeqCst store) → drain (spin on `active`
    /// until 0, SeqCst) → take the post-drain snapshot.
    ///
    /// The snapshot MUST be taken HERE — at the instant the spin loop exits and
    /// `drain()` returns — not by the caller after `join()`ing the writer. A
    /// post-`join` read of `writer_wrote` is trivially true whenever the writer
    /// took the fast path (the thread has fully returned), so it cannot witness
    /// the interleaving hole it is meant to detect.
    fn run_drainer(m: &Model) {
        m.flag.store(true, Ordering::SeqCst);
        // Modeling-only fence (F-84, #912): compensates for loom 0.7's
        // no-op SeqCst-access enforcement — see this module's top-level doc.
        // No counterpart in the real code.
        fence(Ordering::SeqCst);
        while m.active.load(Ordering::SeqCst) != 0 {
            thread::yield_now();
        }
        // drain() has returned — every fast-path writer is out of the drain
        // set. Snapshot writer_wrote RIGHT HERE, at the instant of return, so
        // the assertion samples the state the drainer actually observed at
        // return time rather than the strictly-later post-join state.
        m.wrote_at_drain_return
            .store(m.writer_wrote.load(Ordering::SeqCst), Ordering::SeqCst);
    }

    #[test]
    fn drain_returns_only_after_fast_path_writer_completes() {
        loom::model(|| {
            let m = Model::new();
            let m_w = Arc::clone(&m);
            let writer = thread::spawn(move || run_writer(m_w));
            run_drainer(&m);
            let took_fast = writer.join().unwrap();

            // THE INVARIANT (interleaving contract): once the drainer's drain
            // returns (active hit 0), a fast-path writer that entered the drain
            // set has completed its modeled write — the guard decrement, which
            // is what drove active back to 0, is sequenced AFTER the write.
            //
            // Assert against `wrote_at_drain_return` (sampled inside
            // `run_drainer` at the instant its spin loop exits), NOT against a
            // fresh read of `writer_wrote` here. Reading `writer_wrote` after
            // `join()` is tautological: `join()` blocks until the writer thread
            // has fully returned, and `run_writer` stores `writer_wrote` before
            // returning, so the value is always true — the assertion would pass
            // even if `drain()` were broken to a no-op. The snapshot at
            // drain-return time is the only observation point that can actually
            // witness a violation.
            if took_fast {
                assert!(
                    m.wrote_at_drain_return.load(Ordering::SeqCst),
                    "drain returned before the fast-path writer's write landed — \
                     an interleaving hole in the drain contract"
                );
            }
        });
    }
}
