//! Frozen B04/B04b fixture; Harness 0.1.0 / Tokio 1.49.0 (Cargo.lock).
//! Default N=1000; BENCH_TX_AWARE_SCALE=1 adds scan N=10000/vector N=5000.
//! Untimed: seed, begin, stage 1% and materialize each staged physical ID.
//! Timed: read_tx, projection, wire-ID assertions,
//! then drop the uncommitted transaction, snapshot pin and fresh repo/table.
//! This includes teardown, not pure read latency (rollback, no commit).
//! Baseline e1106477 has the same public API but not B04's snapshot scan/exact
//! ranking dispatch. Do NOT patch its semantics or relax the output oracle:
//! an assertion failure is measurement-blocked, never a timing comparison.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::FilterContext;
use shamir_engine::query::read::QueryResult;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_builder::{ddl, filter, select, Query};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::read::ReadQuery;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::{IsolationLevel, TxContext};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

fn scaled() -> bool {
    std::env::var("BENCH_TX_AWARE_SCALE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

async fn row(table: &TableManager, id: usize, distance: Option<usize>) -> InnerValue {
    let interner = table.interner_for_test().get().await.unwrap();
    let mut map = new_map();
    map.insert(
        interner.touch_ind("ordinal").unwrap().into_key(),
        InnerValue::Int(i64::try_from(id).unwrap()),
    );
    if let Some(distance) = distance {
        // Strict L2 ranking: [distance, 0, ..., 0] against the zero vector.
        // Bounded integer coordinates are exactly representable as f32/f64.
        let mut vector = vec![InnerValue::F64(0.0); 32];
        vector[0] = InnerValue::F64(f64::from(u32::try_from(distance).unwrap()));
        map.insert(
            interner.touch_ind("embedding").unwrap().into_key(),
            InnerValue::List(vector),
        );
    }
    InnerValue::Map(map)
}

async fn fixture(n: usize, vector: bool) -> (RepoInstance, TableManager) {
    let repo = RepoInstance::new(
        "bench".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("items")],
    );
    let table = repo.get_table("items").await.unwrap();
    if vector {
        let BatchOp::CreateIndex(op) = ddl::create_index("vec", "items")
            .field("embedding")
            .index_type("vector")
            .vector_dim(32)
            .vector_metric("l2")
            .build()
        else {
            panic!("bench fixture requires CREATE INDEX");
        };
        table.create_index_v2(&op).await.unwrap();
    }
    for i in 0..n {
        let value = row(&table, i, vector.then_some(i + n)).await;
        table.insert(&value).await.unwrap();
    }
    table.interner_for_test().persist().await.unwrap();
    (repo, table)
}

// Thin common-API adapter: both revisions expose this exact signature.
// Current dispatch selects snapshot_vector_exact; baseline selects the legacy
// filtered-vector pipeline. No direct adapter calls or production backports.
// In particular, baseline COUNT uses the committed counter (N, not N+1%):
// that cell is expected to reject the baseline, rather than certify a speedup.
async fn read(table: &TableManager, query: &ReadQuery, tx: &TxContext) -> QueryResult {
    let interner = table.interner_for_test().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    table.read_tx(query, &ctx, Some(tx)).await.unwrap()
}

fn main() {
    let mut h = Harness::new("tx_aware_reads", env!("CARGO_MANIFEST_DIR"));
    for (isolation, label) in [
        (IsolationLevel::Snapshot, "snapshot"),
        (IsolationLevel::Serializable, "serializable"),
    ] {
        for vector in [false, true] {
            let sizes = if scaled() {
                vec![1000, if vector { 5000 } else { 10000 }]
            } else {
                vec![1000]
            };
            for n in sizes {
                let classes: &[&str] = if vector {
                    &["vector_exact_d32_k10"]
                } else {
                    &["point", "range", "count"]
                };
                for &class in classes {
                    let query = match class {
                        "point" => Query::from("items")
                            .select(["ordinal", "physical_id"])
                            .build()
                            .filter(filter::eq("ordinal", i64::try_from(n).unwrap())),
                        "range" => Query::from("items")
                            .select(["ordinal", "physical_id"])
                            .build()
                            .filter(filter::gte("ordinal", i64::try_from(n / 2).unwrap())),
                        "count" => Query::from("items")
                            .select([select::count_all("answer")])
                            .build(),
                        _ => Query::from("items")
                            .select(["ordinal", "physical_id"])
                            .build()
                            .filter(filter::and([
                                filter::vector_similarity("embedding", vec![0.0; 32], 10),
                                filter::gte("ordinal", 0),
                            ])),
                    };
                    h.bench_batched_async(
                        &format!("tx_aware_reads/{label}/{class}/n_{n}/overlay_1pct"),
                        move || {
                            let query = query.clone();
                            async move {
                                let (repo, table) = fixture(n, vector).await;
                                let (mut tx, guard) = repo.begin_tx(isolation).await.unwrap();
                                let mut staged_ids: Vec<RecordId> = Vec::with_capacity(n / 100);
                                for i in 0..n / 100 {
                                    let mut value = row(&table, n + i, vector.then_some(i)).await;
                                    let id = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
                                    if vector {
                                        // Candidate postprocessing emits Direct rows, not
                                        // InsertedRecord; it does not inject synthetic _id.
                                        // Store the actual allocated ID as physical_id
                                        // fixture data (not the reserved synthetic field)
                                        // so both revisions can project and serialize it.
                                        let interner =
                                            table.interner_for_test().get().await.unwrap();
                                        let InnerValue::Map(ref mut fields) = value else {
                                            panic!("bench row must be a map");
                                        };
                                        fields.insert(
                                            interner.touch_ind("physical_id").unwrap().into_key(),
                                            InnerValue::Str(id.to_string()),
                                        );
                                        table.update_tx(id, &value, Some(&mut tx)).await.unwrap();
                                    }
                                    staged_ids.push(id);
                                }
                                (repo, table, tx, guard, query, staged_ids)
                            }
                        },
                        move |(repo, table, tx, guard, query, staged_ids)| async move {
                            let result = read(&table, &query, &tx).await;
                            if class == "count" {
                                assert_eq!(result.records.len(), 1);
                                assert_eq!(
                                    result.records[0].get_value_i64("answer"),
                                    Some(i64::try_from(n + n / 100).unwrap()),
                                );
                            } else {
                                let mut actual: Vec<_> = result
                                    .records
                                    .iter()
                                    .map(|r| r.get_value_i64("ordinal").unwrap())
                                    .collect();
                                let expected: Vec<_> = match class {
                                    "point" => vec![i64::try_from(n).unwrap()],
                                    "range" => (n / 2..n + n / 100)
                                        .map(|i| i64::try_from(i).unwrap())
                                        .collect(),
                                    _ => (n..n + 10).map(|i| i64::try_from(i).unwrap()).collect(),
                                };
                                if !vector {
                                    actual.sort_unstable();
                                }
                                assert_eq!(actual, expected, "{class}: staged overlay/ranking");
                                if vector {
                                    // Physical IDs, not merely projected values, in rank order.
                                    let ids: Vec<_> = result
                                        .records
                                        .iter()
                                        .map(|r| {
                                            // Direct map serialization preserves the
                                            // explicitly materialized physical-ID field.
                                            let bytes = rmp_serde::to_vec_named(r).unwrap();
                                            let back: shamir_engine::query::read::QueryRecord =
                                                rmp_serde::from_slice(&bytes).unwrap();
                                            back.get_value_str("physical_id").unwrap().to_owned()
                                        })
                                        .collect();
                                    let expected_ids: Vec<_> =
                                        staged_ids[..10].iter().map(ToString::to_string).collect();
                                    assert_eq!(ids, expected_ids);
                                }
                            }
                            // Uncommitted writes are discarded; pin must outlive the read.
                            drop(tx);
                            drop(guard);
                            drop(table);
                            drop(repo);
                        },
                    );
                }
            }
        }
    }
    h.run();
}
