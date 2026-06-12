use crate::predicate_set::PredicateDep;
use crate::staging_store::StagingStore;
use crate::tx_context::TxContext;
use crate::types::{IsolationLevel, TxId};
use crate::version_provider::VersionProvider;
use crate::IndexWriteOp;
use bytes::Bytes;
use proptest::prelude::*;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::TMap;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::HashMap as StdHashMap;
use std::sync::Arc;

#[test]
fn new_tx_context_is_empty() {
    let ctx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);
    assert!(ctx.is_empty());
    assert_eq!(ctx.tx_id.raw(), 1);
    assert_eq!(ctx.snapshot_version, 10);
}

#[test]
fn bump_counter_accumulates() {
    let mut ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    ctx.bump_counter(1, 5);
    ctx.bump_counter(1, 3);
    ctx.bump_counter(2, -1);
    assert_eq!(ctx.counter_deltas[&1], 8);
    assert_eq!(ctx.counter_deltas[&2], -1);
}

#[test]
fn record_read_only_for_serializable() {
    let mut ctx_si = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    ctx_si.record_read(1, Bytes::from_static(b"k"), 5);
    assert!(ctx_si.read_set.is_empty(), "SI should not track reads");

    let mut ctx_ssi = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Serializable);
    ctx_ssi.record_read(1, Bytes::from_static(b"k"), 5);
    assert_eq!(ctx_ssi.read_set.len(), 1);
}

#[test]
fn stage_vector_buffers_per_table() {
    let mut ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    ctx.stage_vector(10, RecordId([0u8; 16]), vec![1.0, 0.0]);
    ctx.stage_vector(10, RecordId([1u8; 16]), vec![0.0, 1.0]);
    ctx.stage_vector(20, RecordId([2u8; 16]), vec![1.0, 1.0]);

    assert_eq!(ctx.staged_vectors_for(10).map(<[_]>::len), Some(2));
    assert_eq!(ctx.staged_vectors_for(20).map(<[_]>::len), Some(1));
    assert_eq!(ctx.staged_vectors_for(30), None);
    assert!(!ctx.is_empty(), "staged vectors make the tx non-empty");
}

#[test]
fn is_empty_after_mutation() {
    let mut ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    assert!(ctx.is_empty());
    ctx.bump_counter(1, 1);
    assert!(!ctx.is_empty());
}

#[test]
fn drop_is_noop() {
    let ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    drop(ctx);
}

#[tokio::test]
async fn apply_id_remap_rewrites_write_set_bytes() {
    let mut tx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);

    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut staging = StagingStore::new(base);

    let mut m = TMap::default();
    m.insert(InternerKey::new(100), InnerValue::Str("v".into()));
    let val = InnerValue::Map(m);
    let key: RecordKey = Bytes::from_static(b"k1");
    staging.set(key.clone(), val.to_bytes().unwrap());

    tx.write_set.insert(7, staging);

    let mut remap = StdHashMap::new();
    remap.insert(100u64, 1000u64);
    tx.apply_id_remap(&remap).await.unwrap();

    let bytes = tx.write_set[&7].get(key).await.unwrap();
    let decoded = InnerValue::from_bytes(&bytes).unwrap();
    if let InnerValue::Map(m) = decoded {
        assert!(
            m.get(&InternerKey::new(1000)).is_some(),
            "key 100 must have been remapped to 1000"
        );
        assert!(m.get(&InternerKey::new(100)).is_none());
    } else {
        panic!("expected Map");
    }
}

#[tokio::test]
async fn apply_id_remap_empty_is_noop() {
    let mut tx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);
    let empty = StdHashMap::new();
    tx.apply_id_remap(&empty).await.unwrap();
}

#[test]
fn validate_read_set_passes_when_versions_unchanged() {
    let mut tx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"k1"), 5);
    tx.record_read(7, Bytes::from_static(b"k2"), 8);

    // Provider returns same versions → no conflict.
    let result = tx.validate_read_set(|_t, k| match k.as_ref() {
        b"k1" => Some(5),
        b"k2" => Some(8),
        _ => Some(0),
    });
    assert!(result.is_ok());
}

#[test]
fn validate_read_set_detects_advance() {
    let mut tx = TxContext::new(TxId::new(2), 0, 10, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"x"), 5);

    // Concurrent writer bumped version → conflict.
    let result = tx.validate_read_set(|_, k| if k.as_ref() == b"x" { Some(9) } else { Some(0) });
    assert!(result.is_err());
    let (table_id, key) = result.unwrap_err();
    assert_eq!(table_id, 7);
    assert_eq!(key, Bytes::from_static(b"x"));
}

