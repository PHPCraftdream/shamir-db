//! Bench for #1107: Zero-cost gate for `rederive_stale_value_ops_post_stage`.
//!
//! Before fix (#1107): The function had a non-firing gate (`tx.base_index_stage_gens.is_empty()`)
//! that never skipped the per-row MVCC read + re-plan work on the common path (quiet repo).
//!
//! After fix: Added a repo-global staleness gate that skips the per-row work when
//! `gate.version_allocation_high_water_mark() <= tx.snapshot_version` (nothing has even
//! started committing since tx opened).
//!
//! This bench measures commit latency on a table with indexes when NO concurrent
//! tx is modifying it. The gate should short-circuit, avoiding per-row MVCC reads.
//!
//! Expected before fix: High latency (O(N) per-row MVCC reads + O(N²) dedup scans).
//! Expected after fix: Low latency (single atomic load gate, per-row work skipped entirely).
//!
//! IMPORTANT: The bench MUST update an INDEXED field to actually exercise the code path
//! being measured. The original version updated "score" (not indexed), so it never triggered
//! the O(N²) dedup loops this gate exists to bound — the benefit was unobservable by construction.
//! This fixed version updates "email" (indexed by idx_email) to actually exercise the path.
//!
//! Note: The gate is repo-global (conservative), so a concurrent commit on a DIFFERENT
//! table defeats the fast path. This bench measures the common case: a quiet repo.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::batch::{
    execute_batch as execute_batch_raw, AccessGate, AdminExecutor, Authorized, BatchError, BatchOp,
    BatchRequest, BatchResponse, FunctionInvoker, QueryEntry, ResultEncoding, TableResolver,
};
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::access::{AccessError, Action, Actor, ResourcePath};
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::mpack;
use shamir_types::types::common::{new_map, new_map_wc};

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
use shamir_types::types::value::{InnerValue, QueryValue};

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

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn record_with_email(email_key: u64, email: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(email_key), InnerValue::Str(email.into()));
    InnerValue::Map(m)
}

