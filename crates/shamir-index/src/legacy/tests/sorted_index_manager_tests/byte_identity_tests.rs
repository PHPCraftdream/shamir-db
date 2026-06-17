//! S9 consistency tests for the sorted-index write path.
//!
//! Replaces the old byte-identity tests that pinned the V1 format.
//! V1 data is disposable (index-format version bump + rebuild-on-open).
//!
//! Assertions:
//!   1. Every indexable scalar type produces a non-empty posting.
//!   2. Dec and containers are skipped (scalar_at returns None).
//!   3. Range-query round-trip with the new encoding.
//!   4. Update moves the entry correctly.
//!   5. Covering projection encodes as `Vec<(String, QueryValue)>` and
//!      decodes as `Vec<(String, InnerValue)>` (wire-compat for scalars).
//!   6. has_indexable_value agrees with scalar_at.
//!   7. Compile-time coercion: planner methods accept `&InnerValue`.

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::core::sort_codec;
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use std::str::FromStr;
use std::sync::Arc;

use crate::legacy::sorted_index_manager::{
    decode_covering_projection, SortedIndexDefinition, SortedIndexManager,
};
use crate::write_ops::IndexWriteOp;

use super::helpers::fresh_mgr;

// Interned field ids.
const F_NULL: u64 = 301;
const F_BOOL: u64 = 302;
const F_INT: u64 = 303;
const F_F64: u64 = 304;
const F_STR: u64 = 305;
const F_BIN: u64 = 306;
const F_DEC: u64 = 307;
const F_LIST: u64 = 308;

/// Build a record carrying every type this test exercises.
fn record_with_every_type() -> InnerValue {
    let mut m = new_map();
    m.insert(InternerKey::new(F_NULL), InnerValue::Null);
    m.insert(InternerKey::new(F_BOOL), InnerValue::Bool(true));
    m.insert(InternerKey::new(F_INT), InnerValue::Int(-42));
    m.insert(InternerKey::new(F_F64), InnerValue::F64(1.5_f64));
    m.insert(
        InternerKey::new(F_STR),
        InnerValue::Str("hello world".to_string()),
    );
    m.insert(
        InternerKey::new(F_BIN),
        InnerValue::Bin(vec![0u8, 1, 2, 0, 255]),
    );
    m.insert(
        InternerKey::new(F_DEC),
        InnerValue::Dec(rust_decimal::Decimal::from_str("123.456").unwrap()),
    );
    let mut set = new_set();
    set.insert(InnerValue::Int(1));
    set.insert(InnerValue::Int(2));
    m.insert(InternerKey::new(F_LIST), InnerValue::Set(set));
    InnerValue::Map(m)
}

struct Case {
    field: u64,
    label: &'static str,
}

fn indexable_cases() -> Vec<Case> {
    vec![
        Case {
            field: F_NULL,
            label: "Null",
        },
        Case {
            field: F_BOOL,
            label: "Bool",
        },
        Case {
            field: F_INT,
            label: "Int",
        },
        Case {
            field: F_F64,
            label: "F64",
        },
        Case {
            field: F_STR,
            label: "Str",
        },
        Case {
            field: F_BIN,
            label: "Bin",
        },
    ]
}

/// 1. Each scalar arm produces exactly one SetPosting.
#[tokio::test]
async fn every_scalar_type_produces_posting() {
    for case in indexable_cases() {
        let (_, mgr) = fresh_mgr().await;
        mgr.register(SortedIndexDefinition::new(case.field, vec![case.field]))
            .await
            .unwrap();

        let rid = RecordId::new();
        let rec = record_with_every_type();
        let ops = mgr.plan_record_created(&rid, &rec, 1).unwrap();

        assert_eq!(
            ops.len(),
            1,
            "{}: expected exactly one SetPosting",
            case.label
        );
        match &ops[0] {
            IndexWriteOp::SetPosting { key, value } => {
                assert!(
                    !key.is_empty(),
                    "{}: posting key must not be empty",
                    case.label
                );
                assert!(
                    value.is_empty(),
                    "{}: non-covering index must keep physical_value empty",
                    case.label
                );
            }
            other => panic!("{}: expected SetPosting, got {other:?}", case.label),
        }
    }
}

/// 2. Dec and the Set container produce no index entry.
#[tokio::test]
async fn dec_and_container_are_skipped() {
    for (field, label) in [(F_DEC, "Dec"), (F_LIST, "Set")] {
        let (_, mgr) = fresh_mgr().await;
        mgr.register(SortedIndexDefinition::new(field, vec![field]))
            .await
            .unwrap();

        let rid = RecordId::new();
        let rec = record_with_every_type();
        let ops = mgr.plan_record_created(&rid, &rec, 1).unwrap();

        assert!(
            ops.is_empty(),
            "{label}: Dec/containers MUST be skipped (scalar_at returns None), got {ops:?}"
        );

        assert!(
            !SortedIndexManager::has_indexable_value(&rec, &[field]),
            "{label}: has_indexable_value must be false for non-indexable kinds"
        );
    }
}

