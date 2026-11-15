//! Byte-estimate pin tests for `query::read::value_bytes`.

use shamir_types::types::common::{new_map, TSet};
use shamir_types::types::value::QueryValue;

use crate::query::read::value_bytes::ValueBytes;

#[test]
fn scalar_slots_estimate_16() {
    assert_eq!(ValueBytes::value(&QueryValue::Null), 16);
    assert_eq!(ValueBytes::value(&QueryValue::Bool(false)), 16);
    assert_eq!(ValueBytes::value(&QueryValue::Bool(true)), 16);
    assert_eq!(ValueBytes::value(&QueryValue::Int(0)), 16);
    assert_eq!(ValueBytes::value(&QueryValue::Int(-1)), 16);
    assert_eq!(ValueBytes::value(&QueryValue::F64(2.5)), 16);
}

#[test]
fn payload_kinds_estimate_shell_plus_bytes() {
    assert_eq!(ValueBytes::value(&QueryValue::Str("".into())), 24);
    // "é" is 2 UTF-8 bytes.
    assert_eq!(ValueBytes::value(&QueryValue::Str("é".into())), 26);
    assert_eq!(ValueBytes::value(&QueryValue::Str("abc".into())), 27);
    assert_eq!(ValueBytes::value(&QueryValue::Bin(Vec::new())), 24);
    assert_eq!(ValueBytes::value(&QueryValue::Bin(vec![1, 2, 3])), 27);
    assert_eq!(ValueBytes::value(&QueryValue::Dec(Default::default())), 56);
    // "0" is 1 decimal text byte.
    assert_eq!(ValueBytes::value(&QueryValue::Big(0.into())), 25);
    // "12345" is 5 decimal text bytes.
    assert_eq!(ValueBytes::value(&QueryValue::Big(12345.into())), 29);
}

#[test]
fn containers_estimate_shell_entries_and_key_bytes() {
    assert_eq!(ValueBytes::value(&QueryValue::List(Vec::new())), 24);
    assert_eq!(ValueBytes::value(&QueryValue::Set(TSet::default())), 24);
    assert_eq!(ValueBytes::value(&QueryValue::Map(new_map())), 24);
    assert_eq!(
        ValueBytes::value(&QueryValue::List(vec![QueryValue::Str("ab".into())])),
        66
    );
    assert_eq!(
        ValueBytes::value(&QueryValue::List(vec![
            QueryValue::Int(1),
            QueryValue::Str("ab".into())
        ])),
        98
    );
    let mut set = TSet::default();
    set.insert(QueryValue::Int(7));
    set.insert(QueryValue::Bool(true));
    assert_eq!(ValueBytes::value(&QueryValue::Set(set)), 88);
    let mut map = new_map();
    map.insert(
        "k".to_string(),
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Bin(vec![1, 2, 3])]),
    );
    assert_eq!(ValueBytes::value(&QueryValue::Map(map)), 140);
    assert_eq!(
        ValueBytes::value(&QueryValue::List(vec![QueryValue::List(vec![
            QueryValue::Null
        ])])),
        96
    );
}

#[test]
fn row_and_sequence_estimates() {
    assert_eq!(ValueBytes::row_bytes(&QueryValue::Null), 64);
    assert_eq!(ValueBytes::row_bytes(&QueryValue::Str("abc".into())), 75);
    assert_eq!(ValueBytes::row_bytes(&QueryValue::List(Vec::new())), 72);
    assert_eq!(ValueBytes::sequence(0), 24);
    assert_eq!(ValueBytes::sequence(3), 72);
}

#[test]
fn deep_chain_is_iterative_and_stable() {
    let mut value = QueryValue::Null;
    for _ in 0..100_000 {
        value = QueryValue::List(vec![value]);
    }
    // 100_000 list shells at 40 bytes each, plus the scalar slot.
    assert_eq!(ValueBytes::value(&value), 100_000u64 * 40 + 16);
    while let QueryValue::List(mut items) = value {
        value = match items.pop() {
            Some(inner) => inner,
            None => break,
        };
    }
}