#[test]
fn validate_read_set_empty_passes() {
    let tx = TxContext::new(TxId::new(3), 0, 10, IsolationLevel::Serializable);
    let result = tx.validate_read_set(|_, _| Some(99u64));
    assert!(result.is_ok(), "empty read_set must pass");
}

#[test]
fn validate_read_set_zero_provider_always_passes_si_pattern() {
    let mut tx = TxContext::new(TxId::new(4), 0, 10, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"a"), 5);
    tx.record_read(7, Bytes::from_static(b"b"), 3);
    // Stub provider returns Some(0) — used by Stage 4.D.5 scaffold.
    // 0 <= any version_seen, so passes trivially.
    let result = tx.validate_read_set(|_, _| Some(0u64));
    assert!(result.is_ok());
}

#[test]
fn ensure_table_staging_creates_new() {
    let mut tx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = tx.ensure_table_staging(42, "users", base);
    assert!(staging.is_empty());
    assert_eq!(tx.table_tokens.get(&42), Some(&"users".to_string()));
}

#[test]
fn ensure_table_staging_returns_same() {
    let mut tx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let _ = tx.ensure_table_staging(42, "users", base.clone());
    let s = tx.ensure_table_staging(42, "users", base);
    assert!(s.is_empty());
    assert_eq!(tx.write_set.len(), 1, "should reuse, not duplicate");
}

#[test]
fn set_version_provider_attaches_to_tx() {
    struct MyProvider;
    impl VersionProvider for MyProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> Option<u64> {
            Some(42)
        }
    }

    let mut tx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Serializable);
    assert!(tx.version_provider.is_none());

    tx.set_version_provider(Arc::new(MyProvider));
    assert!(tx.version_provider.is_some());

    let v = tx
        .version_provider
        .as_ref()
        .unwrap()
        .version_of(0, &Bytes::from_static(b"k"));
    assert_eq!(v, Some(42));
}

#[tokio::test]
async fn staged_bytes_accumulates_across_fields() {
    let mut tx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);

    // Empty tx → 0.
    assert_eq!(tx.staged_bytes(), 0);

    // Add a write_set entry: Set("k1", "val") → 2 + 3 = 5 bytes.
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = tx.ensure_table_staging(42, "users", base);
    staging.set(Bytes::from_static(b"k1"), Bytes::from_static(b"val"));
    let after_write = tx.staged_bytes();
    assert!(after_write > 0, "write_set should contribute");
    assert_eq!(after_write, 5);

    // Add an IndexWriteOp::SetPosting → key(3) + value(4) = 7 more.
    tx.index_write_set.push((
        42,
        IndexWriteOp::SetPosting {
            key: Bytes::from_static(b"idx"),
            value: Bytes::from_static(b"post"),
        },
    ));
    let after_index = tx.staged_bytes();
    assert!(after_index > after_write, "index ops should add bytes");
    assert_eq!(after_index, 5 + 7);

    // Add a staged vector: 2-lane f32 → 16 (rid) + 2*4 = 24 bytes.
    tx.stage_vector(1, RecordId([0u8; 16]), vec![1.0, 2.0]);
    let after_vec = tx.staged_bytes();
    assert!(after_vec > after_index, "staged vectors should add bytes");
    assert_eq!(after_vec, 5 + 7 + 24);
}

#[test]
fn validate_read_set_unknown_table_returns_conflict() {
    let mut tx = TxContext::new(TxId::new(10), 0, 10, IsolationLevel::Serializable);
    tx.record_read(99, Bytes::from_static(b"key"), 5);

    // Provider returns None for table_id 99 → conflict.
    let result = tx.validate_read_set(|_, _| None);
    assert!(result.is_err());
    let (table_id, key) = result.unwrap_err();
    assert_eq!(table_id, 99);
    assert_eq!(key, Bytes::from_static(b"key"));
}

#[test]
fn record_predicate_shared_noop_on_snapshot() {
    let ctx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    ctx.record_predicate_shared(PredicateDep::TableScan { table_token: 7 });
    ctx.record_predicate_shared(PredicateDep::IndexRange {
        table_token: 7,
        index_id: 1,
        lo: std::ops::Bound::Unbounded,
        hi: std::ops::Bound::Unbounded,
    });
    assert!(
        ctx.predicate_set.is_empty(),
        "Snapshot isolation must not record predicate deps"
    );
}

#[test]
fn record_predicate_shared_appends_on_serializable() {
    let ctx = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Serializable);
    ctx.record_predicate_shared(PredicateDep::TableScan { table_token: 7 });
    ctx.record_predicate_shared(PredicateDep::IndexRange {
        table_token: 7,
        index_id: 42,
        lo: std::ops::Bound::Included(Bytes::from_static(b"\x00")),
        hi: std::ops::Bound::Excluded(Bytes::from_static(b"\xff")),
    });
    assert_eq!(ctx.predicate_set.len(), 2);
}

