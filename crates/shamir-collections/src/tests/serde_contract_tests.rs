use crate::{new_map_wc, new_set, TMap};

/// RV-COLLECTIONS-D5FF5F41: duplicate keys on the wire keep the first position
/// with the last value winning (IndexMap insert semantics).
#[test]
fn rv_collections_d5ff5f41_tmap_duplicate_key_last_value_first_position() {
    let json = r#"{
        "x": 1,
        "y": 2,
        "x": 9
    }"#;
    let m: TMap<String, u64> = serde_json::from_str(json).expect("valid json");
    assert_eq!(m.len(), 2);
    assert_eq!(m.get("x"), Some(&9));
    let keys: Vec<_> = m.keys().cloned().collect();
    assert_eq!(keys, vec!["x".to_string(), "y".to_string()]);
}

/// RV-COLLECTIONS-E214C501: local contract suite exists and aliases work.
#[test]
fn rv_collections_e214c501_contract_suite_registered() {
    let mut m = new_map_wc::<String, u64>(4);
    m.insert("a".to_string(), 1);
    m.insert("b".to_string(), 2);
    assert_eq!(m.get("a"), Some(&1));

    let mut s = new_set::<u64>();
    s.insert(7);
    assert!(s.contains(&7));

    let mut wire = new_map_wc::<String, u64>(4);
    wire.insert("k".to_string(), 42u64);
    let text = serde_json::to_string(&wire).expect("serialize");
    let back: TMap<String, u64> = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(wire, back);
}

/// RV-COLLECTIONS-D5FF5F41: serialization emits entries in INSERTION order
/// (not sorted, not hash order) - the documented wire contract. Three keys
/// inserted out of lexicographic order must serialize in that exact order.
#[test]
fn rv_collections_d5ff5f41_tmap_serializes_in_insertion_order() {
    let mut m = new_map_wc::<String, u64>(3);
    m.insert("z".to_string(), 1);
    m.insert("a".to_string(), 2);
    m.insert("m".to_string(), 3);
    let text = serde_json::to_string(&m).expect("serialize");
    assert_eq!(text, r#"{"z":1,"a":2,"m":3}"#);
}
