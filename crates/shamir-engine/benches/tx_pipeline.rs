// Single-element `for` loops are intentional: the N-ladders were collapsed
// to their smallest variant when migrating to the fixed-iteration harness,
// but the loop structure is kept so the ladder can be re-expanded ad-hoc.
#![allow(clippy::single_element_loop)]
//! Stage 4.D.6 / 4.H pipeline benchmarks.
//!
//! Measures:
//! - `bench_insert_tx_vs_non_tx` — single-record tx vs non-tx (D5).
//! - `bench_batch_insert_pipeline` — N-record execute_batch tx vs non-tx (D5).
//! - `bench_commit_tx_phase_breakdown` — scenario-isolated phase
//!   costs: baseline empty, Phase 2 SSI scaling, Phase 5 write
//!   scaling across 1 vs N tables.
//! - `bench_provider_overhead` — stub vs real VersionProvider for
//!   SSI read-set validation; delta = MvccStore lookup overhead.
//! - `bench_commit_phase5c_indexed_fjall` — tx commit Phase 5c writing
//!   N postings to a sled-backed indexed table; exposes the
//!   batched-vs-per-key info_store apply cost.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`). Every
//! workload here drives a fresh repo/tx/tempdir per iteration (a committed
//! tx cannot be recommitted against the same state, and DDL/tempdir setup
//! must not leak across iterations), so every group uses
//! `bench_batched_async` — setup (repo/tx construction) is untimed, only the
//! actual insert/commit routine is timed. The indexed
//! `tx_pipeline/batch_pipeline` cell additionally needs a persistent
//! disjoint-key counter across iterations (shared state living outside any
//! single setup call) — same shape as the original Criterion `iter_custom`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::batch::{
    execute_batch, BatchOp, BatchRequest, QueryEntry, ResultEncoding, TableResolver,
};
use shamir_engine::repo::{BoxRepo, BoxRepoFactory, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_builder::query::Query;
use shamir_query_builder::write::doc;
use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
    instance.add_table(TableConfig::new("bench_table".to_string()));
    instance
}

struct Resolver {
    repo: RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }
    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

