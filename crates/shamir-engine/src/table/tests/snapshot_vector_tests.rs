//! B04b: independent fixture oracle, ordered commits, no sleeps or indexed oracle.

use std::sync::Arc;

use shamir_query_builder::{ddl, filter, Query};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::read::ReadQuery;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::{IsolationLevel, TxContext};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::FilterContext;
use crate::query::read::{QueryRecord, QueryResult};
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::{TableConfig, TableManager};

async fn row(table: &TableManager, score: i64, vector: &[f32], keep: bool) -> InnerValue {
    let interner = table.interner().get().await.unwrap();
    let mut map = new_map();
    for (name, value) in [
        ("score", InnerValue::Int(score)),
        ("keep", InnerValue::Bool(keep)),
        (
            "embedding",
            InnerValue::List(vector.iter().map(|v| InnerValue::F64(*v as f64)).collect()),
        ),
    ] {
        map.insert(interner.touch_ind(name).unwrap().into_key(), value);
    }
    table.interner().persist().await.unwrap();
    InnerValue::Map(map)
}

async fn fixture(metric: &str) -> (RepoInstance, TableManager, Vec<RecordId>) {
    let repo = RepoInstance::new(
        "b04b".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    repo.add_table(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let BatchOp::CreateIndex(op) = ddl::create_index("vec", "t")
        .field("embedding")
        .index_type("vector")
        .vector_dim(2)
        .vector_metric(metric)
        .build()
    else {
        panic!("CREATE INDEX expected")
    };
    table.create_index_v2(&op).await.unwrap();
    let mut ids = Vec::new();
    // Unit vectors, strict ranking for cosine, dot and L2. Row 0 is
    // closer than every matching row but fails the residual predicate.
    for (score, vector, keep) in [
        (0, [1.0, 0.0], false),
        (1, [0.8, 0.6], true),
        (2, [0.0, 1.0], true),
        (3, [-1.0, 0.0], true),
    ] {
        ids.push(
            table
                .insert(&row(&table, score, &vector, keep).await)
                .await
                .unwrap(),
        );
    }
    (repo, table, ids)
}

fn query(residual: bool, k: u32) -> ReadQuery {
    let vector = filter::vector_similarity("embedding", vec![1.0, 0.0], k);
    let predicate = if residual {
        filter::and([vector, filter::eq("keep", true)])
    } else {
        vector
    };
    Query::from("t").select(["score"]).build().filter(predicate)
}

async fn read(table: &TableManager, q: &ReadQuery, tx: Option<&TxContext>) -> QueryResult {
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    table.read_tx(q, &ctx, tx).await.unwrap()
}

fn scores(result: &QueryResult) -> Vec<i64> {
    result
        .records
        .iter()
        .map(|r| r.get_value_i64("score").unwrap())
        .collect()
}

#[tokio::test]
async fn snapshot_vector_later_commit_matrix() {
    for metric in ["cosine", "dot", "l2"] {
        for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
            for residual in [false, true] {
                for mutation in ["insert", "delete", "update"] {
                    for staging in [false, true] {
                        let (repo, table, ids) = fixture(metric).await;
                        let (mut reader, _gr) = repo.begin_tx(isolation).await.unwrap();
                        let (mut writer, _gw) =
                            repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                        let nearest = if residual { 1 } else { 0 };
                        match mutation {
                            "insert" => {
                                table
                                    .insert_tx(
                                        &row(&table, 9, &[1.0, 0.0], true).await,
                                        Some(&mut writer),
                                    )
                                    .await
                                    .unwrap();
                            }
                            "delete" => {
                                assert!(table
                                    .delete_tx(ids[nearest], Some(&mut writer))
                                    .await
                                    .unwrap());
                            }
                            "update" => {
                                assert!(table
                                    .update_tx(
                                        ids[nearest],
                                        &row(&table, 9, &[-0.6, -0.8], true).await,
                                        Some(&mut writer),
                                    )
                                    .await
                                    .unwrap());
                            }
                            _ => unreachable!(),
                        }
                        let outcome = repo.commit_tx(writer).await.unwrap();
                        assert!(outcome.commit_version > reader.snapshot_version);
                        if staging {
                            // Replace a far snapshot vector, delete another, and
                            // insert both a matching and a rejected staged row.
                            table
                                .update_tx(
                                    ids[2],
                                    &row(&table, 20, &[0.6, 0.8], true).await,
                                    Some(&mut reader),
                                )
                                .await
                                .unwrap();
                            table.delete_tx(ids[3], Some(&mut reader)).await.unwrap();
                            for (score, vector, keep) in
                                [(10, [0.96, 0.28], true), (11, [1.0, 0.0], false)]
                            {
                                table
                                    .insert_tx(
                                        &row(&table, score, &vector, keep).await,
                                        Some(&mut reader),
                                    )
                                    .await
                                    .unwrap();
                            }
                        }
                        let q = query(residual, 2);
                        let result = read(&table, &q, Some(&reader)).await;
                        let expected = match (residual, staging) {
                            (true, true) => vec![10, 1],
                            (true, false) => vec![1, 2],
                            (false, true) => vec![0, 11],
                            (false, false) => vec![0, 1],
                        };
                        // The bare staged fixture has two distance-zero rows;
                        // compare membership rather than incidental tie order.
                        let mut actual = scores(&result);
                        let mut expected = expected;
                        if !residual && staging {
                            actual.sort_unstable();
                            expected.sort_unstable();
                        }
                        assert_eq!(
                            actual, expected,
                            "{metric}/{isolation:?}/{residual}/{mutation}/{staging}",
                        );
                        assert!(result.stats.as_ref().unwrap().index_used.is_none());
                        // Latest readers must still observe the writer, not the
                        // snapshot or the reader's uncommitted overlay.
                        let latest = read(&table, &q, None).await;
                        let mut latest_scores = scores(&latest);
                        latest_scores.sort_unstable();
                        let mut expected_latest = match (mutation, residual) {
                            ("insert", false) => vec![0, 9],
                            ("insert", true) => vec![9, 1],
                            ("delete", false) | ("update", false) => vec![1, 2],
                            ("delete", true) => vec![2, 3],
                            ("update", true) => vec![2, 9],
                            _ => unreachable!(),
                        };
                        expected_latest.sort_unstable();
                        assert_eq!(latest_scores, expected_latest);
                        let (pessimistic, _gp) =
                            repo.begin_tx(IsolationLevel::Pessimistic).await.unwrap();
                        let mut pessimistic_scores =
                            scores(&read(&table, &q, Some(&pessimistic)).await);
                        pessimistic_scores.sort_unstable();
                        assert_eq!(pessimistic_scores, expected_latest);
                        assert!(latest.stats.as_ref().unwrap().index_used.is_some());
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn snapshot_vector_unchanged_requires_identity_and_keeps_staging() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        for residual in [false, true] {
            let (repo, table, _) = fixture("cosine").await;
            let (mut tx, _guard) = repo.begin_tx(isolation).await.unwrap();
            table
                .insert_tx(&row(&table, 10, &[0.96, 0.28], true).await, Some(&mut tx))
                .await
                .unwrap();
            let result = read(&table, &query(residual, 2), Some(&tx)).await;
            assert_eq!(
                scores(&result),
                if residual { vec![10, 1] } else { vec![0, 10] },
            );
            // Direct projected rows carry no source ids, so ANN identity
            // cannot be certified even when projected values agree.
            assert!(result.stats.as_ref().unwrap().index_used.is_none());
            assert_eq!(
                scores(&read(&table, &query(residual, 2), None).await),
                if residual { vec![1, 2] } else { vec![0, 1] },
            );
        }
    }
}

#[tokio::test]
async fn snapshot_vector_inflight_allocation_rejects_live_graph() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        let (repo, table, ids) = fixture("cosine").await;
        let (reader, _gr) = repo.begin_tx(isolation).await.unwrap();
        let gate = repo.tx_gate().await.unwrap();
        // Deterministically model the pre-publication index-write window.
        // No committed floor bump: a last_committed-only gate is blind here.
        let pending = gate.assign_next_version_guarded();
        let interner = table.interner().get().await.unwrap();
        let path =
            crate::query::filter::eval::intern_field_path(&["embedding".into()], interner).unwrap();
        let lease = table
            .index2_registry()
            .lease_by_field_and_kind(&path, "vector")
            .await
            .unwrap()
            .unwrap();
        let vb = lease
            .backend
            .as_any()
            .downcast_ref::<crate::index2::vector::vector_backend::VectorBackend>()
            .unwrap();
        vb.adapter_arc().delete(ids[0]).await.unwrap();
        assert_eq!(
            table.mvcc_store_ref().unwrap().current_committed_version(),
            reader.snapshot_version,
        );
        let result = read(&table, &query(false, 2), Some(&reader)).await;
        assert_eq!(scores(&result), vec![0, 1]);
        assert!(result.stats.as_ref().unwrap().index_used.is_none());
        // Phase 5a may publish the floor before completion/index work.
        // A reader opened in that window must not treat allocated == snapshot
        // as evidence that ANN is complete.
        gate.publish_committed_max(pending.version());
        let (during_publish, _gd) = repo.begin_tx(isolation).await.unwrap();
        let result = read(&table, &query(false, 2), Some(&during_publish)).await;
        assert_eq!(scores(&result), vec![0, 1]);
        assert!(result.stats.as_ref().unwrap().index_used.is_none());
        drop(pending);
        // After publication/completion the live graph is still missing the nearest
        // row: only the exact snapshot ranking returns the right rows.
        let (after_publish, _ga) = repo.begin_tx(isolation).await.unwrap();
        let result = read(&table, &query(false, 2), Some(&after_publish)).await;
        assert_eq!(scores(&result), vec![0, 1]);
        assert!(result.stats.as_ref().unwrap().index_used.is_none());
    }
}

#[tokio::test]
async fn snapshot_vector_equal_projection_cannot_certify_identity() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        let (repo, table, ids) = fixture("cosine").await;
        let (tx, _guard) = repo.begin_tx(isolation).await.unwrap();
        let interner = table.interner().get().await.unwrap();
        let path =
            crate::query::filter::eval::intern_field_path(&["embedding".into()], interner).unwrap();
        let lease = table
            .index2_registry()
            .lease_by_field_and_kind(&path, "vector")
            .await
            .unwrap()
            .unwrap();
        let vb = lease
            .backend
            .as_any()
            .downcast_ref::<crate::index2::vector::vector_backend::VectorBackend>()
            .unwrap();
        // A stale graph now picks row 2 instead of row 1. Both rows project
        // the same keep=true, so as_value-only certification accepts it.
        vb.adapter_arc().delete(ids[1]).await.unwrap();
        let q = Query::from("t")
            .select(["keep"])
            .limit(1)
            .count_total(true)
            .with_version()
            .build()
            .filter(filter::and([
                filter::vector_similarity("embedding", vec![1.0, 0.0], 2),
                filter::eq("keep", true),
            ]));
        let result = read(&table, &q, Some(&tx)).await;
        assert_eq!(result.records.len(), 1);
        assert_eq!(
            result.records[0].get_value("keep"),
            Some(&QueryValue::Bool(true)),
        );
        assert!(result.pagination.is_some());
        assert!(result.corrupt_records.is_empty());
        // Identity was erased by projection; do not label the wrong source
        // row as certified ANN even if its value and metadata happen to match.
        assert!(result.stats.as_ref().unwrap().index_used.is_none());
    }
}

#[tokio::test]
async fn snapshot_vector_empty_residual_still_validates_dimension() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        let (repo, table, _) = fixture("cosine").await;
        let (tx, _guard) = repo.begin_tx(isolation).await.unwrap();
        // Force exact dispatch without altering the reader's snapshot.
        table
            .insert(&row(&table, 9, &[1.0, 0.0], true).await)
            .await
            .unwrap();
        let interner = table.interner().get().await.unwrap();
        let refs = new_map();
        let ctx = FilterContext::new(interner, &refs);
        for k in [0, 1] {
            let q = Query::from("t").build().filter(filter::and([
                filter::vector_similarity("embedding", vec![1.0], k),
                filter::eq("score", -1),
            ]));
            let result = table.read_tx(&q, &ctx, Some(&tx)).await;
            assert!(
                matches!(&result, Err(DbError::Internal(message))
                    if message.contains("dimension mismatch: expected 2, got 1")),
                "empty residual must not hide invalid dimensions: {result:?}",
            );
        }
    }
}

