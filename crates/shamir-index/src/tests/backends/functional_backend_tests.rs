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
use shamir_types::record_view::scalar_ref_cmp;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::common::{new_map_wc, new_set_wc};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::collections::BTreeSet;
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
    // Dec vs Big vs Bin. An integral Dec and the equal Big share ONE
    // canonical numeric key (RV-FUNCLIB-5FA455F7, index format v3); a
    // non-integral Dec still hashes apart from any Big.
    assert_eq!(
        FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::from(1))),
        FunctionalBackend::hash_value(&InnerValue::Big(BigInt::from(1)))
    );
    assert_ne!(
        FunctionalBackend::hash_value(&InnerValue::Dec(Decimal::new(15, 1))),
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

#[tokio::test]
async fn rv_index_605eec5d_golden_functional_postings() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_numeric_backend(&interner, store);
    let rid = RecordId([0x11; 16]);

    let mut set = new_set_wc(2);
    set.insert(InnerValue::Int(1));
    set.insert(InnerValue::Int(2));
    let mut map = new_map_wc(2);
    map.insert(InternerKey::new(1), InnerValue::Int(1));
    map.insert(InternerKey::new(258), InnerValue::Int(2));

    let cases: [(InnerValue, &[u8]); 9] = [
        (InnerValue::Null, &[0x00]),
        (InnerValue::Bool(true), &[0x01, 0x01]),
        (InnerValue::Int(1), &[0x02, 0, 0, 0, 0, 0, 0, 0, 1]),
        (InnerValue::F64(1.0), &[0x02, 0, 0, 0, 0, 0, 0, 0, 1]),
        (
            InnerValue::Dec(Decimal::from(1)),
            &[0x02, 0, 0, 0, 0, 0, 0, 0, 1],
        ),
        (
            InnerValue::Big(BigInt::from(1)),
            &[0x02, 0, 0, 0, 0, 0, 0, 0, 1],
        ),
        (
            InnerValue::Str("hi".into()),
            &[0x06, 0, 0, 0, 2, b'h', b'i'],
        ),
        (InnerValue::Bin(vec![1, 2]), &[0x07, 0, 0, 0, 2, 1, 2]),
        (
            InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
            &[
                0x08, 0, 0, 0, 2, 0x02, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 0, 0, 0, 0, 2,
            ],
        ),
    ];
    for (value, expected_identity) in cases {
        let rec = make_num_rec(&interner, value.clone());
        let ops = backend.plan_insert(rid, &rec).await.unwrap();
        assert_eq!(ops.len(), 1, "{value:?}");
        let IndexWriteOp::SetPosting {
            key,
            value: posting_value,
            ..
        } = &ops[0]
        else {
            panic!("expected SetPosting for {value:?}");
        };
        let mut expected = vec![0x02, 0x00, 0x00, 0x00, 0x12];
        expected.extend_from_slice(expected_identity);
        expected.extend_from_slice(&[0x11; 16]);
        assert_eq!(key.as_ref(), expected.as_slice(), "{value:?}");
        assert!(posting_value.is_empty());
    }
    for value in [InnerValue::Set(set), InnerValue::Map(map)] {
        let rec = make_num_rec(&interner, value.clone());
        let ops = backend.plan_insert(rid, &rec).await.unwrap();
        assert_eq!(ops.len(), 1, "{value:?}");
        let IndexWriteOp::SetPosting {
            key,
            value: posting_value,
            ..
        } = &ops[0]
        else {
            panic!("expected SetPosting");
        };
        let expected_identity: &[u8] = match &value {
            InnerValue::Set(_) => &[
                0x09, 0, 0, 0, 2, 0x02, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 0, 0, 0, 0, 2,
            ],
            InnerValue::Map(_) => &[
                0x0A, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0,
                0, 0, 1, 2, 0x02, 0, 0, 0, 0, 0, 0, 0, 2,
            ],
            _ => unreachable!(),
        };
        let mut expected = vec![0x02, 0, 0, 0, 0x12];
        expected.extend_from_slice(expected_identity);
        expected.extend_from_slice(&[0x11; 16]);
        assert_eq!(key.as_ref(), expected.as_slice());
        assert!(posting_value.is_empty());
    }
}

#[tokio::test]
async fn rv_index_fbb72544_functional_map_reversed_insertion_hits_same_posting() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_numeric_backend(&interner, Arc::clone(&store));
    let rid = RecordId::new();
    let a = intern(&interner, "a");
    let b = intern(&interner, "b");
    let mut stored = new_map_wc(2);
    stored.insert(InternerKey::new(a), InnerValue::Int(1));
    stored.insert(InternerKey::new(b), InnerValue::Int(2));
    let mut reversed = new_map_wc(2);
    reversed.insert(InternerKey::new(b), InnerValue::Int(2));
    reversed.insert(InternerKey::new(a), InnerValue::Int(1));
    apply_insert_fn(
        &backend,
        &store,
        rid,
        &make_num_rec(&interner, InnerValue::Map(stored)),
    )
    .await;
    assert!(numeric_lookup(&backend, &InnerValue::Map(reversed))
        .await
        .contains(&rid));
}

