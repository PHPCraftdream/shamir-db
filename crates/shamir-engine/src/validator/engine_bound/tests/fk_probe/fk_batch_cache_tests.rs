//! Batch-scoped FK parent-membership cache proofs (`BatchFkCache`).
//!
//! Moved out of `validator_db_tests.rs` to satisfy the 1000-line layout
//! ratchet; reuses the shared fixtures from that module.

use std::sync::Arc;

use shamir_tx::IsolationLevel;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::TableRef;
use crate::query::TableResolver;

use super::super::validator_db_tests::{setup_with_validator, FinalStateFkValidator};

// ── L4-RV-ENGINE-3C71057D — batch-scoped FK parent-membership cache ────────

/// Batch cache amortization + staged-visibility proof:
/// * N children probing the SAME parent (no index on parent.uid ⇒ the
///   committed-state probe is a full parent-table scan) must perform exactly
///   ONE full scan per batch when a `BatchFkCache` is attached — the
///   `fk_full_scans` counter is the cost oracle. Without the cache the same
///   workload scans N times (the pre-fix O(M × parent_rows) cliff).
/// * A parent row staged INTO the same tx AFTER a cached-absent verdict for
///   a DIFFERENT key must still be found: the staged-overlay probe always
///   runs, so staged parents stay visible (FG-3 read-your-own-writes).
/// * A second distinct value gets its own cache entry and its own scan.
#[tokio::test]
async fn fk_batch_cache_amortizes_parent_scans_and_keeps_staged_visibility() {
    let (resolver, child) = setup_with_validator(Arc::new(FinalStateFkValidator)).await;
    let parent = resolver.resolve(&TableRef::new("parent")).await.unwrap();
    // Seed ONE committed parent row, uid = 1. No index is created on uid on
    // purpose: the committed probe must take the full-scan fallback path.
    let interner = parent.interner().get().await.unwrap();
    let key = interner.touch_ind("uid").unwrap().into_key();
    parent.interner().persist().await.unwrap();
    let mut fields = new_map();
    fields.insert(key, InnerValue::Int(1));
    parent.insert(&InnerValue::Map(fields)).await.unwrap();

    let (mut tx, guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    let cache = crate::validator::BatchFkCache::new();
    crate::validator::engine_bound::validator_db::reset_fk_full_scans();
    // 8 child probes for the same parent key — one committed scan, 7 cache hits.
    // (The handle borrows `tx`, so it is rebuilt after the staged insert below;
    // the cache lives OUTSIDE the handle and keeps its entries across probes.)
    {
        let db =
            crate::validator::ValidatorDb::new_with_fk_cache(&tx, &child, Some(&resolver), &cache);
        for _ in 0..8 {
            assert!(
                db.exists_in(&TableRef::new("parent"), "uid", &QueryValue::Int(1))
                    .await
                    .unwrap(),
                "committed parent uid=1 must be found"
            );
        }
    }
    assert_eq!(
        crate::validator::engine_bound::validator_db::fk_full_scans(),
        1,
        "8 same-key child probes must amortize to exactly ONE committed \
         parent scan with the batch cache attached"
    );
    assert_eq!(cache.len(), 1, "one distinct key → one cache entry");

    // Probe a second key BEFORE any parent for it exists: the committed
    // verdict is cached ABSENT (one more scan).
    {
        let db =
            crate::validator::ValidatorDb::new_with_fk_cache(&tx, &child, Some(&resolver), &cache);
        assert!(
            !db.exists_in(&TableRef::new("parent"), "uid", &QueryValue::Int(2))
                .await
                .unwrap(),
            "no committed or staged parent uid=2 exists yet"
        );
    }
    // A parent row staged into the SAME tx afterwards must still be found,
    // although the committed verdict for that key is cached absent: the
    // staged-overlay probe runs on every call (FG-3).
    let staged_op = shamir_query_builder::write::insert("parent")
        .row(shamir_query_builder::write::doc().set("uid", 2i64).unwrap())
        .build();
    let owned_parent = parent.clone();
    owned_parent
        .execute_insert_tx(
            &staged_op,
            &mut tx,
            true,
            Some(&resolver),
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();
    let db = crate::validator::ValidatorDb::new_with_fk_cache(&tx, &child, Some(&resolver), &cache);
    assert!(
        db.exists_in(&TableRef::new("parent"), "uid", &QueryValue::Int(2))
            .await
            .unwrap(),
        "a parent row staged into the SAME tx must be found through the \
         staged-overlay probe even with a batch cache attached"
    );
    assert_eq!(
        crate::validator::engine_bound::validator_db::fk_full_scans(),
        2,
        "the second key paid one committed scan; the staged hit reuses the          cached-absent verdict without another scan"
    );
    assert_eq!(cache.len(), 2, "second distinct key gets its own entry");
    drop(tx);
    drop(guard);
}

/// Control for the oracle above: WITHOUT the batch cache the same 8-probe
/// workload performs 8 committed full scans (the pre-fix behavior).
#[tokio::test]
async fn fk_without_batch_cache_scans_per_probe() {
    let (resolver, child) = setup_with_validator(Arc::new(FinalStateFkValidator)).await;
    let parent = resolver.resolve(&TableRef::new("parent")).await.unwrap();
    let interner = parent.interner().get().await.unwrap();
    let key = interner.touch_ind("uid").unwrap().into_key();
    parent.interner().persist().await.unwrap();
    let mut fields = new_map();
    fields.insert(key, InnerValue::Int(1));
    parent.insert(&InnerValue::Map(fields)).await.unwrap();

    let (tx, guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    let db = crate::validator::ValidatorDb::new(&tx, &child, Some(&resolver));
    crate::validator::engine_bound::validator_db::reset_fk_full_scans();
    for _ in 0..8 {
        assert!(db
            .exists_in(&TableRef::new("parent"), "uid", &QueryValue::Int(1))
            .await
            .unwrap());
    }
    assert_eq!(
        crate::validator::engine_bound::validator_db::fk_full_scans(),
        8,
        "without the batch cache every probe pays its own full scan"
    );
    drop(tx);
    drop(guard);
}

/// Production wiring: ONE `execute_insert_tx` call carrying 8 child rows that
/// reference the same committed parent must scan the parent table once. If
/// the cache were not threaded through the per-row validator loop, every row
/// would pay its own scan.
#[tokio::test]
async fn fk_batch_insert_through_execute_insert_tx_scans_parent_once() {
    let (resolver, child) = setup_with_validator(Arc::new(FinalStateFkValidator)).await;
    let parent = resolver.resolve(&TableRef::new("parent")).await.unwrap();
    let interner = parent.interner().get().await.unwrap();
    let key = interner.touch_ind("uid").unwrap().into_key();
    parent.interner().persist().await.unwrap();
    let mut fields = new_map();
    fields.insert(key, InnerValue::Int(1));
    parent.insert(&InnerValue::Map(fields)).await.unwrap();

    let (mut tx, guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    let mut op = shamir_query_builder::write::insert("child");
    for _ in 0..8 {
        op = op.row(shamir_query_builder::write::doc().set("uid", 1i64).unwrap());
    }
    crate::validator::engine_bound::validator_db::reset_fk_full_scans();
    child
        .execute_insert_tx(
            &op.build(),
            &mut tx,
            true,
            Some(&resolver),
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();
    assert_eq!(
        crate::validator::engine_bound::validator_db::fk_full_scans(),
        1,
        "8 child rows of one batch must amortize to ONE parent scan"
    );
    drop(tx);
    drop(guard);
}