#[tokio::test]
async fn snapshot_vector_nonfinite_batch_uses_total_order_before_topk() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        let (repo, table, mut ids) = fixture("l2").await;
        let (mut tx, _guard) = repo.begin_tx(isolation).await.unwrap();
        table
            .insert(&row(&table, 9, &[1.0, 0.0], true).await)
            .await
            .unwrap();
        ids.sort_unstable();
        // Place NaN between infinity and finite candidates in RecordId order.
        // partial_cmp treating NaN as equal can truncate away the best row.
        let vectors = [
            [f32::INFINITY, 0.0],
            [f32::NAN, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
        ];
        for (position, (id, vector)) in ids.iter().zip(&vectors).enumerate() {
            table
                .update_tx(
                    *id,
                    &row(&table, position as i64, vector, true).await,
                    Some(&mut tx),
                )
                .await
                .unwrap();
        }
        let interner = table.interner().get().await.unwrap();
        let path =
            crate::query::filter::eval::intern_field_path(&["embedding".into()], interner).unwrap();
        let lease = table
            .index2_registry()
            .lease_by_field_and_kind(&path, "vector")
            .await
            .unwrap()
            .unwrap();
        let vb = lease
            .backend
            .as_any()
            .downcast_ref::<crate::index2::vector::vector_backend::VectorBackend>()
            .unwrap();
        let adapter = vb.adapter_arc();
        let hnsw = adapter.as_hnsw_adapter().unwrap();
        // Score one row at a time: no adapter sort/truncation can influence
        // the oracle. Reuse only the metric kernel, then rank all distances.
        let mut expected = Vec::new();
        for (id, vector) in ids.iter().zip(&vectors) {
            let candidate = [(*id, vector.to_vec())];
            expected.extend(
                hnsw.score_staged_candidates(&[1.0, 0.0], 1, &candidate)
                    .await
                    .unwrap(),
            );
        }
        expected.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        for k in [1, 2, 4] {
            let result = read(&table, &query(false, k), Some(&tx)).await;
            let expected_scores: Vec<_> = expected
                .iter()
                .take(k as usize)
                .map(|(id, _)| ids.iter().position(|candidate| candidate == id).unwrap() as i64)
                .collect();
            assert_eq!(scores(&result), expected_scores);
            assert!(result.stats.as_ref().unwrap().index_used.is_none());
        }
    }
}