fn make_num_rec(interner: &Interner, n: InnerValue) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(intern(interner, "n")), n);
    InnerValue::Map(m)
}

fn make_numeric_backend(interner: &Interner, store: Arc<dyn Store>) -> FunctionalBackend {
    let expr = IndexExpr::Field(vec![intern(interner, "n")]);
    let desc = IndexDescriptor::new(
        2,
        "n_idx",
        intern(interner, "n_idx"),
        SmallVec::new(),
        IndexKind::Functional(Box::new(crate::kind::FunctionalConfig {
            expr: expr.clone(),
        })),
    );
    FunctionalBackend::new(desc, expr, store)
}

async fn numeric_lookup(backend: &FunctionalBackend, probe: &InnerValue) -> BTreeSet<RecordId> {
    let hash = FunctionalBackend::hash_value(probe);
    match backend
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![hash.to_vec()],
        })
        .await
        .unwrap()
    {
        IndexResult::Set(s) => s,
        _ => panic!("expected Set"),
    }
}

/// RV-FUNCLIB-5FA455F7: pins the BACKEND-LEVEL union semantics (one
/// Point lookup per IN member, results unioned) that any functional IN
/// planning would compose from. The functional planner path today serves
/// only `eq` single-value Point probes (read_planner.rs `Filter::Computed`);
/// the union-of-probes pattern mirrors the regular-index planner's In arm
/// (read_planner.rs `try_plan_index_scan`).
#[tokio::test]
async fn rv_funclib_5fa455f7_functional_in_probe_union_matches_scan() {
    let corpus: &[InnerValue] = &[
        InnerValue::Int(0),
        InnerValue::F64(0.0),
        InnerValue::F64(-0.0),
        InnerValue::Int(1),
        InnerValue::F64(1.0),
        InnerValue::F64(f64::NAN),
        InnerValue::Int(9007199254740992),
        InnerValue::F64(9007199254740992.0),
        InnerValue::Int(i64::MAX),
        InnerValue::Int(i64::MIN),
        InnerValue::F64(9.3e18),
    ];
    let as_scalar = |v: &InnerValue| match v {
        InnerValue::Int(i) => ScalarRef::Int(*i),
        InnerValue::F64(f) => ScalarRef::F64(*f),
        _ => unreachable!("corpus holds only numeric leaves"),
    };
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_numeric_backend(&interner, Arc::clone(&store));
    let mut indexed = Vec::with_capacity(corpus.len());
    for leaf in corpus {
        let rid = RecordId::new();
        let rec = make_num_rec(&interner, leaf.clone());
        let ops = backend.plan_insert(rid, &rec).await.unwrap();
        apply_index_ops(&ops, &store, &backend).await.unwrap();
        if !ops.is_empty() {
            indexed.push((rid, leaf));
        }
    }
    let in_sets: Vec<Vec<InnerValue>> = vec![
        vec![InnerValue::Int(1), InnerValue::F64(-0.0)],
        vec![InnerValue::F64(1.0), InnerValue::F64(f64::NAN)],
        vec![
            InnerValue::Int(9007199254740992),
            InnerValue::F64(9007199254740992.0),
        ],
        vec![InnerValue::Int(i64::MAX), InnerValue::F64(9.3e18)],
    ];
    for (set_i, set) in in_sets.iter().enumerate() {
        let mut union: BTreeSet<RecordId> = BTreeSet::new();
        for probe in set {
            union.extend(numeric_lookup(&backend, probe).await);
        }
        let expected: BTreeSet<RecordId> = indexed
            .iter()
            .filter(|(_, leaf)| {
                set.iter()
                    .any(|probe| scalar_ref_cmp(as_scalar(probe), leaf) == Some(Ordering::Equal))
            })
            .map(|(rid, _)| *rid)
            .collect();
        assert_eq!(
            union, expected,
            "IN set #{set_i}: union of per-value probes must equal the scan-filter oracle"
        );
    }
}

