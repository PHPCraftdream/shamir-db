use crate::backend::{IndexBackend, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::expr::IndexExpr;
use crate::functional_backend::FunctionalBackend;
use crate::kind::IndexKind;
use crate::write_ops::{apply_index_ops, IndexWriteOp};
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
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
