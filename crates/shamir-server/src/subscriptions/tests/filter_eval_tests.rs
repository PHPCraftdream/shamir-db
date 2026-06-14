use std::sync::Arc;

use shamir_collections::TMap;
use shamir_db::core::interner::Interner;
use shamir_db::types::value::InnerValue;
use shamir_query_types::filter::{Filter, FilterValue};
use tokio::sync::OnceCell;

use crate::subscriptions::filter_eval::filter_matches_inner;

/// Build an `(InnerValue, Arc<OnceCell<Interner>>)` from a list of
/// `(field, InnerValue)` pairs, interning all keys.
fn make_record(fields: &[(&str, InnerValue)]) -> (InnerValue, Arc<OnceCell<Interner>>) {
    let interner = Interner::new();
    let mut map: TMap<_, InnerValue> = TMap::default();
    for (field, val) in fields {
        let key = interner.touch_ind(*field).expect("intern field").into_key();
        map.insert(key, val.clone());
    }
    let cell = OnceCell::new();
    cell.set(interner).unwrap();
    (InnerValue::Map(map), Arc::new(cell))
}

#[test]
fn filter_matches_value_eq() {
    let filter = Filter::Eq {
        field: vec!["name".to_string()],
        value: FilterValue::String("alice".to_string()),
    };
    let (yes, cell) = make_record(&[("name", InnerValue::Str("alice".to_string()))]);
    let (no, cell_no) = make_record(&[("name", InnerValue::Str("bob".to_string()))]);
    assert!(filter_matches_inner(&filter, &yes, &cell));
    assert!(!filter_matches_inner(&filter, &no, &cell_no));
}

#[test]
fn filter_matches_value_and() {
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
    let (yes, cell_yes) = make_record(&[
        ("status", InnerValue::Str("active".to_string())),
        ("age", InnerValue::Int(25)),
    ]);
    let (no, cell_no) = make_record(&[
        ("status", InnerValue::Str("active".to_string())),
        ("age", InnerValue::Int(15)),
    ]);
    assert!(filter_matches_inner(&filter, &yes, &cell_yes));
    assert!(!filter_matches_inner(&filter, &no, &cell_no));
}

#[test]
fn filter_matches_value_nested_field() {
    let filter = Filter::Eq {
        field: vec!["address".to_string(), "city".to_string()],
        value: FilterValue::String("Jerusalem".to_string()),
    };

    // Build inner city map, then outer address map
    let (yes, cell_yes) = {
        let interner = Interner::new();
        let city_key = interner.touch_ind("city").unwrap().into_key();
        let addr_key = interner.touch_ind("address").unwrap().into_key();
        let mut city_map: TMap<_, InnerValue> = TMap::default();
        city_map.insert(city_key, InnerValue::Str("Jerusalem".to_string()));
        let mut root_map: TMap<_, InnerValue> = TMap::default();
        root_map.insert(addr_key, InnerValue::Map(city_map));
        let cell = OnceCell::new();
        cell.set(interner).unwrap();
        (InnerValue::Map(root_map), Arc::new(cell))
    };

    let (no, cell_no) = {
        let interner = Interner::new();
        let city_key = interner.touch_ind("city").unwrap().into_key();
        let addr_key = interner.touch_ind("address").unwrap().into_key();
        let mut city_map: TMap<_, InnerValue> = TMap::default();
        city_map.insert(city_key, InnerValue::Str("Tel Aviv".to_string()));
        let mut root_map: TMap<_, InnerValue> = TMap::default();
        root_map.insert(addr_key, InnerValue::Map(city_map));
        let cell = OnceCell::new();
        cell.set(interner).unwrap();
        (InnerValue::Map(root_map), Arc::new(cell))
    };

    assert!(filter_matches_inner(&filter, &yes, &cell_yes));
    assert!(!filter_matches_inner(&filter, &no, &cell_no));
}
