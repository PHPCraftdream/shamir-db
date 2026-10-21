// Single-element `for` loops are intentional: the N-ladders were collapsed
// to their smallest variant when migrating to the fixed-iteration harness,
// but the loop structure is kept so the ladder can be re-expanded ad-hoc.
#![allow(clippy::single_element_loop)]
//! Transaction concurrency / SSI conflict / Level-3 pessimistic-lock bench
//! coverage. Complements the single-threaded `tx_pipeline` / `tx_overhead`
//! benches (which only measure the no-contention floor).
//!
//! Eight groups (four original + four follow-ups added after first-round
//! review surfaced the structural-zero-aborts / zero-wounds findings):
//!
//! NOTE: the N-ladders in every group were collapsed to their smallest
//! variant (n=1 for group 1, n=2 for the rest) when this bench moved to
//! the fixed-iteration `bench_scale_tool` harness — the harness now owns
//! repetition count, so each registered call must stay a cheap unit. The
//! per-group `N ∈ {…}` ranges below describe the original Criterion
//! coverage and are kept for historical context; only the smallest N is
//! registered now, except the disjoint-tables n=4/8 family below.
//!
//! 1. `tx_concurrent/disjoint_inserts` — N concurrent writers, each inserting
//!    into the same table at DISJOINT keys (no SSI / lock conflict). Reveals
//!    the "bus contention floor": how much commit throughput drops purely
//!    from coordination (per-repo commit serialisation in `RepoTxGate`,
//!    `scc::HashMap` CAS traffic, runtime scheduling).  N ∈ {1,2,4,8}.
//!
//! 2. `tx_concurrent/hot_key_snapshot` — N concurrent Snapshot-isolation
//!    writers all updating the SAME existing rid. Measures pure write
//!    serialisation on a hot key under Snapshot: aborts are STRUCTURALLY
//!    impossible here (Snapshot's read-set is empty, validate_read_set is a
//!    no-op — see `shamir-tx/src/tx_context.rs::record_read_shared`). The
//!    retry-loop + abort counter (printed via `eprintln!`) are defensive
//!    scaffolding for a `hot_key_serializable` sibling to be added later,
//!    which WILL exercise real aborts. Total wall-clock IS the user-visible
//!    commit latency under contention.  N ∈ {2,4,8}.
//!
//! 3. `tx_concurrent/pess_lock_uncontended` — `MvccStore::lock_key` acquire +
//!    release on a single key with no contender. The cost floor of the Level-3
//!    lock primitive — the bench future tuning regressions will hit first.
//!
//! 4. `tx_concurrent/pess_lock_contended` — N concurrent tasks all racing
//!    for the SAME key in pessimistic (wound-wait) mode. Measures
//!    contended-acquire / wait throughput. Empirical wound rate is
//!    expected ≈0 in this setup because tx_v is assigned in spawn order
//!    (older first), so arrival order ≈ age order — younger requesters
//!    simply wait on older holders rather than wound them. The whole
//!    iteration is bounded — a real deadlock would manifest as a hang,
//!    not a wrong number. Counter printed via `eprintln!`.  N ∈ {2,4,8}.
//!
//! 5. `tx_concurrent/hot_key_serializable` — Group 2's sibling under
//!    Serializable isolation. `update_tx`'s internal old-value read
//!    populates the read-set; concurrent updates create overlapping
//!    read/write sets; `validate_read_set` aborts the losers; the retry-
//!    loop kicks in. Abort rates measured (eprintln'd): ~0.49 (n=2) →
//!    1.33 (n=4) → 3.01 (n=8) aborts per successful commit. This IS the
//!    real SSI conflict cost — Group 2 is the floor where it doesn't fire.
//!    Retry cap raised to 20 (vs Group 2's 10) since Serializable
//!    contention can chain.  N ∈ {2,4,8}.
//!
//! 6. `tx_concurrent/pess_lock_contended_reverse_age` — Group 4's mirror
//!    with `tx_v = base + (n - 1 - w)`: youngest tasks acquire first,
//!    older arrivals SHOULD wound the holder. **In current measurement
//!    the wound rate is still 0** — the critical section (one
//!    `yield_now`) is too short; by the time the older arrival reaches
//!    `lock_key`, the younger holder has already released. Real follow-up
//!    needed: insert a synchronisation barrier so all N tasks hold the
//!    lock-attempt phase simultaneously before any release, then wounds
//!    will fire. Group 6 lands the spawn-order-correctness scaffold; the
//!    barrier-driven variant is the next bench-debt item.  N ∈ {2,4,8}.
//!
//! 7. `tx_concurrent/pess_lock_contended_barrier` — Group 6's follow-up
//!    with a `tokio::sync::Barrier::new(n)` released RIGHT BEFORE each
//!    task's `lock_key` call. All N tasks reach the barrier together,
//!    are released together, and race for `lock_key` in the same
//!    instant. Empirical result: **still 0 wounds** at n ∈ {2,4,8}
//!    over hundreds of thousands of acquires. Finding: the critical
//!    section is `yield_now` only — by the time a losing contender's
//!    `lock_key` enqueues, the winner has already released, so the
//!    wound path is never entered. To actually trigger wounds, the
//!    holder must STAY in the CS while contenders enqueue — i.e. a
//!    second barrier (or a `Notify`) parked inside the held section.
//!    Group 7 lands the pre-lock barrier scaffold; the in-CS barrier
//!    is the next bench-debt item.  N ∈ {2,4,8}.
//!
//! 8. `tx_concurrent/pess_lock_contended_in_cs_barrier` — Group 7's
//!    follow-up. Two synchronisation points: (a) the same pre-lock
//!    `Barrier::new(n)` to align all N tasks before they race for the
//!    lock, and (b) an in-CS HOLD — the winning acquirer sleeps for
//!    2 ms before releasing, long enough for the N-1 contenders to
//!    enter `lock_key` and reach the wound/wait decision while the
//!    holder is still in the CS. Reverse-age `tx_v` (same shape as
//!    Group 6/7). Crucially, the wound SIGNAL is read from the
//!    holder's `wounded` AtomicBool AFTER the CS — not from the
//!    contender's `lock_key` return — because in wound-wait the older
//!    contender just flips the younger holder's `wounded` flag and
//!    then itself acquires `Ok(())` once the (now-dead) holder
//!    releases. Empirically this fires the wound path: at n=2 ~1
//!    wound/iter, at n=4/8 most spawn-groups produce ≥1 wound.
//!    Per-iter time is ≥2 ms by construction (the in-CS sleep is the
//!    dominant cost). Alternative considered: a `tokio::sync::Notify`
//!    coordinated from contenders into the holder; rejected because
//!    contenders that park in `lock_key`'s wait queue cannot easily
//!    signal "I have enqueued" without instrumenting the lock itself.
//!    The 2 ms sleep is a heuristic — long enough for wake-up jitter
//!    on a multi-thread runtime, short enough to keep the bench
//!    tractable.  N ∈ {2,4,8}.
//!
//! 9. `tx_concurrent/fk_child_disjoint_inserts` — F-46 (#857, P0) concurrency
//!    impact measurement. N concurrent Snapshot writers, each inserting a
//!    DISJOINT row into a table with a REQUIRED foreign key (so every write
//!    runs `require_footprint_if_fk_child` and gets `footprint_tokens`
//!    populated). Since F-46 widened `commit_tx_lockfree`'s commit-lock
//!    condition to also fire on non-empty `footprint_tokens`
//!    (`crates/shamir-engine/src/tx/commit.rs`), EVERY commit into this
//!    table now takes the process-wide `commit_lock` — previously these
//!    disjoint-key writers ran fully lock-free (same shape as Group 1). This
//!    group is the direct before/after comparator for that regression: same
//!    N-writer/K-row/disjoint-key shape as Group 1's `disjoint_inserts`, the
//!    only difference is the target table is an FK child. Compare this
//!    group's throughput against Group 1's at the same N to isolate the
//!    fix's marginal cost. Driven through `execute_batch` (not raw
//!    `insert_tx`) because `require_footprint_if_fk_child` is a
//!    `query_runner.rs` hook — the raw `TableManager::insert_tx` entry point
//!    Group 1 uses bypasses it entirely.  N ∈ {1,2,4,8}.
//!
//! Noise budget. Contention benches are inherently noisier than
//! single-thread benches: schedulers, futex wake latency, and abort-retry
//! tails all add jitter. Expect ±30% variance on contended groups;
//! trends across N matter more than absolute numbers.
//!
//! This bench drives the engine's typed `insert_tx`/`update_tx` directly
//! (same pattern as `tx_pipeline`) — the groups under test ARE the engine
//! primitives, not the query-builder surface. The Level-3 lock benches go
//! one level deeper, into `MvccStore::lock_key`, for the same reason.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`). Every
//! group provisions a FRESH repo/MvccStore per iteration (disjoint keys,
//! retry-history, and lock state must not bleed across iterations), so
//! every workload uses `bench_batched_async` — setup builds the fresh
//! repo/store, the routine spawns and joins the N concurrent tasks. The
//! observational abort/wound counters are process-global `Arc<...>`
//! constructed once outside the harness registration and dumped via
//! `eprintln!` after `h.run()` returns — same "outside the timed window"
//! contract as the original Criterion version.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::batch::{
    execute_batch as execute_batch_raw, AccessGate, AdminExecutor, Authorized, BatchError,
    BatchRequest, BatchResponse, FunctionInvoker, TableResolver,
};
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::access::{AccessError, Action, Actor, ResourcePath};

