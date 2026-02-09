use crate::db::engine::index::index_record::IndexRecordKey;

#[test]
fn test_simple_index_key_creation() {
    let path = vec![vec![1, 2, 3]]; // простой индекс с вложенным путем
    let value = "test@example.com";

    let key = IndexRecordKey::new(true, path.clone()).with_values(&[&value]);

    assert_eq!(key.is_unique, 1);
    assert_eq!(key.path, path);
    assert_ne!(key.hash1, 0);
    assert_ne!(key.hash2, 0);
    assert_ne!(key.hash1, key.hash2);
}

#[test]
fn test_composite_index_key_creation() {
    // Составной индекс: (city, age) - используем строки для простоты
    let paths = vec![vec![2], vec![3]]; // city, age
    let city = "Minsk";
    let age = "30";

    let key = IndexRecordKey::new(false, paths.clone()).with_values(&[&city, &age]);

    assert_eq!(key.is_unique, 0);
    assert_eq!(key.path, paths);
    assert_ne!(key.hash1, 0);
    assert_ne!(key.hash2, 0);
}

#[test]
fn test_simple_index_to_bytes() {
    let path = vec![vec![1, 2, 3]];
    let value = "test@example.com";
    let key = IndexRecordKey::new(false, path).with_values(&[&value]);

    let bytes = key.to_bytes();

    // Размер: is_unique(1) + num_paths(1) + path_len(4) + path_data(3*8) + hash1(8) + hash2(8)
    // = 1 + 1 + 4 + 24 + 8 + 8 = 46 байт
    assert_eq!(bytes.len(), 46);

    // is_unique = 0
    assert_eq!(bytes[0], 0);

    // num_paths = 1
    assert_eq!(bytes[1], 1);

    // path_len = 3
    assert_eq!(u32::from_le_bytes(bytes[2..6].try_into().unwrap()), 3);
}

#[test]
fn test_composite_index_to_bytes() {
    let paths = vec![vec![1], vec![2]]; // два поля
    let value1 = "Minsk";
    let value2 = "30";
    let key = IndexRecordKey::new(true, paths).with_values(&[&value1, &value2]);

    let bytes = key.to_bytes();

    // Размер: 1 + 1 + (4+8) + (4+8) + 8 + 8 = 42 байт
    assert_eq!(bytes.len(), 42);

    // is_unique = 1
    assert_eq!(bytes[0], 1);

    // num_paths = 2
    assert_eq!(bytes[1], 2);
}

#[test]
fn test_simple_index_from_bytes_roundtrip() {
    let path = vec![vec![1, 2, 3]];
    let value = "test@example.com";
    let original = IndexRecordKey::new(true, path.clone()).with_values(&[&value]);

    let bytes = original.to_bytes();
    let restored = IndexRecordKey::from_bytes(&bytes).unwrap();

    assert_eq!(restored.is_unique, original.is_unique);
    assert_eq!(restored.path, original.path);
    assert_eq!(restored.hash1, original.hash1);
    assert_eq!(restored.hash2, original.hash2);
}

#[test]
fn test_composite_index_from_bytes_roundtrip() {
    let paths = vec![vec![1], vec![2, 5]];
    let value1 = "Minsk";
    let value2 = "30";
    let original = IndexRecordKey::new(false, paths.clone()).with_values(&[&value1, &value2]);

    let bytes = original.to_bytes();
    let restored = IndexRecordKey::from_bytes(&bytes).unwrap();

    assert_eq!(restored.is_unique, original.is_unique);
    assert_eq!(restored.path, original.path);
    assert_eq!(restored.hash1, original.hash1);
    assert_eq!(restored.hash2, original.hash2);
}

#[test]
fn test_from_bytes_too_short() {
    let result = IndexRecordKey::from_bytes(&[1, 2, 3]);
    assert!(result.is_err());
}

