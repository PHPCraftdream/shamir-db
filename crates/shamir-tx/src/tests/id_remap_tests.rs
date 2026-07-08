use crate::id_remap::{collect_referenced_ids, remap_inner_value_bytes, remap_value};
use shamir_collections::{TFxMap, THasher};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::TMap;
use shamir_types::types::value::InnerValue;
#[allow(clippy::disallowed_types)]
use std::collections::HashMap;

#[test]
fn remap_replaces_top_level_map_keys() {
    let mut m = TMap::default();
    m.insert(InternerKey::new(100), InnerValue::Str("hello".into()));
    m.insert(InternerKey::new(200), InnerValue::Int(42));
    let mut value = InnerValue::Map(m);

    let mut remap = TFxMap::default();
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

    let mut remap = TFxMap::default();
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

    let remap = TFxMap::<u64, u64>::default();
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

    let mut remap = TFxMap::default();
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

    let mut remap = TFxMap::default();
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

    let new_bytes = remap_inner_value_bytes(bytes, &TFxMap::<u64, u64>::default()).unwrap();
    assert_eq!(new_bytes, original);
}

// ===== A8 — collect_referenced_ids tests =====

#[test]
fn collect_ids_top_level_map_keys() {
    let mut m = TMap::default();
    m.insert(InternerKey::new(100), InnerValue::Str("hello".into()));
    m.insert(InternerKey::new(200), InnerValue::Int(42));
    let value = InnerValue::Map(m);

    #[allow(clippy::disallowed_types)]
    let mut out = HashMap::with_hasher(THasher::default());
    collect_referenced_ids(&value, &mut out);
    assert_eq!(out.len(), 2);
    assert!(out.contains_key(&100));
    assert!(out.contains_key(&200));
}

#[test]
fn collect_ids_recurses_into_nested_map_and_list() {
    let mut inner = TMap::default();
    inner.insert(InternerKey::new(50), InnerValue::Str("nested".into()));
    let mut outer = TMap::default();
    outer.insert(InternerKey::new(1), InnerValue::Map(inner));
    let mut list_map = TMap::default();
    list_map.insert(InternerKey::new(77), InnerValue::Bool(true));
    let value = InnerValue::Map(outer);
    let value = {
        let mut m = match value {
            InnerValue::Map(m) => m,
            _ => unreachable!(),
        };
        m.insert(
            InternerKey::new(2),
            InnerValue::List(vec![InnerValue::Map(list_map)]),
        );
        InnerValue::Map(m)
    };

    #[allow(clippy::disallowed_types)]
    let mut out = HashMap::with_hasher(THasher::default());
    collect_referenced_ids(&value, &mut out);
    // ids 1, 2 (outer), 50 (nested map), 77 (list elem map) — all collected.
    assert_eq!(out.len(), 4);
    assert!(out.contains_key(&1));
    assert!(out.contains_key(&2));
    assert!(out.contains_key(&50));
    assert!(out.contains_key(&77));
}

#[test]
fn collect_ids_empty_and_scalar_collect_nothing() {
    #[allow(clippy::disallowed_types)]
    let mut out = HashMap::with_hasher(THasher::default());

    // Scalar value — nothing to collect.
    collect_referenced_ids(&InnerValue::Int(7), &mut out);
    assert!(out.is_empty());

    // Empty map — nothing to collect.
    let value = InnerValue::Map(TMap::default());
    collect_referenced_ids(&value, &mut out);
    assert!(out.is_empty());
}

#[test]
fn collect_ids_merges_into_caller_supplied_set() {
    // Two values folded into the same set.
    let mut m1 = TMap::default();
    m1.insert(InternerKey::new(10), InnerValue::Null);
    let v1 = InnerValue::Map(m1);
    let mut m2 = TMap::default();
    m2.insert(InternerKey::new(20), InnerValue::Null);
    m2.insert(InternerKey::new(10), InnerValue::Null); // duplicate id, must dedup
    let v2 = InnerValue::Map(m2);

    #[allow(clippy::disallowed_types)]
    let mut out = HashMap::with_hasher(THasher::default());
    collect_referenced_ids(&v1, &mut out);
    collect_referenced_ids(&v2, &mut out);
    // 10 appears in both — deduped; 20 once → 2 total.
    assert_eq!(out.len(), 2);
    assert!(out.contains_key(&10));
    assert!(out.contains_key(&20));
}