// ── conflicts_with: write-set overlap detection ───────────────────

fn make_tx_with_writes(token: u64, keys: &[&[u8]]) -> TxContext {
    let mut tx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = tx.ensure_table_staging(token, "t", base);
    for k in keys {
        staging.set(Bytes::copy_from_slice(k), Bytes::from_static(b"v"));
    }
    tx
}

#[test]
fn conflicts_with_same_table_same_key_is_true() {
    let tx1 = make_tx_with_writes(1, &[b"k1", b"k2"]);
    let tx2 = make_tx_with_writes(1, &[b"k2", b"k3"]);
    assert!(tx1.conflicts_with(&tx2));
}

#[test]
fn conflicts_with_same_table_different_keys_is_false() {
    let tx1 = make_tx_with_writes(1, &[b"a"]);
    let tx2 = make_tx_with_writes(1, &[b"b"]);
    assert!(!tx1.conflicts_with(&tx2));
}

#[test]
fn conflicts_with_different_tables_same_key_is_false() {
    let tx1 = make_tx_with_writes(1, &[b"k"]);
    let tx2 = make_tx_with_writes(2, &[b"k"]);
    assert!(!tx1.conflicts_with(&tx2));
}

#[test]
fn conflicts_with_one_empty_is_false() {
    let tx1 = make_tx_with_writes(1, &[b"k"]);
    let tx2 = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Snapshot);
    assert!(!tx1.conflicts_with(&tx2));
    assert!(!tx2.conflicts_with(&tx1));
}

#[test]
fn conflicts_with_both_empty_is_false() {
    let tx1 = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    let tx2 = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Snapshot);
    assert!(!tx1.conflicts_with(&tx2));
}

// ── proptest: SSI read-set validation properties ──────────────────

/// Reference oracle: independently compute the expected validate_read_set
/// outcome given the recorded reads and the provider map. Mirrors the
/// IFF rule: Ok iff every recorded key has Some(current) with
/// current <= version_seen.
fn oracle_conflict(
    recorded: &StdHashMap<(u64, Vec<u8>), u64>,
    provider: &StdHashMap<(u64, Vec<u8>), Option<u64>>,
) -> bool {
    for ((t, k), version_seen) in recorded {
        match provider.get(&(*t, k.clone())) {
            None => return true,
            Some(None) => return true,
            Some(Some(current)) if *current > *version_seen => return true,
            Some(Some(_)) => {}
        }
    }
    false
}