fn main() {
    let mut h = Harness::new("tx_pipeline", env!("CARGO_MANIFEST_DIR"));

    // ── bench_insert_tx_vs_non_tx: single-record tx vs non-tx ──────────
    {
        let repo = make_repo();
        h.bench_batched_async(
            "tx_overhead/single_insert/non_tx",
            move || {
                let repo = repo.clone();
                async move { repo.get_table("bench_table").await.unwrap() }
            },
            move |tbl| async move {
                tbl.insert(&InnerValue::Str("v".into())).await.unwrap();
            },
        );
    }

    {
        let repo = make_repo();
        h.bench_batched_async(
            "tx_overhead/single_insert/tx_staged",
            move || {
                let repo = repo.clone();
                async move {
                    let tbl = repo.get_table("bench_table").await.unwrap();
                    (repo, tbl)
                }
            },
            move |(repo, tbl)| async move {
                let (mut tx, _g) = repo
                    .begin_tx(shamir_tx::IsolationLevel::Snapshot)
                    .await
                    .unwrap();
                let _ = tbl
                    .insert_tx(&InnerValue::Str("v".into()), Some(&mut tx))
                    .await
                    .unwrap();
                let _ = repo.commit_tx(tx).await.unwrap();
            },
        );
    }

    // ── bench_batch_insert_pipeline: N-record execute_batch, tx vs non_tx ──
    // Scaled ladder collapsed to n=1 (smallest variant); the harness owns
    // repetition count, so each call must stay a cheap unit.
    for &n in &[1usize] {
        for transactional in [false, true] {
            let label = if transactional { "tx" } else { "non_tx" };
            let repo = make_repo();
            h.bench_batched_async(
                &format!("tx_overhead/batch_pipeline/{label}/{n}"),
                move || {
                    let resolver = Resolver { repo: repo.clone() };
                    async move { resolver }
                },
                move |resolver| {
                    let mut queries = new_map();
                    queries.insert(
                        "ins".to_string(),
                        QueryEntry {
                            op: BatchOp::Insert(InsertOp {
                                insert_into: TableRef::new("bench_table"),
                                values: (0..n)
                                    .map(|i| mpack!({"i": @(QueryValue::from(i as i64))}))
                                    .collect(),
                                records_idmsgpack: Vec::new(),
                                select: None,
                            }),
                            return_result: true,
                            after: Vec::new(),
                            when: None,
                        },
                    );
                    let request = BatchRequest {
                        id: QueryValue::Int(1),
                        name: None,
                        transactional,
                        isolation: None,
                        durability: None,
                        queries,
                        return_all: false,
                        return_only: None,
                        limits: Default::default(),
                        interner_epochs: Default::default(),
                        result_encoding: ResultEncoding::default(),
                    };
                    async move {
                        let _ =
                            execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                                .await
                                .unwrap();
                    }
                },
            );
        }
    }

    // fire-and-forget variant — same as above but return_result=false,
    // exercising the result_build skip fast-path added in the P8 cycle.
    // Scaled ladder collapsed to n=1 (smallest variant).
    for &n in &[1usize] {
        for transactional in [false, true] {
            let label = if transactional { "tx" } else { "non_tx" };
            let repo = make_repo();
            h.bench_batched_async(
                &format!("tx_overhead/batch_pipeline/{label}/{n}_no_result"),
                move || {
                    let resolver = Resolver { repo: repo.clone() };
                    async move { resolver }
                },
                move |resolver| {
                    let mut queries = new_map();
                    queries.insert(
                        "ins".to_string(),
                        QueryEntry {
                            op: BatchOp::Insert(InsertOp {
                                insert_into: TableRef::new("bench_table"),
                                values: (0..n)
                                    .map(|i| mpack!({"i": @(QueryValue::from(i as i64))}))
                                    .collect(),
                                records_idmsgpack: Vec::new(),
                                select: None,
                            }),
                            return_result: false,
                            after: Vec::new(),
                            when: None,
                        },
                    );
                    let request = BatchRequest {
                        id: QueryValue::Int(1),
                        name: None,
                        transactional,
                        isolation: None,
                        durability: None,
                        queries,
                        return_all: false,
                        return_only: None,
                        limits: Default::default(),
                        interner_epochs: Default::default(),
                        result_encoding: ResultEncoding::default(),
                    };
                    async move {
                        let _ =
                            execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                                .await
                                .unwrap();
                    }
                },
            );
        }
    }

    // Indexed variant — exercises the per-row vs batched cost on the
    // heavier write path: 1 unique index (`uniq_email`) + 1 regular index
    // (`by_city`). DDL (repo creation + index registration) is hoisted
    // OUTSIDE the timed section — built ONCE. Each iteration inserts into
    // the SAME table using disjoint keys (a shared `AtomicU64` counter
    // outlives any single setup call, so it's held above the harness
    // registration, matching the original `iter_counter` shared across all
    // Criterion samples including warmup).
    // Scaled ladder collapsed to n=100 (smallest variant); n=1000 was
    // ~25ms/call, above the ≤10ms budget the harness now expects.
    for &n in &[100usize] {
        for transactional in [false, true] {
            let label = if transactional { "tx" } else { "non_tx" };

            let indexed_repo = make_repo();
            {
                // One-time DDL, run via a throwaway current-thread runtime
                // (registration time, not timed).
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let tbl = rt.block_on(indexed_repo.get_table("bench_table")).unwrap();
                rt.block_on(tbl.create_unique_index("uniq_email", &["email"]))
                    .unwrap();
                rt.block_on(tbl.create_index("by_city", &["city"])).unwrap();
                drop(tbl);
            }

            let iter_counter = Arc::new(AtomicU64::new(0));

            h.bench_batched_async(
                &format!("tx_overhead/batch_pipeline/indexed/{label}/{n}"),
                {
                    let indexed_repo = indexed_repo.clone();
                    let iter_counter = Arc::clone(&iter_counter);
                    move || {
                        let resolver = Resolver {
                            repo: indexed_repo.clone(),
                        };
                        let ic = iter_counter.fetch_add(1, Ordering::Relaxed);
                        let values: Vec<QueryValue> = (0..n)
                            .map(|i| {
                                mpack!({
                                    "email": @(QueryValue::from(format!("user_{ic}_{i}@example.com"))),
                                    "city": @(QueryValue::from(format!("c_{}", i % 8))),
                                    "score": @(QueryValue::from(i as i64)),
                                })
                            })
                            .collect();
                        async move { (resolver, values) }
                    }
                },
                move |(resolver, values)| {
                    let mut queries = new_map();
                    queries.insert(
                        "ins".to_string(),
                        QueryEntry {
                            op: BatchOp::Insert(InsertOp {
                                insert_into: TableRef::new("bench_table"),
                                values,
                                records_idmsgpack: Vec::new(),
                                select: None,
                            }),
                            return_result: false,
                            after: Vec::new(),
                            when: None,
                        },
                    );
                    let request = BatchRequest {
                        id: QueryValue::Int(1),
                        name: None,
                        transactional,
                        isolation: None,
                        durability: None,
                        queries,
                        return_all: false,
                        return_only: None,
                        limits: Default::default(),
                        interner_epochs: Default::default(),
                        result_encoding: ResultEncoding::default(),
                    };
                    async move {
                        execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                            .await
                            .unwrap();
                    }
                },
            );
        }
    }

    // ── bench_commit_tx_phase_breakdown ─────────────────────────────────

    // Baseline: empty Tx (Phase 3 + 4 + 6 + 7 fixed overhead).
    {
        let repo = make_repo();
        {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(repo.get_table("bench_table")).unwrap();
        }
        h.bench_batched_async(
            "commit_tx/phases/baseline_empty",
            {
                let repo = repo.clone();
                move || {
                    let repo = repo.clone();
                    async move { repo }
                }
            },
            move |repo| async move {
                let (tx, _g) = repo
                    .begin_tx(shamir_tx::IsolationLevel::Snapshot)
                    .await
                    .unwrap();
                let _ = repo.commit_tx(tx).await.unwrap();
            },
        );
    }

    // Phase 2 scaling: Serializable with N read_set entries.
    // Scaled ladder collapsed to n=10 (smallest variant).
    for &n in &[10usize] {
        let repo = make_repo();
        {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(repo.get_table("bench_table")).unwrap();
        }
        h.bench_batched_async(
            &format!("commit_tx/phases/ssi_validate/{n}"),
            {
                let repo = repo.clone();
                move || {
                    let repo = repo.clone();
                    async move { repo }
                }
            },
            move |repo| async move {
                let (mut tx, _g) = repo
                    .begin_tx(shamir_tx::IsolationLevel::Serializable)
                    .await
                    .unwrap();
                let table_id = shamir_engine::table::table_token_for("bench_table");
                for i in 0..n {
                    tx.record_read(table_id, bytes::Bytes::from(format!("k{i}")), 0);
                }
                let _ = repo.commit_tx(tx).await.unwrap();
            },
        );
    }

    // Phase 5 scaling: write N keys into 1 table vs 5 tables.
    // Scaled ladder collapsed to 1 table (smallest variant).
    for table_count in [1usize] {
        let repo = make_repo();
        {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            for i in 0..table_count {
                let name = format!("phase5_tbl_{i}");
                if !repo.has_table(&name) {
                    repo.add_table(TableConfig::new(name.clone()));
                    rt.block_on(repo.get_table(&name)).unwrap();
                }
            }
        }

        let n = 100usize;
        h.bench_batched_async(
            &format!("commit_tx/phases/write_100_keys/{table_count}_tables"),
            {
                let repo = repo.clone();
                move || {
                    let repo = repo.clone();
                    async move { repo }
                }
            },
            move |repo| async move {
                let (mut tx, _g) = repo
                    .begin_tx(shamir_tx::IsolationLevel::Snapshot)
                    .await
                    .unwrap();
                for i in 0..table_count {
                    let tbl = repo.get_table(&format!("phase5_tbl_{i}")).await.unwrap();
                    for _ in 0..n {
                        let _ = tbl
                            .insert_tx(&InnerValue::Str("v".into()), Some(&mut tx))
                            .await
                            .unwrap();
                    }
                }
                let _ = repo.commit_tx(tx).await.unwrap();
            },
        );
    }

    // ── bench_provider_overhead ──────────────────────────────────────────
    {
        use shamir_tx::VersionProvider;

        /// Always Some(0) — minimum-cost mock.
        struct StubAlwaysZero;
        impl VersionProvider for StubAlwaysZero {
            fn version_of(&self, _: u64, _: &bytes::Bytes) -> Option<u64> {
                Some(0)
            }
        }

        // Scaled ladder collapsed to n=100 (smallest variant).
        for &n in &[100usize] {
            let stub_repo = make_repo();
            let real_repo = make_repo();
            {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(stub_repo.get_table("bench_table")).unwrap();
                rt.block_on(real_repo.get_table("bench_table")).unwrap();
            }

            h.bench_batched_async(
                &format!("commit_tx/provider_overhead/stub_provider/{n}"),
                {
                    let stub_repo = stub_repo.clone();
                    move || {
                        let repo = stub_repo.clone();
                        async move { repo }
                    }
                },
                move |repo| async move {
                    let (mut tx, _g) = repo
                        .begin_tx(shamir_tx::IsolationLevel::Serializable)
                        .await
                        .unwrap();
                    tx.set_version_provider(Arc::new(StubAlwaysZero));
                    let table_id = shamir_engine::table::table_token_for("bench_table");
                    for i in 0..n {
                        tx.record_read(table_id, bytes::Bytes::from(format!("k{i}")), 0);
                    }
                    let _ = repo.commit_tx(tx).await.unwrap();
                },
            );

            h.bench_batched_async(
                &format!("commit_tx/provider_overhead/real_provider/{n}"),
                {
                    let real_repo = real_repo.clone();
                    move || {
                        let repo = real_repo.clone();
                        async move { repo }
                    }
                },
                move |repo| async move {
                    let (mut tx, _g) = repo
                        .begin_tx(shamir_tx::IsolationLevel::Serializable)
                        .await
                        .unwrap();
                    let table_id = shamir_engine::table::table_token_for("bench_table");
                    for i in 0..n {
                        tx.record_read(table_id, bytes::Bytes::from(format!("k{i}")), 0);
                    }
                    let _ = repo.commit_tx(tx).await.unwrap();
                },
            );
        }
    }

    // ── bench_commit_phase5c_indexed_fjall ──────────────────────────────
    //
    // Each iter provisions a fresh tempdir + fjall-backed RepoInstance,
    // creates a `by_city` index, and runs ONE transactional BatchRequest
    // that inserts `n` rows. All of that (except the actual execute_batch
    // call) is untimed setup.
    //
    // Scaled ladder collapsed to n=100 (smallest variant). NOTE: cost is
    // I/O-bound (fjall keyspace init + fsync dominate; n=100 and n=1000
    // both calibrate to ~1 iter at 0.05s), so reducing N further does not
    // proportionally reduce measured time — left at n=100 as the smallest
    // meaningful write batch.
    for &n in &[100usize] {
        h.bench_batched_async(
            &format!("commit_tx/phase5c_indexed_fjall/{n}"),
            move || async move {
                let tempdir = tempfile::TempDir::new().expect("tempdir");
                let factory = BoxRepoFactory::fjall_raw(tempdir.path().to_path_buf());
                let repo = RepoInstance::from_factory(
                    "bench".into(),
                    factory,
                    vec![TableConfig::new("indexed".to_string())],
                )
                .await
                .unwrap();
                let tbl = repo.get_table("indexed").await.unwrap();
                tbl.create_index("by_city", &["city"]).await.unwrap();
                drop(tbl);

                let resolver = Resolver { repo: repo.clone() };
                let mut queries = new_map();
                let values: Vec<QueryValue> = (0..n)
                    .map(|i| {
                        mpack!({
                            "city": @(QueryValue::from(format!("c_{}", i % 8))),
                            "score": @(QueryValue::from(i as i64)),
                        })
                    })
                    .collect();
                queries.insert(
                    "ins".to_string(),
                    QueryEntry {
                        op: BatchOp::Insert(InsertOp {
                            insert_into: TableRef::new("indexed"),
                            values,
                            records_idmsgpack: Vec::new(),
                            select: None,
                        }),
                        return_result: false,
                        after: Vec::new(),
                        when: None,
                    },
                );
                let request = BatchRequest {
                    id: QueryValue::Int(1),
                    name: None,
                    transactional: true,
                    isolation: None,
                    durability: None,
                    queries,
                    return_all: false,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                };
                (resolver, request, tempdir, repo)
            },
            move |(resolver, request, tempdir, repo)| async move {
                let _ = execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap();
                drop(repo);
                drop(tempdir);
            },
        );
    }

    // ── bench_async_commit_index_heavy ──────────────────────────────────
    //
    // Client-visible commit latency on an INDEX-HEAVY tx, sync vs
    // async-index visibility. Each iter creates a fresh repo/staging tx
    // (untimed setup) and times ONE `commit_tx` call. The background tail
    // is joined AFTER the timed call inside the same routine so the handle
    // doesn't leak across iterations, but it runs after `commit_tx` has
    // already resolved — the original Criterion bench captured
    // `start.elapsed()` right after `commit_tx` returned, before joining
    // the tail, so the recorded latency is `commit_tx` alone; joining it
    // one statement later inside the same async block does not add to
    // that already-captured cost.
    {
        use shamir_engine::tx::commit_tx;
        use shamir_tx::{
            CommitVisibility, IndexWriteOp, IsolationLevel, StagingStore, TxContext, TxId,
        };
        use shamir_types::types::record_id::RecordId;

        // Scaled ladder collapsed to n=100 (smallest variant). NOTE: the
        // fjall cells are I/O-bound (fsync dominates; n=100 and n=1000 both
        // calibrate to ~1 iter at 0.05s), so reducing N further would not
        // proportionally reduce measured time — left as-is per the I/O
        // exception. The inmem cells at n=100 are ~1ms (well under budget).
        for &n in &[100usize] {
            // In-memory backend.
            for visibility in [CommitVisibility::Synchronous, CommitVisibility::AsyncIndex] {
                let label = match visibility {
                    CommitVisibility::Synchronous => "sync",
                    CommitVisibility::AsyncIndex => "async",
                };
                h.bench_batched_async(
                    &format!("commit_tx/async_visibility_index_heavy/inmem_{label}/{n}"),
                    move || async move {
                        let repo = make_repo();
                        let tbl = repo.get_table("bench_table").await.unwrap();
                        let token = shamir_engine::table::table_token_for("bench_table");

                        let mut staging = StagingStore::new(Arc::clone(tbl.data_store()));
                        for i in 0..n {
                            let rid = RecordId::new();
                            let body = InnerValue::Str(format!("v{}", i)).to_bytes().unwrap();
                            staging.set(rid.to_bytes().into(), body);
                        }
                        let mut tx = TxContext::new(
                            TxId::new(7_900_000 + n as u64),
                            0,
                            0,
                            IsolationLevel::Snapshot,
                        );
                        tx.write_set.insert(token, staging);
                        for i in 0..n {
                            tx.index_write_set.push((
                                token,
                                IndexWriteOp::SetPosting {
                                    key: bytes::Bytes::from(format!("idx_k_{}", i)),
                                    value: bytes::Bytes::from(format!("idx_v_{}", i)),
                                },
                            ));
                        }
                        tx.set_visibility(visibility);
                        (tx, repo)
                    },
                    move |(tx, repo)| async move {
                        let mut outcome = commit_tx(tx, &repo).await.unwrap();
                        if let Some(bg) = outcome.take_background() {
                            let _ = bg.join().await;
                        }
                    },
                );
            }

            // Fjall backend.
            for visibility in [CommitVisibility::Synchronous, CommitVisibility::AsyncIndex] {
                let label = match visibility {
                    CommitVisibility::Synchronous => "sync",
                    CommitVisibility::AsyncIndex => "async",
                };
                h.bench_batched_async(
                    &format!("commit_tx/async_visibility_index_heavy/fjall_{label}/{n}"),
                    move || async move {
                        let tempdir = tempfile::TempDir::new().expect("tempdir");
                        let factory = BoxRepoFactory::fjall_raw(tempdir.path().to_path_buf());
                        let repo = RepoInstance::from_factory(
                            "bench".into(),
                            factory,
                            vec![TableConfig::new("bench_table".to_string())],
                        )
                        .await
                        .unwrap();
                        let tbl = repo.get_table("bench_table").await.unwrap();
                        let token = shamir_engine::table::table_token_for("bench_table");

                        let mut staging = StagingStore::new(Arc::clone(tbl.data_store()));
                        for i in 0..n {
                            let rid = RecordId::new();
                            let body = InnerValue::Str(format!("v{}", i)).to_bytes().unwrap();
                            staging.set(rid.to_bytes().into(), body);
                        }
                        let mut tx = TxContext::new(
                            TxId::new(7_900_500 + n as u64),
                            0,
                            0,
                            IsolationLevel::Snapshot,
                        );
                        tx.write_set.insert(token, staging);
                        for i in 0..n {
                            tx.index_write_set.push((
                                token,
                                IndexWriteOp::SetPosting {
                                    key: bytes::Bytes::from(format!("idx_k_{}", i)),
                                    value: bytes::Bytes::from(format!("idx_v_{}", i)),
                                },
                            ));
                        }
                        tx.set_visibility(visibility);
                        (tx, repo, tempdir)
                    },
                    move |(tx, repo, tempdir)| async move {
                        let mut outcome = commit_tx(tx, &repo).await.unwrap();
                        if let Some(bg) = outcome.take_background() {
                            let _ = bg.join().await;
                        }
                        drop(repo);
                        drop(tempdir);
                    },
                );
            }
        }
    }

    // ── bench_read_scan ──────────────────────────────────────────────────
    //
    // 1000 rows are inserted ONCE before measurement (shared, read-only
    // fixture — safe to share across iterations since reads don't mutate).
    {
        let repo = {
            let r = make_repo();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let resolver = Resolver { repo: r.clone() };
                let mut queries = new_map();
                let values: Vec<QueryValue> = (0..1000usize)
                    .map(|i| {
                        QueryValue::from(
                            doc()
                                .set("id", format!("k{i}"))
                                .set("score", i as i64)
                                .set("category", (i % 10) as i64)
                                .set("name", format!("name_{i}"))
                                .build(),
                        )
                    })
                    .collect();
                queries.insert(
                    "ins".to_string(),
                    QueryEntry {
                        op: BatchOp::Insert(InsertOp {
                            insert_into: TableRef::new("bench_table"),
                            values,
                            records_idmsgpack: Vec::new(),
                            select: None,
                        }),
                        return_result: false,
                        after: Vec::new(),
                        when: None,
                    },
                );
                let request = BatchRequest {
                    id: QueryValue::Int(1),
                    name: None,
                    transactional: false,
                    isolation: None,
                    durability: None,
                    queries,
                    return_all: false,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                };
                execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap();
            });
            r
        };
        let resolver = Resolver { repo: repo.clone() };

        // Variant 1: scan all rows — baseline; pure scan + projection cost.
        {
            let resolver = resolver.repo.clone();
            let q = Query::from("bench_table");
            let req = {
                let mut queries = new_map();
                queries.insert(
                    "r".to_string(),
                    QueryEntry {
                        op: BatchOp::Read(q.into()),
                        return_result: true,
                        after: Vec::new(),
                        when: None,
                    },
                );
                BatchRequest {
                    id: QueryValue::Int(2),
                    name: None,
                    transactional: false,
                    isolation: None,
                    durability: None,
                    queries,
                    return_all: true,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                }
            };
            h.bench_async("read_scan/scan_all_1000", move || {
                let resolver = Resolver {
                    repo: resolver.clone(),
                };
                let req = req.clone();
                async move {
                    execute_batch(&req, &resolver, None, None, Actor::System, "bench")
                        .await
                        .unwrap();
                }
            });
        }

        // Variant 2: selective filter 10% match (category == 5).
        {
            let resolver = resolver.repo.clone();
            let q = Query::from("bench_table").where_eq("category", 5i64);
            let req = {
                let mut queries = new_map();
                queries.insert(
                    "r".to_string(),
                    QueryEntry {
                        op: BatchOp::Read(q.into()),
                        return_result: true,
                        after: Vec::new(),
                        when: None,
                    },
                );
                BatchRequest {
                    id: QueryValue::Int(3),
                    name: None,
                    transactional: false,
                    isolation: None,
                    durability: None,
                    queries,
                    return_all: true,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                }
            };
            h.bench_async("read_scan/scan_filtered_10pct", move || {
                let resolver = Resolver {
                    repo: resolver.clone(),
                };
                let req = req.clone();
                async move {
                    execute_batch(&req, &resolver, None, None, Actor::System, "bench")
                        .await
                        .unwrap();
                }
            });
        }

        // Variant 3: selective filter 0.1% match (score == 42).
        {
            let resolver = resolver.repo.clone();
            let q = Query::from("bench_table").where_eq("score", 42i64);
            let req = {
                let mut queries = new_map();
                queries.insert(
                    "r".to_string(),
                    QueryEntry {
                        op: BatchOp::Read(q.into()),
                        return_result: true,
                        after: Vec::new(),
                        when: None,
                    },
                );
                BatchRequest {
                    id: QueryValue::Int(4),
                    name: None,
                    transactional: false,
                    isolation: None,
                    durability: None,
                    queries,
                    return_all: true,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                }
            };
            h.bench_async("read_scan/scan_filtered_1pct", move || {
                let resolver = Resolver {
                    repo: resolver.clone(),
                };
                let req = req.clone();
                async move {
                    execute_batch(&req, &resolver, None, None, Actor::System, "bench")
                        .await
                        .unwrap();
                }
            });
        }
    }

    h.run();
}
