use crate::backend::{FtsMode, IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::bm25::FtsStatsSnapshot;
use crate::descriptor::IndexDescriptor;
use crate::fts_backend::FtsBackend;
use crate::fts_ranked_backend::{ranked_top_k, FtsRankedBackend};
use crate::kind::{IndexKind, TokenizerKind};
use crate::tests::support::log_sink::capture_session;
use crate::tokenizer::token_hash;
use crate::write_ops::IndexWriteOp;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IndexFamily, Provenance};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::record_view::RecordRef;
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

#[cfg(target_pointer_width = "64")]
#[tokio::test]
async fn rv_index_605eec5d_golden_fts_posting() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, store);
    let rid = RecordId::try_from_bytes(&[0x11; 16]).unwrap();
    let ops = backend
        .plan_insert(rid, &make_rec(&interner, "hello"))
        .await
        .unwrap();
    let postings: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            IndexWriteOp::SetPosting { key, .. } => Some(key.as_ref()),
            _ => None,
        })
        .collect();
    // Index id 20, FTS tag 1, little-endian hello hash, and the fixed record id.
    let expected: [u8; 29] = [
        0x14, 0, 0, 0, 1, 0xe4, 0xe6, 0xbc, 0xc8, 0x07, 0x6c, 0x13, 0x15, 0x11, 0x11, 0x11, 0x11,
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    ];
    assert_eq!(postings, vec![expected.as_slice()]);
}

#[tokio::test]
async fn rv_index_9733f1f3_corrupt_posting_rejected_empty_legacy_accepted() {
    let (_guard, logs) = capture_session().await;
    logs.clear();
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&i, Arc::clone(&store));
    let rid = RecordId::new();
    let ops = backend
        .plan_insert(rid, &make_rec(&i, "alpha"))
        .await
        .unwrap();
    let key = ops
        .iter()
        .find_map(|op| match op {
            IndexWriteOp::SetPosting { key, .. } => Some(key.clone()),
            _ => None,
        })
        .unwrap();
    let query = || IndexQuery::Fts {
        tokens: vec![token_hash("alpha")],
        mode: FtsMode::OrAny,
        limit: None,
    };
    store
        .set(key.clone().into(), Bytes::from_static(b"bad"))
        .await
        .unwrap();
    assert!(backend.lookup(query()).await.is_err());
    assert!(logs.contains("corrupt FTS posting for index 20"));
    store.set(key.into(), Bytes::new()).await.unwrap();
    backend.lookup(query()).await.unwrap();
    assert!(logs.contains("empty legacy FTS posting for index 20"));
    assert!(
        matches!(backend.lookup(query()).await.unwrap(), IndexResult::Ranked(rows) if rows.len() == 1)
    );
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
    let (_guard, logs) = capture_session().await;
    logs.clear();
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

    let malformed_rid = RecordId::new();
    // 0xc1 is the MessagePack "never used" byte; prove the row genuinely
    // fails storage decode before inserting it (a positive-fixint-looking
    // payload like b"malformed row" would decode as the integer 0x6d).
    assert!(InnerValue::from_bytes([0xc1_u8]).is_err());
    data_store
        .set(
            malformed_rid.to_bytes().into(),
            Bytes::from_static(&[0xc1_u8]),
        )
        .await
        .unwrap();

    // Rebuild from data_store.
    fts.rebuild(Arc::clone(&data_store)).await.unwrap();
    assert!(logs.contains("skipping malformed FTS rebuild row for index 20"));
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

struct CountedRecord {
    record: InnerValue,
    reads: std::sync::atomic::AtomicUsize,
}

impl RecordRef for CountedRecord {
    fn scalar_at(&self, path: &[InternerKey]) -> Option<shamir_types::record_view::ScalarRef<'_>> {
        self.record.scalar_at(path)
    }
    fn str_at(&self, path: &[InternerKey]) -> Option<&str> {
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.record.str_at(path)
    }
    fn present_kind_at(&self, path: &[InternerKey]) -> Option<shamir_types::record_view::Kind> {
        self.record.present_kind_at(path)
    }
    fn any_seq_elem(
        &self,
        path: &[InternerKey],
        f: &mut dyn FnMut(shamir_types::record_view::ScalarRef<'_>) -> bool,
    ) -> Option<bool> {
        self.record.any_seq_elem(path, f)
    }
    fn materialize_at(&self, path: &[InternerKey]) -> Option<InnerValue> {
        self.record.materialize_at(path)
    }
    fn for_each_field(&self, f: &mut dyn FnMut(InternerKey, InnerValue)) {
        self.record.for_each_field(f)
    }
    fn to_query_value(
        &self,
        interner: &Interner,
    ) -> Result<shamir_types::types::value::QueryValue, shamir_types::codecs::CodecError> {
        self.record.to_query_value(interner)
    }
}

#[tokio::test]
async fn rv_index_872016d9_ranked_update_reads_old_once() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = make_backend(&interner, store);
    let old = CountedRecord {
        record: make_rec(&interner, "old old text"),
        reads: std::sync::atomic::AtomicUsize::new(0),
    };
    backend
        .plan_update(RecordId::new(), &old, &make_rec(&interner, "new text"))
        .await
        .unwrap();
    assert_eq!(old.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[tokio::test]
async fn rv_index_0695aada_budget_and_legacy_paths() {
    let interner = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    for (n, text) in [(2, "a".repeat(1_048_578)), (255, "a".repeat(70_000))] {
        let desc = IndexDescriptor::new(
            20,
            "budget",
            intern(&interner, "budget"),
            SmallVec::new(),
            IndexKind::Fts {
                tokenizer: TokenizerKind::Ngram { n },
                language: None,
            },
        );
        let path = vec![intern(&interner, "body")];
        let simple = FtsBackend::new(desc.clone(), path.clone(), Arc::clone(&store));
        let ranked = FtsRankedBackend::new(desc, path, Arc::clone(&store));
        let oversized = make_rec(&interner, &text);
        let short = make_rec(&interner, "short");
        let rid = RecordId::new();
        for backend in [&simple as &dyn IndexBackend, &ranked as &dyn IndexBackend] {
            assert!(matches!(
                backend.plan_insert(rid, &oversized).await,
                Err(IndexError::TokenBudgetExceeded { .. })
            ));
            assert!(matches!(
                backend.plan_update(rid, &short, &oversized).await,
                Err(IndexError::TokenBudgetExceeded { .. })
            ));
            assert!(matches!(
                backend.tokenize_query(&text),
                Err(IndexError::TokenBudgetExceeded { .. })
            ));
            assert!(backend.plan_update(rid, &oversized, &short).await.is_ok());
            assert!(backend.plan_delete(rid, &oversized).await.is_ok());
        }
        let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        data.set(rid.to_bytes().into(), oversized.to_bytes().unwrap())
            .await
            .unwrap();
        ranked.rebuild(data).await.unwrap();
        assert_eq!(ranked.doc_count(), 1);
        assert_eq!(
            ranked.sum_doc_len(),
            (text.len() - usize::from(n) + 1) as u64
        );
    }
}
