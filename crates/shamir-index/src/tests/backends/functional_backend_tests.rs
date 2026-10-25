use crate::backend::{IndexBackend, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::expr::IndexExpr;
use crate::functional_backend::FunctionalBackend;
use crate::kind::IndexKind;
use crate::write_ops::{apply_index_ops, IndexWriteOp};
use futures::StreamExt;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::{new_map_wc, new_set_wc};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::sync::Arc;

fn intern(i: &Interner, s: &str) -> u64 {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn make_rec(interner: &Interner, email: &str, age: i64) -> InnerValue {
    let mut m = new_map_wc(3);
    m.insert(
        InternerKey::new(intern(interner, "email")),
        InnerValue::Str(email.into()),
    );
    m.insert(
        InternerKey::new(intern(interner, "age")),
        InnerValue::Int(age),
    );
    InnerValue::Map(m)
}

async fn apply_insert_fn(
    backend: &FunctionalBackend,
    store: &Arc<dyn Store>,
    rid: RecordId,
    rec: &InnerValue,
) {
    let ops = backend.plan_insert(rid, rec).await.unwrap();
    apply_index_ops(&ops, store, backend).await.unwrap();
}

async fn apply_update_fn(
    backend: &FunctionalBackend,
    store: &Arc<dyn Store>,
    rid: RecordId,
    old: &InnerValue,
    new: &InnerValue,
) {
    let ops = backend.plan_update(rid, old, new).await.unwrap();
    apply_index_ops(&ops, store, backend).await.unwrap();
}

fn make_backend(interner: &Interner, store: Arc<dyn Store>) -> FunctionalBackend {
    let expr = IndexExpr::Lower(Box::new(IndexExpr::Trim(Box::new(IndexExpr::Field(vec![
        intern(interner, "email"),
    ])))));
    let desc = IndexDescriptor::new(
        1,
        "email_lower",
        intern(interner, "email_lower"),
        SmallVec::new(),
        IndexKind::Functional(Box::new(crate::kind::FunctionalConfig {
            expr: expr.clone(),
        })),
    );
    FunctionalBackend::new(desc, expr, store)
}

#[tokio::test]
async fn insert_and_lookup() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, Arc::clone(&store));

    let rid = RecordId::new();
    let rec = make_rec(&interner, "  Alice@FOO.COM  ", 30);
    apply_insert_fn(&backend, &store, rid, &rec).await;

    let lookup_val = InnerValue::Str("alice@foo.com".into());
    let hash = FunctionalBackend::hash_value(&lookup_val);
    let result = backend
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![hash.to_vec()],
        })
        .await
        .unwrap();

    match result {
        IndexResult::Set(s) => assert!(s.contains(&rid)),
        _ => panic!("expected Set"),
    }
}

#[tokio::test]
async fn update_changes_posting() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, Arc::clone(&store));

    let rid = RecordId::new();
    let old = make_rec(&interner, "alice@old.com", 25);
    apply_insert_fn(&backend, &store, rid, &old).await;

    let new_rec = make_rec(&interner, "bob@new.com", 25);
    apply_update_fn(&backend, &store, rid, &old, &new_rec).await;

    let old_hash = FunctionalBackend::hash_value(&InnerValue::Str("alice@old.com".into()));
    let r = backend
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![old_hash.to_vec()],
        })
        .await
        .unwrap();
    match r {
        IndexResult::Set(s) => assert!(s.is_empty(), "old posting should be gone"),
        _ => panic!("expected Set"),
    }

    let new_hash = FunctionalBackend::hash_value(&InnerValue::Str("bob@new.com".into()));
    let r = backend
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![new_hash.to_vec()],
        })
        .await
        .unwrap();
    match r {
        IndexResult::Set(s) => assert!(s.contains(&rid)),
        _ => panic!("expected Set"),
    }
}

#[tokio::test]
async fn plan_insert_returns_one_set_posting() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, store);

    let rid = RecordId::new();
    let rec = make_rec(&interner, "  Alice@FOO.COM  ", 30);
    let ops = backend.plan_insert(rid, &rec).await.unwrap();

    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], IndexWriteOp::SetPosting { value, .. } if value.is_empty()));
}

#[tokio::test]
async fn plan_update_returns_remove_old_set_new_if_hash_changes() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, store);

    let rid = RecordId::new();
    let old = make_rec(&interner, "alice@old.com", 25);
    let new_rec = make_rec(&interner, "bob@new.com", 25);
    let ops = backend.plan_update(rid, &old, &new_rec).await.unwrap();

    assert_eq!(ops.len(), 2);
    assert!(matches!(&ops[0], IndexWriteOp::RemovePosting { .. }));
    assert!(matches!(&ops[1], IndexWriteOp::SetPosting { .. }));
}

#[tokio::test]
async fn plan_update_returns_empty_if_hash_same() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, store);

    let rid = RecordId::new();
    let rec1 = make_rec(&interner, "same@email.com", 20);
    let rec2 = make_rec(&interner, "same@email.com", 99);
    let ops = backend.plan_update(rid, &rec1, &rec2).await.unwrap();

    assert!(ops.is_empty());
}

#[tokio::test]
async fn plan_apply_round_trip() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, Arc::clone(&store));

    let rid = RecordId::new();
    let rec = make_rec(&interner, "  Test@Example.COM  ", 42);

    apply_insert_fn(&backend, &store, rid, &rec).await;

    let lookup_val = InnerValue::Str("test@example.com".into());
    let hash = FunctionalBackend::hash_value(&lookup_val);
    let result = backend
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![hash.to_vec()],
        })
        .await
        .unwrap();
    match result {
        IndexResult::Set(s) => assert!(s.contains(&rid)),
        _ => panic!("expected Set"),
    }
}