/// Bench-only always-allow [`AccessGate`] (#1199): this bench measures
/// execution mechanics, not the authorization seam, so the bypass is an
/// explicit, named choice rather than a silent default.
struct BenchAllowAll;

#[async_trait::async_trait]
impl AccessGate for BenchAllowAll {
    async fn check(
        &self,
        _actor: &Actor,
        _path: &ResourcePath,
        _action: Action,
    ) -> Result<(), AccessError> {
        Ok(())
    }
}

/// Pre-#1199 call shape, preserved for this bench: mints an [`Authorized`]
/// token via [`BenchAllowAll`] and calls the real `execute_batch`.
async fn execute_batch(
    request: &BatchRequest,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: Actor,
    db_name: &str,
) -> Result<BatchResponse, BatchError> {
    let auth = Authorized::authorize(request, actor, db_name, &BenchAllowAll)
        .await
        .expect("BenchAllowAll never denies");
    execute_batch_raw(auth, resolver, admin, invoker).await
}

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let tables = vec![TableConfig::new("bench_table".to_string())];
    RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), tables)
}

fn make_mvcc() -> Arc<MvccStore> {
    let gate = Arc::new(RepoTxGate::fresh());
    Arc::new(MvccStore::new(Arc::new(InMemoryStore::new()), gate))
}

