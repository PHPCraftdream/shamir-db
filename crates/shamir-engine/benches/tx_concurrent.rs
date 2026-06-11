//! Transaction concurrency / SSI conflict / Level-3 pessimistic-lock bench
//! coverage. Complements the single-threaded `tx_pipeline` / `tx_overhead`
//! benches (which only measure the no-contention floor).
//!
//! Seven groups (four original + three follow-ups added after first-round
//! review surfaced the structural-zero-aborts / zero-wounds findings):
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
//! Noise budget. Contention benches are inherently noisier than
//! single-thread benches: schedulers, futex wake latency, and abort-retry
//! tails all add jitter. Expect ±30% variance on contended groups;
//! trends across N matter more than absolute numbers. `sample_size = 20`
//! and `measurement_time = 5s` are picked accordingly.
//!
//! This bench drives the engine's typed `insert_tx`/`update_tx` directly
//! (same pattern as `tx_pipeline`) — the groups under test ARE the engine
//! primitives, not the query-builder surface. The Level-3 lock benches go
//! one level deeper, into `MvccStore::lock_key`, for the same reason.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_tx::mvcc_store::LockMode;
use shamir_tx::{IsolationLevel, MvccStore, RepoTxGate};
use shamir_types::types::value::InnerValue;

// --- runtime / resolver plumbing -------------------------------------------

fn rt() -> tokio::runtime::Runtime {
    // Multi-thread is load-bearing for this whole file — current_thread
    // would serialise the N spawned tasks and erase the contention signal.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
    instance.add_table(TableConfig::new("bench_table".to_string()));
    instance
}

// NOTE: Engine-internal tx APIs (`insert_tx` / `update_tx` / `begin_tx` /
// `commit_tx`) are used directly here. These are NOT "query construction"
// — they are the engine's typed entry points; the wire-shape builders in
// `shamir-query-builder` produce DTOs for the SDBQL surface (which then
// fan out to these same engine calls). Existing `tx_pipeline` benches
// follow the same pattern.

// --- Group 1: disjoint-key concurrent inserts ------------------------------

fn bench_disjoint_inserts(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("tx_concurrent/disjoint_inserts");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    const K: usize = 10; // rows per writer

    for &n in &[1usize, 2, 4, 8] {
        group.throughput(Throughput::Elements((n * K) as u64));
        let fn_name = format!("n_{}", n);
        group.bench_function(BenchmarkId::from_parameter(&fn_name), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for iter_i in 0..iters {
                    // Fresh repo per iter — disjoint keys never collide across iters either.
                    let repo = make_repo();
                    let _ = repo.get_table("bench_table").await.unwrap();

                    let start = Instant::now();
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let repo = repo.clone();
                        let base = (iter_i as usize * n + w) * K;
                        handles.push(tokio::spawn(async move {
                            let (mut tx, _g) =
                                repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
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
                            black_box(out);
                        }));
                    }
                    for h in handles {
                        h.await.unwrap();
                    }
                    total += start.elapsed();
                }
                total
            });
        });
    }

    group.finish();
}

// --- Group 2: hot-key SSI conflicts ----------------------------------------

