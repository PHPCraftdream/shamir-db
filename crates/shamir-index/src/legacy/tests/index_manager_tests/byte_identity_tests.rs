//! Byte-identity gate test for the W2a hash/unique migration
//! (`extract_index_values*` / `build_index_key_from_refs` →
//! `extract_index_leaves` + `build_index_key` via `RecordRef::materialize_at`).
//!
//! CRITICAL: legacy + unique index keys are PERSISTED — recovery reads them.
//! The migration from `&InnerValue` to `&(impl RecordRef + ?Sized)` MUST
//! produce byte-identical keys for every leaf type, INCLUDING Dec/Big/Map/List
//! (which `materialize_at` preserves — `scalar_at` would have dropped them,
//! silently corrupting the index).
//!
//! The legacy key is `FxHash(<InnerValue as Hash>::hash(leaf))` with a leading
//! `std::mem::discriminant(Value<InternerKey>)`. `ScalarRef` is a DIFFERENT enum
//! → hashing it directly DIVERGES. This module asserts the new path
//! (materialize_at → unchanged `with_values`) keeps the bytes identical by
//! constructing the OLD reference key independently and comparing byte-for-byte.
//!
//! Assertions:
//!   (a) For a record with a scalar of every type + Dec + Big + a Map-valued
//!       field + a List-valued field + a composite (multi-field) index, the NEW
//!       path's `IndexRecordKey::to_bytes()` == the OLD path's bytes,
//!       BYTE-FOR-BYTE.
//!   (b) An index built + queried via the NEW path returns the IDENTICAL
//!       `BTreeSet<RecordId>` as the OLD path (round-trip query equivalence),
//!       INCLUDING the Dec/Big/Map/List fields.

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::{new_map, new_set};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use std::str::FromStr;
use std::sync::Arc;

use num_bigint::BigInt;

use crate::legacy::index_info_item::IndexInfoItem;
use crate::legacy::index_keys::{build_index_key, extract_index_leaves};
use crate::legacy::index_manager::IndexManager;
use crate::legacy::index_record_key::IndexRecordKey;

use super::helpers::create_manager;

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
        InnerValue::Str("héllo\0wörld".to_string()),
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
    // (Set is also exercised — same wire shape as List, but distinct variant.)
    let mut set_val = new_set();
    set_val.insert(InnerValue::Int(3));
    m.insert(InternerKey::new(411), InnerValue::Set(set_val));
    InnerValue::Map(m)
}

