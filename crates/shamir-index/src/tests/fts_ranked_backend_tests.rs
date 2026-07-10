use crate::backend::{FtsMode, IndexBackend, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::fts_ranked_backend::FtsRankedBackend;
use crate::kind::{IndexKind, TokenizerKind};
use crate::tokenizer::token_hash;
use crate::write_ops::IndexWriteOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::sync::atomic::Ordering;
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
    backend: &FtsRankedBackend,
    store: &Arc<dyn Store>,
    rid: RecordId,
    rec: &InnerValue,
) {
    let ops = backend.plan_insert(rid, rec).await.unwrap();
    crate::apply_index_ops(&ops, store, backend).await.unwrap();
}

async fn apply_delete(
    backend: &FtsRankedBackend,
    store: &Arc<dyn Store>,
    rid: RecordId,
    rec: &InnerValue,
) {
    let ops = backend.plan_delete(rid, rec).await.unwrap();
    crate::apply_index_ops(&ops, store, backend).await.unwrap();
}

fn make_backend(interner: &Interner, store: Arc<dyn Store>) -> FtsRankedBackend {
    let desc = IndexDescriptor::new(
        20,
        "body_fts_ranked",
        intern(interner, "body_fts_ranked"),
        SmallVec::new(),
        IndexKind::Fts {
            tokenizer: TokenizerKind::Whitespace,
            language: None,
        },
    );
    FtsRankedBackend::new(desc, vec![intern(interner, "body")], store)
}

#[tokio::test]
async fn ranked_and_query_returns_scores() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));

    let r1 = RecordId::new();
    let r2 = RecordId::new();
    // r1 mentions "rust" 3 times → higher tf
    apply_insert(&fts, &store, r1, &make_rec(&i, "rust rust rust is great")).await;
    apply_insert(&fts, &store, r2, &make_rec(&i, "rust is ok")).await;

    let result = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("rust")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();

    match result {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 2);
            // r1 has higher tf → higher score → should be first
            assert_eq!(ranked[0].0, r1);
            assert!(ranked[0].1 > ranked[1].1);
        }
        _ => panic!("expected Ranked"),
    }
}

#[tokio::test]
async fn ranked_or_query_union() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));

    let r1 = RecordId::new();
    let r2 = RecordId::new();
    let r3 = RecordId::new();
    apply_insert(&fts, &store, r1, &make_rec(&i, "alpha beta")).await;
    apply_insert(&fts, &store, r2, &make_rec(&i, "gamma delta")).await;
    apply_insert(&fts, &store, r3, &make_rec(&i, "alpha gamma")).await;

    let result = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("alpha"), token_hash("gamma")],
            mode: FtsMode::OrAny,
        })
        .await
        .unwrap();

    match result {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 3);
            // r3 matches both terms → highest
            assert_eq!(ranked[0].0, r3);
        }
        _ => panic!("expected Ranked"),
    }
}

#[tokio::test]
async fn stats_track_across_insert_delete() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));

    let r1 = RecordId::new();
    let rec = make_rec(&i, "hello world foo bar");
    apply_insert(&fts, &store, r1, &rec).await;

    assert_eq!(fts.stats.doc_count.load(Ordering::Relaxed), 1);
    assert!((fts.stats.avg_doc_len() - 4.0).abs() < 0.01);

    apply_delete(&fts, &store, r1, &rec).await;
    assert_eq!(fts.stats.doc_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn longer_doc_gets_lower_score() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));

    let r_short = RecordId::new();
    let r_long = RecordId::new();
    // Same tf=1 for "rust", but r_long has many more words.
    apply_insert(&fts, &store, r_short, &make_rec(&i, "rust rocks")).await;
    apply_insert(
        &fts,
        &store,
        r_long,
        &make_rec(
            &i,
            "rust is just one of many many many many many words here",
        ),
    )
    .await;

    let result = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("rust")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();

    match result {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 2);
            assert_eq!(ranked[0].0, r_short, "shorter doc should rank higher");
            assert!(ranked[0].1 > ranked[1].1);
        }
        _ => panic!("expected Ranked"),
    }
}

