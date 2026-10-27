use crate::backend::{FtsMode, IndexBackend, IndexQuery, IndexResult};
use crate::bm25::FtsStatsSnapshot;
use crate::descriptor::IndexDescriptor;
use crate::fts_ranked_backend::{ranked_top_k, FtsRankedBackend};
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

#[test]
fn rv_index_dc5c642e_ranked_top_k_matches_full_order_and_bounds_heap() {
    let candidates = (0..64_u64).map(|id| {
        let rid = RecordId::from_ts_seq(1_700_000_000_000_000, id as u32);
        (rid, if id % 3 == 0 { 4.0 } else { (id % 7) as f64 })
    });
    let all: Vec<_> = candidates.collect();
    let mut expected: Vec<_> = all
        .iter()
        .map(|(rid, score)| (*rid, *score as f32))
        .collect();
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let k = 7;
    let (limited, bounded_max) = ranked_top_k(all, k);
    assert_eq!(limited, expected[..k]);
    assert_eq!(bounded_max, k);
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
            limit: None,
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
            limit: None,
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

    assert_eq!(fts.stats.doc_count(), 1);
    assert!((fts.stats.avg_doc_len() - 4.0).abs() < 0.01);

    apply_delete(&fts, &store, r1, &rec).await;
    assert_eq!(fts.stats.doc_count(), 0);
}

#[tokio::test]
async fn rv_index_68011961_update_empty_to_nonempty_counts_document() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));
    let rid = RecordId::new();
    let empty = make_rec(&i, "");
    let nonempty = make_rec(&i, "alpha beta");

    let ops = fts.plan_update(rid, &empty, &nonempty).await.unwrap();
    let bumps: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            IndexWriteOp::BumpFtsStats { doc_len, sign, .. } => Some((*doc_len, *sign)),
            _ => None,
        })
        .collect();
    assert_eq!(bumps, vec![(2, 1)]);
    crate::apply_index_ops(&ops, &store, &fts).await.unwrap();
    assert_eq!(
        fts.stats.snapshot(),
        FtsStatsSnapshot {
            doc_count: 1,
            sum_doc_len: 2,
        }
    );
}

#[tokio::test]
async fn rv_index_68011961_update_nonempty_to_empty_removes_document() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let fts = make_backend(&i, Arc::clone(&store));
    let rid = RecordId::new();
    let nonempty = make_rec(&i, "alpha beta");
    let empty = make_rec(&i, "");
    apply_insert(&fts, &store, rid, &nonempty).await;

    let ops = fts.plan_update(rid, &nonempty, &empty).await.unwrap();
    crate::apply_index_ops(&ops, &store, &fts).await.unwrap();
    assert_eq!(fts.stats.snapshot(), FtsStatsSnapshot::default());
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
            limit: None,
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
    assert_eq!(
        fts.stats.snapshot(),
        FtsStatsSnapshot {
            doc_count: 4,
            sum_doc_len: 14,
        }
    );

    // Reset stats to zero (simulates reopen where counters start at 0).
    fts.stats.reset();

    assert_eq!(fts.stats.doc_count(), 0);

    // Rebuild from data_store.
    fts.rebuild(Arc::clone(&data_store)).await.unwrap();

    // Stats must be restored.
    assert_eq!(
        fts.stats.snapshot(),
        FtsStatsSnapshot {
            doc_count: 4,
            sum_doc_len: 14,
        }
    );
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

    // Build a placeholder provenance (never checked in this unit test path)
    use shamir_tx::{IndexFamily, Provenance};
    let provenance = Provenance {
        family: IndexFamily::Index2,
        name_interned: 1,
        instance_epoch: 1,
    };

    let ops = vec![
        IndexWriteOp::BumpFtsStats {
            doc_len: 10,
            sign: 1,
            provenance,
        },
        IndexWriteOp::BumpFtsStats {
            doc_len: 3,
            sign: 1,
            provenance,
        },
    ];
    fts.apply_in_memory(&ops).await.unwrap();
    assert_eq!(
        fts.stats.snapshot(),
        FtsStatsSnapshot {
            doc_count: 2,
            sum_doc_len: 13,
        }
    );
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
    assert_eq!(backend.stats.doc_count(), 1);

    // Lookup should find the record
    let result = backend
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("rust")],
            mode: FtsMode::AndAll,
            limit: None,
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

#[test]
fn rv_index_dc5c642e_heap_high_water_and_adversarial_order() {
    let all: Vec<_> = [1.0, 10.0, 5.0]
        .into_iter()
        .enumerate()
        .map(|(i, score)| {
            (
                RecordId::from_ts_seq(1_700_000_000_000_000, i as u32),
                score,
            )
        })
        .collect();
    let (limited, high_water) = ranked_top_k(all.clone(), 2);
    let mut expected: Vec<_> = all
        .iter()
        .map(|(rid, score)| (*rid, *score as f32))
        .collect();
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    assert_eq!(limited, expected[..2]);
    assert_eq!(high_water, 2);
    let (_, zero_high_water) = ranked_top_k(all.iter().copied(), 0);
    assert_eq!(zero_high_water, 0);
    let (unbounded, huge_high_water) = ranked_top_k(all, usize::MAX);
    assert_eq!(unbounded, expected);
    assert_eq!(huge_high_water, expected.len());
}

#[tokio::test]
async fn rv_index_dc5c642e_backend_collector_high_water_is_bounded() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, Arc::clone(&store));
    for id in 0..128_u32 {
        apply_insert(
            &backend,
            &store,
            RecordId::from_ts_seq(1_700_000_000_000_000, id),
            &make_rec(&interner, "common"),
        )
        .await;
    }
    let result = backend
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("common")],
            mode: FtsMode::OrAny,
            limit: Some(3),
        })
        .await
        .unwrap();
    assert!(matches!(result, IndexResult::Ranked(ref rows) if rows.len() == 3));
    assert_eq!(backend.collector_high_water(), 3);
    assert!(backend.collector_high_water() <= 3);
    assert!(
        backend.collector_high_water() > 0,
        "bounded collector must run"
    );
}

#[tokio::test]
async fn rv_index_dc5c642e_limited_backend_matches_unlimited_for_modes_ties_and_edges() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&i, Arc::clone(&store));
    let ids = [
        RecordId::from_ts_seq(1_700_000_000_000_000, 3),
        RecordId::from_ts_seq(1_700_000_000_000_000, 1),
        RecordId::from_ts_seq(1_700_000_000_000_000, 2),
    ];
    for rid in ids {
        apply_insert(&backend, &store, rid, &make_rec(&i, "alpha beta")).await;
    }
    for mode in [FtsMode::AndAll, FtsMode::OrAny] {
        let lookup = |limit| {
            backend.lookup(IndexQuery::Fts {
                tokens: vec![token_hash("alpha"), token_hash("beta")],
                mode,
                limit,
            })
        };
        let unlimited = match lookup(None).await.unwrap() {
            IndexResult::Ranked(rows) => rows,
            _ => panic!("expected Ranked"),
        };
        let mut expected_ids = ids.to_vec();
        expected_ids.sort();
        assert_eq!(
            unlimited.iter().map(|row| row.0).collect::<Vec<_>>(),
            expected_ids
        );
        for (limit, count) in [
            (Some(0), 0),
            (Some(1), 1),
            (Some(2), 2),
            (Some(usize::MAX), 3),
        ] {
            let limited = match lookup(limit).await.unwrap() {
                IndexResult::Ranked(rows) => rows,
                _ => panic!("expected Ranked"),
            };
            assert_eq!(limited, unlimited[..count]);
        }
    }
}
