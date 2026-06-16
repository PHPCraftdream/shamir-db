//! Byte-identity gate test for the W2a-sorted migration
//! (`extract_and_encode` / `build_covering_projection` → `RecordRef`).
//!
//! CRITICAL: sorted-index keys are PERSISTED — recovery reads them. The
//! migration from `&InnerValue` to `&(impl RecordRef + ?Sized)` MUST produce
//! byte-identical keys and projection envelopes for every scalar type, and
//! MUST keep returning `None` for Dec/Big/containers (so they stay un-indexed).
//!
//! This module asserts that by:
//!   1. Building an `InnerValue` record with one field of each kind:
//!      Null, Bool, Int, F64, Str, Bin (the six indexable scalars), plus a
//!      `Dec` and a `List` container (the non-indexable kinds).
//!   2. For every indexable scalar field, calling `plan_record_created` and
//!      asserting the produced index-entry key equals the key built directly
//!      from `sort_codec::encode_*` (the byte-identity reference).
//!   3. For the Dec and List fields, asserting NO index entry is produced
//!      (i.e. `scalar_at` returns `None` exactly like the old match's `_ =>` arm).
//!   4. For a covering index, asserting the projection envelope decodes to the
//!      same `(String, InnerValue)` pairs that the old `resolve_path_ref` +
//!      `.clone()` path produced.

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::core::sort_codec;
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use std::str::FromStr;
use std::sync::Arc;

use crate::legacy::sorted_index_definition::SORTED_TAG;
use crate::legacy::sorted_index_manager::{
    decode_covering_projection, SortedIndexDefinition, SortedIndexManager,
};
use crate::write_ops::IndexWriteOp;

use super::helpers::fresh_mgr;

// Interned field ids — arbitrary, picked to be distinct from each other.
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
        InnerValue::Str("héllo\0wörld".to_string()), // includes a 0x00 to exercise the str escape
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

/// Build the expected index-entry physical key for one field, mirroring
/// `SortedIndexManager::build_entry_key` (kept here to compute the byte-identity
/// reference independently of the manager).
fn expected_key(name_interned: u64, encoded: &[u8], rid: &RecordId) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 8 + encoded.len() + 16);
    buf.push(SORTED_TAG);
    buf.extend_from_slice(&name_interned.to_be_bytes());
    buf.extend_from_slice(encoded);
    buf.extend_from_slice(&rid.to_bytes());
    buf
}

/// One indexable scalar field, its interned id, and the expected sort_codec
/// bytes (the byte-identity reference).
struct Case {
    field: u64,
    expected_enc: Vec<u8>,
    label: &'static str,
}

fn indexable_cases() -> Vec<Case> {
    let mut b = Vec::new();
    sort_codec::encode_null(&mut b);
    let null_enc = std::mem::take(&mut b);

    sort_codec::encode_bool(&mut b, true);
    let bool_enc = std::mem::take(&mut b);

    sort_codec::encode_i64(&mut b, -42);
    let int_enc = std::mem::take(&mut b);

    sort_codec::encode_f64(&mut b, 1.5_f64).unwrap();
    let f64_enc = std::mem::take(&mut b);

    sort_codec::encode_str(&mut b, "héllo\0wörld");
    let str_enc = std::mem::take(&mut b);

    sort_codec::encode_bytes(&mut b, &[0u8, 1, 2, 0, 255]);
    let bin_enc = std::mem::take(&mut b);

    vec![
        Case {
            field: F_NULL,
            expected_enc: null_enc,
            label: "Null",
        },
        Case {
            field: F_BOOL,
            expected_enc: bool_enc,
            label: "Bool",
        },
        Case {
            field: F_INT,
            expected_enc: int_enc,
            label: "Int",
        },
        Case {
            field: F_F64,
            expected_enc: f64_enc,
            label: "F64",
        },
        Case {
            field: F_STR,
            expected_enc: str_enc,
            label: "Str",
        },
        Case {
            field: F_BIN,
            expected_enc: bin_enc,
            label: "Bin",
        },
    ]
}

