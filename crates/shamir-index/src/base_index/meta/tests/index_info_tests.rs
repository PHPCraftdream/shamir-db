use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use super::IndexInfo;
use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;

#[derive(Serialize)]
struct LegacyDefinition {
    name_interned: u64,
    paths: Vec<IndexInfoItem>,
}

#[test]
fn constructors_deserializers_and_clone_share_cached_topology() {
    let expected = shamir_numa::detect_cached();
    let empty = IndexInfo::new();
    let from = IndexInfo::from_definitions([IndexDefinition::new(7, Vec::new())]);
    let cloned = from.clone();
    let bytes = bincode::serialize(&from).unwrap();
    let decoded: IndexInfo = bincode::deserialize(&bytes).unwrap();
    let legacy = BTreeMap::from([(
        7_u64,
        LegacyDefinition {
            name_interned: 7,
            paths: Vec::new(),
        },
    )]);
    let legacy_bytes = bincode::serialize(&legacy).unwrap();
    assert!(bincode::deserialize::<IndexInfo>(&legacy_bytes).is_err());
    let legacy_decoded = IndexInfo::decode_bytes(&legacy_bytes).unwrap();
    for info in [&empty, &from, &cloned, &decoded, &legacy_decoded] {
        assert!(Arc::ptr_eq(info.indexes.topology(), &expected));
    }
    assert_eq!(legacy_decoded.len(), 1);
}
