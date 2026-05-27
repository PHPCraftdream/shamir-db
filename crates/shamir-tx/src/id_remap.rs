//! Apply an overlay-id → base-id remap to staged record bytes.
//!
//! After `commit_interner_overlay` merges the per-tx interner overlay
//! into the base interner, some staged record bytes may still contain
//! references to overlay ids (>= `OVERLAY_ID_BASE`). The executor calls
//! [`remap_inner_value_bytes`] for each staged value to rewrite those
//! references before the bytes hit `transact()`.

use std::collections::HashMap;

use bytes::Bytes;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::InnerValue;

/// Recursively replace `InternerKey` ids in `value` according to
/// `remap`. Keys not present in the remap are left unchanged.
pub fn remap_value(value: &mut InnerValue, remap: &HashMap<u64, u64>) {
    match value {
        InnerValue::Map(m) => {
            let entries: Vec<(InternerKey, InnerValue)> = m.drain(..).collect();
            for (k, mut v) in entries {
                let new_key = match remap.get(&k.id()) {
                    Some(&new_id) => InternerKey::new(new_id),
                    None => k,
                };
                remap_value(&mut v, remap);
                m.insert(new_key, v);
            }
        }
        InnerValue::List(l) => {
            for elem in l {
                remap_value(elem, remap);
            }
        }
        InnerValue::Set(_) => {}
        InnerValue::Null
        | InnerValue::Bool(_)
        | InnerValue::Int(_)
        | InnerValue::F64(_)
        | InnerValue::Dec(_)
        | InnerValue::Big(_)
        | InnerValue::Str(_)
        | InnerValue::Bin(_) => {}
    }
}

/// Decode `Bytes` as `InnerValue`, apply [`remap_value`], re-encode.
///
/// Returns `Err` only on serde failure. If `remap` is empty this is a
/// no-op decode+encode round-trip — caller can skip the call when the
/// remap is empty.
pub fn remap_inner_value_bytes(
    bytes: Bytes,
    remap: &HashMap<u64, u64>,
) -> Result<Bytes, rmp_serde::encode::Error> {
    let mut value = InnerValue::from_bytes(&bytes)
        .map_err(|e| rmp_serde::encode::Error::Syntax(format!("decode failed: {e}")))?;
    remap_value(&mut value, remap);
    value.to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_types::core::interner::InternerKey;
    use shamir_types::types::common::TMap;
    use shamir_types::types::value::InnerValue;

    #[test]
    fn remap_replaces_top_level_map_keys() {
        let mut m = TMap::default();
        m.insert(InternerKey::new(100), InnerValue::Str("hello".into()));
        m.insert(InternerKey::new(200), InnerValue::Int(42));
        let mut value = InnerValue::Map(m);

        let mut remap = HashMap::new();
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

        let mut remap = HashMap::new();
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

        let remap = HashMap::new();
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

        let mut remap = HashMap::new();
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

        let mut remap = HashMap::new();
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

        let new_bytes = remap_inner_value_bytes(bytes, &HashMap::new()).unwrap();
        assert_eq!(new_bytes, original);
    }
}
