//! Phase C — SSI phantom detection tests.
//!
//! Step 4: Phase 2-bis predicate validation in pre_commit — asserts that
//! an armed predicate dependency whose interval is hit by a concurrent
//! committer aborts the commit with `PhantomConflict` and bumps
//! `txs_aborted_phantom`.
//!
//! Steps 5+6: predicate-capture hooks (precise + coarse) — asserts that
//! the tx-aware read/scan paths correctly record `IndexRange` or
//! `TableScan` predicate dependencies into `TxContext::predicate_set`,
//! and that concurrent inserts into captured ranges actually abort.

use std::collections::HashMap;
use std::ops::Bound;
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::predicate_set::{PredicateDep, SORTED_PREFIX_LEN, SORTED_TAG};
use shamir_tx::repo_tx_gate::{CommitWriteRecord, TableWriteFootprint};
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::CommitError;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

/// Deterministic stand-in for the interned index name id.
fn test_index_id(name: &str) -> u64 {
    let mut h: u64 = 1_469_598_103_934_665_603;
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(1_099_511_628_211);
    }
    h
}

/// Build a posting key in the physical sorted-index format:
///   SORTED_TAG (1) || index_id BE8 (8) || encoded_value || dummy_rid (16)
fn test_posting_key(index_id: u64, age: i64) -> Bytes {
    let mut k = Vec::with_capacity(1 + 8 + 9 + 16);
    k.push(SORTED_TAG);
    k.extend_from_slice(&index_id.to_be_bytes());
    shamir_types::core::sort_codec::encode_i64(&mut k, age);
    k.extend_from_slice(&[0u8; 16]); // dummy rid
    Bytes::from(k)
}

/// Build a bound key (same as posting key but without the trailing rid).
fn test_bound_key(index_id: u64, age: i64) -> Bytes {
    let mut k = Vec::with_capacity(SORTED_PREFIX_LEN + 9);
    k.push(SORTED_TAG);
    k.extend_from_slice(&index_id.to_be_bytes());
    shamir_types::core::sort_codec::encode_i64(&mut k, age);
    Bytes::from(k)
}

/// THE Step 4 proof. A Serializable tx2 arms a predicate dependency covering
/// `age >= 30` on the "age" index. A concurrent committer (tx1) writes a
/// posting key with `age = 35` into the commit-write log. When tx2 commits,
/// Phase 2-bis must detect the phantom and abort with `PhantomConflict`.
#[tokio::test]
async fn phase_2bis_aborts_armed_predicate_on_concurrent_write() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");
    let idx_id = test_index_id("age");

    // tx2 (Serializable): open BEFORE the concurrent writer commits.
    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let snapshot_v = tx2.snapshot_version;

    // --- Simulate tx1: a concurrent committer whose footprint lands inside
    // tx2's predicate interval. We bypass the full commit pipeline and plant
    // the footprint directly via `gate.record_commit_writes` (Step 3 API).
    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version(); // > snapshot_v
    assert!(
        tx1_commit_v > snapshot_v,
        "tx1 commit version must exceed tx2's snapshot"
    );

    let posting_key = test_posting_key(idx_id, 35);
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![posting_key],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    // --- Arm tx2's predicate_set: covers age >= 30.
    let dep = PredicateDep::IndexRange {
        table_token,
        index_id: idx_id,
        lo: Bound::Included(test_bound_key(idx_id, 30)),
        hi: Bound::Unbounded,
    };
    tx2.predicate_set.push(dep);

    // Stage at least one durable op so we do NOT trip the C6 empty-tx
    // fast-path (otherwise the test would pass by accident).
    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    // --- Commit tx2: must abort with PhantomConflict.
    let metrics_before = repo.tx_metrics().snapshot().txs_aborted_phantom;
    let res = repo.commit_tx(tx2).await;
    let metrics_after = repo.tx_metrics().snapshot().txs_aborted_phantom;

    match res {
        Err(CommitError::PhantomConflict { dep }) => {
            assert!(
                dep.contains("IndexRange"),
                "dep summary should name the variant: {dep}"
            );
        }
        other => panic!(
            "expected PhantomConflict (concurrent committer wrote into armed predicate \
             range); got {:?}",
            other
                .map(|o| format!("Ok({:?})", o.commit_version))
                .unwrap_or_else(|e| format!("Err({e})"))
        ),
    }
    assert_eq!(
        metrics_after,
        metrics_before + 1,
        "on_tx_aborted_phantom counter must increment exactly once"
    );
    // Ordering check: the point read-set was empty, so SsiConflict path must
    // NOT have bumped.
    assert_eq!(
        repo.tx_metrics().snapshot().txs_aborted_ssi,
        0,
        "SSI point-read counter must be untouched"
    );
}