/// 3. Range query round-trip with the new encoding.
#[tokio::test]
async fn range_query_round_trip() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(F_INT, vec![F_INT]))
        .await
        .unwrap();
    let scores = [-100, -1, 0, 1, 50, 99];
    let mut id_by_score: Vec<(i64, RecordId)> = Vec::new();
    for s in scores {
        let rid = RecordId::new();
        let mut m = new_map();
        m.insert(InternerKey::new(F_INT), InnerValue::Int(s));
        let rec = InnerValue::Map(m);
        mgr.on_record_created(&rid, &rec, 1).await.unwrap();
        id_by_score.push((s, rid));
    }
    // Range [-1 ..= 50] -> 4 records.
    let mut lo = Vec::new();
    sort_codec::encode_i64(&mut lo, -1);
    let mut hi = Vec::new();
    sort_codec::encode_i64(&mut hi, 50);
    let result = mgr
        .lookup_range(F_INT, Some(&lo), Some(&hi))
        .await
        .unwrap();
    assert_eq!(result.len(), 4);
    for (s, rid) in &id_by_score {
        let expected = (-1..=50).contains(s);
        assert_eq!(result.contains(rid), expected, "score {s}");
    }
}

/// 4. Update moves the entry.
#[tokio::test]
async fn update_moves_entry() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(F_INT, vec![F_INT]))
        .await
        .unwrap();
    let rid = RecordId::new();

    let mut old_m = new_map();
    old_m.insert(InternerKey::new(F_INT), InnerValue::Int(5));
    let old = InnerValue::Map(old_m);

    let mut new_m = new_map();
    new_m.insert(InternerKey::new(F_INT), InnerValue::Int(25));
    let new_val = InnerValue::Map(new_m);

    mgr.on_record_created(&rid, &old, 1).await.unwrap();
    mgr.on_record_updated(&rid, &old, &new_val, 2)
        .await
        .unwrap();

    let mut lo5 = Vec::new();
    sort_codec::encode_i64(&mut lo5, 5);
    let r5 = mgr
        .lookup_range(F_INT, Some(&lo5), Some(&lo5))
        .await
        .unwrap();
    assert!(r5.is_empty(), "old slot must be cleared");

    let mut lo25 = Vec::new();
    sort_codec::encode_i64(&mut lo25, 25);
    let r25 = mgr
        .lookup_range(F_INT, Some(&lo25), Some(&lo25))
        .await
        .unwrap();
    assert!(r25.contains(&rid), "new slot must contain the record");
}

/// 5. Covering projection encodes as QueryValue, decodes as InnerValue.
#[tokio::test]
async fn covering_projection_roundtrip() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::with_included_interned(
        501,
        vec![F_INT],
        vec![vec!["str".to_string()], vec!["bin".to_string()]],
        vec![vec![F_STR], vec![F_BIN]],
    ))
    .await
    .unwrap();

    let rid = RecordId::new();
    let rec = record_with_every_type();
    let ops = mgr.plan_record_created(&rid, &rec, 7).unwrap();
    assert_eq!(ops.len(), 1, "one SetPosting for the covering index");
    match &ops[0] {
        IndexWriteOp::SetPosting { key: _, value } => {
            assert!(
                !value.is_empty(),
                "covering index must produce a projection"
            );
            let (ver, proj) = decode_covering_projection(value.as_ref()).expect("decode envelope");
            assert_eq!(ver, 7);
            assert_eq!(proj.len(), 2, "two included fields");

            let mut got: std::collections::BTreeMap<&str, &InnerValue> =
                proj.iter().map(|(k, v)| (k.as_str(), v)).collect();
            assert_eq!(
                got.remove("str").unwrap(),
                &InnerValue::Str("hello world".to_string()),
                "str projection leaf must match"
            );
            assert_eq!(
                got.remove("bin").unwrap(),
                &InnerValue::Bin(vec![0u8, 1, 2, 0, 255]),
                "bin projection leaf must match"
            );
        }
        other => panic!("expected SetPosting, got {other:?}"),
    }
}

/// 6. has_indexable_value agrees with scalar_at.
#[tokio::test]
async fn has_indexable_value_matches_scalar_set() {
    let rec = record_with_every_type();
    for case in indexable_cases() {
        assert!(
            SortedIndexManager::has_indexable_value(&rec, &[case.field]),
            "{}: has_indexable_value must be true for indexable scalars",
            case.label
        );
    }
    for field in [F_DEC, F_LIST] {
        assert!(
            !SortedIndexManager::has_indexable_value(&rec, &[field]),
            "field {field}: has_indexable_value must be false for non-indexable kinds"
        );
    }
}

/// 7a. Planner methods accept `&InnerValue`.
#[tokio::test]
async fn planner_accepts_inner_value_ref() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(F_INT, vec![F_INT]))
        .await
        .unwrap();
    let rid = RecordId::new();
    let rec = record_with_every_type();
    mgr.on_record_created(&rid, &rec, 1).await.unwrap();
    let _ = mgr.plan_record_updated(&rid, &rec, &rec, 1).unwrap();
    let _ = mgr.plan_record_deleted(&rid, &rec).unwrap();
}

/// 7b. Batch accepts `(&RecordId, &InnerValue)`.
#[tokio::test]
async fn batch_accepts_inner_value_refs() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    mgr.register(SortedIndexDefinition::new(F_INT, vec![F_INT]))
        .await
        .unwrap();
    let rid = RecordId::new();
    let rec = record_with_every_type();
    let pairs: Vec<(&RecordId, &InnerValue)> = vec![(&rid, &rec)];
    mgr.on_records_created_batch(pairs, 1).await.unwrap();
}
