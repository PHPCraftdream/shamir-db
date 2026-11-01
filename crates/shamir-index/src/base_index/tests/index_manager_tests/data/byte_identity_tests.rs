//! S9 consistency tests for the lens-native leaf hash scheme.
//!
//! Replaces the old byte-identity tests that pinned the V1 format
//! (`<Value<InternerKey> as Hash>` with `std::mem::discriminant` tags).
//! V1 data is disposable (index-format version bump + rebuild-on-open).
//!
//! Assertions:
//!   (a) Round-trip: index a record, look it up by the indexed value,
//!       get the same RecordId back. Every indexable leaf type.
//!   (b) Equal-value determinism: two records with equal indexed values
//!       produce equal posting keys.
//!   (c) Inequality: two records with different indexed values produce
//!       different posting keys (no collision for the test corpus).
//!   (d) Set/Map order-independence: same multiset → same key.
//!   (e) Composite index round-trip.
//!   (f) Unique-flag bit flips the key.
//!   (g) Compile-time coercion: planner methods accept `&InnerValue`.

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use std::str::FromStr;
use std::sync::Arc;

use num_bigint::BigInt;

use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_keys::{build_index_key, build_index_key_from_record};
use crate::base_index::index_manager::IndexManager;

use crate::base_index::tests::index_manager_tests::helpers::create_manager;

// Identity v2 golden (rewritten from the v3 rustc-hash pins by
// rv_index_605eec5d; design P34-12 (a) freezes the canonical codec, so any
// byte below is an identity-version bump and MUST fail CI). Key layout:
// `[ns][name_interned LE][C]` with ns 0x02 regular / 0x03 unique; C is the
// canonical encoding computed by hand from the frozen codec (leaf tags in
// `model::identity::canonical`). name_interned 5000 LE = 0x88 0x13.
#[test]
fn rv_index_605eec5d_golden_regular_unique_keys() {
    // (label, lookup values, canonical bytes C expected for that value)
    type GoldenCase = (&'static str, Vec<InnerValue>, &'static [u8]);
    let cases: Vec<GoldenCase> = vec![
        // Int(-42): tag 0x02 + i64 BE (-42).
        (
            "Int",
            vec![InnerValue::Int(-42)],
            &[0x02, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xD6],
        ),
        // Str("hello"): tag 0x06 + u32 BE len + bytes.
        (
            "Str",
            vec![InnerValue::Str("hello".to_string())],
            &[0x06, 0x00, 0x00, 0x00, 0x05, b'h', b'e', b'l', b'l', b'o'],
        ),
        // F64(42.0): integral f64 lands in the INTEGER class — the exact
        // bytes of Int(42) (the ONE NumericKey rule).
        (
            "F64 integral",
            vec![InnerValue::F64(42.0)],
            &[0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2A],
        ),
        // Dec(123.456): non-integral exact decimal — tag 0x05 + i128 BE
        // mantissa (123456) + u32 BE scale (3).
        (
            "Dec",
            vec![InnerValue::Dec(
                rust_decimal::Decimal::from_str("123.456").unwrap(),
            )],
            &[
                0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xE2, 0x40, 0x00, 0x00, 0x00, 0x03,
            ],
        ),
        // Big(2^63): canonical BigBits class (i64 overflow), tag 0x03, non-negative sign, u32 length 8, magnitude BE.
        (
            "Big",
            vec![InnerValue::Big(BigInt::from(1_u64) << 63_usize)],
            &[
                0x03, 0x00, 0x00, 0x00, 0x00, 0x08, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ],
        ),
        // composite (Int 7, Int -11): two integer leaves concatenated.
        (
            "composite Int,Int",
            vec![InnerValue::Int(7), InnerValue::Int(-11)],
            &[
                0x02, 0, 0, 0, 0, 0, 0, 0, 7, 0x02, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF5,
            ],
        ),
    ];

    // Intentionally pinned IDV2 literals; the Big(2^63) row is BigBits (i64 overflow).
    let _frozen_namespace_bytes = [0x02u8, 0x03];
    for (label, values, canonical) in cases {
        let mut fields = new_map();
        let mut paths = Vec::new();
        for (id, value) in [401_u64, 402].into_iter().zip(&values) {
            fields.insert(InternerKey::new(id), value.clone());
            paths.push(IndexInfoItem::new(vec![id]));
        }
        let record = InnerValue::Map(fields);
        for (is_unique, ns) in [(false, 0x02u8), (true, 0x03u8)] {
            // Golden literals above remain independent of the codec constants.
            assert_eq!(ns, if is_unique { 0x03 } else { 0x02 });
            let mut expected = Vec::with_capacity(9 + canonical.len());
            expected.push(ns);
            expected.extend_from_slice(&5000u64.to_le_bytes());
            if canonical.len() <= crate::model::identity::identity_bytes::INLINE_MAX {
                expected.extend_from_slice(canonical);
            } else {
                expected.push(0xFF);
                expected.extend_from_slice(
                    &crate::model::identity::identity_bytes::digest_identity(canonical),
                );
            }

            let (written, written_canonical) =
                build_index_key_from_record(is_unique, 5000, &record, &paths)
                    .expect("indexed fields exist");
            assert_eq!(
                written_canonical, canonical,
                "{label} canonical {is_unique}"
            );
            assert_eq!(
                written.to_bytes().as_ref(),
                expected.as_slice(),
                "{label} write {is_unique}"
            );
            let lookup = build_index_key(is_unique, 5000, &values).expect("probe has identity");
            assert_eq!(
                lookup.to_bytes().as_ref(),
                expected.as_slice(),
                "{label} lookup {is_unique}"
            );
        }
    }
}