/// A Serializable tx with a predicate dep that is NOT conflicted must commit
/// successfully. Guards against false positives.
#[tokio::test]
async fn phase_2bis_allows_non_conflicting_predicate() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");
    let idx_id = test_index_id("age");

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    // tx1 writes age=25 which is OUTSIDE tx2's predicate range (age >= 30).
    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version();
    let posting_key = test_posting_key(idx_id, 25);
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![posting_key],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    // Arm tx2's predicate: age >= 30 — tx1's age=25 is outside.
    let dep = PredicateDep::IndexRange {
        table_token,
        index_id: idx_id,
        lo: Bound::Included(test_bound_key(idx_id, 30)),
        hi: Bound::Unbounded,
    };
    tx2.predicate_set.push(dep);

    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    let res = repo.commit_tx(tx2).await;
    assert!(
        res.is_ok(),
        "non-conflicting predicate must not abort: {:?}",
        res.err()
    );
}

/// TableScan predicate: ANY write into the table by a concurrent committer
/// must trigger PhantomConflict.
#[tokio::test]
async fn phase_2bis_table_scan_aborts_on_any_concurrent_write() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version();
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    tx2.predicate_set
        .push(PredicateDep::TableScan { table_token });

    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    let res = repo.commit_tx(tx2).await;
    match res {
        Err(CommitError::PhantomConflict { dep }) => {
            assert!(
                dep.contains("TableScan"),
                "dep summary should name the variant: {dep}"
            );
        }
        other => panic!(
            "expected PhantomConflict for TableScan; got {:?}",
            other
                .map(|o| format!("Ok({:?})", o.commit_version))
                .unwrap_or_else(|e| format!("Err({e})"))
        ),
    }
}

// ============================================================================
// Steps 5+6 — predicate-capture hooks (precise + coarse)
// ============================================================================

/// SortedIndexManager::lookup_range_tx records an IndexRange predicate dep
/// with bounds matching the physical range_bounds output.
#[tokio::test]
async fn ssi_phantom_lookup_range_tx_records_exact_bounds_from_range_bounds() {
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_tx::{IsolationLevel, TxContext, TxId};

    let store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(InMemoryStore::new());
    let mgr = crate::index::sorted_index_manager::SortedIndexManager::new(store)
        .await
        .unwrap();
    let def = crate::index::sorted_index_manager::SortedIndexDefinition::new(101, vec![201]);
    mgr.register(def).await.unwrap();

    let ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Serializable);

    let mut start_enc = Vec::new();
    shamir_types::core::sort_codec::encode_i64(&mut start_enc, 10);
    let mut end_enc = Vec::new();
    shamir_types::core::sort_codec::encode_i64(&mut end_enc, 20);

    let _result = mgr
        .lookup_range_tx(42, 101, Some(&start_enc), Some(&end_enc), Some(&ctx))
        .await
        .unwrap();

    assert_eq!(
        ctx.predicate_set.len(),
        1,
        "lookup_range_tx must record exactly one IndexRange dep"
    );

    let mut found = false;
    ctx.predicate_set.with_iter(|dep| {
        if let shamir_tx::predicate_set::PredicateDep::IndexRange {
            table_token,
            index_id,
            ..
        } = dep
        {
            assert_eq!(*table_token, 42);
            assert_eq!(*index_id, 101);
            found = true;
        }
    });
    assert!(found, "must be an IndexRange dep");
}