#[tokio::test]
async fn plan_insert_writes_expected_postings() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, Arc::clone(&store));

    let rid = RecordId::new();
    let rec = make_rec(&interner, "alice@example.com", 30);

    let ops = backend.plan_insert(rid, &rec).await.unwrap();
    let set_ops = ops
        .iter()
        .filter(|o| matches!(o, IndexWriteOp::SetPosting { .. }))
        .count();
    assert_eq!(
        set_ops, 1,
        "expected exactly one SetPosting from functional plan_insert"
    );

    apply_index_ops(&ops, &store, &backend).await.unwrap();

    let stream = store.iter_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(
        count, 1,
        "store should hold exactly one posting after apply"
    );
}

fn make_dec_backend(interner: &Interner, store: Arc<dyn Store>) -> FunctionalBackend {
    let expr = IndexExpr::Field(vec![intern(interner, "amount")]);
    let desc = IndexDescriptor::new(
        2,
        "amount_idx",
        intern(interner, "amount_idx"),
        SmallVec::new(),
        IndexKind::Functional(Box::new(crate::kind::FunctionalConfig {
            expr: expr.clone(),
        })),
    );
    FunctionalBackend::new(desc, expr, store)
}

fn make_dec_rec(interner: &Interner, amount: Decimal) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(intern(interner, "amount")),
        InnerValue::Dec(amount),
    );
    InnerValue::Map(m)
}

// RV-INDEX-56A505F0: Dec/Big/Bin/Set previously collapsed to one shared
// hash via the `_ => h.write_u8(255)` wildcard arm.
#[tokio::test]
async fn rv_index_56a505f0_dec_big_bin_hash_distinctly() {
    // Dec vs Dec.
    assert_ne!(
        FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::from(1))),
        FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::from(2)))
    );
    // Dec vs Big vs Bin.
    assert_ne!(
        FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::from(1))),
        FunctionalBackend::hash_value(&InnerValue::Big(BigInt::from(1)))
    );
    assert_ne!(
        FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::from(1))),
        FunctionalBackend::hash_value(&InnerValue::Bin(vec![1]))
    );
    // Two different Bin vectors.
    assert_ne!(
        FunctionalBackend::hash_value(&InnerValue::Bin(vec![1])),
        FunctionalBackend::hash_value(&InnerValue::Bin(vec![2]))
    );

    // Two different Big integers (content sensitivity).
    assert_ne!(
        FunctionalBackend::hash_value(&InnerValue::Big(BigInt::from(1))),
        FunctionalBackend::hash_value(&InnerValue::Big(BigInt::from(2)))
    );

    // Equal Big / Dec values hash EQUAL (determinism, not just distinctness).
    assert_eq!(
        FunctionalBackend::hash_value(&InnerValue::Big(BigInt::from(1))),
        FunctionalBackend::hash_value(&InnerValue::Big(BigInt::from(1)))
    );
    assert_eq!(
        FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::from(7))),
        FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::from(7)))
    );

    // Two different Sets (content sensitivity). TSet is an IndexSet whose
    // PartialEq is order-insensitive, which is why the order-equality
    // assertion below is the correct semantic pairing.
    let mut s1p = new_set_wc(2);
    s1p.insert(InnerValue::Int(1));
    s1p.insert(InnerValue::Int(2));
    let mut s2p = new_set_wc(2);
    s2p.insert(InnerValue::Int(1));
    s2p.insert(InnerValue::Int(3));
    assert_ne!(
        FunctionalBackend::hash_value(&InnerValue::Set(s1p)),
        FunctionalBackend::hash_value(&InnerValue::Set(s2p))
    );

    // Different cardinality (different write_usize(len)).
    let mut s3p = new_set_wc(2);
    s3p.insert(InnerValue::Int(1));
    s3p.insert(InnerValue::Int(2));
    let mut s4p = new_set_wc(2);
    s4p.insert(InnerValue::Int(1));
    assert_ne!(
        FunctionalBackend::hash_value(&InnerValue::Set(s3p)),
        FunctionalBackend::hash_value(&InnerValue::Set(s4p))
    );

    // Same Set elements in different insertion order hash EQUAL.
    // (TSet is an IndexSet whose PartialEq is order-insensitive, so the
    // pairing below is the correct semantic comparison.)
    let mut s1 = new_set_wc(2);
    s1.insert(InnerValue::Int(1));
    s1.insert(InnerValue::Int(2));
    let mut s2 = new_set_wc(2);
    s2.insert(InnerValue::Int(2));
    s2.insert(InnerValue::Int(1));
    assert_eq!(
        FunctionalBackend::hash_value(&InnerValue::Set(s1)),
        FunctionalBackend::hash_value(&InnerValue::Set(s2))
    );

    // End-to-end: two records with different Dec amounts; look up the
    // first Dec value's hash and expect ONLY the first rid.
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_dec_backend(&interner, Arc::clone(&store));

    let rid1 = RecordId::new();
    let rid2 = RecordId::new();
    apply_insert_fn(
        &backend,
        &store,
        rid1,
        &make_dec_rec(&interner, Decimal::from(10)),
    )
    .await;
    apply_insert_fn(
        &backend,
        &store,
        rid2,
        &make_dec_rec(&interner, Decimal::from(20)),
    )
    .await;

    let hash = FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::from(10)));
    let result = backend
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![hash.to_vec()],
        })
        .await
        .unwrap();
    match result {
        IndexResult::Set(s) => {
            assert!(s.contains(&rid1), "rid1 must be found");
            assert!(!s.contains(&rid2), "rid2 must NOT be found (hash collapse)");
        }
        _ => panic!("expected Set"),
    }
}