// Interned field ids — arbitrary, distinct from each other.
const F_NULL: u64 = 401;
const F_BOOL: u64 = 402;
const F_INT: u64 = 403;
const F_F64: u64 = 404;
const F_STR: u64 = 405;
const F_BIN: u64 = 406;
const F_DEC: u64 = 407;
const F_BIG: u64 = 408;
const F_MAP: u64 = 409;
const F_LIST: u64 = 410;

/// Build a record carrying every type this test exercises, including the
/// non-scalar kinds (Dec, Big, Map, List) that `scalar_at` would drop but
/// `materialize_at` preserves.
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
    m.insert(
        InternerKey::new(F_BIG),
        InnerValue::Big(BigInt::from(999999999999_i64)),
    );
    // Map-valued field.
    let mut inner_map = new_map();
    inner_map.insert(InternerKey::new(1), InnerValue::Int(7));
    m.insert(InternerKey::new(F_MAP), InnerValue::Map(inner_map));
    // List-valued field.
    m.insert(
        InternerKey::new(F_LIST),
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    // Set.
    let mut set_val = new_set();
    set_val.insert(InnerValue::Int(3));
    m.insert(InternerKey::new(411), InnerValue::Set(set_val));
    InnerValue::Map(m)
}

/// One single-field index case: field id + label.
struct Case {
    field: u64,
    label: &'static str,
}

fn single_field_cases() -> Vec<Case> {
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
        Case {
            field: F_DEC,
            label: "Dec",
        },
        Case {
            field: F_BIG,
            label: "Big",
        },
        Case {
            field: F_MAP,
            label: "Map",
        },
        Case {
            field: F_LIST,
            label: "List",
        },
    ]
}

/// (a) Round-trip: index a record, look it up, get the record back.
#[tokio::test]
async fn round_trip_single_field_every_type() {
    for case in single_field_cases() {
        let (_, _, manager) = create_manager();
        let name_interned = 6000 + case.field;
        let def = crate::base_index::index_definition::IndexDefinition::new(
            name_interned,
            vec![IndexInfoItem::new(vec![case.field])],
        );
        manager.create_index(def).await.unwrap();

        let rid = RecordId::new();
        let rec = record_with_every_type();
        manager.on_record_created(&rid, &rec).await.unwrap();

        // Look up by the literal InnerValue leaf (old tree-descent reference).
        let mut cur = &rec;
        cur = match cur {
            InnerValue::Map(map) => &map[&InternerKey::new(case.field)],
            _ => unreachable!(),
        };
        let lookup_values = vec![cur.clone()];

        let result = manager
            .lookup_by_index(name_interned, &lookup_values)
            .await
            .unwrap()
            .unwrap();
        // Audit 3.2: `lookup_by_index` now returns `Arc<[RecordId]>` — a
        // sorted, duplicate-free slice; compare against the expected slice.
        let expected: Vec<RecordId> = vec![rid];
        assert_eq!(
            result.as_ref(),
            expected.as_slice(),
            "{}: round-trip lookup did not return the record",
            case.label
        );
    }
}

