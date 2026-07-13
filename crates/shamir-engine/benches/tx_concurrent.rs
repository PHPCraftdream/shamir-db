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
//! registered now.
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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::RecordKey;
use shamir_tx::mvcc_store::LockMode;
use shamir_tx::{IsolationLevel, MvccStore, RepoTxGate};
use shamir_types::types::value::InnerValue;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
    instance.add_table(TableConfig::new("bench_table".to_string()));
    instance
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

fn main() {
    let mut h = Harness::new("tx_concurrent", env!("CARGO_MANIFEST_DIR"));

    // Global iteration counter — makes keys unique across ALL calls to a
    // given workload's routine, mirroring the original `iter_i` Criterion
    // gave per-sample-iteration.
    let iter_ctr = Arc::new(AtomicU64::new(0));

    // --- Group 1: disjoint-key concurrent inserts ---------------------------

    // rows per writer.
    //
    // Scaled ladder collapsed to the smallest variant (n=1); the harness
    // now owns repetition count, so each call must stay a cheap unit.
    const K: usize = 10;
    for &n in &[1usize] {
        let iter_ctr = Arc::clone(&iter_ctr);
        h.bench_batched_async(
            &format!("disjoint_inserts/n_{n}"),
            move || {
                let repo = make_repo();
                let iter_i = iter_ctr.fetch_add(1, Ordering::Relaxed);
                async move {
                    let _ = repo.get_table("bench_table").await.unwrap();
                    (repo, iter_i)
                }
            },
            move |(repo, iter_i)| async move {
                let mut handles = Vec::with_capacity(n);
                for w in 0..n {
                    let repo = repo.clone();
                    let base = (iter_i as usize * n + w) * K;
                    handles.push(tokio::spawn(async move {
                        let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                        let tbl = repo.get_table("bench_table").await.unwrap();
                        for i in 0..K {
                            tbl.insert_tx(
                                &InnerValue::Str(format!("v_{}_{}", base, i)),
                                Some(&mut tx),
                            )
                            .await
                            .unwrap();
                        }
                        let out = repo.commit_tx(tx).await.unwrap();
                        std::hint::black_box(out);
                    }));
                }
                for hd in handles {
                    hd.await.unwrap();
                }
            },
        );
    }

    // --- Group 2: hot-key SSI conflicts (Snapshot) ---------------------------

    let hot_key_snapshot_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize].iter().enumerate() {
        let counters = Arc::clone(&hot_key_snapshot_counts);
        h.bench_batched_async(
            &format!("hot_key_snapshot/n_{n}"),
            move || {
                let repo = make_repo();
                async move {
                    let tbl = repo.get_table("bench_table").await.unwrap();
                    let rid = tbl.insert(&InnerValue::Str("seed".into())).await.unwrap();
                    drop(tbl);
                    (repo, rid)
                }
            },
            move |(repo, rid)| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let repo = repo.clone();
                        let counters = Arc::clone(&counters);
                        handles.push(tokio::spawn(async move {
                            let mut attempts = 0u64;
                            loop {
                                attempts += 1;
                                let (mut tx, _g) =
                                    repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                                let tbl = repo.get_table("bench_table").await.unwrap();
                                let upd = tbl
                                    .update_tx(
                                        rid,
                                        &InnerValue::Str(format!("v{}_{}", w, attempts)),
                                        Some(&mut tx),
                                    )
                                    .await;
                                if upd.is_err() {
                                    drop(tx);
                                    if attempts < 10 {
                                        continue;
                                    }
                                    panic!("hot-key writer exhausted 10 retries on stage");
                                }
                                let res = repo.commit_tx(tx).await;
                                match res {
                                    Ok(out) => {
                                        std::hint::black_box(&out);
                                        counters[idx].1.fetch_add(attempts - 1, Ordering::Relaxed);
                                        counters[idx].2.fetch_add(1, Ordering::Relaxed);
                                        return;
                                    }
                                    Err(_) if attempts < 10 => continue,
                                    Err(e) => panic!("hot-key writer exhausted 10 retries: {e:?}"),
                                }
                            }
                        }));
                    }
                    for hd in handles {
                        hd.await.unwrap();
                    }
                }
            },
        );
    }

    // --- Group 2b: hot-key SSI conflicts (Serializable) ----------------------

    let hot_key_serializable_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize].iter().enumerate() {
        let counters = Arc::clone(&hot_key_serializable_counts);
        h.bench_batched_async(
            &format!("hot_key_serializable/n_{n}"),
            move || {
                let repo = make_repo();
                async move {
                    let tbl = repo.get_table("bench_table").await.unwrap();
                    let rid = tbl.insert(&InnerValue::Str("seed".into())).await.unwrap();
                    drop(tbl);
                    (repo, rid)
                }
            },
            move |(repo, rid)| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let repo = repo.clone();
                        let counters = Arc::clone(&counters);
                        handles.push(tokio::spawn(async move {
                            let mut attempts = 0u64;
                            loop {
                                attempts += 1;
                                let (mut tx, _g) =
                                    repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
                                let tbl = repo.get_table("bench_table").await.unwrap();
                                let upd = tbl
                                    .update_tx(
                                        rid,
                                        &InnerValue::Str(format!("v{}_{}", w, attempts)),
                                        Some(&mut tx),
                                    )
                                    .await;
                                if upd.is_err() {
                                    drop(tx);
                                    if attempts < 20 {
                                        continue;
                                    }
                                    panic!(
                                        "hot-key serializable writer exhausted 20 retries on stage"
                                    );
                                }
                                let res = repo.commit_tx(tx).await;
                                match res {
                                    Ok(out) => {
                                        std::hint::black_box(&out);
                                        counters[idx].1.fetch_add(attempts - 1, Ordering::Relaxed);
                                        counters[idx].2.fetch_add(1, Ordering::Relaxed);
                                        return;
                                    }
                                    Err(_) if attempts < 20 => continue,
                                    Err(e) => panic!(
                                        "hot-key serializable writer exhausted 20 retries: {e:?}"
                                    ),
                                }
                            }
                        }));
                    }
                    for hd in handles {
                        hd.await.unwrap();
                    }
                }
            },
        );
    }

    // --- Group 3: pessimistic-lock uncontended -------------------------------

    {
        let mvcc = make_mvcc();
        let key = RecordKey::from_slice(b"hot_key");
        let ctr = Arc::new(AtomicU64::new(1_000_000));
        h.bench_batched_async(
            "pess_lock_uncontended/acquire_release_single_key",
            {
                let mvcc = Arc::clone(&mvcc);
                let key = key.clone();
                let ctr = Arc::clone(&ctr);
                move || {
                    let mvcc = Arc::clone(&mvcc);
                    let key = key.clone();
                    let tx_v = ctr.fetch_add(1, Ordering::Relaxed);
                    async move { (mvcc, key, tx_v) }
                }
            },
            move |(mvcc, key, tx_v)| async move {
                let wounded = Arc::new(AtomicBool::new(false));
                let notify = Arc::new(tokio::sync::Notify::new());
                mvcc.lock_key(
                    key.clone(),
                    tx_v,
                    Arc::clone(&wounded),
                    Arc::clone(&notify),
                    LockMode::Exclusive,
                )
                .await
                .unwrap();
                mvcc.release_locks(tx_v, std::slice::from_ref(&key)).await;
            },
        );
    }

    // --- Group 4: pessimistic-lock contended ---------------------------------

    let pess_contended_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize].iter().enumerate() {
        let counters = Arc::clone(&pess_contended_counts);
        let ctr = Arc::new(AtomicU64::new(0));
        h.bench_batched_async(
            &format!("pess_lock_contended/n_{n}"),
            {
                let ctr = Arc::clone(&ctr);
                move || {
                    let mvcc = make_mvcc();
                    let key = RecordKey::from_slice(b"contended_key");
                    let iter_i = ctr.fetch_add(1, Ordering::Relaxed);
                    async move { (mvcc, key, iter_i) }
                }
            },
            move |(mvcc, key, iter_i)| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let tx_v = 2_000_000u64 + iter_i * 100 + w as u64;
                        let mvcc = Arc::clone(&mvcc);
                        let key = key.clone();
                        let counters = Arc::clone(&counters);
                        handles.push(tokio::spawn(async move {
                            let wounded = Arc::new(AtomicBool::new(false));
                            let notify = Arc::new(tokio::sync::Notify::new());
                            let res = mvcc
                                .lock_key(
                                    key.clone(),
                                    tx_v,
                                    Arc::clone(&wounded),
                                    Arc::clone(&notify),
                                    LockMode::Exclusive,
                                )
                                .await;
                            match res {
                                Ok(()) => {
                                    tokio::task::yield_now().await;
                                    mvcc.release_locks(tx_v, std::slice::from_ref(&key)).await;
                                    counters[idx].2.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    counters[idx].1.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }));
                    }
                    for hd in handles {
                        hd.await.unwrap();
                    }
                }
            },
        );
    }

    // --- Group 4b: pessimistic-lock contended, REVERSE age -------------------

    let pess_reverse_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize].iter().enumerate() {
        let counters = Arc::clone(&pess_reverse_counts);
        let ctr = Arc::new(AtomicU64::new(0));
        h.bench_batched_async(
            &format!("pess_lock_contended_reverse_age/n_{n}"),
            {
                let ctr = Arc::clone(&ctr);
                move || {
                    let mvcc = make_mvcc();
                    let key = RecordKey::from_slice(b"contended_key");
                    let iter_i = ctr.fetch_add(1, Ordering::Relaxed);
                    async move { (mvcc, key, iter_i) }
                }
            },
            move |(mvcc, key, iter_i)| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let tx_v = 2_000_000u64 + iter_i * 100 + (n as u64 - 1 - w as u64);
                        let mvcc = Arc::clone(&mvcc);
                        let key = key.clone();
                        let counters = Arc::clone(&counters);
                        handles.push(tokio::spawn(async move {
                            let wounded = Arc::new(AtomicBool::new(false));
                            let notify = Arc::new(tokio::sync::Notify::new());
                            let res = mvcc
                                .lock_key(
                                    key.clone(),
                                    tx_v,
                                    Arc::clone(&wounded),
                                    Arc::clone(&notify),
                                    LockMode::Exclusive,
                                )
                                .await;
                            match res {
                                Ok(()) => {
                                    tokio::task::yield_now().await;
                                    mvcc.release_locks(tx_v, std::slice::from_ref(&key)).await;
                                    counters[idx].2.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    counters[idx].1.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }));
                    }
                    for hd in handles {
                        hd.await.unwrap();
                    }
                }
            },
        );
    }

    // --- Group 7: pessimistic-lock contended, REVERSE age + BARRIER ---------

    let pess_barrier_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize].iter().enumerate() {
        let counters = Arc::clone(&pess_barrier_counts);
        let ctr = Arc::new(AtomicU64::new(0));
        h.bench_batched_async(
            &format!("pess_lock_contended_barrier/n_{n}"),
            {
                let ctr = Arc::clone(&ctr);
                move || {
                    let mvcc = make_mvcc();
                    let key = RecordKey::from_slice(b"contended_key");
                    let barrier = Arc::new(tokio::sync::Barrier::new(n));
                    let iter_i = ctr.fetch_add(1, Ordering::Relaxed);
                    async move { (mvcc, key, barrier, iter_i) }
                }
            },
            move |(mvcc, key, barrier, iter_i)| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let tx_v = 2_000_000u64 + iter_i * 100 + (n as u64 - 1 - w as u64);
                        let mvcc = Arc::clone(&mvcc);
                        let key = key.clone();
                        let counters = Arc::clone(&counters);
                        let barrier = Arc::clone(&barrier);
                        handles.push(tokio::spawn(async move {
                            let wounded = Arc::new(AtomicBool::new(false));
                            let notify = Arc::new(tokio::sync::Notify::new());
                            barrier.wait().await;
                            let res = mvcc
                                .lock_key(
                                    key.clone(),
                                    tx_v,
                                    Arc::clone(&wounded),
                                    Arc::clone(&notify),
                                    LockMode::Exclusive,
                                )
                                .await;
                            match res {
                                Ok(()) => {
                                    tokio::task::yield_now().await;
                                    mvcc.release_locks(tx_v, std::slice::from_ref(&key)).await;
                                    counters[idx].2.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(_) => {
                                    counters[idx].1.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }));
                    }
                    for hd in handles {
                        hd.await.unwrap();
                    }
                }
            },
        );
    }

    // --- Group 8: pessimistic-lock contended, pre-lock barrier + in-CS hold -

    let pess_in_cs_barrier_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize].iter().enumerate() {
        let counters = Arc::clone(&pess_in_cs_barrier_counts);
        let ctr = Arc::new(AtomicU64::new(0));
        h.bench_batched_async(
            &format!("pess_lock_contended_in_cs_barrier/n_{n}"),
            {
                let ctr = Arc::clone(&ctr);
                move || {
                    let mvcc = make_mvcc();
                    let key = RecordKey::from_slice(b"contended_key");
                    let barrier = Arc::new(tokio::sync::Barrier::new(n));
                    let iter_i = ctr.fetch_add(1, Ordering::Relaxed);
                    async move { (mvcc, key, barrier, iter_i) }
                }
            },
            move |(mvcc, key, barrier, iter_i)| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let tx_v = 2_000_000u64 + iter_i * 100 + (n as u64 - 1 - w as u64);
                        let mvcc = Arc::clone(&mvcc);
                        let key = key.clone();
                        let counters = Arc::clone(&counters);
                        let barrier = Arc::clone(&barrier);
                        handles.push(tokio::spawn(async move {
                            let wounded = Arc::new(AtomicBool::new(false));
                            let notify = Arc::new(tokio::sync::Notify::new());
                            barrier.wait().await;
                            let res = mvcc
                                .lock_key(
                                    key.clone(),
                                    tx_v,
                                    Arc::clone(&wounded),
                                    Arc::clone(&notify),
                                    LockMode::Exclusive,
                                )
                                .await;
                            match res {
                                Ok(()) => {
                                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                                    mvcc.release_locks(tx_v, std::slice::from_ref(&key)).await;
                                    if wounded.load(Ordering::Acquire) {
                                        counters[idx].1.fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        counters[idx].2.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(_) => {
                                    counters[idx].1.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }));
                    }
                    for hd in handles {
                        hd.await.unwrap();
                    }
                }
            },
        );
    }

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
