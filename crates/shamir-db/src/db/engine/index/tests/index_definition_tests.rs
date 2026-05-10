use crate::codecs::basic::bincode;
use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info_item::IndexInfoItem;

#[test]
fn test_index_definition_creation() {
    let def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    assert_eq!(def.name_interned, 1001);
    assert_eq!(def.paths.len(), 1);
}

#[test]
fn test_index_definition_bincode() {
    let def = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![1, 2])]);
    let serialized = bincode::to_bytes(&def).unwrap();
    let deserialized: IndexDefinition = bincode::from_bytes(&serialized).unwrap();
    assert_eq!(def, deserialized);
}

#[test]
fn test_index_definition_roundtrip() {
    let def = IndexDefinition::new(
        1003,
        vec![
            IndexInfoItem::new(vec![1, 2]),
            IndexInfoItem::new(vec![3, 4, 5]),
        ],
    );

    let bytes = bincode::to_bytes(&def).unwrap();
    let deserialized: IndexDefinition = bincode::from_bytes(&bytes).unwrap();
    assert_eq!(def, deserialized);
}

#[test]
fn test_index_definition_zero_copy() {
    let def = IndexDefinition::new(1004, vec![IndexInfoItem::new(vec![10, 20, 30])]);

    let bytes = bincode::to_bytes(&def).unwrap();
    let def2 = bincode::from_bytes::<IndexDefinition>(&bytes).unwrap();
    assert_eq!(def2.name_interned, 1004);
    assert_eq!(def2.paths.len(), 1);
    assert_eq!(&def2.paths[0].path[..], &[10, 20, 30]);
}
