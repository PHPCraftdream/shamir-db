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
/// # Memory model (F-56, #882 — corrected)
///
/// The four cross-atomic operations are ALL `Ordering::SeqCst`:
///   - writer  `active.fetch_add` (`enter_writer`)
///   - writer  `flag.load` (`needs_write_barrier`)
///   - drainer `flag.store(true)` (`set_schema_activation_barrier` / `IndexCreateBarrierGuard`)
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
/// raised. It is called from `SchemaActivationBarrierGuard::raise`
/// (schema-activation DDL, F-48) and from `create_index_v2`
/// (`table_manager_index_mgmt.rs`, F-56) — after raising the intent flag,
/// before the snapshot/proof point — identically.
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
    /// The caller MUST have already (1) raised its intent flag (`Release`)
    /// so NEW writers take the slow (locked) path, and (2) hold
    /// `unique_write_lock` so slow-path writers are blocked. Then this
    /// catches any writer that read `false` before the flag went up.
    ///
    /// When no writers are active, returns after a single `SeqCst` load.
    pub async fn drain(&self) {
        // F-68 (#895) cluster D / task #124 diagnostic instrumentation.
        // NOT a behavior change: this is timestamped logging only, no new
        // await point, no retry/timeout.
        //
        // This is the DDL side of the suspected lock-order inversion (task
        // #897): every caller (`create_index_v2`, `create_index`,
        // `create_unique_index_locked`, `SchemaActivationBarrierGuard::raise`)
        // acquires `unique_write_lock` FIRST and calls `drain()` SECOND, while
        // `pre_commit_prelock` (crates/shamir-engine/src/tx/pre_commit.rs)
        // enters this SAME drain set first and may later acquire
        // `unique_write_lock` for a DIFFERENT table while still holding this
        // table's drain-set membership. If a DDL's `drain()` call never
        // returns, this loop logs a "still waiting" warning periodically (not
        // every iteration — that would flood the log) so a stuck CI run shows
        // the entered-but-never-drained state with a timestamp gap that lines
        // up with the kill.
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
//   cargo test -p shamir-engine --features loom --lib \
//       table::writer_drain_barrier::loom_model -- --nocapture
//
// ## Honest scope — what this model does and does NOT prove
//
// loom explores all SEQUENTIALLY-CONSISTENT thread interleavings of the model.
// The F-56 memory-ordering bug fixed above is NOT an interleaving bug: it is a
// cross-atomic WEAK-MEMORY reordering (a `Relaxed fetch_add` on `active` whose
// visibility is not ordered relative to an `Acquire load` on a DIFFERENT
// atomic, `flag`). No SC interleaving exhibits it — in every interleaving where
// the writer's `flag.load` reads `false`, the writer's `fetch_add` has already
// executed in program order, so the drainer's `active.load` observes `>= 1`.
// Consequently this model PASSES for BOTH the old (Relaxed/Acquire/Release)
// and the new (SeqCst) orderings; it CANNOT red/green the old code. The
// weak-memory soundness of the SeqCst fix rests entirely on the worked proof
// in the [`WriterDrainBarrier`] doc comment above, NOT on this model.
//
// What this model IS good for: guarding the protocol's INTERLEAVING contract
// against future regressions — a broken `drain` loop (e.g. one that exits while
// `active != 0`), a missing guard decrement, a slow path that fails to exit the
// drain set before blocking on the lock (the deadlock shape the comment above
// warns about), or a reordering of `enter_writer` AFTER the flag read. Any of
// those would show up here as an interleaving that violates the invariant
// "after `drain()` returns, every fast-path writer that entered the drain set
// has completed its write". That is the contract the SeqCst proof's step 5
// (drain-load observes the increment) ultimately serves, so it is worth
// pinning down — just not as a stand-in for the weak-memory argument.
// ============================================================================
#[cfg(loom)]
mod loom_model {
    use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use loom::sync::Arc;
    use loom::thread;

    /// Minimal model of the two-atomics that matter: the `active` drain counter
    /// and the intent `flag` (one of `index2_create_barrier` /
    /// `schema_activation_barrier`). `writer_wrote` models the fast-path
    /// writer's data-store write landing — it is the thing the drain exists to
    /// make visible to the drainer's post-drain snapshot.
    struct Model {
        active: AtomicUsize,
        flag: AtomicBool,
        writer_wrote: AtomicBool,
    }

    impl Model {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                active: AtomicUsize::new(0),
                flag: AtomicBool::new(false),
                writer_wrote: AtomicBool::new(false),
            })
        }
    }

    /// Writer thread (fast path only — the drain contract concerns fast-path
    /// writers): `enter_writer` (SeqCst fetch_add) → read `flag` (SeqCst) → if
    /// `false`, model the write → drop the guard (SeqCst fetch_sub). Returns
    /// whether it took the fast path.
    fn run_writer(m: Arc<Model>) -> bool {
        m.active.fetch_add(1, Ordering::SeqCst);
        let fast = !m.flag.load(Ordering::SeqCst);
        if fast {
            m.writer_wrote.store(true, Ordering::SeqCst);
        }
        m.active.fetch_sub(1, Ordering::SeqCst);
        fast
    }

    /// Drainer thread: raise `flag` (SeqCst store) → drain (spin on `active`
    /// until 0, SeqCst) → take the post-drain snapshot.
    fn run_drainer(m: &Model) {
        m.flag.store(true, Ordering::SeqCst);
        while m.active.load(Ordering::SeqCst) != 0 {
            thread::yield_now();
        }
        // drain() has returned — every fast-path writer is out of the drain set.
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
            if took_fast {
                assert!(
                    m.writer_wrote.load(Ordering::SeqCst),
                    "drain returned before the fast-path writer's write landed — \
                     an interleaving hole in the drain contract"
                );
            }
        });
    }
}
