use crate::backend::{FtsMode, IndexBackend, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::fts_backend::FtsBackend;
use crate::kind::{IndexKind, TokenizerKind};
use crate::tests::backends::remove_fail_store::RemoveFailStore;
use crate::tokenizer::{build_tokenizer, token_hash};
use crate::write_ops::IndexWriteOp;
use bytes::Bytes;
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

fn make_rec(interner: &Interner, body: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(intern(interner, "body")),
        InnerValue::Str(body.into()),
    );
    InnerValue::Map(m)
}

async fn apply_insert(
    backend: &FtsBackend,
    store: &Arc<dyn Store>,
    rid: RecordId,
    rec: &InnerValue,
) {
    let ops = backend.plan_insert(rid, rec).await.unwrap();
    crate::apply_index_ops(&ops, store, backend).await.unwrap();
}

async fn apply_update(
    backend: &FtsBackend,
    store: &Arc<dyn Store>,
    rid: RecordId,
    old: &InnerValue,
    new: &InnerValue,
) {
    let ops = backend.plan_update(rid, old, new).await.unwrap();
    crate::apply_index_ops(&ops, store, backend).await.unwrap();
}

async fn apply_delete(
    backend: &FtsBackend,
    store: &Arc<dyn Store>,
    rid: RecordId,
    rec: &InnerValue,
) {
    let ops = backend.plan_delete(rid, rec).await.unwrap();
    crate::apply_index_ops(&ops, store, backend).await.unwrap();
}

fn make_fts(interner: &Interner, store: Arc<dyn Store>) -> FtsBackend {
    let desc = IndexDescriptor::new(
        10,
        "body_fts",
        intern(interner, "body_fts"),
        SmallVec::new(),
        IndexKind::Fts {
            tokenizer: TokenizerKind::Whitespace,
            language: None,
        },
    );
    FtsBackend::new(desc, vec![intern(interner, "body")], store)
}

#[tokio::test]
async fn rv_index_0b756318_fts_drop_propagates_remove_failure() {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    // Seed a key under the FTS drop prefix (descriptor id 10, raw le-bytes)
    // so the scan actually yields and the injected remove failure fires.
    let id: u32 = 10;
    let mut seed_key = id.to_le_bytes().to_vec();
    seed_key.push(b'x');
    inner
        .set(
            bytes::Bytes::from(seed_key).into(),
            bytes::Bytes::from_static(b"junk"),
        )
        .await
        .unwrap();
    let fault = Arc::new(RemoveFailStore::new(inner));
    let i = Interner::new();
    let fts = make_fts(&i, fault);
    assert!(fts.drop_all().await.is_err());
}
#[tokio::test]
async fn and_query() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_fts(&i, Arc::clone(&store));

    let r1 = RecordId::new();
    let r2 = RecordId::new();
    let r3 = RecordId::new();
    apply_insert(&fts, &store, r1, &make_rec(&i, "hello world foo")).await;
    apply_insert(&fts, &store, r2, &make_rec(&i, "hello bar")).await;
    apply_insert(&fts, &store, r3, &make_rec(&i, "world bar")).await;

    // AND("hello", "world") → only r1
    let result = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("hello"), token_hash("world")],
            mode: FtsMode::AndAll,
            limit: None,
        })
        .await
        .unwrap();
    match result {
        IndexResult::Set(s) => {
            assert!(s.contains(&r1));
            assert!(!s.contains(&r2));
            assert!(!s.contains(&r3));
        }
        _ => panic!("expected Set"),
    }
}

#[tokio::test]
async fn or_query() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_fts(&i, Arc::clone(&store));

    let r1 = RecordId::new();
    let r2 = RecordId::new();
    let r3 = RecordId::new();
    apply_insert(&fts, &store, r1, &make_rec(&i, "hello world")).await;
    apply_insert(&fts, &store, r2, &make_rec(&i, "hello bar")).await;
    apply_insert(&fts, &store, r3, &make_rec(&i, "baz qux")).await;

    // OR("hello", "baz") → r1, r2, r3
    let result = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("hello"), token_hash("baz")],
            mode: FtsMode::OrAny,
            limit: None,
        })
        .await
        .unwrap();
    match result {
        IndexResult::Set(s) => {
            assert!(s.contains(&r1));
            assert!(s.contains(&r2));
            assert!(s.contains(&r3));
        }
        _ => panic!("expected Set"),
    }
}