/// Each scalar arm produces a byte-identical index-entry key.
#[tokio::test]
async fn byte_identity_every_scalar_type() {
    for case in indexable_cases() {
        let (_, mgr) = fresh_mgr().await;
        // One index keyed on this single field.
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
                let expected = expected_key(case.field, &case.expected_enc, &rid);
                assert_eq!(
                    key.as_ref(),
                    &expected[..],
                    "{}: index-entry key is NOT byte-identical to the sort_codec reference",
                    case.label
                );
                // Non-covering index → empty physical_value (unchanged behaviour).
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

/// Dec and the Set container MUST produce no index entry — the same
/// `_ => return Ok(None)` skip as the old `&InnerValue` match arm.
#[tokio::test]
async fn byte_identity_dec_and_container_are_skipped() {
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

        // has_indexable_value MUST also be false for these kinds.
        assert!(
            !SortedIndexManager::has_indexable_value(&rec, &[field]),
            "{label}: has_indexable_value must be false for non-indexable kinds"
        );
    }
}

/// `has_indexable_value` is true for every indexable scalar and false for Dec/Set.
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

/// Covering-projection envelope: the materialized leaf for each scalar type
/// is byte-identical to what the old `resolve_path_ref` + `.clone()` path
/// produced (the same `InnerValue` leaf). Decode and compare per-field.
#[tokio::test]
async fn byte_identity_covering_projection() {
    let (_, mgr) = fresh_mgr().await;
    // Covering index keyed on F_INT, including F_STR and F_BIN.
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
        IndexWriteOp::SetPosting { key, value } => {
            // Index key still byte-identical.
            let mut enc = Vec::new();
            sort_codec::encode_i64(&mut enc, -42);
            assert_eq!(key.as_ref(), &expected_key(501, &enc, &rid)[..]);

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
                &InnerValue::Str("héllo\0wörld".to_string()),
                "str projection leaf must match the materialized InnerValue"
            );
            assert_eq!(
                got.remove("bin").unwrap(),
                &InnerValue::Bin(vec![0u8, 1, 2, 0, 255]),
                "bin projection leaf must match the materialized InnerValue"
            );
        }
        other => panic!("expected SetPosting, got {other:?}"),
    }
}

/// Range query result is unchanged: seeded records surface in the right
/// range bounds (proves the byte-identical key sorts the same way).
#[tokio::test]
async fn byte_identity_range_query_unchanged() {
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
    // Range [-1 ..= 50] → 4 records.
    let mut lo = Vec::new();
    sort_codec::encode_i64(&mut lo, -1);
    let mut hi = Vec::new();
    sort_codec::encode_i64(&mut hi, 50);
    let result = mgr.lookup_range(F_INT, Some(&lo), Some(&hi)).await.unwrap();
    assert_eq!(result.len(), 4);
    for (s, rid) in &id_by_score {
        let expected = (-1..=50).contains(s);
        assert_eq!(result.contains(rid), expected, "score {s}");
    }
}

/// Update path is also byte-identical: the old slot is removed and the new
/// slot is keyed on the new encoded value.
#[tokio::test]
async fn byte_identity_update_moves_entry() {
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
    let new = InnerValue::Map(new_m);

    mgr.on_record_created(&rid, &old, 1).await.unwrap();
    mgr.on_record_updated(&rid, &old, &new, 2).await.unwrap();

    let mut lo5 = Vec::new();
    sort_codec::encode_i64(&mut lo5, 5);
    let mut hi5 = Vec::new();
    sort_codec::encode_i64(&mut hi5, 5);
    let r5 = mgr
        .lookup_range(F_INT, Some(&lo5), Some(&hi5))
        .await
        .unwrap();
    assert!(r5.is_empty(), "old slot must be cleared");

    let mut lo25 = Vec::new();
    sort_codec::encode_i64(&mut lo25, 25);
    let mut hi25 = Vec::new();
    sort_codec::encode_i64(&mut hi25, 25);
    let r25 = mgr
        .lookup_range(F_INT, Some(&lo25), Some(&hi25))
        .await
        .unwrap();
    assert!(r25.contains(&rid), "new slot must contain the record");
}

/// Compile-time: the planner methods accept `&InnerValue` (coercion through
/// the `RecordRef` impl). If this compiles, existing call-sites keep working.
#[tokio::test]
async fn planner_accepts_inner_value_ref() {
    let (_, mgr) = fresh_mgr().await;
    mgr.register(SortedIndexDefinition::new(F_INT, vec![F_INT]))
        .await
        .unwrap();
    let rid = RecordId::new();
    let rec = record_with_every_type();
    // on_record_created takes &(impl RecordRef + ?Sized); &InnerValue coerces.
    mgr.on_record_created(&rid, &rec, 1).await.unwrap();
    // plan_record_updated with two &InnerValue args.
    let _ = mgr.plan_record_updated(&rid, &rec, &rec, 1).unwrap();
    // plan_record_deleted.
    let _ = mgr.plan_record_deleted(&rid, &rec).unwrap();
}

/// Compile-time + runtime: `on_records_created_batch` accepts an iterator of
/// `(&RecordId, &InnerValue)`.
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