#[tokio::test]
async fn snapshot_id_order_and_keyset_matrix() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        for descending in [false, true] {
            for cursor in [false, true] {
                for after_id in [false, true] {
                    let (repo, table, mut ids) = fixture("cosine").await;
                    let (mut tx, _guard) = repo.begin_tx(isolation).await.unwrap();
                    table.delete_tx(ids.remove(0), Some(&mut tx)).await.unwrap();
                    ids.push(
                        table
                            .insert_tx(&row(&table, 10, &[0.96, 0.28], true).await, Some(&mut tx))
                            .await
                            .unwrap(),
                    );
                    ids.sort_unstable();
                    if descending {
                        ids.reverse();
                    }
                    let q = Query::from("t").select(["score"]);
                    let q = if descending {
                        q.order_by_desc("_id")
                    } else {
                        q.order_by_asc("_id")
                    };
                    let q = if cursor {
                        q.after_with_id(
                            vec![QueryValue::Str(ids[1].to_string())],
                            Some(2),
                            after_id.then_some(ids[1]),
                        )
                    } else {
                        q.limit(2)
                    }
                    .build();
                    let result = read(&table, &q, Some(&tx)).await;
                    let actual: Vec<_> = result
                        .records
                        .iter()
                        .map(|r| match r {
                            QueryRecord::Inserted(r) => r.id.unwrap(),
                            _ => panic!("id-bearing result expected"),
                        })
                        .collect();
                    let expected = if cursor {
                        ids[2..].to_vec()
                    } else {
                        ids[..2].to_vec()
                    };
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}