#[tokio::test]
async fn update_diff_tokens() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_fts(&i, Arc::clone(&store));

    let rid = RecordId::new();
    let old = make_rec(&i, "alpha beta gamma");
    apply_insert(&fts, &store, rid, &old).await;

    let new_rec = make_rec(&i, "alpha delta gamma");
    apply_update(&fts, &store, rid, &old, &new_rec).await;

    // "beta" gone
    let r = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("beta")],
            mode: FtsMode::AndAll,
            limit: None,
        })
        .await
        .unwrap();
    match r {
        IndexResult::Set(s) => assert!(s.is_empty()),
        _ => panic!("expected Set"),
    }

    // "delta" present
    let r = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("delta")],
            mode: FtsMode::AndAll,
            limit: None,
        })
        .await
        .unwrap();
    match r {
        IndexResult::Set(s) => assert!(s.contains(&rid)),
        _ => panic!("expected Set"),
    }

    // "alpha" still present (unchanged)
    let r = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("alpha")],
            mode: FtsMode::AndAll,
            limit: None,
        })
        .await
        .unwrap();
    match r {
        IndexResult::Set(s) => assert!(s.contains(&rid)),
        _ => panic!("expected Set"),
    }
}

#[tokio::test]
async fn delete_removes_all_tokens() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_fts(&i, Arc::clone(&store));

    let rid = RecordId::new();
    let rec = make_rec(&i, "hello world");
    apply_insert(&fts, &store, rid, &rec).await;
    apply_delete(&fts, &store, rid, &rec).await;

    let r = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("hello")],
            mode: FtsMode::AndAll,
            limit: None,
        })
        .await
        .unwrap();
    match r {
        IndexResult::Set(s) => assert!(s.is_empty()),
        _ => panic!("expected Set"),
    }
}

#[tokio::test]
async fn empty_query_returns_empty() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_fts(&i, store);

    let r = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![],
            mode: FtsMode::AndAll,
            limit: None,
        })
        .await
        .unwrap();
    match r {
        IndexResult::Set(s) => assert!(s.is_empty()),
        _ => panic!("expected Set"),
    }
}

#[tokio::test]
async fn plan_insert_returns_set_postings() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_fts(&i, Arc::clone(&store));
    let rec = make_rec(&i, "hello world hello");
    let rid = RecordId::new();
    let ops = backend.plan_insert(rid, &rec).await.unwrap();
    // "hello" and "world" -> 2 unique tokens -> 2 SetPostings
    assert_eq!(ops.len(), 2);
    assert!(ops
        .iter()
        .all(|op| matches!(op, IndexWriteOp::SetPosting { .. })));
}

#[tokio::test]
async fn plan_update_returns_diff_ops() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_fts(&i, Arc::clone(&store));
    let old_rec = make_rec(&i, "hello world");
    let new_rec = make_rec(&i, "hello rust");
    let rid = RecordId::new();
    let ops = backend.plan_update(rid, &old_rec, &new_rec).await.unwrap();
    // "world" removed, "rust" added, "hello" unchanged
    let removes: Vec<_> = ops
        .iter()
        .filter(|o| matches!(o, IndexWriteOp::RemovePosting { .. }))
        .collect();
    let sets: Vec<_> = ops
        .iter()
        .filter(|o| matches!(o, IndexWriteOp::SetPosting { .. }))
        .collect();
    assert_eq!(removes.len(), 1); // "world"
    assert_eq!(sets.len(), 1); // "rust"
}