fn bench_hot_key_snapshot(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("tx_concurrent/hot_key_snapshot");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    // Observational abort counters. Printed after the criterion run; NOT in
    // the timed window.
    let abort_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize, 4, 8]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize, 4, 8].iter().enumerate() {
        group.throughput(Throughput::Elements(n as u64));
        let fn_name = format!("n_{}", n);
        let counters = Arc::clone(&abort_counts);
        group.bench_function(BenchmarkId::from_parameter(&fn_name), |b| {
            let counters = Arc::clone(&counters);
            b.to_async(&rt).iter_custom(move |iters| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        // Fresh repo per iter so retry-history doesn't bleed.
                        let repo = make_repo();
                        let tbl = repo.get_table("bench_table").await.unwrap();
                        // Seed the hot row.
                        let rid = tbl.insert(&InnerValue::Str("seed".into())).await.unwrap();
                        drop(tbl);

                        let start = Instant::now();
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
                                    // Stage the update of the same hot rid.
                                    let upd = tbl
                                        .update_tx(
                                            rid,
                                            &InnerValue::Str(format!("v{}_{}", w, attempts)),
                                            Some(&mut tx),
                                        )
                                        .await;
                                    if upd.is_err() {
                                        // Drop tx implicitly aborts.
                                        drop(tx);
                                        if attempts < 10 {
                                            continue;
                                        }
                                        panic!("hot-key writer exhausted 10 retries on stage");
                                    }
                                    let res = repo.commit_tx(tx).await;
                                    match res {
                                        Ok(out) => {
                                            black_box(&out);
                                            counters[idx]
                                                .1
                                                .fetch_add(attempts - 1, Ordering::Relaxed);
                                            counters[idx].2.fetch_add(1, Ordering::Relaxed);
                                            return;
                                        }
                                        Err(_) if attempts < 10 => continue,
                                        Err(e) => {
                                            panic!("hot-key writer exhausted 10 retries: {e:?}")
                                        }
                                    }
                                }
                            }));
                        }
                        for h in handles {
                            h.await.unwrap();
                        }
                        total += start.elapsed();
                    }
                    total
                }
            });
        });
    }

    group.finish();

    // Observational dump — outside the timed window.
    for (n, aborts, commits) in abort_counts.iter() {
        let a = aborts.load(Ordering::Relaxed);
        let c = commits.load(Ordering::Relaxed);
        if c > 0 {
            eprintln!(
                "hot_key_snapshot n={n}: {a} aborts over {c} successful commits (~{:.2} aborts/commit)",
                a as f64 / c as f64
            );
        }
    }
}

// --- Group 3: pessimistic-lock uncontended ---------------------------------

fn make_mvcc() -> Arc<MvccStore> {
    let gate = Arc::new(RepoTxGate::fresh());
    Arc::new(MvccStore::new(Arc::new(InMemoryStore::new()), gate))
}

fn bench_pess_lock_uncontended(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("tx_concurrent/pess_lock_uncontended");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));
    group.throughput(Throughput::Elements(1));

    group.bench_function("acquire_release_single_key", |b| {
        let mvcc = make_mvcc();
        let key = Bytes::from_static(b"hot_key");
        b.to_async(&rt).iter_custom(|iters| {
            let mvcc = Arc::clone(&mvcc);
            let key = key.clone();
            async move {
                let start = Instant::now();
                for i in 0..iters {
                    let tx_v = 1_000_000u64 + i;
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
                }
                start.elapsed()
            }
        });
    });

    group.finish();
}

// --- Group 4: pessimistic-lock contended -----------------------------------

fn bench_pess_lock_contended(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("tx_concurrent/pess_lock_contended");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    // Observational wound counters.
    let wound_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize, 4, 8]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize, 4, 8].iter().enumerate() {
        group.throughput(Throughput::Elements(n as u64));
        let fn_name = format!("n_{}", n);
        let counters = Arc::clone(&wound_counts);
        group.bench_function(BenchmarkId::from_parameter(&fn_name), |b| {
            let counters = Arc::clone(&counters);
            b.to_async(&rt).iter_custom(move |iters| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut total = Duration::ZERO;
                    for iter_i in 0..iters {
                        // Fresh MvccStore per iter — no carry-over locks.
                        let mvcc = make_mvcc();
                        let key = Bytes::from_static(b"contended_key");

                        let start = Instant::now();
                        let mut handles = Vec::with_capacity(n);
                        for w in 0..n {
                            // tx_v strictly ordered: smaller = older = wound-wait
                            // winner. Iter offset keeps tx_vs globally unique.
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
                                        // Trivial critical section: yield once
                                        // so the runtime can wake waiters
                                        // and we exercise the contended path,
                                        // then release.
                                        tokio::task::yield_now().await;
                                        mvcc.release_locks(tx_v, std::slice::from_ref(&key)).await;
                                        counters[idx].2.fetch_add(1, Ordering::Relaxed);
                                    }
                                    Err(_) => {
                                        // Wounded — older requester aborted us.
                                        counters[idx].1.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }));
                        }
                        for h in handles {
                            // A real deadlock manifests as a hang here;
                            // wound-wait is deadlock-free by construction,
                            // so this MUST complete.
                            h.await.unwrap();
                        }
                        total += start.elapsed();
                    }
                    total
                }
            });
        });
    }

    group.finish();

    for (n, wounds, acquires) in wound_counts.iter() {
        let w = wounds.load(Ordering::Relaxed);
        let a = acquires.load(Ordering::Relaxed);
        let tot = w + a;
        if tot > 0 {
            eprintln!(
                "pess_lock_contended n={n}: {w} wounds, {a} clean acquires \
                 ({:.1}% wound rate)",
                100.0 * w as f64 / tot as f64
            );
        }
    }
}