/// Reconstruct the OLD key-encoding path INDEPENDENTLY of the manager:
/// descend the `InnerValue::Map` tree by `InternerKey` (exactly what the old
/// `extract_value_by_path_ref` did) and feed the cloned leaf through the
/// UNCHANGED `IndexRecordKey::with_values`. This is the byte-identity reference.
fn old_path_key(is_unique: bool, name_interned: u64, rec: &InnerValue, path: &[u64]) -> Vec<u8> {
    let mut cur = rec;
    for &id in path {
        match cur {
            InnerValue::Map(map) => {
                cur = map.get(&InternerKey::new(id)).unwrap_or_else(|| {
                    panic!("old_path_key: path segment {id} missing — test record is misconfigured")
                });
            }
            _ => panic!("old_path_key: descended through a non-Map — misconfigured path"),
        }
    }
    let leaf_refs: Vec<&InnerValue> = vec![cur];
    IndexRecordKey::new(is_unique, name_interned)
        .with_values(&leaf_refs)
        .to_bytes()
        .to_vec()
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
        // The crux types: scalar_at returns None for these → the OLD path
        // indexed them (tree descent), and the NEW path MUST too (via
        // materialize_at). If the new path used scalar_at these would vanish.
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

/// (a) For every single-field type, the NEW `extract_index_leaves` +
/// `build_index_key` produces byte-for-byte the same key as the OLD
/// tree-descent + `with_values` reference.
#[tokio::test]
async fn byte_identity_single_field_every_type() {
    let rec = record_with_every_type();
    for case in single_field_cases() {
        let paths = vec![IndexInfoItem::new(vec![case.field])];
        // NEW path.
        let leaves = extract_index_leaves(&rec, &paths)
            .unwrap_or_else(|| panic!("{}: extract_index_leaves returned None", case.label));
        let new_bytes = build_index_key(false, 5000, &leaves).to_bytes();
        // OLD reference.
        let old_bytes = old_path_key(false, 5000, &rec, &[case.field]);
        assert_eq!(
            new_bytes.as_ref(),
            &old_bytes[..],
            "{}: NEW index key is NOT byte-identical to the OLD reference",
            case.label
        );
    }
}

/// (a-cont) Composite (multi-field) index: the NEW path produces byte-for-byte
/// the same key as the OLD multi-field reference. Built by hashing
/// `[leaf_a, leaf_b]` in field order through the unchanged `with_values`.
#[tokio::test]
async fn byte_identity_composite_index() {
    let rec = record_with_every_type();
    let paths = vec![
        IndexInfoItem::new(vec![F_INT]),
        IndexInfoItem::new(vec![F_STR]),
        IndexInfoItem::new(vec![F_DEC]),
    ];
    // NEW path.
    let leaves =
        extract_index_leaves(&rec, &paths).expect("composite: extract_index_leaves returned None");
    let new_bytes = build_index_key(false, 5001, &leaves).to_bytes();

    // OLD reference: descend each path, collect leaves, hash in order.
    let mut old_leaves: Vec<&InnerValue> = Vec::with_capacity(paths.len());
    for item in &paths {
        let mut cur = &rec;
        for &id in &item.path {
            cur = match cur {
                InnerValue::Map(map) => &map[&InternerKey::new(id)],
                _ => unreachable!(),
            };
        }
        old_leaves.push(cur);
    }
    let old_key = IndexRecordKey::new(false, 5001)
        .with_values(&old_leaves)
        .to_bytes();
    assert_eq!(
        new_bytes.as_ref(),
        old_key.as_ref(),
        "composite: NEW index key is NOT byte-identical to the OLD reference"
    );
}

/// (a-cont) The unique-index flag (`is_unique = true`) also produces
/// byte-identical keys (the only difference is the leading flag byte, which
/// both paths set identically via `IndexRecordKey::new`).
#[tokio::test]
async fn byte_identity_unique_flag() {
    let rec = record_with_every_type();
    let paths = vec![IndexInfoItem::new(vec![F_DEC])];
    let leaves = extract_index_leaves(&rec, &paths).expect("Dec leaf present");
    let new_bytes = build_index_key(true, 5002, &leaves).to_bytes();
    let old_bytes = old_path_key(true, 5002, &rec, &[F_DEC]);
    assert_eq!(
        new_bytes.as_ref(),
        &old_bytes[..],
        "unique: NEW index key is NOT byte-identical to the OLD reference"
    );
    // And it must differ from the non-unique key (flag byte 0 vs 1).
    let nonunique_bytes = build_index_key(false, 5002, &leaves).to_bytes();
    assert_ne!(
        new_bytes.as_ref(),
        nonunique_bytes.as_ref(),
        "unique flag must change the key"
    );
}

/// (b) Round-trip query equivalence: an index built via the NEW write path
/// (`on_record_created` → `extract_index_leaves`) and queried via the UNCHANGED
/// read path (`lookup_by_index` with literal `&[InnerValue]`) returns the
/// IDENTICAL `BTreeSet<RecordId>` for every leaf type, including Dec/Big/Map/List.
#[tokio::test]
async fn round_trip_query_equivalence_every_type() {
    for case in single_field_cases() {
        let (_, _, manager) = create_manager();
        let name_interned = 6000 + case.field;
        let def = crate::legacy::index_definition::IndexDefinition::new(
            name_interned,
            vec![IndexInfoItem::new(vec![case.field])],
        );
        manager.create_index(def).await.unwrap();

        // Insert one record carrying every type; index it via the new path.
        let rid = RecordId::new();
        let rec = record_with_every_type();
        manager.on_record_created(&rid, &rec).await.unwrap();

        // Build the literal lookup values via the OLD tree-descent reference
        // (independent of the new extract path) — proves read-path parity.
        let mut cur = &rec;
        cur = match cur {
            InnerValue::Map(map) => &map[&InternerKey::new(case.field)],
            _ => unreachable!(),
        };
        let lookup_values = vec![cur.clone()];

        let result = manager
            .lookup_by_index(name_interned, &lookup_values)
            .await
            .unwrap();
        let expected: std::collections::BTreeSet<RecordId> = [rid].into_iter().collect();
        assert_eq!(
            result, expected,
            "{}: round-trip lookup did not return the record — key diverged (materialize_at dropped it?)",
            case.label
        );
    }
}

/// (b-cont) Composite index round-trip: build via new path, query with the
/// multi-field literal values, get the record back.
#[tokio::test]
async fn round_trip_query_equivalence_composite() {
    let (_, _, manager) = create_manager();
    let name_interned = 7000;
    let paths = vec![
        IndexInfoItem::new(vec![F_INT]),
        IndexInfoItem::new(vec![F_STR]),
        IndexInfoItem::new(vec![F_DEC]),
    ];
    let def = crate::legacy::index_definition::IndexDefinition::new(name_interned, paths.clone());
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
        .unwrap();
    assert!(
        result.contains(&rid),
        "composite: round-trip lookup did not return the record"
    );
}

/// Compile-time + runtime: the planner/extract methods accept `&InnerValue`
/// (coercion through the `RecordRef` impl). If this compiles, existing engine
/// call-sites keep working.
#[tokio::test]
async fn planner_accepts_inner_value_ref() {
    let (_, _, manager) = create_manager();
    let rec = record_with_every_type();
    // extract_index_leaves takes &(impl RecordRef + ?Sized); &InnerValue coerces.
    let _ = extract_index_leaves(&rec, &[IndexInfoItem::new(vec![F_INT])]);
    // unique_keys_for takes &(impl RecordRef).
    let _ = manager.unique_keys_for(&rec);
}

/// Compile-time + runtime: `plan_records_created_batch` accepts an iterator of
/// `(&RecordId, &InnerValue)` (R = InnerValue via the generic bound).
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
