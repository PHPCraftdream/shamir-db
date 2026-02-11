use crate::codecs::basic::bincode;
use crate::db::engine::index::index_info_item::IndexInfoItem;

#[test]
fn test_index_info_item_creation() {
    let item = IndexInfoItem::new(vec![1, 2, 3]);
    assert_eq!(item.path, vec![1, 2, 3]);
}

#[test]
fn test_index_info_item_bincode() {
    let item = IndexInfoItem {
        path: vec![1, 2, 3],
    };

    let serialized = bincode::to_bytes(&item).unwrap();
    let deserialized: IndexInfoItem = bincode::from_bytes(&serialized).unwrap();

    assert_eq!(deserialized, item);
}

#[test]
fn test_index_info_item_roundtrip() {
    let item = IndexInfoItem {
        path: vec![1, 2, 3, 4, 5],
    };

    let bytes = bincode::to_bytes(&item).unwrap();
    let deserialized: IndexInfoItem = bincode::from_bytes(&bytes).unwrap();
    assert_eq!(deserialized, item);
}

#[test]
fn test_index_info_item_zero_copy() {
    let item = IndexInfoItem {
        path: vec![10, 20, 30],
    };

    let bytes = bincode::to_bytes(&item).unwrap();
    let item2 = bincode::from_bytes::<IndexInfoItem>(&bytes).unwrap();
    assert_eq!(&item2.path[..], &[10, 20, 30]);
}