/// (b) Determinism: same record indexed twice produces identical keys.
#[test]
fn deterministic_same_record_same_key() {
    let rec = record_with_every_type();
    for case in single_field_cases() {
        let paths = vec![IndexInfoItem::new(vec![case.field])];
        let key1 = build_index_key_from_record(false, 5000, &rec, &paths)
            .unwrap_or_else(|| panic!("{}: must produce a key", case.label))
            .0;
        let key2 = build_index_key_from_record(false, 5000, &rec, &paths)
            .unwrap_or_else(|| panic!("{}: must produce a key", case.label))
            .0;
        assert_eq!(
            key1.to_bytes(),
            key2.to_bytes(),
            "{}: keys must be identical for same input",
            case.label
        );
    }
}

/// (b-cont) Write-path key equals lookup-path key for the same value.
#[test]
fn write_path_matches_lookup_path() {
    let rec = record_with_every_type();
    for case in single_field_cases() {
        let paths = vec![IndexInfoItem::new(vec![case.field])];
        // Write path (via RecordRef).
        let write_key = build_index_key_from_record(false, 5000, &rec, &paths)
            .unwrap_or_else(|| panic!("{}: must produce a key", case.label))
            .0;
        // Lookup path (via InnerValue).
        let mut cur = &rec;
        cur = match cur {
            InnerValue::Map(map) => &map[&InternerKey::new(case.field)],
            _ => unreachable!(),
        };
        let lookup_key = build_index_key(false, 5000, &[cur.clone()]).expect("probe has identity");
        assert_eq!(
            write_key.to_bytes(),
            lookup_key.to_bytes(),
            "{}: write-path and lookup-path keys must match",
            case.label
        );
    }
}

/// (c) Inequality: different values produce different keys.
#[test]
fn different_values_different_keys() {
    let mut m1 = new_map();
    m1.insert(InternerKey::new(F_INT), InnerValue::Int(42));
    let rec1 = InnerValue::Map(m1);

    let mut m2 = new_map();
    m2.insert(InternerKey::new(F_INT), InnerValue::Int(43));
    let rec2 = InnerValue::Map(m2);

    let paths = vec![IndexInfoItem::new(vec![F_INT])];
    let key1 = build_index_key_from_record(false, 5000, &rec1, &paths)
        .unwrap()
        .0;
    let key2 = build_index_key_from_record(false, 5000, &rec2, &paths)
        .unwrap()
        .0;
    assert_ne!(
        key1.to_bytes(),
        key2.to_bytes(),
        "different Int values must produce different keys"
    );
}

/// (d) Set order-independence: same elements in different insertion order
/// produce the same key.
#[test]
fn set_order_independent() {
    let mut s1 = new_set();
    s1.insert(InnerValue::Int(1));
    s1.insert(InnerValue::Int(2));
    s1.insert(InnerValue::Int(3));

    let mut s2 = new_set();
    s2.insert(InnerValue::Int(3));
    s2.insert(InnerValue::Int(1));
    s2.insert(InnerValue::Int(2));

    let mut m1 = new_map();
    m1.insert(InternerKey::new(100), InnerValue::Set(s1));
    let rec1 = InnerValue::Map(m1);

    let mut m2 = new_map();
    m2.insert(InternerKey::new(100), InnerValue::Set(s2));
    let rec2 = InnerValue::Map(m2);

    let paths = vec![IndexInfoItem::new(vec![100])];
    let key1 = build_index_key_from_record(false, 5000, &rec1, &paths)
        .unwrap()
        .0;
    let key2 = build_index_key_from_record(false, 5000, &rec2, &paths)
        .unwrap()
        .0;
    assert_eq!(
        key1.to_bytes(),
        key2.to_bytes(),
        "Set with same elements must produce the same key regardless of insertion order"
    );
}