// NOTE: Engine-internal tx APIs (`insert_tx` / `update_tx` / `begin_tx` /
// `commit_tx`) are used directly here. These are NOT "query construction"
// — they are the engine's typed entry points; the wire-shape builders in
// `shamir-query-builder` produce DTOs for the SDBQL surface (which then
// fan out to these same engine calls). Existing `tx_pipeline` benches
// follow the same pattern.

// Split out of this file (layout sweep Q2): workload groups 1-8 and the
// F-46 FK-child group live in `tx_concurrent_parts/`; this root keeps the
// helpers, the harness wiring and the observational dumps.
#[path = "tx_concurrent_parts/mod.rs"]
mod tx_concurrent_parts;

fn main() {
    let mut h = Harness::new("tx_concurrent", env!("CARGO_MANIFEST_DIR"));

    let counts = tx_concurrent_parts::workloads::run(&mut h);
    tx_concurrent_parts::fk_child_workload::run(&mut h);

    let tx_concurrent_parts::workloads::WCounts {
        hot_key_snapshot_counts,
        hot_key_serializable_counts,
        pess_contended_counts,
        pess_reverse_counts,
        pess_barrier_counts,
        pess_in_cs_barrier_counts,
    } = counts;

    h.run();

    // Observational dumps — outside the timed window.
    for (n, aborts, commits) in hot_key_snapshot_counts.iter() {
        let a = aborts.load(Ordering::Relaxed);
        let c = commits.load(Ordering::Relaxed);
        if c > 0 {
            eprintln!(
                "hot_key_snapshot n={n}: {a} aborts over {c} successful commits (~{:.2} aborts/commit)",
                a as f64 / c as f64
            );
        }
    }
    for (n, aborts, commits) in hot_key_serializable_counts.iter() {
        let a = aborts.load(Ordering::Relaxed);
        let c = commits.load(Ordering::Relaxed);
        if c > 0 {
            eprintln!(
                "hot_key_serializable n={n}: {a} aborts over {c} successful commits (~{:.2} aborts/commit)",
                a as f64 / c as f64
            );
        }
    }
    for (n, wounds, acquires) in pess_contended_counts.iter() {
        let w = wounds.load(Ordering::Relaxed);
        let a = acquires.load(Ordering::Relaxed);
        let tot = w + a;
        if tot > 0 {
            eprintln!(
                "pess_lock_contended n={n}: {w} wounds, {a} clean acquires ({:.1}% wound rate)",
                100.0 * w as f64 / tot as f64
            );
        }
    }
    for (n, wounds, acquires) in pess_reverse_counts.iter() {
        let w = wounds.load(Ordering::Relaxed);
        let a = acquires.load(Ordering::Relaxed);
        let tot = w + a;
        if tot > 0 {
            eprintln!(
                "pess_lock_contended_reverse_age n={n}: {w} wounds, {a} clean acquires \
                 ({:.1}% wound rate)",
                100.0 * w as f64 / tot as f64
            );
        }
    }
    for (n, wounds, acquires) in pess_barrier_counts.iter() {
        let w = wounds.load(Ordering::Relaxed);
        let a = acquires.load(Ordering::Relaxed);
        let tot = w + a;
        if tot > 0 {
            eprintln!(
                "pess_lock_contended_barrier n={n}: {w} wounds, {a} clean acquires \
                 ({:.1}% wound rate)",
                100.0 * w as f64 / tot as f64
            );
        }
    }
    for (n, wounds, acquires) in pess_in_cs_barrier_counts.iter() {
        let w = wounds.load(Ordering::Relaxed);
        let a = acquires.load(Ordering::Relaxed);
        let tot = w + a;
        if tot > 0 {
            eprintln!(
                "pess_lock_contended_in_cs_barrier n={n}: {w} wounds, {a} clean acquires \
                 ({:.1}% wound rate)",
                100.0 * w as f64 / tot as f64
            );
        }
    }

}