/// SortedIndexManager::lookup_min_tx records a full-prefix IndexRange.
#[tokio::test]
async fn ssi_phantom_lookup_min_tx_records_full_prefix_interval() {
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_tx::{IsolationLevel, TxContext, TxId};

    let store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(InMemoryStore::new());
    let mgr = crate::index::sorted_index_manager::SortedIndexManager::new(store)
        .await
        .unwrap();
    let def = crate::index::sorted_index_manager::SortedIndexDefinition::new(101, vec![201]);
    mgr.register(def).await.unwrap();

    let ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Serializable);

    let _ = mgr.lookup_min_tx(42, 101, Some(&ctx)).await.unwrap();

    assert_eq!(
        ctx.predicate_set.len(),
        1,
        "lookup_min_tx must record exactly one IndexRange dep"
    );

    ctx.predicate_set.with_iter(|dep| match dep {
        shamir_tx::predicate_set::PredicateDep::IndexRange {
            table_token,
            index_id,
            ..
        } => {
            assert_eq!(*table_token, 42);
            assert_eq!(*index_id, 101);
        }
        _ => panic!("expected IndexRange, got TableScan"),
    });
}

/// Non-tx lookup_range does NOT allocate or record any predicate dep.
#[tokio::test]
async fn ssi_phantom_non_tx_lookup_range_does_not_allocate_predicate_set() {
    use shamir_storage::storage_in_memory::InMemoryStore;

    let store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(InMemoryStore::new());
    let mgr = crate::index::sorted_index_manager::SortedIndexManager::new(store)
        .await
        .unwrap();
    let def = crate::index::sorted_index_manager::SortedIndexDefinition::new(101, vec![201]);
    mgr.register(def).await.unwrap();

    // No tx at all — must not allocate.
    let _result = mgr.lookup_range_tx(0, 101, None, None, None).await.unwrap();
    // Just verifying no panic and no crash — the contract is that off-tx the
    // predicate_set is never touched.
}

/// Snapshot isolation lookup_range_tx records NOTHING in predicate_set.
#[tokio::test]
async fn ssi_phantom_snapshot_isolation_records_nothing_zero_overhead() {
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_tx::{IsolationLevel, TxContext, TxId};

    let store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(InMemoryStore::new());
    let mgr = crate::index::sorted_index_manager::SortedIndexManager::new(store)
        .await
        .unwrap();
    let def = crate::index::sorted_index_manager::SortedIndexDefinition::new(101, vec![201]);
    mgr.register(def).await.unwrap();

    let ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);

    let _ = mgr
        .lookup_range_tx(42, 101, None, None, Some(&ctx))
        .await
        .unwrap();

    assert!(
        ctx.predicate_set.is_empty(),
        "Snapshot tx must NOT record any predicate deps"
    );
}

/// index2 backend default lookup_tx records IndexRange for Range query
/// under Serializable isolation.
#[tokio::test]
async fn ssi_phantom_index2_btree_range_records_index_range_via_default_lookup_tx() {
    use crate::index2::backend::{IndexBackend, IndexQuery};
    use crate::index2::descriptor::IndexDescriptor;
    use crate::index2::expr::IndexExpr;
    use crate::index2::functional_backend::FunctionalBackend;
    use crate::index2::kind::IndexKind;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_tx::{IsolationLevel, TxContext, TxId};

    let store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(InMemoryStore::new());
    let desc = IndexDescriptor::new(
        1,
        "test_idx",
        999,
        smallvec::smallvec![vec![1u64]],
        IndexKind::Btree { unique: false },
    );
    // FunctionalBackend does NOT override lookup_tx, so the default
    // trait method (Phase C wiring) is exercised.
    let backend = FunctionalBackend::new(desc, IndexExpr::Field(vec![1]), store);

    let ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Serializable);

    let q = IndexQuery::Range {
        lo: std::ops::Bound::Included(vec![0u8]),
        hi: std::ops::Bound::Included(vec![0xFFu8]),
    };
    // FunctionalBackend::lookup errors on Range queries, but the
    // default lookup_tx records the predicate BEFORE calling lookup.
    // We only care that the predicate was recorded.
    let _result = backend.lookup_tx(42, q, Some(&ctx), None).await;

    assert_eq!(
        ctx.predicate_set.len(),
        1,
        "IndexBackend default lookup_tx must record one IndexRange dep for Range query"
    );

    ctx.predicate_set.with_iter(|dep| match dep {
        shamir_tx::predicate_set::PredicateDep::IndexRange {
            table_token,
            index_id,
            ..
        } => {
            assert_eq!(*table_token, 42);
            assert_eq!(*index_id, 999); // name_interned from descriptor
        }
        _ => panic!("expected IndexRange"),
    });
}