#[tokio::test]
async fn rebuild_restores_stats_from_data_store() {
    let i = Interner::new();
    // Separate stores: data_store holds records, info_store holds postings.
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&info_store));

    // Insert records via data_store directly (simulating persisted data).
    let recs = [
        make_rec(&i, "alpha beta gamma"),    // 3 tokens
        make_rec(&i, "hello world foo bar"), // 4 tokens
        make_rec(&i, "short"),               // 1 token
        make_rec(&i, "a b c d e f"),         // 6 tokens
        make_rec(&i, ""),                    // 0 tokens — skipped
    ];
    for rec in &recs {
        let rid = RecordId::new();
        data_store
            .set(rid.to_bytes().into(), rec.to_bytes().unwrap())
            .await
            .unwrap();
        // Also feed into FTS so postings exist (rebuild only updates stats).
        apply_insert(&fts, &info_store, rid, rec).await;
    }

    // Verify stats are correct after inserts.
    assert_eq!(fts.stats.doc_count.load(Ordering::Relaxed), 4); // "" has doc_len=0 → not counted
    assert_eq!(fts.stats.sum_doc_len.load(Ordering::Relaxed), 14); // 3+4+1+6

    // Reset stats to zero (simulates reopen where counters start at 0).
    fts.stats.doc_count.store(0, Ordering::Relaxed);
    fts.stats.sum_doc_len.store(0, Ordering::Relaxed);

    assert_eq!(fts.stats.doc_count.load(Ordering::Relaxed), 0);

    // Rebuild from data_store.
    fts.rebuild(Arc::clone(&data_store)).await.unwrap();

    // Stats must be restored.
    assert_eq!(fts.stats.doc_count.load(Ordering::Relaxed), 4);
    assert_eq!(fts.stats.sum_doc_len.load(Ordering::Relaxed), 14);
    assert!((fts.stats.avg_doc_len() - 3.5).abs() < 0.01); // 14/4
}

#[tokio::test]
async fn plan_insert_returns_postings_plus_bump() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));
    let rec = make_rec(&i, "hello world");
    let rid = RecordId::new();
    let ops = fts.plan_insert(rid, &rec).await.unwrap();
    let set_count = ops
        .iter()
        .filter(|o| matches!(o, IndexWriteOp::SetPosting { .. }))
        .count();
    let bump_count = ops
        .iter()
        .filter(|o| matches!(o, IndexWriteOp::BumpFtsStats { sign: 1, .. }))
        .count();
    assert_eq!(set_count, 2); // "hello" + "world"
    assert_eq!(bump_count, 1);
}

#[tokio::test]
async fn plan_delete_returns_removes_plus_bump_negative() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));
    let rec = make_rec(&i, "alpha beta");
    let rid = RecordId::new();
    let ops = fts.plan_delete(rid, &rec).await.unwrap();
    let rem_count = ops
        .iter()
        .filter(|o| matches!(o, IndexWriteOp::RemovePosting { .. }))
        .count();
    let bump_neg = ops
        .iter()
        .filter(|o| matches!(o, IndexWriteOp::BumpFtsStats { sign: -1, .. }))
        .count();
    assert_eq!(rem_count, 2);
    assert_eq!(bump_neg, 1);
}

#[tokio::test]
async fn apply_in_memory_bumps_stats() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));
    let ops = vec![
        IndexWriteOp::BumpFtsStats {
            doc_len: 10,
            sign: 1,
        },
        IndexWriteOp::BumpFtsStats {
            doc_len: 3,
            sign: 1,
        },
    ];
    fts.apply_in_memory(&ops).await.unwrap();
    assert_eq!(fts.stats.doc_count.load(Ordering::Relaxed), 2);
    assert_eq!(fts.stats.sum_doc_len.load(Ordering::Relaxed), 13);
}

#[tokio::test]
async fn plan_insert_emits_bump_stats() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));
    let rid = RecordId::new();
    let rec = make_rec(&i, "hello world");

    let ops = fts.plan_insert(rid, &rec).await.unwrap();
    let has_bump = ops
        .iter()
        .any(|o| matches!(o, IndexWriteOp::BumpFtsStats { .. }));
    assert!(has_bump, "FtsRanked plan_insert must emit BumpFtsStats");
}

#[tokio::test]
async fn plan_apply_round_trip_ranked() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&i, Arc::clone(&store));
    let rec = make_rec(&i, "rust is fast and safe");
    let rid = RecordId::new();

    apply_insert(&backend, &store, rid, &rec).await;

    // Stats should reflect the insert
    assert_eq!(backend.stats.doc_count.load(Ordering::Relaxed), 1);

    // Lookup should find the record
    let result = backend
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("rust")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();
    match result {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 1);
            assert_eq!(ranked[0].0, rid);
        }
        _ => panic!("expected Ranked"),
    }
}