/// Build a TxContext (Serializable) and replay the generated reads. Returns
/// the tx plus the de-duplicated reference map. The oracle MUST mirror the
/// implementation's dedup rule exactly: `record_read_shared` is
/// **first-read-wins** (keeps the earliest observed version for a key —
/// `Occupied(_) => {}`), so the reference keeps the first `v` it sees and
/// ignores later duplicates. (Under a real snapshot, repeat reads of one
/// key always return the same version, so first == min == last; the
/// divergence only surfaces with the generator's arbitrary triples.)
fn build_tx_with_reads(
    reads: &[(u64, Vec<u8>, u64)],
) -> (TxContext, StdHashMap<(u64, Vec<u8>), u64>) {
    let mut tx = TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Serializable);
    let mut recorded: StdHashMap<(u64, Vec<u8>), u64> = StdHashMap::new();
    for (t, k, v) in reads {
        tx.record_read(*t, Bytes::copy_from_slice(k), *v);
        // First-read-wins: only the first version observed for a key is
        // kept (matches `record_read_shared`'s `Occupied(_) => {}`).
        recorded.entry((*t, k.clone())).or_insert(*v);
    }
    (tx, recorded)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// Property 1 (AGREEMENT / IFF):
    /// `validate_read_set` returns Ok iff for every recorded read the
    /// provider yields Some(current) with current <= version_seen.
    #[test]
    fn prop_validate_read_set_iff_oracle(
        reads in proptest::collection::vec(
            (
                0u64..4,
                proptest::collection::vec(any::<u8>(), 1..=4),
                0u64..=20,
            ),
            0..=8,
        ),
        provider_overrides in proptest::collection::vec(
            (0u64..=25, any::<bool>()),
            0..=8,
        ),
    ) {
        let (tx, recorded) = build_tx_with_reads(&reads);

        let mut provider_map: StdHashMap<(u64, Vec<u8>), Option<u64>> =
            StdHashMap::new();
        let keys: Vec<(u64, Vec<u8>)> = recorded.keys().cloned().collect();
        for (i, k) in keys.iter().enumerate() {
            if provider_overrides.is_empty() {
                provider_map.insert(k.clone(), Some(0));
            } else {
                let (cur, is_none) =
                    provider_overrides[i % provider_overrides.len()];
                provider_map
                    .insert(k.clone(), if is_none { None } else { Some(cur) });
            }
        }

        let expected_conflict = oracle_conflict(&recorded, &provider_map);

        let pm_ref = &provider_map;
        let result = tx.validate_read_set(|t, k| {
            let kv: Vec<u8> = k.as_ref().to_vec();
            match pm_ref.get(&(t, kv)) {
                Some(opt) => *opt,
                None => Some(0),
            }
        });

        prop_assert_eq!(result.is_err(), expected_conflict);

        if let Err((conflict_t, conflict_k)) = result {
            let kv: Vec<u8> = conflict_k.as_ref().to_vec();
            prop_assert!(recorded.contains_key(&(conflict_t, kv.clone())));
            let version_seen = recorded[&(conflict_t, kv.clone())];
            let provided = pm_ref.get(&(conflict_t, kv)).copied();
            let is_real_conflict = match provided {
                Some(None) => true,
                Some(Some(cur)) => cur > version_seen,
                None => false,
            };
            prop_assert!(
                is_real_conflict,
                "validate_read_set returned a non-conflicting key"
            );
        }
    }

    /// Property 2 (MONOTONE BUMP):
    /// Start from an exactly-passing provider (current = version_seen for
    /// every recorded read). Validation MUST succeed. Then, for any
    /// recorded key, bumping its current_version by `bump >= 1` MUST flip
    /// the result to a conflict.
    #[test]
    fn prop_validate_read_set_bump_creates_conflict(
        reads in proptest::collection::vec(
            (
                0u64..4,
                proptest::collection::vec(any::<u8>(), 1..=4),
                0u64..=20,
            ),
            1..=8,
        ),
        pick in 0usize..1024,
        bump in 1u64..=50,
    ) {
        let (tx, recorded) = build_tx_with_reads(&reads);
        prop_assume!(!recorded.is_empty());

        let baseline: StdHashMap<(u64, Vec<u8>), u64> = recorded.clone();

        let baseline_ref = &baseline;
        let baseline_result = tx.validate_read_set(|t, k| {
            let kv: Vec<u8> = k.as_ref().to_vec();
            Some(*baseline_ref.get(&(t, kv)).unwrap_or(&0))
        });
        prop_assert!(
            baseline_result.is_ok(),
            "baseline provider (current == version_seen) must NOT conflict"
        );

        let keys: Vec<(u64, Vec<u8>)> = recorded.keys().cloned().collect();
        let target = keys[pick % keys.len()].clone();

        let mut bumped = baseline.clone();
        let target_seen = bumped[&target];
        bumped.insert(target.clone(), target_seen.saturating_add(bump));

        let bumped_ref = &bumped;
        let bumped_result = tx.validate_read_set(|t, k| {
            let kv: Vec<u8> = k.as_ref().to_vec();
            Some(*bumped_ref.get(&(t, kv)).unwrap_or(&0))
        });
        prop_assert!(
            bumped_result.is_err(),
            "bumping any recorded key's current_version above version_seen \
             must trigger an SSI conflict"
        );
    }

    /// Property 3 (NONE-PROVIDER):
    /// If the provider returns None for ANY recorded key while every other
    /// key passes, validation must conflict.
    #[test]
    fn prop_validate_read_set_none_provider_is_conflict(
        reads in proptest::collection::vec(
            (
                0u64..4,
                proptest::collection::vec(any::<u8>(), 1..=4),
                0u64..=20,
            ),
            1..=8,
        ),
        pick in 0usize..1024,
    ) {
        let (tx, recorded) = build_tx_with_reads(&reads);
        prop_assume!(!recorded.is_empty());

        let keys: Vec<(u64, Vec<u8>)> = recorded.keys().cloned().collect();
        let nilled = keys[pick % keys.len()].clone();
        let recorded_ref = &recorded;
        let nilled_ref = &nilled;
        let result = tx.validate_read_set(|t, k| {
            let kv: Vec<u8> = k.as_ref().to_vec();
            if (t, kv.clone()) == (nilled_ref.0, nilled_ref.1.clone()) {
                None
            } else {
                Some(*recorded_ref.get(&(t, kv)).unwrap_or(&0))
            }
        });
        prop_assert!(
            result.is_err(),
            "a None provider response for a recorded key must conflict"
        );
    }
}
