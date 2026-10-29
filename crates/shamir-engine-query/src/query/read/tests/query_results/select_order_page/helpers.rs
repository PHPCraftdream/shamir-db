//! Shared helpers for the select/order/page test modules (moved verbatim
//! from the former `exec_tests.rs`).

use bytes::Bytes;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// S4 helper: encode `InnerValue` records to `Bytes` for the lens-fed
/// aggregate pipeline.
pub(in crate::query::read::tests) fn to_bytes_records(
    records: &[(RecordId, InnerValue)],
) -> Vec<(RecordId, Bytes)> {
    records
        .iter()
        .map(|(id, iv)| {
            let bytes = iv.to_bytes().expect("encode InnerValue to bytes");
            (*id, bytes)
        })
        .collect()
}

/// Helper: intern a string and return its u64 id.
pub(in crate::query::read::tests) fn intern(interner: &Interner, s: &str) -> u64 {
    match interner.touch_ind(s) {
        Ok(TouchInd::New(k)) | Ok(TouchInd::Exists(k)) => k.id(),
        Err(e) => panic!("intern failed: {}", e),
    }
}

/// Build a simple record: `{ "name": Str, "age": Int, "city": Str }`.
pub(in crate::query::read::tests) fn make_record(
    interner: &Interner,
    name: &str,
    age: i64,
    city: &str,
) -> InnerValue {
    let mut map = new_map();
    map.insert(
        InternerKey::new(intern(interner, "name")),
        InnerValue::Str(name.into()),
    );
    map.insert(
        InternerKey::new(intern(interner, "age")),
        InnerValue::Int(age),
    );
    map.insert(
        InternerKey::new(intern(interner, "city")),
        InnerValue::Str(city.into()),
    );
    InnerValue::Map(map)
}

pub(in crate::query::read::tests) fn make_records(
    interner: &Interner,
) -> Vec<(RecordId, InnerValue)> {
    vec![
        (RecordId::new(), make_record(interner, "Alice", 30, "NYC")),
        (RecordId::new(), make_record(interner, "Bob", 25, "LA")),
        (RecordId::new(), make_record(interner, "Carol", 35, "NYC")),
        (RecordId::new(), make_record(interner, "Dave", 25, "LA")),
    ]
}