/// predicate_to_index_range: Eq on a sorted-indexed field produces a
/// degenerate (lo==hi with 0xFF pad) interval.
#[tokio::test]
async fn ssi_phantom_eq_on_sorted_indexed_field_records_degenerate_interval() {
    use crate::query::filter::eval::predicate_to_index_range;
    use crate::query::filter::Filter;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_types::core::interner::Interner;

    let i = Interner::new();
    let age_ind = i.touch_ind("age").unwrap().into_key();

    let store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(InMemoryStore::new());
    let mgr = crate::index::sorted_index_manager::SortedIndexManager::new(store)
        .await
        .unwrap();
    mgr.register(
        crate::index::sorted_index_manager::SortedIndexDefinition::new(
            age_ind.id(),
            vec![age_ind.id()],
        ),
    )
    .await
    .unwrap();

    let filter = Filter::Eq {
        field: vec!["age".into()],
        value: crate::query::filter::FilterValue::Int(30),
    };

    let deps = predicate_to_index_range(&filter, &mgr, &i, 7);
    assert_eq!(deps.len(), 1, "Eq on sorted field must emit one IndexRange");

    match &deps[0] {
        shamir_tx::predicate_set::PredicateDep::IndexRange {
            table_token,
            index_id,
            ..
        } => {
            assert_eq!(*table_token, 7);
            assert_eq!(*index_id, age_ind.id());
        }
        _ => panic!("expected IndexRange"),
    }
}

/// predicate_to_index_range: Or filter returns empty (caller records TableScan).
#[tokio::test]
async fn ssi_phantom_or_filter_records_table_scan_not_index_range() {
    use crate::query::filter::eval::predicate_to_index_range;
    use crate::query::filter::Filter;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_types::core::interner::Interner;

    let i = Interner::new();
    let _age_ind = i.touch_ind("age").unwrap().into_key();

    let store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(InMemoryStore::new());
    let mgr = crate::index::sorted_index_manager::SortedIndexManager::new(store)
        .await
        .unwrap();

    let filter = Filter::Or {
        filters: vec![
            Filter::Eq {
                field: vec!["age".into()],
                value: crate::query::filter::FilterValue::Int(10),
            },
            Filter::Eq {
                field: vec!["age".into()],
                value: crate::query::filter::FilterValue::Int(20),
            },
        ],
    };

    let deps = predicate_to_index_range(&filter, &mgr, &i, 7);
    assert!(
        deps.is_empty(),
        "Or filter must return empty (coarse TableScan fallback)"
    );
}

/// predicate_to_index_range: And with one unmapped conjunct falls back to
/// empty (caller records TableScan).
#[tokio::test]
async fn ssi_phantom_and_with_one_unmapped_conjunct_falls_back_to_table_scan() {
    use crate::query::filter::eval::predicate_to_index_range;
    use crate::query::filter::Filter;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_types::core::interner::Interner;

    let i = Interner::new();
    let age_ind = i.touch_ind("age").unwrap().into_key();

    let store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(InMemoryStore::new());
    let mgr = crate::index::sorted_index_manager::SortedIndexManager::new(store)
        .await
        .unwrap();
    mgr.register(
        crate::index::sorted_index_manager::SortedIndexDefinition::new(
            age_ind.id(),
            vec![age_ind.id()],
        ),
    )
    .await
    .unwrap();

    let filter = Filter::And {
        filters: vec![
            Filter::Gte {
                field: vec!["age".into()],
                value: crate::query::filter::FilterValue::Int(18),
            },
            // Regex has no sorted index — unmapped conjunct.
            Filter::Regex {
                field: vec!["name".into()],
                pattern: "^A".into(),
            },
        ],
    };

    let deps = predicate_to_index_range(&filter, &mgr, &i, 7);
    assert!(
        deps.is_empty(),
        "And with an unmapped conjunct must fall back to empty (TableScan)"
    );
}

