//! B04: independent numeric/text snapshot + overlay oracle; no indexed oracle reads.
//! Before B04 every shortcut below used committed postings/counters and ignored
//! staging. Snapshot scans and vectored fallbacks also used current MVCC state.

use std::sync::Arc;

use futures::StreamExt;
use shamir_query_builder::{ddl, filter, select, Query};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::read::ReadQuery;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::RecordKey;
use shamir_tx::predicate_set::PredicateDep;
use shamir_tx::{IsolationLevel, TxContext};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::FilterContext;
use crate::query::read::QueryResult;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::{TableConfig, TableManager};
use crate::tx::CommitError;

fn repo() -> RepoInstance {
    RepoInstance::new(
        "b04".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    )
}

async fn row(table: &TableManager, score: i64, body: &str) -> InnerValue {
    let interner = table.interner().get().await.unwrap();
    let mut map = new_map();
    for (name, value) in [
        ("score", InnerValue::Int(score)),
        ("body", InnerValue::Str(body.into())),
        ("tag", InnerValue::Str("yes".into())),
    ] {
        map.insert(interner.touch_ind(name).unwrap().into_key(), value);
    }
    table.interner().persist().await.unwrap();
    InnerValue::Map(map)
}

async fn fixture(indexed: bool) -> (RepoInstance, TableManager, Vec<(RecordId, i64, bool)>) {
    let repo = repo();
    repo.add_table(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let mut rows = Vec::new();
    for (score, matches) in [(10, true), (30, false), (50, true)] {
        let body = if matches { "hello world" } else { "goodbye" };
        let value = row(&table, score, body).await;
        rows.push((table.insert(&value).await.unwrap(), score, matches));
    }
    // Repo tables attach MVCC to the repo gate. Direct insert calls
    // set_versioned, whose VersionGuard commits before insert returns.
    let (tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let mvcc = table.mvcc_store_ref().expect("repo fixture must use MVCC");
    for (id, _, _) in &rows {
        assert!(mvcc.version_of(id.as_bytes()) <= tx.snapshot_version);
    }
    let keys: Vec<_> = rows.iter().map(|(id, _, _)| id.to_bytes()).collect();
    let visible = mvcc.get_at_many(&keys, tx.snapshot_version).await.unwrap();
    assert_eq!(visible.len(), rows.len());
    assert!(visible.iter().all(Option::is_some));
    drop(guard);
    if indexed {
        table.create_index("tag_idx", &["tag"]).await.unwrap();
        table
            .create_sorted_index("score_idx", &["score"])
            .await
            .unwrap();
        let BatchOp::CreateIndex(op) = ddl::create_index("search", "t")
            .field("body")
            .index_type("fts")
            .build()
        else {
            panic!("CREATE INDEX expected")
        };
        table.create_index_v2(&op).await.unwrap();
    }
    (repo, table, rows)
}

async fn read(table: &TableManager, query: &ReadQuery, tx: Option<&TxContext>) -> QueryResult {
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    table.read_tx(query, &ctx, tx).await.unwrap()
}

fn scores(result: &QueryResult) -> Vec<i64> {
    result
        .records
        .iter()
        .map(|r| r.get_value_i64("score").unwrap())
        .collect()
}

fn query(class: &str) -> ReadQuery {
    let q = Query::from("t");
    match class {
        "counter" => q.select([select::count_all("answer")]).build(),
        "min" => q.select([select::min("score", "answer")]).build(),
        "max" => q.select([select::max("score", "answer")]).build(),
        "fts" => q
            .select(["score"])
            .build()
            .filter(filter::fts("body", "hello", "and")),
        "base" => q.select(["score"]).build().filter(filter::eq("tag", "yes")),
        "range" => q.select(["score"]).build().filter(filter::gte("score", 0)),
        "and_range" => q.select(["score"]).build().filter(filter::and([
            filter::gte("score", 0),
            filter::contains("body", "o"),
        ])),
        "order" => q.select(["score"]).order_by_asc("score").limit(2).build(),
        "keyset" => q
            .select(["score"])
            .order_by_asc("score")
            .after(vec![QueryValue::Int(0)], Some(2))
            .build(),
        "scan" => q
            .select(["score"])
            .build()
            .filter(filter::contains("body", "o")),
        _ => panic!("unknown class"),
    }
}

async fn mutate(
    table: &TableManager,
    tx: &mut TxContext,
    rows: &mut Vec<(RecordId, i64, bool)>,
    mutation: &str,
    maximum: bool,
) {
    let score = if maximum { 60 } else { 5 };
    let value = row(table, score, "hello staged").await;
    let position = if maximum { rows.len() - 1 } else { 0 };
    match mutation {
        "insert" => {
            let id = table.insert_tx(&value, Some(tx)).await.unwrap();
            rows.push((id, score, true));
        }
        "update" => {
            assert!(table
                .update_tx(rows[position].0, &value, Some(tx))
                .await
                .unwrap());
            rows[position].1 = score;
            rows[position].2 = true;
        }
        "delete" => {
            assert!(table.delete_tx(rows[position].0, Some(tx)).await.unwrap());
            rows.remove(position);
        }
        _ => panic!("unknown mutation"),
    }
}

fn assert_oracle(result: &QueryResult, class: &str, rows: &[(RecordId, i64, bool)]) {
    let mut expected: Vec<_> = rows
        .iter()
        .filter(|(_, _, matches)| class != "fts" || *matches)
        .map(|(_, score, _)| *score)
        .collect();
    expected.sort_unstable();
    if matches!(class, "counter" | "min" | "max") {
        let answer = match class {
            "counter" => rows.len() as i64,
            "min" => expected[0],
            _ => *expected.last().unwrap(),
        };
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].get_value_i64("answer"), Some(answer));
    } else if matches!(class, "order" | "keyset") {
        expected.truncate(2);
        assert_eq!(scores(result), expected, "{class}");
    } else {
        let mut actual = scores(result);
        actual.sort_unstable();
        assert_eq!(actual, expected, "{class}");
    }
}

