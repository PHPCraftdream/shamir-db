use std::collections::{BTreeMap, BTreeSet};

use shamir_query_builder::write::insert;
use shamir_query_types::batch::BatchOp;
use shamir_types::mpack;
use shamir_types::types::common::TSet;
use shamir_types::types::value::{QueryValue, Value};

use super::{collect_field_names, encode_record_idmsgpack, try_deintern_map};
use crate::interner_cache::FieldMap;

#[test]
fn malformed_id_rows_are_not_partial_records() {
    let fields = FieldMap::new();
    fields.insert_entry("first", 1);
    fields.insert_entry("second", 2);
    let malformed: &[&[u8]] = &[
        &[0x81],
        &[0x82, 0xc4, 1, 1, 42],
        &[0x81, 0xc4, 1, 1, 0x81, 0xc4, 1, 2],
        // Nested array and map headers promise more elements than supplied.
        &[0x81, 0xc4, 1, 1, 0x92, 42],
        &[0x81, 0xc4, 1, 1, 0x82, 0xc4, 1, 2, 42],
        // A complete nested map with a non-canonical three-byte id key.
        &[0x81, 0xc4, 1, 1, 0x81, 0xc4, 3, 1, 2, 3, 42],
        &[0x81, 0xc4, 1, 1, 0x91, 0x81, 0xa1, b'x', 1],
        &[0x81, 0xc4, 1, 1, 42, 0xc0],
        &[0x82, 0xc4, 1, 1, 42, 0xc4, 1, 1, 43],
    ];
    for bytes in malformed {
        assert!(try_deintern_map(bytes, &fields).is_err(), "{bytes:?}");
    }
    let row = try_deintern_map(&[0x81, 0xc4, 1, 1, 42], &fields)
        .unwrap()
        .unwrap();
    assert_eq!(row.get("first"), Some(&Value::Int(42)));
    let nested = [0x81, 0xc4, 1, 1, 0x91, 0x81, 0xc4, 1, 2, 42];
    let row = try_deintern_map(&nested, &fields).unwrap().unwrap();
    assert_eq!(row, mpack!({ "first": [ { "second": 42 } ] }));
    assert!(try_deintern_map(&[0x81, 0xc4, 1, 2, 42], &FieldMap::new())
        .unwrap()
        .is_none());
}

#[test]
fn repeated_and_set_nested_keys_are_borrowed_and_deduplicated() {
    let mut set = TSet::<QueryValue>::default();
    set.insert(mpack!({ "inner": 1 }));
    let record = mpack!({ "outer": @(Value::Set(set)), "inner": 2 });
    let op = BatchOp::Insert(insert("items").row(record.clone()).row(record).build());
    let mut repos = BTreeMap::new();
    collect_field_names(&op, &mut repos);
    let names = repos.get("main").unwrap();
    assert_eq!(names, &BTreeSet::from(["inner", "outer"]));
    let BatchOp::Insert(insert) = &op else {
        panic!("expected insert")
    };
    let Value::Map(first) = &insert.values[0] else {
        panic!("expected map")
    };
    let original = first.keys().find(|key| key.as_str() == "outer").unwrap();
    assert_eq!(names.get("outer").unwrap().as_ptr(), original.as_ptr());
}

#[test]
fn id_encoding_preserves_wire_bytes() {
    let fields = FieldMap::new();
    fields.insert_entry("first", 1);
    assert_eq!(
        encode_record_idmsgpack(&mpack!({ "first": 42 }), &fields).unwrap(),
        vec![0x81, 0xc4, 1, 1, 42]
    );
}