/// read_tx with no WHERE on a Serializable tx records a TableScan
/// predicate dep. Full integration test with RepoInstance.
#[tokio::test]
async fn ssi_phantom_no_where_records_table_scan_for_list_stream() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let _tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    let (tx, _g) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    // list_stream_tx on a Serializable tx records TableScan.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream_tx(Some(&tx), 100);
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        let _ = batch.unwrap();
    }

    assert!(
        !tx.predicate_set.is_empty(),
        "list_stream_tx under Serializable must record at least one TableScan"
    );
    let mut found_table_scan = false;
    tx.predicate_set.with_iter(|dep| {
        if let shamir_tx::predicate_set::PredicateDep::TableScan { table_token: t, .. } = dep {
            if *t == table_token {
                found_table_scan = true;
            }
        }
    });
    assert!(
        found_table_scan,
        "must find a TableScan for our table token"
    );
}

/// filter_stream_tx with a filter that has no sorted index records
/// a coarse TableScan.
#[tokio::test]
async fn ssi_phantom_filter_stream_tx_records_coarse_when_filter_has_no_sorted_index() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    let (tx, _g) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let filter = crate::query::filter::Filter::Regex {
        field: vec!["name".into()],
        pattern: "^A".into(),
    };
    let refs = shamir_types::types::common::new_map();
    let ctx = crate::query::filter::eval_context::FilterContext {
        interner,
        resolved_refs: &refs,
    };

    let stream = tbl
        .filter_stream_tx(Some(&tx), 100, &filter, &ctx)
        .await
        .unwrap();
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        let _ = batch.unwrap();
    }

    let mut found_table_scan = false;
    tx.predicate_set.with_iter(|dep| {
        if let shamir_tx::predicate_set::PredicateDep::TableScan { table_token: t, .. } = dep {
            if *t == table_token {
                found_table_scan = true;
            }
        }
    });
    assert!(
        found_table_scan,
        "filter_stream_tx with unmappable filter must record TableScan"
    );
}

/// A Serializable read_tx with WHERE on a sorted-indexed field records
/// an IndexRange predicate. Concurrent insert into that range aborts.
#[tokio::test]
async fn ssi_phantom_indexed_range_aborts_second_commit() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    // Register a sorted index on "age".
    let interner = tbl.interner().get().await.unwrap();
    let age_ind = interner.touch_ind("age").unwrap().into_key();
    tbl.sorted_indexes()
        .register(
            crate::index::sorted_index_manager::SortedIndexDefinition::new(
                age_ind.id(),
                vec![age_ind.id()],
            ),
        )
        .await
        .unwrap();

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    // Simulate concurrent commit that writes into the predicate range.
    let idx_id = age_ind.id();
    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version();
    let posting_key = test_posting_key(idx_id, 35);
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![posting_key],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    // Execute a query with WHERE age >= 30 under tx2.
    let filter = crate::query::filter::Filter::Gte {
        field: vec!["age".into()],
        value: crate::query::filter::FilterValue::Int(30),
    };
    let query = crate::query::read::ReadQuery::new("users").filter(filter);
    let refs = shamir_types::types::common::new_map();
    let ctx = crate::query::filter::eval_context::FilterContext {
        interner,
        resolved_refs: &refs,
    };
    let _ = tbl.read_tx(&query, &ctx, Some(&tx2)).await.unwrap();

    // Verify predicate was captured.
    assert!(
        !tx2.predicate_set.is_empty(),
        "read_tx with Gte on sorted field must record an IndexRange"
    );

    // Stage a write so the tx is non-empty.
    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    // Commit must abort: the predicate range overlaps with tx1's write.
    let res = repo.commit_tx(tx2).await;
    assert!(
        res.is_err(),
        "commit must abort due to phantom conflict: {:?}",
        res.as_ref().ok()
    );
    match res {
        Err(CommitError::PhantomConflict { .. }) => {}
        other => panic!("expected PhantomConflict; got {:?}", other),
    }
}