#[tokio::test]
async fn tx_read_shortcut_overlay_matrix() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        for class in [
            "counter",
            "min",
            "max",
            "fts",
            "base",
            "range",
            "and_range",
            "order",
            "keyset",
        ] {
            for mutation in ["insert", "update", "delete"] {
                let (repo, table, mut expected) = fixture(true).await;
                let committed = expected.clone();
                let (mut tx, _guard) = repo.begin_tx(isolation).await.unwrap();
                mutate(&table, &mut tx, &mut expected, mutation, class == "max").await;
                let q = query(class);
                let actual = read(&table, &q, Some(&tx)).await;
                assert_oracle(&actual, class, &expected);
                // Even COUNT + UPDATE (unchanged cardinality) must bypass the
                // counter; values alone cannot detect that broken dispatch.
                assert!(
                    actual.stats.as_ref().unwrap().index_used.is_none(),
                    "{class}/{mutation}"
                );
                let outside = read(&table, &q, None).await;
                assert_oracle(&outside, class, &committed);
                assert!(
                    outside.stats.as_ref().unwrap().index_used.is_some(),
                    "{class}"
                );
            }
        }
    }
}

#[tokio::test]
async fn tx_read_shortcut_later_commits_and_direct_fallbacks() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        for mutation in ["insert", "update", "delete"] {
            for staging in [false, true] {
                let (repo, table, snapshot) = fixture(true).await;
                let (mut tx1, _g1) = repo.begin_tx(isolation).await.unwrap();
                let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                let mut latest = snapshot.clone();
                mutate(&table, &mut tx2, &mut latest, mutation, false).await;
                let outcome = repo.commit_tx(tx2).await.unwrap();
                assert!(outcome.commit_version > tx1.snapshot_version);
                let mut expected = snapshot.clone();
                if staging {
                    // A separate own insert makes a staging store exist while
                    // original ids still exercise committed fallback resolution.
                    let value = row(&table, 70, "hello own").await;
                    let id = table.insert_tx(&value, Some(&mut tx1)).await.unwrap();
                    expected.push((id, 70, true));
                }
                for class in ["counter", "base", "scan"] {
                    assert_oracle(
                        &read(&table, &query(class), Some(&tx1)).await,
                        class,
                        &expected,
                    );
                    assert_oracle(&read(&table, &query(class), None).await, class, &latest);
                }
                let interner = table.interner().get().await.unwrap();
                let refs = new_map();
                let ctx = FilterContext::new(interner, &refs);
                let f = filter::eq("tag", "yes");
                let stream = table
                    .filter_stream_tx(Some(&tx1), 1, &f, &ctx)
                    .await
                    .unwrap();
                futures::pin_mut!(stream);
                let mut actual = Vec::new();
                while let Some(batch) = stream.next().await {
                    for (id, cow) in batch.unwrap() {
                        actual.push((id, cow.into_inner().unwrap()));
                    }
                }
                assert_eq!(actual.len(), expected.len());
                for (id, score, _) in &expected {
                    let body = if *score == 30 {
                        "goodbye"
                    } else if *score == 70 {
                        "hello own"
                    } else {
                        "hello world"
                    };
                    let value = row(&table, *score, body).await;
                    assert!(actual.iter().any(|(rid, v)| rid == id && v == &value));
                }
                let mut ids: Vec<_> = expected.iter().map(|(id, _, _)| *id).collect();
                let inserted = latest
                    .iter()
                    .find(|(id, _, _)| !snapshot.iter().any(|(old, _, _)| old == id));
                if let Some((id, _, _)) = inserted {
                    ids.push(*id);
                }
                ids.push(RecordId::system("missing-b04"));
                let bytes = table.get_many_bytes_tx(&ids, Some(&tx1)).await.unwrap();
                assert_eq!(bytes.len(), ids.len());
                for (position, (_, score, _)) in expected.iter().enumerate() {
                    let value = InnerValue::from_bytes(bytes[position].clone().unwrap()).unwrap();
                    let expected_value =
                        actual.iter().find(|(id, _)| *id == ids[position]).unwrap();
                    assert_eq!(value, expected_value.1, "snapshot score {score}");
                }
                assert!(bytes[expected.len()..].iter().all(Option::is_none));
            }
        }
    }
}

