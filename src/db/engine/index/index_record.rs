use crate::db::error::{DbError, DbResult};
use crate::types::common::TSet;
use crate::types::record_id::RecordId;
use bytes::Bytes;
use fxhash::FxHasher;
use std::hash::{Hash, Hasher};

/// Record in an index - stores a set of record IDs.
/// Currently unused but reserved for future index implementations.
#[allow(dead_code)]
pub struct IndexRecord(TSet<RecordId>);

/// Ключ для записи индекса.
///
/// Используется для быстрого поиска записей по значению индексируемого поля.
/// Поддерживает простые и составные индексы.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexRecordKey {
    /// Флаг уникальности (0 = non-unique, 1 = unique)
    pub is_unique: u8,
    /// Первый хеш значения (primary)
    pub hash1: u64,
    /// Второй хеш значения (для разрешения коллизий)
    pub hash2: u64,
    /// Пути к индексируемым полям (интернированные компоненты)
    /// Для простого индекса: [[1, 2]]
    /// Для составного: [[1], [2, 3]]  (city, address.zip)
    pub path: Vec<Vec<u64>>,
}

impl IndexRecordKey {
    /// Размер бинарного представления ключа без учета переменной части paths
    /// is_unique(1) + num_paths(1) + hash1(8) + hash2(8)
    const HEADER_SIZE: usize = 1 + 1 + 8 + 8;

    /// Создает новый ключ для записи индекса (без значений, только пути).
    ///
    /// # Arguments
    /// * `unique` - флаг уникальности индекса
    /// * `path` - пути к индексируемым полям (интернированные компоненты)
    pub fn new(unique: bool, path: Vec<Vec<u64>>) -> Self {
        Self {
            is_unique: if unique { 1 } else { 0 },
            path,
            hash1: 0,
            hash2: 0,
        }
    }

    /// Добавляет хеши значений к ключу.
    pub fn with_values<T: Hash>(mut self, values: &[&T]) -> Self {
        // Вычисляем комбинированный хеш всех значений
        let mut hasher = FxHasher::default();
        for value in values {
            value.hash(&mut hasher);
        }
        let hash1 = hasher.finish();

        // Второй хеш — инвертированный первый с добавлением всех путей
        let path_hash: u64 = self.path.iter()
            .flat_map(|p| p.iter())
            .fold(0u64, |acc, &id| acc.wrapping_add(id));
        let hash2 = hash1.wrapping_neg() ^ path_hash;

        self.hash1 = hash1;
        self.hash2 = hash2;
        self
    }

    /// Преобразует ключ в байты для хранения в БД.
    ///
    /// # Формат
    /// ```text
    /// +------------+------------------+
    /// | is_unique  | num_paths (u8)   |
    /// | (u8)       |                  |
    /// +------------+------------------+
    /// | path_0_len (u32) | path_0 (u64[]) |
    /// | path_1_len (u32) | path_1 (u64[]) |
    /// | ...               |               |
    /// +-----------------------------------+
    /// | hash1 (u64) | hash2 (u64)        |
    /// +-------------+---------------------+
    /// ```
    pub fn to_bytes(&self) -> Bytes {
        // Считаем общий размер: хедер + размеры путей + данные путей + хеши
        let paths_data_size: usize = self.path.iter()
            .map(|p| 4 + p.len() * 8)  // u32 длина + данные
            .sum();

        let total_size = Self::HEADER_SIZE + paths_data_size;

        let mut bytes = Vec::with_capacity(total_size);

        // is_unique (1 байт)
        bytes.push(self.is_unique);

        // num_paths (1 байт)
        bytes.push(self.path.len() as u8);

        // Пути: для каждого path - длина (u32) + данные (u64[])
        for path in &self.path {
            bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
            for &id in path {
                bytes.extend_from_slice(&id.to_le_bytes());
            }
        }

        // hash1 (8 байт)
        bytes.extend_from_slice(&self.hash1.to_le_bytes());

        // hash2 (8 байт)
        bytes.extend_from_slice(&self.hash2.to_le_bytes());

        Bytes::from(bytes)
    }

    /// Восстанавливает ключ из байтов (обратное к `to_bytes`).
    ///
    /// # Errors
    /// Возвращает `DbError` если:
    /// - Недостаточно байт для чтения заголовка
    /// - Некорректная длина путей
    pub fn from_bytes(bytes: &[u8]) -> DbResult<Self> {
        let min_size = Self::HEADER_SIZE;
        if bytes.len() < min_size {
            return Err(DbError::Internal(format!(
                "Invalid IndexRecordKey: expected at least {} bytes, got {}",
                min_size,
                bytes.len()
            )));
        }

        let is_unique = bytes[0];
        let num_paths = bytes[1] as usize;

        let mut offset = 2;

        // Читаем пути
        let mut path = Vec::with_capacity(num_paths);
        for _ in 0..num_paths {
            if offset + 4 > bytes.len() {
                return Err(DbError::Internal("Invalid IndexRecordKey: unexpected end reading path_len".to_string()));
            }

            let path_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into()
                .map_err(|_| DbError::Internal("Invalid IndexRecordKey: failed to read path_len".to_string()))?) as usize;

            offset += 4;

            if offset + path_len * 8 > bytes.len() {
                return Err(DbError::Internal(format!(
                    "Invalid IndexRecordKey: not enough bytes for path data, need {}, got {}",
                    offset + path_len * 8,
                    bytes.len()
                )));
            }

            let mut current_path = Vec::with_capacity(path_len);
            for _ in 0..path_len {
                let id_bytes = bytes[offset..offset + 8].try_into()
                    .map_err(|_| DbError::Internal("Invalid IndexRecordKey: failed to read path item".to_string()))?;
                let id = u64::from_le_bytes(id_bytes);
                current_path.push(id);
                offset += 8;
            }

            path.push(current_path);
        }

        // Читаем хеши
        if offset + 16 > bytes.len() {
            return Err(DbError::Internal("Invalid IndexRecordKey: not enough bytes for hashes".to_string()));
        }

        let hash1 = u64::from_le_bytes(bytes[offset..offset + 8].try_into()
            .map_err(|_| DbError::Internal("Invalid IndexRecordKey: failed to read hash1".to_string()))?);
        offset += 8;

        let hash2 = u64::from_le_bytes(bytes[offset..offset + 8].try_into()
            .map_err(|_| DbError::Internal("Invalid IndexRecordKey: failed to read hash2".to_string()))?);

        Ok(Self {
            is_unique,
            path,
            hash1,
            hash2,
        })
    }

    /// Возвращает ссылку на пути индекса.
    pub fn paths(&self) -> &[Vec<u64>] {
        &self.path
    }

    /// Вычисляет хеш значения (для тестирования и отладки)
    #[cfg(test)]
    #[allow(dead_code)]
    fn hash_values<T: Hash>(values: &[&T], paths: &[Vec<u64>]) -> (u64, u64) {
        let mut hasher = FxHasher::default();
        for value in values {
            value.hash(&mut hasher);
        }
        let hash1 = hasher.finish();

        let path_hash: u64 = paths.iter()
            .flat_map(|p| p.iter())
            .fold(0u64, |acc, &id| acc.wrapping_add(id));
        let hash2 = hash1.wrapping_neg() ^ path_hash;

        (hash1, hash2)
    }

    /// Проверяет, что ключ соответствует указанным путям
    pub fn matches_paths(&self, paths: &[Vec<u64>]) -> bool {
        if self.path.len() != paths.len() {
            return false;
        }
        self.path.iter().zip(paths.iter()).all(|(a, b)| a == b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