/// RV-FUNCLIB-5FA455F7: update/delete lifecycle on one rid across NaN
/// transitions — canonical hash equality (Int(1) vs F64(1.0)) must be a
/// no-op on the update path, and NaN claims/removes no posting.
#[tokio::test]
async fn rv_funclib_5fa455f7_functional_nan_update_delete_lifecycle() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_numeric_backend(&interner, Arc::clone(&store));
    let rid = RecordId::new();

    let rec_int1 = make_num_rec(&interner, InnerValue::Int(1));
    let rec_nan = make_num_rec(&interner, InnerValue::F64(f64::NAN));
    let rec_two = make_num_rec(&interner, InnerValue::F64(2.0));
    let rec_f64_one = make_num_rec(&interner, InnerValue::F64(1.0));

    let ops = backend.plan_insert(rid, &rec_int1).await.unwrap();
    apply_index_ops(&ops, &store, &backend).await.unwrap();

    // Int(1) -> NaN: exactly one RemovePosting for the old key.
    let ops = backend.plan_update(rid, &rec_int1, &rec_nan).await.unwrap();
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], IndexWriteOp::RemovePosting { .. }));
    apply_index_ops(&ops, &store, &backend).await.unwrap();
    assert!(numeric_lookup(&backend, &InnerValue::Int(1))
        .await
        .is_empty());

    // NaN -> F64(2.0): exactly one SetPosting.
    let ops = backend.plan_update(rid, &rec_nan, &rec_two).await.unwrap();
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], IndexWriteOp::SetPosting { .. }));
    apply_index_ops(&ops, &store, &backend).await.unwrap();
    assert!(numeric_lookup(&backend, &InnerValue::F64(2.0))
        .await
        .contains(&rid));

    // NaN -> NaN: empty.
    assert!(backend
        .plan_update(rid, &rec_nan, &rec_nan)
        .await
        .unwrap()
        .is_empty());
    // Int(1) -> Int(1) (equal hash): empty.
    assert!(backend
        .plan_update(rid, &rec_int1, &rec_int1)
        .await
        .unwrap()
        .is_empty());
    // Int(1) -> F64(1.0) (canonical hash equality): empty — the parity
    // invariant on the update path.
    assert!(backend
        .plan_update(rid, &rec_int1, &rec_f64_one)
        .await
        .unwrap()
        .is_empty());

    // plan_delete on a NaN-valued record: empty (nothing to remove).
    assert!(backend.plan_delete(rid, &rec_nan).await.unwrap().is_empty());

    // Delete the live F64(2.0) record; afterwards NO probe finds the rid.
    let ops = backend.plan_delete(rid, &rec_two).await.unwrap();
    apply_index_ops(&ops, &store, &backend).await.unwrap();
    for probe in [
        InnerValue::Int(1),
        InnerValue::F64(1.0),
        InnerValue::F64(2.0),
        InnerValue::F64(f64::NAN),
    ] {
        assert!(
            !numeric_lookup(&backend, &probe).await.contains(&rid),
            "probe {probe:?} must not find the deleted rid"
        );
    }
}

/// RV-FUNCLIB-5FA455F7 (D1-S1): functional-index probes must match the
/// scan-filter oracle for mixed numeric values — canonical hash_inner
/// (Int<->F64 parity, -0.0 -> 0) on BOTH the posting and the probe side,
/// and NaN claims no posting at all.
#[tokio::test]
async fn rv_funclib_5fa455f7_functional_numeric_probe_parity() {
    let corpus: &[InnerValue] = &[
        InnerValue::Int(0),
        InnerValue::F64(0.0),
        InnerValue::F64(-0.0),
        InnerValue::Int(1),
        InnerValue::F64(1.0),
        InnerValue::F64(1.5),
        InnerValue::F64(f64::NAN),
        InnerValue::Int(9007199254740993),   // 2^53 + 1
        InnerValue::Int(9007199254740992),   // 2^53
        InnerValue::F64(9007199254740992.0), // 2^53, exactly representable
        InnerValue::Int(i64::MAX),
        InnerValue::Int(i64::MIN),
        InnerValue::F64(9.3e18), // > 2^63: keeps the bit key, equals no Int
    ];
    let as_scalar = |v: &InnerValue| match v {
        InnerValue::Int(i) => ScalarRef::Int(*i),
        InnerValue::F64(f) => ScalarRef::F64(*f),
        _ => unreachable!("corpus holds only numeric leaves"),
    };

    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_numeric_backend(&interner, Arc::clone(&store));

    let mut indexed = Vec::with_capacity(corpus.len());
    for leaf in corpus {
        let rid = RecordId::new();
        let rec = make_num_rec(&interner, leaf.clone());
        let ops = backend.plan_insert(rid, &rec).await.unwrap();
        // NaN claims no posting: plan_insert returns no ops for it.
        if matches!(leaf, InnerValue::F64(f) if f.is_nan()) {
            assert!(ops.is_empty(), "NaN must produce no posting ops");
            continue;
        }
        apply_index_ops(&ops, &store, &backend).await.unwrap();
        indexed.push((rid, leaf));
    }

    for probe in [
        InnerValue::Int(0),
        InnerValue::F64(0.0),
        InnerValue::F64(-0.0),
        InnerValue::Int(1),
        InnerValue::F64(1.0),
        InnerValue::F64(1.5),
        InnerValue::F64(f64::NAN),
        InnerValue::Int(9007199254740993),
        InnerValue::Int(9007199254740992),
        InnerValue::F64(9007199254740992.0),
        InnerValue::Int(i64::MAX),
        InnerValue::Int(i64::MIN),
        InnerValue::F64(9.3e18),
        InnerValue::F64(1.7976931348623157e308),
    ] {
        let found = numeric_lookup(&backend, &probe).await;
        let expected: BTreeSet<RecordId> = indexed
            .iter()
            .filter(|(_, leaf)| scalar_ref_cmp(as_scalar(&probe), leaf) == Some(Ordering::Equal))
            .map(|(rid, _)| *rid)
            .collect();
        assert_eq!(
            found, expected,
            "probe {probe:?} membership must equal the scan-filter oracle"
        );
    }
}