fn main() {
    let mut h = Harness::new("p1107_stale_value_gate", env!("CARGO_MANIFEST_DIR"));

    // Benchmark: Transactional UPDATE N rows on a table WITH indexes.
    // This exercises `rederive_stale_value_ops_post_stage` because UPDATE
    // requires re-planning the removal ops for indexed fields.
    //
    // The common path: NO concurrent tx committed since this tx opened.
    // The gate should short-circuit, avoiding per-row MVCC reads.
    //
    // IMPORTANT: We UPDATE the "email" field (indexed by idx_email), NOT "score" (unindexed).
    // Updating an indexed field triggers the O(N²) dedup loops this gate exists to bound.
    // The original version updated "score", which never exercised this path by construction.
    //
    // Before fix (#1107): Per-row MVCC reads + O(N²) dedup scans on the indexed field.
    // After fix: Single atomic load gate, per-row work skipped entirely.

    for &n in &[400usize, 800usize, 1600usize] {
        h.bench_batched_async(
            &format!("tx_update_quiet_repo/n_{n}"),
            move || async move {
                // FRESH instance per iteration (no shared state across bench iterations)
                let repo = Arc::new(InMemoryRepo::new());
                let instance =
                    RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
                instance.add_table(TableConfig::new("users".to_string()));

                // Create table with regular and unique indexes
                let tbl = instance.get_table("users").await.unwrap();
                tbl.create_index("idx_email", &["email"]).await.unwrap();
                tbl.create_unique_index("uniq_id", &["id"]).await.unwrap();
                drop(tbl);

                let resolver = Resolver {
                    repo: instance.clone(),
                };
                let insert_values: Vec<QueryValue> = (0..n)
                    .map(|i| {
                        mpack!({
                            "id": @(QueryValue::from(i as i64)),
                            "email": @(QueryValue::from(format!("user_{i}@example.com"))),
                            "name": @(QueryValue::from(format!("name_{i}"))),
                            "score": @(QueryValue::from(i as i64)),
                        })
                    })
                    .collect();

                // First, insert N rows (setup, not timed)
                let mut insert_queries = new_map();
                insert_queries.insert(
                    "ins".to_string(),
                    QueryEntry {
                        op: BatchOp::Insert(InsertOp {
                            insert_into: TableRef::new("users"),
                            values: insert_values,
                            records_idmsgpack: Vec::new(),
                            select: None,
                        }),
                        return_result: false,
                        after: Vec::new(),
                        when: None,
                    },
                );
                let insert_request = BatchRequest {
                    id: QueryValue::Int(1),
                    name: None,
                    transactional: false, // non-tx insert for fast setup
                    isolation: None,
                    durability: None,
                    queries: insert_queries,
                    return_all: false,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                };

                // Build the update request (timed)
                // CRITICAL: Update "email" (INDEXED by idx_email), NOT "score" (unindexed)
                // This actually exercises the O(N²) dedup loops this gate exists to bound
                let update_values = mpack!({
                    "email": @(QueryValue::from(format!("updated_{}@example.com", 999))),
                });

                let mut update_queries = new_map();
                update_queries.insert(
                    "upd".to_string(),
                    QueryEntry {
                        op: BatchOp::Update(shamir_query_types::write::UpdateOp {
                            update: TableRef::new("users"),
                            where_clause: Some(shamir_engine::query::filter::Filter::Gte {
                                field: vec!["id".to_string()],
                                value: shamir_engine::query::filter::FilterValue::Int(0),
                            }),
                            set: update_values,
                            select: None,
                            expected_version: None,
                        }),
                        return_result: false,
                        after: Vec::new(),
                        when: None,
                    },
                );
                let update_request = BatchRequest {
                    id: QueryValue::Int(2),
                    name: None,
                    transactional: true, // TRANSACTIONAL UPDATE (the measured path)
                    isolation: None,
                    durability: None,
                    queries: update_queries,
                    return_all: false,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                };

                // Insert rows (setup, not timed)
                execute_batch(
                    &insert_request,
                    &resolver,
                    None,
                    None,
                    Actor::System,
                    "bench",
                )
                .await
                .unwrap();
                (resolver, update_request)
            },
            |(resolver, update_request)| async move {
                // Transactional UPDATE (timed)
                // This runs in a quiet repo: nothing committed between setup and here,
                // so `gate.last_committed() == tx.snapshot_version` should hold.
                execute_batch(
                    &update_request,
                    &resolver,
                    None,
                    None,
                    Actor::System,
                    "bench",
                )
                .await
                .unwrap();
            },
        );
    }

    // P1 fix (group 6, cross-crate rush review 2026-08-14): `tx_update_dirty_repo/n_*`
    // forces the gate OPEN (a concurrent non-tx write commits between this
    // tx's BEGIN and its staged updates, advancing
    // `gate.version_allocation_high_water_mark()` past `tx.snapshot_version`
    // — see `pre_commit.rs`'s gate check) so the timed commit actually runs
    // `rederive_stale_value_ops_post_stage`'s per-row re-planning loop over
    // all N staged rows, unlike `tx_update_quiet_repo/n_*` above (which the
    // #1107/#1110 gate always short-circuits before touching this loop at
    // all). Before the P1 fix, that loop rebuilt `staged_removals_by_rid`
    // from scratch and linearly `.any()`-rescanned `tx.index_write_set` once
    // PER re-planned op — O(N²·K) total for N staged rows. After the fix,
    // the dedup caches are built once per table and looked up in O(1), so
    // total per-commit work should scale close to O(N) — the harness's
    // `ns/op` here should stay roughly flat/near-linear across N instead of
    // growing superlinearly.
    for &n in &[100usize, 200usize, 400usize] {
        h.bench_batched_async(
            &format!("tx_update_dirty_repo/n_{n}"),
            move || async move {
                // FRESH instance per iteration (no shared state across bench iterations)
                let repo = Arc::new(InMemoryRepo::new());
                let instance =
                    RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
                instance.add_table(TableConfig::new("users".to_string()));
                let tbl = instance.get_table("users").await.unwrap();
                tbl.create_index("idx_email", &["email"]).await.unwrap();
                let email_key = key_id(&tbl, "email").await;

                // Insert N rows (setup, not timed).
                let mut rids = Vec::with_capacity(n);
                for i in 0..n {
                    let rid = tbl
                        .insert(&record_with_email(
                            email_key,
                            &format!("user_{i}@example.com"),
                        ))
                        .await
                        .unwrap();
                    rids.push(rid);
                }

                // Open the tx whose COMMIT is timed (snapshot taken now).
                let (mut tx, guard) = instance.begin_tx(IsolationLevel::Snapshot).await.unwrap();

                // Force the slow path: a concurrent non-tx write on this table
                // commits AFTER this tx's snapshot was taken, so the gate does
                // NOT short-circuit at commit time.
                tbl.insert(&record_with_email(email_key, "concurrent@example.com"))
                    .await
                    .unwrap();

                // Stage an UPDATE of every row's indexed "email" field
                // (untimed setup) — this is what makes
                // `rederive_stale_value_ops_post_stage` re-plan N rows.
                for (i, &rid) in rids.iter().enumerate() {
                    tbl.update_tx(
                        rid,
                        &record_with_email(email_key, &format!("updated_{i}@example.com")),
                        Some(&mut tx),
                    )
                    .await
                    .unwrap();
                }

                (instance, tx, guard)
            },
            |(instance, tx, guard)| async move {
                // Commit (timed): runs `rederive_stale_value_ops_post_stage`'s
                // slow path over all N staged rows.
                instance.commit_tx(tx).await.unwrap();
                drop(guard);
            },
        );
    }

    h.run();
}