#[tokio::test]
async fn tx_read_shortcut_direct_overlay_insert_update_delete() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        for mutation in ["insert", "update", "delete"] {
            let (repo, table, snapshot) = fixture(true).await;
            let (mut tx, _guard) = repo.begin_tx(isolation).await.unwrap();
            let mut expected = snapshot.clone();
            mutate(&table, &mut tx, &mut expected, mutation, false).await;
            // Move a nonmatching row INTO FTS, then a matching row OUT.
            // Filtering before overlay merge must not lose entering updates.
            let entering = row(&table, 31, "hello entering").await;
            table
                .update_tx(snapshot[1].0, &entering, Some(&mut tx))
                .await
                .unwrap();
            let leaving = row(&table, 51, "goodbye").await;
            table
                .update_tx(snapshot[2].0, &leaving, Some(&mut tx))
                .await
                .unwrap();
            let interner = table.interner().get().await.unwrap();
            let refs = new_map();
            let ctx = FilterContext::new(interner, &refs);
            let f = filter::fts("body", "hello", "and");
            let stream = table
                .filter_stream_tx(Some(&tx), 1, &f, &ctx)
                .await
                .unwrap();
            futures::pin_mut!(stream);
            let mut actual = Vec::new();
            while let Some(batch) = stream.next().await {
                actual.extend(
                    batch
                        .unwrap()
                        .into_iter()
                        .map(|(id, cow)| (id, cow.into_inner().unwrap())),
                );
            }
            let mut wanted = vec![(snapshot[1].0, entering.clone())];
            for (id, score, _) in &expected {
                if *id != snapshot[1].0 && *id != snapshot[2].0 {
                    let body = if *score == 5 {
                        "hello staged"
                    } else {
                        "hello world"
                    };
                    wanted.push((*id, row(&table, *score, body).await));
                }
            }
            actual.sort_by_key(|(id, _)| *id);
            wanted.sort_by_key(|(id, _)| *id);
            assert_eq!(actual, wanted);
            let mut ids: Vec<_> = snapshot.iter().map(|(id, _, _)| *id).collect();
            for (id, _, _) in &expected {
                if !ids.contains(id) {
                    ids.push(*id);
                }
            }
            let bytes = table.get_many_bytes_tx(&ids, Some(&tx)).await.unwrap();
            for (id, bytes) in ids.iter().zip(bytes) {
                if *id == snapshot[0].0 && mutation == "delete" {
                    assert!(bytes.is_none());
                } else {
                    let value = InnerValue::from_bytes(bytes.unwrap()).unwrap();
                    let expected_value = if *id == snapshot[2].0 {
                        &leaving
                    } else {
                        &wanted.iter().find(|(rid, _)| rid == id).unwrap().1
                    };
                    assert_eq!(&value, expected_value);
                }
            }
        }
    }
}