#[test]
fn test_from_bytes_wrong_size() {
    let path = vec![vec![1]];
    let key = IndexRecordKey::new(true, path).with_values(&[&"42"]);
    let mut bytes = key.to_bytes().to_vec();

    // Укорачиваем на 1 байт
    bytes.pop();
    let result = IndexRecordKey::from_bytes(&bytes);
    assert!(result.is_err());
}

#[test]
fn test_composite_index_deterministic() {
    let paths = vec![vec![1], vec![2]];
    let value1 = "Minsk";
    let value2 = "30";

    let key1 = IndexRecordKey::new(true, paths.clone()).with_values(&[&value1, &value2]);
    let key2 = IndexRecordKey::new(true, paths).with_values(&[&value1, &value2]);

    assert_eq!(key1.hash1, key2.hash1);
    assert_eq!(key1.hash2, key2.hash2);

    let bytes1 = key1.to_bytes();
    let bytes2 = key2.to_bytes();
    assert_eq!(bytes1, bytes2);
}

#[test]
fn test_different_values_different_hashes() {
    let paths = vec![vec![1], vec![2]];

    let key1 = IndexRecordKey::new(true, paths.clone()).with_values(&[&"Minsk", &"30"]);
    let key2 = IndexRecordKey::new(true, paths.clone()).with_values(&[&"Moscow", &"30"]);

    assert_ne!(key1.hash1, key2.hash1);
}

#[test]
fn test_matches_paths() {
    let paths = vec![vec![1], vec![2, 3]];
    let key = IndexRecordKey::new(true, paths.clone()).with_values(&[&"a", &"42"]);

    assert!(key.matches_paths(&paths));
    assert!(key.matches_paths(&[vec![1], vec![2, 3]]));

    // Разные пути
    assert!(!key.matches_paths(&[vec![1]]));
    assert!(!key.matches_paths(&[vec![1], vec![2]]));
}

#[test]
fn test_empty_paths_not_allowed() {
    // Пустой список путей — допустим (но бесполезен)
    let paths: Vec<Vec<u64>> = vec![];
    let key = IndexRecordKey::new(true, paths).with_values(&[&"test"]);

    assert_eq!(key.path.len(), 0);
    // Размер: 1 + 1 + 0 + 8 + 8 = 18 байт
    assert_eq!(key.to_bytes().len(), 18);
}

#[test]
fn test_nested_path_single_component() {
    // Простое поле: email → [5]
    let paths = vec![vec![5]];
    let key = IndexRecordKey::new(false, paths).with_values(&[&"test@example.com"]);

    assert_eq!(key.path.len(), 1);
    assert_eq!(key.path[0], vec![5]);
}

#[test]
fn test_nested_path_multiple_components() {
    // Вложенное поле: user.profile.age → [1, 10, 20]
    let paths = vec![vec![1, 10, 20]];
    let key = IndexRecordKey::new(false, paths).with_values(&[&"30"]);

    assert_eq!(key.path.len(), 1);
    assert_eq!(key.path[0], vec![1, 10, 20]);
}

#[test]
fn test_three_component_composite_index() {
    // Составной индекс из 3 полей: (city, street, house)
    let paths = vec![vec![1], vec![2], vec![3]];
    let key = IndexRecordKey::new(true, paths.clone()).with_values(&[&"Minsk", &"Main", &"42"]);

    assert_eq!(key.path.len(), 3);
    assert!(key.matches_paths(&paths));

    // Размер: 1 + 1 + 3*(4+8) + 8 + 8 = 54 байт
    assert_eq!(key.to_bytes().len(), 54);
}

#[test]
fn test_doc_new() {
    // Простой индекс по email
    let key = IndexRecordKey::new(true, vec![vec![1]]);
    assert_eq!(key.is_unique, 1);

    // Составной индекс (city, age)
    let key2 = IndexRecordKey::new(false, vec![vec![2], vec![3]]);
    assert_eq!(key2.is_unique, 0);
}
