use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::codecs::bytes;

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

    let serialized = bincode::serialize(&item).unwrap();
    let deserialized: IndexInfoItem = bincode::deserialize(&serialized).unwrap();

    assert_eq!(deserialized, item);
}

#[test]
fn test_index_info_item_roundtrip() {
    let item = IndexInfoItem {
        path: vec![1, 2, 3, 4, 5],
    };

    let bytes = bytes::to_bytes(&item).unwrap();
    let deserialized: IndexInfoItem = bytes::from_bytes(&bytes).unwrap();
    assert_eq!(deserialized, item);
}

#[test]
fn test_index_info_item_zero_copy() {
    let item = IndexInfoItem {
        path: vec![10, 20, 30],
    };

    let bytes = bytes::to_bytes(&item).unwrap();
    let item2 = bytes::from_bytes::<IndexInfoItem>(&bytes).unwrap();
    assert_eq!(&item2.path[..], &[10, 20, 30]);
}