#[tokio::test]
async fn tx_read_shortcut_keyset_ties_both_directions() {
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        for descending in [false, true] {
            let (repo, table, _) = fixture(true).await;
            let mut ties = Vec::new();
            for _ in 0..3 {
                ties.push(
                    table
                        .insert(&row(&table, 20, "hello tie").await)
                        .await
                        .unwrap(),
                );
            }
            ties.sort_unstable();
            if descending {
                ties.reverse();
            }
            let (mut tx, _guard) = repo.begin_tx(isolation).await.unwrap();
            let staged = table
                .insert_tx(&row(&table, 20, "hello tie").await, Some(&mut tx))
                .await
                .unwrap();
            let cursor = ties[0];
            let mut expected = ties
                .into_iter()
                .chain([staged])
                .filter(|id| {
                    if descending {
                        *id < cursor
                    } else {
                        *id > cursor
                    }
                })
                .collect::<Vec<_>>();
            expected.sort_unstable();
            if descending {
                expected.reverse();
            }
            let q = Query::from("t").select(["score"]);
            let q = if descending {
                q.order_by_desc("score")
            } else {
                q.order_by_asc("score")
            };
            let q = q
                .after_with_id(
                    vec![QueryValue::Int(20)],
                    Some(expected.len() as u64),
                    Some(cursor),
                )
                .build();
            let result = read(&table, &q, Some(&tx)).await;
            let ids: Vec<_> = result
                .records
                .iter()
                .map(|r| match r {
                    crate::query::read::QueryRecord::Inserted(r) => r.id.unwrap(),
                    _ => panic!("id-bearing result expected"),
                })
                .collect();
            assert_eq!(ids, expected);
            assert_eq!(scores(&result), vec![20; expected.len()]);
        }
    }
}