// --- Group 2b: hot-key SSI Serializable ------------------------------------
//
// Sibling of `bench_hot_key_snapshot`. The ONLY difference is the isolation
// level: Serializable. Under Serializable, `update_tx`'s internal old-value
// read populates the SSI read-set; concurrent updates produce overlapping
// read/write sets; `validate_read_set` aborts the losers — the retry-loop
// kicks in. Retry cap bumped to 20 (vs 10 for Snapshot) because contention
// tails can be longer here.

fn bench_hot_key_serializable(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("tx_concurrent/hot_key_serializable");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    let abort_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize, 4, 8]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize, 4, 8].iter().enumerate() {
        group.throughput(Throughput::Elements(n as u64));
        let fn_name = format!("n_{}", n);
        let counters = Arc::clone(&abort_counts);
        group.bench_function(BenchmarkId::from_parameter(&fn_name), |b| {
            let counters = Arc::clone(&counters);
            b.to_async(&rt).iter_custom(move |iters| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let repo = make_repo();
                        let tbl = repo.get_table("bench_table").await.unwrap();
                        let rid = tbl.insert(&InnerValue::Str("seed".into())).await.unwrap();
                        drop(tbl);

                        let start = Instant::now();
                        let mut handles = Vec::with_capacity(n);
                        for w in 0..n {
                            let repo = repo.clone();
                            let counters = Arc::clone(&counters);
                            handles.push(tokio::spawn(async move {
                                let mut attempts = 0u64;
                                loop {
                                    attempts += 1;
                                    let (mut tx, _g) = repo
                                        .begin_tx(IsolationLevel::Serializable)
                                        .await
                                        .unwrap();
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
                                        panic!("hot-key serializable writer exhausted 20 retries on stage");
                                    }
                                    let res = repo.commit_tx(tx).await;
                                    match res {
                                        Ok(out) => {
                                            black_box(&out);
                                            counters[idx]
                                                .1
                                                .fetch_add(attempts - 1, Ordering::Relaxed);
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
                        for h in handles {
                            h.await.unwrap();
                        }
                        total += start.elapsed();
                    }
                    total
                }
            });
        });
    }

    group.finish();

    for (n, aborts, commits) in abort_counts.iter() {
        let a = aborts.load(Ordering::Relaxed);
        let c = commits.load(Ordering::Relaxed);
        if c > 0 {
            eprintln!(
                "hot_key_serializable n={n}: {a} aborts over {c} successful commits (~{:.2} aborts/commit)",
                a as f64 / c as f64
            );
        }
    }
}

// --- Group 4b: pessimistic-lock contended, REVERSE age ---------------------
//
// Sibling of `bench_pess_lock_contended`. The only difference: tx_v is
// assigned in REVERSE spawn order — spawn-0 gets the highest tx_v
// (youngest), spawn-(n-1) gets the lowest (oldest). Older arrivals then
// wound the younger holder. (Empirically the wound path may still stay
// near-zero if the critical section is short enough that arrivals
// serialize cleanly via the wait queue.)

fn bench_pess_lock_contended_reverse_age(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("tx_concurrent/pess_lock_contended_reverse_age");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    let wound_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize, 4, 8]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize, 4, 8].iter().enumerate() {
        group.throughput(Throughput::Elements(n as u64));
        let fn_name = format!("n_{}", n);
        let counters = Arc::clone(&wound_counts);
        group.bench_function(BenchmarkId::from_parameter(&fn_name), |b| {
            let counters = Arc::clone(&counters);
            b.to_async(&rt).iter_custom(move |iters| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut total = Duration::ZERO;
                    for iter_i in 0..iters {
                        let mvcc = make_mvcc();
                        let key = Bytes::from_static(b"contended_key");

                        let start = Instant::now();
                        let mut handles = Vec::with_capacity(n);
                        for w in 0..n {
                            // REVERSE age: spawn-0 = youngest (highest tx_v),
                            // spawn-(n-1) = oldest (lowest tx_v). Older
                            // arrivals wound the younger holder.
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
                        for h in handles {
                            h.await.unwrap();
                        }
                        total += start.elapsed();
                    }
                    total
                }
            });
        });
    }

    group.finish();

    for (n, wounds, acquires) in wound_counts.iter() {
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
}