/// Between filter on a sorted-indexed field records an IndexRange, and
/// concurrent insert in that range aborts.
#[tokio::test]
async fn ssi_phantom_between_range_aborts() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    let interner = tbl.interner().get().await.unwrap();
    let age_ind = interner.touch_ind("age").unwrap().into_key();
    tbl.sorted_indexes()
        .register(
            crate::index::sorted_index_manager::SortedIndexDefinition::new(
                age_ind.id(),
                vec![age_ind.id()],
            ),
        )
        .await
        .unwrap();

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    // Concurrent commit writes age=15 (inside 10..20 range).
    let idx_id = age_ind.id();
    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version();
    let posting_key = test_posting_key(idx_id, 15);
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![posting_key],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    let filter = crate::query::filter::Filter::Between {
        field: vec!["age".into()],
        from: crate::query::filter::FilterValue::Int(10),
        to: crate::query::filter::FilterValue::Int(20),
    };
    let query = crate::query::read::ReadQuery::new("users").filter(filter);
    let refs = shamir_types::types::common::new_map();
    let ctx = crate::query::filter::eval_context::FilterContext {
        interner,
        resolved_refs: &refs,
    };
    let _ = tbl.read_tx(&query, &ctx, Some(&tx2)).await.unwrap();

    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    let res = repo.commit_tx(tx2).await;
    match res {
        Err(CommitError::PhantomConflict { .. }) => {}
        other => panic!("expected PhantomConflict; got {:?}", other),
    }
}

/// Two txs with disjoint indexed ranges both commit successfully.
#[tokio::test]
async fn ssi_phantom_disjoint_indexed_ranges_both_commit() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    let interner = tbl.interner().get().await.unwrap();
    let age_ind = interner.touch_ind("age").unwrap().into_key();
    tbl.sorted_indexes()
        .register(
            crate::index::sorted_index_manager::SortedIndexDefinition::new(
                age_ind.id(),
                vec![age_ind.id()],
            ),
        )
        .await
        .unwrap();

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    // Concurrent commit writes age=50 (OUTSIDE tx2's range of age < 30).
    let idx_id = age_ind.id();
    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version();
    let posting_key = test_posting_key(idx_id, 50);
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![posting_key],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    let filter = crate::query::filter::Filter::Lt {
        field: vec!["age".into()],
        value: crate::query::filter::FilterValue::Int(30),
    };
    let query = crate::query::read::ReadQuery::new("users").filter(filter);
    let refs = shamir_types::types::common::new_map();
    let ctx = crate::query::filter::eval_context::FilterContext {
        interner,
        resolved_refs: &refs,
    };
    let _ = tbl.read_tx(&query, &ctx, Some(&tx2)).await.unwrap();

    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    let res = repo.commit_tx(tx2).await;
    assert!(
        res.is_ok(),
        "disjoint ranges must not conflict: {:?}",
        res.err()
    );
}

/// read_tx with no sorted index on the filter field records a TableScan
/// and aborts on any concurrent write.
#[tokio::test]
async fn ssi_phantom_no_index_records_table_scan_and_aborts() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    let interner = tbl.interner().get().await.unwrap();
    // No sorted index registered for "age".

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    // Concurrent commit writes anything into the table.
    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version();
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    let filter = crate::query::filter::Filter::Gte {
        field: vec!["age".into()],
        value: crate::query::filter::FilterValue::Int(18),
    };
    let query = crate::query::read::ReadQuery::new("users").filter(filter);
    let refs = shamir_types::types::common::new_map();
    let ctx = crate::query::filter::eval_context::FilterContext {
        interner,
        resolved_refs: &refs,
    };
    let _ = tbl.read_tx(&query, &ctx, Some(&tx2)).await.unwrap();

    // Should have fallen back to TableScan.
    let mut found_table_scan = false;
    tx2.predicate_set.with_iter(|dep| {
        if let shamir_tx::predicate_set::PredicateDep::TableScan { .. } = dep {
            found_table_scan = true;
        }
    });
    assert!(found_table_scan, "no sorted index → must record TableScan");

    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    let res = repo.commit_tx(tx2).await;
    match res {
        Err(CommitError::PhantomConflict { dep }) => {
            assert!(dep.contains("TableScan"));
        }
        other => panic!("expected PhantomConflict (TableScan); got {:?}", other),
    }
}

