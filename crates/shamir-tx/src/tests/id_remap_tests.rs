use crate::id_remap::{remap_inner_value_bytes, remap_value};
use shamir_collections::THasher;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::TMap;
use shamir_types::types::value::InnerValue;
use std::collections::HashMap;

#[test]
fn remap_replaces_top_level_map_keys() {
    let mut m = TMap::default();
    m.insert(InternerKey::new(100), InnerValue::Str("hello".into()));
    m.insert(InternerKey::new(200), InnerValue::Int(42));
    let mut value = InnerValue::Map(m);

    let mut remap = HashMap::<_, _, THasher>::default();
    remap.insert(100, 1000);
    remap.insert(200, 2000);

    remap_value(&mut value, &remap);

    if let InnerValue::Map(m) = value {
        assert!(m.get(&InternerKey::new(1000)).is_some());
        assert!(m.get(&InternerKey::new(2000)).is_some());
        assert!(m.get(&InternerKey::new(100)).is_none());
        assert!(m.get(&InternerKey::new(200)).is_none());
    } else {
        panic!("expected Map");
    }
}

#[test]
fn remap_recurses_into_nested_map() {
    let mut inner = TMap::default();
    inner.insert(InternerKey::new(50), InnerValue::Str("nested".into()));
    let mut outer = TMap::default();
    outer.insert(InternerKey::new(1), InnerValue::Map(inner));
    let mut value = InnerValue::Map(outer);

    let mut remap = HashMap::<_, _, THasher>::default();
    remap.insert(50, 5000);
    remap.insert(1, 100);

    remap_value(&mut value, &remap);

    if let InnerValue::Map(o) = value {
        let inner_val = o.get(&InternerKey::new(100)).unwrap();
        if let InnerValue::Map(i) = inner_val {
            assert!(i.get(&InternerKey::new(5000)).is_some());
        } else {
            panic!("inner not Map");
        }
    } else {
        panic!("outer not Map");
    }
}

#[test]
fn remap_leaves_unknown_keys_untouched() {
    let mut m = TMap::default();
    m.insert(InternerKey::new(7), InnerValue::Bool(true));
    let mut value = InnerValue::Map(m);

    let remap = HashMap::<_, _, THasher>::default();
    remap_value(&mut value, &remap);

    if let InnerValue::Map(m) = value {
        assert!(m.get(&InternerKey::new(7)).is_some());
    } else {
        panic!("expected Map");
    }
}

#[test]
fn remap_inside_list() {
    let mut inner_map = TMap::default();
    inner_map.insert(InternerKey::new(9), InnerValue::Int(1));
    let mut v = InnerValue::List(vec![InnerValue::Map(inner_map)]);

    let mut remap = HashMap::<_, _, THasher>::default();
    remap.insert(9, 99);
    remap_value(&mut v, &remap);

    if let InnerValue::List(items) = v {
        if let InnerValue::Map(m) = &items[0] {
            assert!(m.get(&InternerKey::new(99)).is_some());
        } else {
            panic!("expected Map inside list");
        }
    } else {
        panic!("expected List");
    }
}

#[test]
fn remap_inner_value_bytes_roundtrip() {
    let mut m = TMap::default();
    m.insert(InternerKey::new(11), InnerValue::Str("hi".into()));
    let value = InnerValue::Map(m);
    let bytes = value.to_bytes().unwrap();

    let mut remap = HashMap::<_, _, THasher>::default();
    remap.insert(11, 111);

    let new_bytes = remap_inner_value_bytes(bytes, &remap).unwrap();
    let decoded = InnerValue::from_bytes(&new_bytes).unwrap();
    if let InnerValue::Map(m) = decoded {
        assert!(m.get(&InternerKey::new(111)).is_some());
    } else {
        panic!("expected Map");
    }
}

#[test]
fn remap_inner_value_bytes_empty_remap_is_noop() {
    let mut m = TMap::default();
    m.insert(InternerKey::new(11), InnerValue::Str("hi".into()));
    let value = InnerValue::Map(m);
    let bytes = value.to_bytes().unwrap();
    let original = bytes.clone();

    let new_bytes = remap_inner_value_bytes(bytes, &HashMap::<_, _, THasher>::default()).unwrap();
    assert_eq!(new_bytes, original);
}