// --- Group 7: pessimistic-lock contended, REVERSE age + BARRIER ------------
//
// Follow-up to Group 6. The reverse-age variant alone produced zero wounds
// because each task's critical section was so short the younger holder
// always released before the older arrival reached `lock_key`. Here a
// `tokio::sync::Barrier::new(n)` is placed RIGHT BEFORE each `lock_key`
// call: all N tasks reach the barrier, are released together, and race
// for the lock in the same instant. The youngest (highest tx_v) wins,
// then older arrivals find a younger holder and wound it.

fn bench_pess_lock_contended_barrier(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("tx_concurrent/pess_lock_contended_barrier");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    let wound_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>> = Arc::new(
        [2usize, 4, 8]
            .iter()
            .map(|&n| (n, AtomicU64::new(0), AtomicU64::new(0)))
            .collect(),
    );

    for (idx, &n) in [2usize, 4, 8].iter().enumerate() {
        group.throughput(Throughput::Elements(n as u64));
        let fn_name = format!("n_{}", n);
        let counters = Arc::clone(&wound_counts);
        group.bench_function(BenchmarkId::from_parameter(&fn_name), |b| {
            let counters = Arc::clone(&counters);
            b.to_async(&rt).iter_custom(move |iters| {
                let counters = Arc::clone(&counters);
                async move {
                    let mut total = Duration::ZERO;
                    for iter_i in 0..iters {
                        let mvcc = make_mvcc();
                        let key = Bytes::from_static(b"contended_key");
                        let barrier = Arc::new(tokio::sync::Barrier::new(n));

                        let start = Instant::now();
                        let mut handles = Vec::with_capacity(n);
                        for w in 0..n {
                            // REVERSE age, same as Group 6.
                            let tx_v = 2_000_000u64 + iter_i * 100 + (n as u64 - 1 - w as u64);
                            let mvcc = Arc::clone(&mvcc);
                            let key = key.clone();
                            let counters = Arc::clone(&counters);
                            let barrier = Arc::clone(&barrier);
                            handles.push(tokio::spawn(async move {
                                let wounded = Arc::new(AtomicBool::new(false));
                                let notify = Arc::new(tokio::sync::Notify::new());
                                // Synchronise the lock-attempt phase: all N
                                // tasks reach here, then all leave together.
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
                        for h in handles {
                            h.await.unwrap();
                        }
                        total += start.elapsed();
                    }
                    total
                }
            });
        });
    }

    group.finish();

    for (n, wounds, acquires) in wound_counts.iter() {
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
}

criterion_group!(
    benches,
    bench_disjoint_inserts,
    bench_hot_key_snapshot,
    bench_hot_key_serializable,
    bench_pess_lock_uncontended,
    bench_pess_lock_contended,
    bench_pess_lock_contended_reverse_age,
    bench_pess_lock_contended_barrier,
);
criterion_main!(benches);