#[tokio::test]
async fn rv_index_0695aada_delete_streams_all_distinct_legacy_grams() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let text = "A1b2C3d4".repeat(9_000);
    // Prove the over-budget premise: for one 72_000-char word with n = 255 the
    // emitted n-gram byte total is (chars - n + 1) * n ≈ 18.3 MB, above
    // MAX_NGRAM_BYTES_PER_VALUE (16 MiB). The materialized path would need
    // ~71_746 distinct 255-byte gram strings (>18 MB) just for the grams.
    let n = 255_usize;
    let emitted_bytes = (text.len() - n + 1) * n;
    assert!(emitted_bytes as u64 > crate::fts::ngram_budget::MAX_NGRAM_BYTES_PER_VALUE);
    let desc = IndexDescriptor::new(
        11,
        "legacy_ngram",
        intern(&interner, "legacy_ngram"),
        SmallVec::new(),
        IndexKind::Fts {
            tokenizer: TokenizerKind::Ngram { n: 255 },
            language: None,
        },
    );
    let backend = FtsBackend::new(desc, vec![intern(&interner, "body")], store);
    let rec = make_rec(&interner, &text);
    let rid = RecordId::new();
    // Memory oracle: count allocated bytes while streaming plan_delete over
    // the over-budget record; the materialized path would need >18 MB of
    // gram strings alone.
    let window = crate::tests::support::allocation_counter::Window::start();
    let ops = backend.plan_delete(rid, &rec).await.unwrap();
    let allocated_bytes = window.finish();
    assert!(
        allocated_bytes < 8 * 1024 * 1024,
        "plan_delete allocated {allocated_bytes} bytes; materializing the grams would need >18 MB"
    );
    let removed: std::collections::BTreeSet<_> = ops
        .iter()
        .filter_map(|op| match op {
            IndexWriteOp::RemovePosting { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect();
    let expected_hashes: std::collections::BTreeSet<_> =
        build_tokenizer(&TokenizerKind::Ngram { n: 255 })
            .tokenize(&text)
            .iter()
            .map(|token| token_hash(token))
            .collect();
    let expected: std::collections::BTreeSet<Bytes> = expected_hashes
        .iter()
        .map(|hash| {
            Bytes::from(crate::posting_layout::build_posting_key(
                11,
                crate::posting_layout::type_tag::FTS,
                &hash.to_le_bytes(),
                &rid,
            ))
        })
        .collect();
    assert_eq!(removed, expected);
    assert_eq!(
        ops.iter()
            .filter(|op| matches!(op, IndexWriteOp::RemovePosting { .. }))
            .count(),
        expected.len()
    );
    assert!(ops
        .iter()
        .all(|op| matches!(op, IndexWriteOp::RemovePosting { .. })));
}

#[tokio::test]
async fn plan_delete_returns_remove_for_all_postings() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_fts(&i, Arc::clone(&store));
    let rec = make_rec(&i, "foo bar baz");
    let rid = RecordId::new();
    let ops = backend.plan_delete(rid, &rec).await.unwrap();
    assert_eq!(ops.len(), 3); // 3 unique tokens
    assert!(ops
        .iter()
        .all(|op| matches!(op, IndexWriteOp::RemovePosting { .. })));
}

#[tokio::test]
async fn plan_apply_round_trip() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_fts(&i, Arc::clone(&store));
    let rec = make_rec(&i, "alpha beta gamma");
    let rid = RecordId::new();

    apply_insert(&backend, &store, rid, &rec).await;

    // Verify all 3 tokens are searchable
    for token in &["alpha", "beta", "gamma"] {
        let result = backend
            .lookup(IndexQuery::Fts {
                tokens: vec![token_hash(token)],
                mode: FtsMode::AndAll,
                limit: None,
            })
            .await
            .unwrap();
        match result {
            IndexResult::Set(s) => assert!(s.contains(&rid), "token '{token}' should match"),
            _ => panic!("expected Set"),
        }
    }
}

#[tokio::test]
async fn plan_insert_writes_postings_per_token() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_fts(&i, Arc::clone(&store));
    let rid = RecordId::new();
    let rec = make_rec(&i, "hello world hello");

    let ops = backend.plan_insert(rid, &rec).await.unwrap();
    let set_ops = ops
        .iter()
        .filter(|o| matches!(o, IndexWriteOp::SetPosting { .. }))
        .count();
    assert_eq!(set_ops, 2, "expected exactly 2 SetPostings (hello + world)");

    crate::apply_index_ops(&ops, &store, &backend)
        .await
        .unwrap();

    let stream = store.iter_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(
        count, 2,
        "store should hold exactly two postings after apply"
    );
}