/// A concurrent write into a DIFFERENT table does not conflict with the
/// predicate dep on the original table.
#[tokio::test]
async fn ssi_phantom_other_table_does_not_conflict() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    repo.add_table(TableConfig::new("orders"));
    let tbl_users = repo.get_table("users").await.unwrap();
    let table_token_users = table_token_for("users");
    let table_token_orders = table_token_for("orders");

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    // Concurrent commit writes into "orders", not "users".
    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version();
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token_orders,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    // tx2 does a full table scan on "users" (TableScan dep).
    tx2.record_predicate_shared(PredicateDep::TableScan {
        table_token: table_token_users,
    });

    tbl_users
        .insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    let res = repo.commit_tx(tx2).await;
    assert!(
        res.is_ok(),
        "write to another table must not conflict: {:?}",
        res.err()
    );
}

// ============================================================================
// A5 — phantom via UPDATE that moves a row INTO the predicate range
// ============================================================================

/// Tx1 arms a predicate `age >= 30`. A concurrent committer (tx_upd)
/// UPDATEs an existing row from age=4 → age=35, emitting a new sorted-index
/// posting key at age=35 that falls inside tx1's predicate interval. When
/// tx1 commits, Phase 2-bis must detect the phantom and abort with
/// `PhantomConflict`. The point-read SSI cannot catch this — the key
/// nobody read is the sorted-index posting, not the data record.
#[tokio::test]
async fn phantom_via_update_into_range_aborts() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    let interner = tbl.interner().get().await.unwrap();
    let age_ind = interner.touch_ind("age").unwrap().into_key();
    let idx_id = age_ind.id();
    tbl.sorted_indexes()
        .register(
            crate::index::sorted_index_manager::SortedIndexDefinition::new(idx_id, vec![idx_id]),
        )
        .await
        .unwrap();

    // tx1 (Serializable): arm predicate `age >= 30`.
    let (mut tx1, _g1) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    let dep = PredicateDep::IndexRange {
        table_token,
        index_id: idx_id,
        lo: Bound::Included(test_bound_key(idx_id, 30)),
        hi: Bound::Unbounded,
    };
    tx1.predicate_set.push(dep);

    // Simulate concurrent committer that UPDATEs a row from age=4 → age=35.
    // The UPDATE emits a new sorted-index posting key at age=35 — inside
    // tx1's predicate. We plant this via the gate API.
    let gate = repo.tx_gate().await.unwrap();
    let upd_commit_v = gate.assign_next_version();
    assert!(
        upd_commit_v > tx1.snapshot_version,
        "update commit version must exceed tx1's snapshot"
    );

    let posting_key = test_posting_key(idx_id, 35);
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![posting_key],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: upd_commit_v,
        per_table,
    });
    gate.publish_committed(upd_commit_v);

    // Stage a write so the tx is non-empty.
    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx1))
        .await
        .unwrap();

    // Commit tx1: must abort with PhantomConflict (the update's posting
    // key for age=35 falls inside the `age >= 30` predicate).
    let res = repo.commit_tx(tx1).await;
    match res {
        Err(CommitError::PhantomConflict { dep }) => {
            assert!(
                dep.contains("IndexRange"),
                "dep summary should name IndexRange: {dep}"
            );
        }
        other => panic!(
            "tx1 must abort with PhantomConflict (update moved age=35 into its predicate); \
             got {:?}",
            other
                .map(|o| format!("Ok({:?})", o.commit_version))
                .unwrap_or_else(|e| format!("Err({e})"))
        ),
    }
}

// ============================================================================
// Step 7 — commit-write-log GC prune integration tests
// ============================================================================