/// (d-cont) Map order-independence.
#[test]
fn map_order_independent() {
    let mut inner1 = new_map();
    inner1.insert(InternerKey::new(1), InnerValue::Int(10));
    inner1.insert(InternerKey::new(2), InnerValue::Int(20));

    let mut inner2 = new_map();
    inner2.insert(InternerKey::new(2), InnerValue::Int(20));
    inner2.insert(InternerKey::new(1), InnerValue::Int(10));

    let mut m1 = new_map();
    m1.insert(InternerKey::new(100), InnerValue::Map(inner1));
    let rec1 = InnerValue::Map(m1);

    let mut m2 = new_map();
    m2.insert(InternerKey::new(100), InnerValue::Map(inner2));
    let rec2 = InnerValue::Map(m2);

    let paths = vec![IndexInfoItem::new(vec![100])];
    let key1 = build_index_key_from_record(false, 5000, &rec1, &paths)
        .unwrap()
        .0;
    let key2 = build_index_key_from_record(false, 5000, &rec2, &paths)
        .unwrap()
        .0;
    assert_eq!(
        key1.to_bytes(),
        key2.to_bytes(),
        "Map with same entries must produce the same key regardless of insertion order"
    );
}

/// (e) Composite index round-trip.
#[tokio::test]
async fn round_trip_composite_index() {
    let (_, _, manager) = create_manager();
    let name_interned = 7000;
    let paths = vec![
        IndexInfoItem::new(vec![F_INT]),
        IndexInfoItem::new(vec![F_STR]),
        IndexInfoItem::new(vec![F_DEC]),
    ];
    let def =
        crate::base_index::index_definition::IndexDefinition::new(name_interned, paths.clone());
    manager.create_index(def).await.unwrap();

    let rid = RecordId::new();
    let rec = record_with_every_type();
    manager.on_record_created(&rid, &rec).await.unwrap();

    // Literal lookup values (old tree-descent reference).
    let lookup_values: Vec<InnerValue> = paths
        .iter()
        .map(|item| {
            let mut cur = &rec;
            for &id in &item.path {
                cur = match cur {
                    InnerValue::Map(map) => &map[&InternerKey::new(id)],
                    _ => unreachable!(),
                };
            }
            cur.clone()
        })
        .collect();

    let result = manager
        .lookup_by_index(name_interned, &lookup_values)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.contains(&rid),
        "composite: round-trip lookup did not return the record"
    );
}

/// (f) Unique-flag bit flips the key.
#[test]
fn unique_flag_flips_key() {
    let rec = record_with_every_type();
    let paths = vec![IndexInfoItem::new(vec![F_DEC])];
    let unique_key = build_index_key_from_record(true, 5002, &rec, &paths)
        .unwrap()
        .0;
    let nonunique_key = build_index_key_from_record(false, 5002, &rec, &paths)
        .unwrap()
        .0;
    assert_ne!(
        unique_key.to_bytes(),
        nonunique_key.to_bytes(),
        "unique flag must change the key"
    );
}

/// (g) Compile-time + runtime: planner/extract methods accept `&InnerValue`.
#[tokio::test]
async fn planner_accepts_inner_value_ref() {
    let (_, _, manager) = create_manager();
    let rec = record_with_every_type();
    // unique_keys_for takes &(impl RecordRef).
    let _ = manager.unique_keys_for(&rec);
}

/// (g-cont) `on_records_created_batch` accepts `(&RecordId, &InnerValue)`.
#[tokio::test]
async fn batch_accepts_inner_value_refs() {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    let rid = RecordId::new();
    let rec = record_with_every_type();
    let pairs: Vec<(&RecordId, &InnerValue)> = vec![(&rid, &rec)];
    manager.on_records_created_batch(pairs).await.unwrap();
}
