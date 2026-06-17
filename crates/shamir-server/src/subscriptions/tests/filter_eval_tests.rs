use std::sync::Arc;

use shamir_collections::TMap;
use shamir_db::core::interner::Interner;
use shamir_db::types::value::InnerValue;
use shamir_query_types::filter::{Filter, FilterValue};
use tokio::sync::OnceCell;

use crate::subscriptions::filter_eval::{filter_matches_bytes, filter_matches_inner};

/// Build an `InnerValue::Map` from flat key-value pairs, serialize it to
/// bytes, and return `(bytes, InnerValue, Arc<OnceCell<Interner>>)`.
fn make_record(
    fields: &[(&str, InnerValue)],
) -> (Vec<u8>, InnerValue, Arc<OnceCell<Interner>>) {
    let interner = Interner::new();
    let mut map: TMap<_, InnerValue> = TMap::default();
    for (field, val) in fields {
        let key = interner.touch_ind(*field).expect("intern field").into_key();
        map.insert(key, val.clone());
    }
    let inner = InnerValue::Map(map);
    let bytes = Vec::from(inner.to_bytes().expect("serialize").as_ref());
    let cell = OnceCell::new();
    cell.set(interner).unwrap();
    (bytes, inner, Arc::new(cell))
}

#[test]
fn filter_matches_bytes_eq() {
    let filter = Filter::Eq {
        field: vec!["name".to_string()],
        value: FilterValue::String("alice".to_string()),
    };
    let (yes_bytes, _, cell) =
        make_record(&[("name", InnerValue::Str("alice".to_string()))]);
    let (no_bytes, _, cell_no) =
        make_record(&[("name", InnerValue::Str("bob".to_string()))]);
    assert!(filter_matches_bytes(&filter, &yes_bytes, &cell));
    assert!(!filter_matches_bytes(&filter, &no_bytes, &cell_no));
}

#[test]
fn filter_matches_bytes_and() {
    let filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
        ],
    };
    let (yes_bytes, _, cell_yes) = make_record(&[
        ("status", InnerValue::Str("active".to_string())),
        ("age", InnerValue::Int(25)),
    ]);
    let (no_bytes, _, cell_no) = make_record(&[
        ("status", InnerValue::Str("active".to_string())),
        ("age", InnerValue::Int(15)),
    ]);
    assert!(filter_matches_bytes(&filter, &yes_bytes, &cell_yes));
    assert!(!filter_matches_bytes(&filter, &no_bytes, &cell_no));
}

#[test]
fn filter_matches_bytes_nested_field() {
    let filter = Filter::Eq {
        field: vec!["address".to_string(), "city".to_string()],
        value: FilterValue::String("Jerusalem".to_string()),
    };

    // Build inner city map, then outer address map
    let (yes_bytes, _, cell_yes) = {
        let interner = Interner::new();
        let city_key = interner.touch_ind("city").unwrap().into_key();
        let addr_key = interner.touch_ind("address").unwrap().into_key();
        let mut city_map: TMap<_, InnerValue> = TMap::default();
        city_map.insert(city_key, InnerValue::Str("Jerusalem".to_string()));
        let mut root_map: TMap<_, InnerValue> = TMap::default();
        root_map.insert(addr_key, InnerValue::Map(city_map));
        let inner = InnerValue::Map(root_map);
        let bytes = Vec::from(inner.to_bytes().unwrap().as_ref());
        let cell = OnceCell::new();
        cell.set(interner).unwrap();
        (bytes, inner, Arc::new(cell))
    };

    let (no_bytes, _, cell_no) = {
        let interner = Interner::new();
        let city_key = interner.touch_ind("city").unwrap().into_key();
        let addr_key = interner.touch_ind("address").unwrap().into_key();
        let mut city_map: TMap<_, InnerValue> = TMap::default();
        city_map.insert(city_key, InnerValue::Str("Tel Aviv".to_string()));
        let mut root_map: TMap<_, InnerValue> = TMap::default();
        root_map.insert(addr_key, InnerValue::Map(city_map));
        let inner = InnerValue::Map(root_map);
        let bytes = Vec::from(inner.to_bytes().unwrap().as_ref());
        let cell = OnceCell::new();
        cell.set(interner).unwrap();
        (bytes, inner, Arc::new(cell))
    };

    assert!(filter_matches_bytes(&filter, &yes_bytes, &cell_yes));
    assert!(!filter_matches_bytes(&filter, &no_bytes, &cell_no));
}

// --- Backward-compat tests for filter_matches_inner ---

#[test]
fn filter_matches_inner_eq() {
    let filter = Filter::Eq {
        field: vec!["name".to_string()],
        value: FilterValue::String("alice".to_string()),
    };
    let (_, yes, cell) = make_record(&[("name", InnerValue::Str("alice".to_string()))]);
    let (_, no, cell_no) = make_record(&[("name", InnerValue::Str("bob".to_string()))]);
    assert!(filter_matches_inner(&filter, &yes, &cell));
    assert!(!filter_matches_inner(&filter, &no, &cell_no));
}

#[test]
fn filter_matches_inner_and() {
    let filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
        ],
    };
    let (_, yes, cell_yes) = make_record(&[
        ("status", InnerValue::Str("active".to_string())),
        ("age", InnerValue::Int(25)),
    ]);
    let (_, no, cell_no) = make_record(&[
        ("status", InnerValue::Str("active".to_string())),
        ("age", InnerValue::Int(15)),
    ]);
    assert!(filter_matches_inner(&filter, &yes, &cell_yes));
    assert!(!filter_matches_inner(&filter, &no, &cell_no));
}