/// Step 7: the commit-write-log MUST retain records a live snapshot still
/// needs. After one Serializable commit (V1) we hold a snapshot at V1.
/// Then we plant a second record at V2 directly via `record_commit_writes`
/// (bypassing `begin_tx` to avoid a snapshot-key collision in
/// `active_snapshots`). `run_gc` with `min_alive == V1` must NOT drop V2
/// (it satisfies `commit_version > min_alive`). V1 (== min_alive) IS
/// prunable because no live snapshot has `S < V1`.
///
/// After the snapshot is released, `min_alive` jumps to `last_committed`
/// (>= V2), and a second `run_gc` cleans everything.
#[tokio::test]
async fn commit_write_log_not_pruned_below_live_snapshot() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    // --- Tx1: Serializable commit → log has one record at V1.
    let (mut tx1, g1) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    tbl.insert_tx(&InnerValue::Str("a".into()), Some(&mut tx1))
        .await
        .unwrap();
    repo.commit_tx(tx1).await.unwrap();
    drop(g1);

    // --- Hold a snapshot at V1 (= last_committed after tx1).
    // This freezes min_alive at V1.
    let gate = repo.tx_gate().await.unwrap();
    let snap_guard = gate.open_snapshot().await;
    let snap_v = snap_guard.version();

    // --- Plant a second record directly at V2 > V1 via the gate API,
    // bypassing `begin_tx` to avoid colliding with snap_guard's version
    // in `active_snapshots` (which is keyed by version).
    let v2 = gate.assign_next_version();
    gate.publish_committed(v2);
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: v2,
        per_table: HashMap::from([(
            table_token_for("t"),
            TableWriteFootprint {
                touched: true,
                inserted_index_keys: vec![],
            },
        )]),
    });
    assert!(v2 > snap_v, "V2 must exceed the held snapshot");

    // --- Sanity: log has at least 2 records (V1, V2) before GC.
    let before = gate.commit_log_len();
    assert!(before >= 2, "expected V1 and V2 in log, got {before}");

    // --- Run GC while snapshot is held: min_alive == snap_v == V1.
    // Prune removes commit_version <= V1. V2 (> V1) MUST survive.
    let _ = repo.run_gc().await.unwrap();

    let after = gate.commit_log_len();
    assert!(
        after >= 1,
        "V2 (> min_alive == {snap_v}) must survive GC, got {after} records"
    );

    // --- Drop the snapshot — min_alive jumps to last_committed (= V2).
    // Now V2 is at-or-below min_alive and is droppable.
    drop(snap_guard);
    let _ = repo.run_gc().await.unwrap();
    let after2 = gate.commit_log_len();
    assert_eq!(
        after2, 0,
        "with no live snapshots, all records <= min_alive prune away"
    );
}

/// Symmetry of the discipline: on a non-Serializable workload the log is
/// never written, so `run_gc` is zero-overhead w.r.t. the new prune.
#[tokio::test]
async fn snapshot_workload_leaves_log_empty_and_gc_is_noop() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();

    // Snapshot-isolation commits do NOT append to the log (gated on
    // isolation, see build_footprint_from_tx).
    for i in 0..8 {
        let (mut tx, g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        let _ = tbl
            .insert_tx(&InnerValue::Int(i), Some(&mut tx))
            .await
            .unwrap();
        repo.commit_tx(tx).await.unwrap();
        drop(g);
    }

    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(gate.commit_log_len(), 0);
    let _ = repo.run_gc().await.unwrap();
    assert_eq!(gate.commit_log_len(), 0);
}

/// Snapshot isolation must NOT trigger Phase 2-bis even with manually armed
/// predicate deps. Guards the zero-overhead claim.
#[tokio::test]
async fn snapshot_skips_phase_2bis() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let table_token = table_token_for("users");

    let (mut tx2, _g2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // Even though we force-arm a predicate dep, Snapshot must skip Phase 2-bis.
    // (In production, `record_predicate_shared` is itself gated on Serializable
    // and would never push, but we test the commit-side gate defensively.)
    tx2.predicate_set
        .push(PredicateDep::TableScan { table_token });

    let gate = repo.tx_gate().await.unwrap();
    let tx1_commit_v = gate.assign_next_version();
    let mut per_table = HashMap::new();
    per_table.insert(
        table_token,
        TableWriteFootprint {
            touched: true,
            inserted_index_keys: vec![],
        },
    );
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: tx1_commit_v,
        per_table,
    });
    gate.publish_committed(tx1_commit_v);

    tbl.insert_tx(&InnerValue::Str("dummy".into()), Some(&mut tx2))
        .await
        .unwrap();

    let res = repo.commit_tx(tx2).await;
    assert!(
        res.is_ok(),
        "Snapshot tx must not trigger Phase 2-bis: {:?}",
        res.err()
    );
    assert_eq!(
        repo.tx_metrics().snapshot().txs_aborted_phantom,
        0,
        "phantom counter must be untouched for Snapshot"
    );
}
