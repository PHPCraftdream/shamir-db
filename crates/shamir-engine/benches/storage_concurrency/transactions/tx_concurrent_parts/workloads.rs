use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::RecordKey;
use shamir_tx::mvcc_store::LockMode;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;
use smallvec::smallvec;

use crate::{execute_batch, make_mvcc, make_repo};

/// Observational counters filled by the workload groups below and dumped
/// by the bench root after `h.run()`.
pub(crate) struct WCounts {
    pub(crate) hot_key_snapshot_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>>,
    pub(crate) hot_key_serializable_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>>,
    pub(crate) pess_contended_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>>,
    pub(crate) pess_reverse_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>>,
    pub(crate) pess_barrier_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>>,
    pub(crate) pess_in_cs_barrier_counts: Arc<Vec<(usize, AtomicU64, AtomicU64)>>,
}

pub(crate) fn run(h: &mut Harness) -> WCounts {

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

    // --- Disjoint TABLES: staging + barrier-aligned commit + join + readback ---
    // Fixed fan-out (at most eight tasks); fresh tables in untimed setup.
    // One call is the whole N-writer round, not per-commit latency.
    for &n in &[4usize, 8] {
        h.bench_batched_async(
            &format!("disjoint_tables/n_{n}"),
            move || async move {
                let storage = Arc::new(InMemoryRepo::new());
                let names: Vec<String> = (0..n).map(|w| format!("writer_{w}")).collect();
                let configs = names.iter().cloned().map(TableConfig::new).collect();
                let repo = RepoInstance::new("bench".into(), BoxRepo::InMemory(storage), configs);
                let mut tables = Vec::with_capacity(n);
                for name in &names {
                    tables.push(repo.get_table(name).await.unwrap());
                }
                (repo, tables)
            },
            move |(repo, tables)| async move {
                let barrier = Arc::new(tokio::sync::Barrier::new(n));
                let mut handles = Vec::with_capacity(n);
                for (w, table) in tables.into_iter().enumerate() {
                    let repo = repo.clone();
                    let barrier = Arc::clone(&barrier);
                    handles.push(tokio::spawn(async move {
                        let (mut tx, _guard) =
                            repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                        let mut rows = Vec::with_capacity(K);
                        for i in 0..K {
                            let value = InnerValue::Str(format!("writer_{w}_row_{i}"));
                            let rid = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
                            rows.push((rid, value));
                        }
                        // All writers finish staging before ANY commit starts.
                        barrier.wait().await;
                        let outcome = repo.commit_tx(tx).await.unwrap();
                        for (rid, expected) in rows {
                            assert_eq!(table.get(rid).await.unwrap(), expected);
                        }
                        outcome.commit_version
                    }));
                }
                let mut versions = Vec::with_capacity(n);
                for handle in handles {
                    versions.push(handle.await.unwrap());
                }
                versions.sort_unstable();
                assert_eq!(versions.len(), n);
                assert!(versions[0] > 0);
                assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
                std::hint::black_box(versions);
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

    WCounts {
        hot_key_snapshot_counts,
        hot_key_serializable_counts,
        pess_contended_counts,
        pess_reverse_counts,
        pess_barrier_counts,
        pess_in_cs_barrier_counts,
    }
}