#[tokio::test]
async fn tx_read_shortcut_ssi_predicate_survives_scan_gate() {
    for indexed in [false, true] {
        let (repo, table, _) = fixture(indexed).await;
        let (mut reader, _gr) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
        let q = Query::from("t").build().filter(filter::gte("score", 100));
        assert!(read(&table, &q, Some(&reader)).await.records.is_empty());
        let mut recorded = false;
        reader.predicate_set.with_iter(|dep| {
            recorded |= match dep {
                PredicateDep::IndexRange { table_token, .. } => {
                    indexed && *table_token == table.table_token()
                }
                PredicateDep::TableScan { table_token } => {
                    !indexed && *table_token == table.table_token()
                }
            };
        });
        assert!(recorded);
        // Only Serializable committers publish a write footprint for SSI.
        let (mut writer, _gw) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
        table
            .insert_tx(&row(&table, 110, "hello phantom").await, Some(&mut writer))
            .await
            .unwrap();
        repo.commit_tx(writer).await.unwrap();
        assert!(read(&table, &q, Some(&reader)).await.records.is_empty());
        table
            .insert_tx(&row(&table, 90, "hello own").await, Some(&mut reader))
            .await
            .unwrap();
        let committed = repo.commit_tx(reader).await;
        assert!(
            matches!(committed, Err(CommitError::PhantomConflict { .. })),
            "indexed={indexed}: expected PhantomConflict, got {committed:?}"
        );
    }
}

#[tokio::test]
async fn tx_read_shortcut_pessimistic_latest_and_actual_lock_owner() {
    let (repo, table, snapshot) = fixture(true).await;
    // Older contender can wound the reader: this probes the real MVCC lock
    // holder, not merely the transaction's bookkeeping registry.
    let (mut contender, _gc) = repo.begin_tx(IsolationLevel::Pessimistic).await.unwrap();
    let (reader, _gr) = repo.begin_tx(IsolationLevel::Pessimistic).await.unwrap();
    let (mut writer, _gw) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let mut latest = snapshot.clone();
    mutate(&table, &mut writer, &mut latest, "update", false).await;
    repo.commit_tx(writer).await.unwrap();
    let result = read(&table, &query("base"), Some(&reader)).await;
    assert_oracle(&result, "base", &latest);
    assert!(result.stats.as_ref().unwrap().index_used.is_some());
    let key = RecordKey::from_slice(snapshot[0].0.as_bytes());
    assert!(reader
        .locked_keys
        .read_sync(&(table.table_token(), key), |_, _| ())
        .is_some());
    assert!(!reader.is_wounded());
    let update = row(&table, 80, "hello contender").await;
    assert!(table
        .update_tx(snapshot[0].0, &update, Some(&mut contender))
        .await
        .unwrap());
    assert!(
        reader.is_wounded(),
        "query read must own a real Shared lock"
    );
}

#[tokio::test]
async fn tx_read_shortcut_filter_stream_decodable_nonmap_fallback() {
    let (repo, table, _) = fixture(false).await;
    let value = InnerValue::Int(7);
    let bytes = value.to_bytes().unwrap();
    assert!(shamir_types::record_view::RecordView::new(&bytes).is_err());
    assert_eq!(InnerValue::from_bytes(bytes).unwrap(), value);
    let id = table.insert(&value).await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    // Empty conjunction is true even for a non-map record.
    let f = filter::and(Vec::<crate::query::filter::Filter>::new());
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        let (tx, _guard) = repo.begin_tx(isolation).await.unwrap();
        let stream = table
            .filter_stream_tx(Some(&tx), 1, &f, &ctx)
            .await
            .unwrap();
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (rid, cow) in batch.unwrap() {
                if rid == id {
                    assert_eq!(cow.into_inner().unwrap(), value);
                    found = true;
                }
            }
        }
        assert!(
            found,
            "decodable Borrowed row must survive the tree fallback"
        );
    }
}

#[tokio::test]
async fn tx_read_shortcut_snapshot_keyset_synthetic_id_rejected() {
    let (repo, table, _) = fixture(true).await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let q = Query::from("t")
        .order_by_asc("_id")
        .after(vec![QueryValue::Str("cursor".into())], Some(2))
        .build();
    for isolation in [IsolationLevel::Snapshot, IsolationLevel::Serializable] {
        let (tx, _guard) = repo.begin_tx(isolation).await.unwrap();
        assert!(matches!(
            table.read_tx(&q, &ctx, Some(&tx)).await,
            Err(DbError::Validation(message)) if message.contains("synthetic _id")
        ));
    }
}
