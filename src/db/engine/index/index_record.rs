use crate::db::error::{DbError, DbResult};
use crate::types::common::TSet;
use crate::types::record_id::RecordId;
use crate::types::value::UserValue;
use bytes::Bytes;
use fxhash::FxHasher;
use std::hash::{Hash, Hasher};

pub struct IndexRecord(TSet<RecordId>);

/// Ключ для записи индекса.
///
/// Используется для быстрого поиска записей по значению индексируемого поля.
/// Состоит из метаданных индекса и двух хешей значения для коллизионного разрешения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRecordKey {
    /// Флаг уникальности (0 = non-unique, 1 = unique)
    pub is_unique: u8,
    /// Путь к индексируемому полю (интернированные компоненты)
    pub path: Vec<u64>,
    /// Первый хеш значения (primary)
    pub hash1: u64,
    /// Второй хеш значения (для разрешения коллизий)
    pub hash2: u64,
}

impl IndexRecordKey {
    /// Размер бинарного представления ключа без учета переменной части path
    const HEADER_SIZE: usize = 1 + 4 + 8 + 8; // is_unique + path_len + hash1 + hash2

    /// Создает новый ключ для записи индекса.
    ///
    /// # Arguments
    /// * `unique` - флаг уникальности индекса
    /// * `path` - путь к индексируемому полю (интернированные компоненты)
    /// * `value` - значение для хеширования
    ///
    /// # Хеширование
    /// Используется двухуровневое хеширование для минимизации коллизий:
    /// - `hash1` — основной хеш значения
    /// - `hash2` — дополнительный хеш для разрешения коллизий (инвертированный)
    pub fn new<T: Hash>(unique: bool, path: Vec<u64>, value: &T) -> Self {
        // Вычисляем хеш значения
        let mut hasher = FxHasher::default();
        value.hash(&mut hasher);
        let hash1 = hasher.finish();

        // Второй хеш — инвертированный первый с добавлением пути
        let hash2 = hash1.wrapping_neg() ^ path.iter().fold(0u64, |acc, &id| acc.wrapping_add(id));

        Self {
            is_unique: if unique { 1 } else { 0 },
            path,
            hash1,
            hash2,
        }
    }

    /// Преобразует ключ в байты для хранения в БД.
    ///
    /// # Формат
    /// ```text
    /// +-----------+------------------+------------------+
    /// | is_unique | path_len (u32)  | path (u64[])     |
    /// | (u8)      |                  |                  |
    /// +-----------+------------------+------------------+
    /// | hash1     | hash2           |
    /// | (u64)     | (u64)           |
    /// +-----------+------------------+
    /// ```
    pub fn to_bytes(&self) -> Bytes {
        let path_len = self.path.len();
        let total_size = Self::HEADER_SIZE + (path_len * 8);

        let mut bytes = Vec::with_capacity(total_size);

        // is_unique (1 байт)
        bytes.push(self.is_unique);

        // path_len (4 байта, little endian)
        bytes.extend_from_slice(&(path_len as u32).to_le_bytes());

        // path (8 байт на элемент)
        for &id in &self.path {
            bytes.extend_from_slice(&id.to_le_bytes());
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
    /// - Некорректная длина path
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
        let path_len = u32::from_le_bytes(bytes[1..5].try_into()
            .map_err(|_| DbError::Internal("Invalid IndexRecordKey: failed to read path_len".to_string()))?) as usize;

        let expected_size = min_size + (path_len * 8);
        if bytes.len() != expected_size {
            return Err(DbError::Internal(format!(
                "Invalid IndexRecordKey: expected {} bytes, got {}",
                expected_size,
                bytes.len()
            )));
        }

        let mut path = Vec::with_capacity(path_len);
        for i in 0..path_len {
            let offset = 5 + (i * 8);
            let id_bytes = bytes[offset..offset + 8].try_into()
                .map_err(|_| DbError::Internal("Invalid IndexRecordKey: failed to read path item".to_string()))?;
            let id = u64::from_le_bytes(id_bytes);
            path.push(id);
        }

        let hash1_offset = 5 + (path_len * 8);
        let hash1 = u64::from_le_bytes(bytes[hash1_offset..hash1_offset + 8].try_into()
            .map_err(|_| DbError::Internal("Invalid IndexRecordKey: failed to read hash1".to_string()))?);

        let hash2_offset = hash1_offset + 8;
        let hash2 = u64::from_le_bytes(bytes[hash2_offset..hash2_offset + 8].try_into()
            .map_err(|_| DbError::Internal("Invalid IndexRecordKey: failed to read hash2".to_string()))?);

        Ok(Self {
            is_unique,
            path,
            hash1,
            hash2,
        })
    }

    /// Возвращает ссылку на путь индекса.
    pub fn get_path(&self) -> &[u64] {
        &self.path
    }

    /// Вычисляет хеш значения (для тестирования и отладки)
    #[cfg(test)]
    fn hash_value<T: Hash>(value: &T, path: &[u64]) -> (u64, u64) {
        let mut hasher = FxHasher::default();
        value.hash(&mut hasher);
        let hash1 = hasher.finish();
        let hash2 = hash1.wrapping_neg() ^ path.iter().fold(0u64, |acc, &id| acc.wrapping_add(id));
        (hash1, hash2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_record_key_creation() {
        use crate::types::value::UserValue;

        let value = UserValue::Str("test@example.com".to_string());
        let path = vec![1, 2, 3]; // interned path components

        let key = IndexRecordKey::new(true, path.clone(), &value);

        assert_eq!(key.is_unique, 1);
        assert_eq!(key.path, path);
        assert_ne!(key.hash1, 0);
        assert_ne!(key.hash2, 0);
        // Разные сида дают разные хеши
        assert_ne!(key.hash1, key.hash2);
    }

    #[test]
    fn test_index_record_key_to_bytes() {
        use crate::types::value::UserValue;

        let value = UserValue::Str("test@example.com".to_string());
        let path = vec![1, 2, 3];
        let key = IndexRecordKey::new(false, path.clone(), &value);

        let bytes = key.to_bytes();

        // Проверяем размер: 1 + 4 + (3 * 8) + 8 + 8 = 45 байт
        assert_eq!(bytes.len(), 45);

        // is_unique = 0
        assert_eq!(bytes[0], 0);

        // Проверка восстановленных данных
        assert_eq!(u32::from_le_bytes(bytes[1..5].try_into().unwrap()), 3);
    }

    #[test]
    fn test_index_record_key_unique_flag() {
        use crate::types::value::UserValue;

        let value = UserValue::Str("test".to_string());
        let path = vec![1];

        let unique_key = IndexRecordKey::new(true, path.clone(), &value);
        let non_unique_key = IndexRecordKey::new(false, path, &value);

        assert_eq!(unique_key.is_unique, 1);
        assert_eq!(non_unique_key.is_unique, 0);
    }

    #[test]
    fn test_index_record_key_deterministic() {
        use crate::types::value::UserValue;

        let value = UserValue::Str("same value".to_string());
        let path = vec![5, 10, 15];

        let key1 = IndexRecordKey::new(true, path.clone(), &value);
        let key2 = IndexRecordKey::new(true, path, &value);

        // Одинаковые входные данные должны давать одинаковые хеши
        assert_eq!(key1.hash1, key2.hash1);
        assert_eq!(key1.hash2, key2.hash2);

        // Бинарное представление должно быть идентичным
        let bytes1 = key1.to_bytes();
        let bytes2 = key2.to_bytes();
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn test_index_record_key_different_values() {
        use crate::types::value::UserValue;

        let value1 = UserValue::Str("alice@example.com".to_string());
        let value2 = UserValue::Str("bob@example.com".to_string());
        let path = vec![1];

        let key1 = IndexRecordKey::new(true, path.clone(), &value1);
        let key2 = IndexRecordKey::new(true, path, &value2);

        // Разные значения должны давать разные хеши
        assert_ne!(key1.hash1, key2.hash1);
        assert_ne!(key1.hash2, key2.hash2);
    }

    #[test]
    fn test_index_record_key_empty_path() {
        use crate::types::value::UserValue;

        let value = UserValue::Str("test".to_string());
        let key = IndexRecordKey::new(false, vec![], &value);

        assert_eq!(key.path.len(), 0);
        // Размер: 1 + 4 + 0 + 8 + 8 = 21 байт
        assert_eq!(key.to_bytes().len(), 21);
    }

    #[test]
    fn test_index_record_key_large_path() {
        use crate::types::value::UserValue;

        let value = UserValue::Str("test".to_string());
        let path: Vec<u64> = (1..=100).collect();
        let key = IndexRecordKey::new(true, path.clone(), &value);

        assert_eq!(key.path.len(), 100);
        // Размер: 1 + 4 + (100 * 8) + 8 + 8 = 821 байт
        assert_eq!(key.to_bytes().len(), 821);
    }

    #[test]
    fn test_index_record_key_numeric_value() {
        use crate::types::value::UserValue;

        // Проверка хеширования числовых значений
        let value = UserValue::Int(42);
        let key = IndexRecordKey::new(true, vec![1], &value);

        let bytes = key.to_bytes();
        // Размер: 1 + 4 + (1 * 8) + 8 + 8 = 29 байт
        assert_eq!(bytes.len(), 29);
        assert_ne!(key.hash1, 0);
    }

    #[test]
    fn test_index_record_key_from_bytes_roundtrip() {
        use crate::types::value::UserValue;

        let value = UserValue::Str("test@example.com".to_string());
        let path = vec![1, 2, 3];
        let original = IndexRecordKey::new(true, path.clone(), &value);

        let bytes = original.to_bytes();
        let restored = IndexRecordKey::from_bytes(&bytes).unwrap();

        assert_eq!(restored.is_unique, original.is_unique);
        assert_eq!(restored.path, original.path);
        assert_eq!(restored.hash1, original.hash1);
        assert_eq!(restored.hash2, original.hash2);
    }

    #[test]
    fn test_index_record_key_from_bytes_too_short() {
        let result = IndexRecordKey::from_bytes(&[1, 2, 3]); // недостаточно байт
        assert!(result.is_err());
    }

    #[test]
    fn test_index_record_key_from_bytes_wrong_size() {
        // Создаём валидные байты
        let value = UserValue::Str("test".to_string());
        let key = IndexRecordKey::new(true, vec![1, 2], &value);
        let mut bytes = key.to_bytes().to_vec();

        // Укорачиваем на 1 байт — должно быть ошибкой
        bytes.pop();
        let result = IndexRecordKey::from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_index_record_key_get_path() {
        use crate::types::value::UserValue;

        let value = UserValue::Str("test".to_string());
        let path = vec![5, 10, 15, 20];
        let key = IndexRecordKey::new(false, path.clone(), &value);

        assert_eq!(key.get_path(), &path);
        assert_eq!(key.get_path().len(), 4);
    }

    #[test]
    fn test_index_record_key_empty_path_roundtrip() {
        use crate::types::value::UserValue;

        let value = UserValue::Bool(true);
        let original = IndexRecordKey::new(false, vec![], &value);

        let bytes = original.to_bytes();
        let restored = IndexRecordKey::from_bytes(&bytes).unwrap();

        assert_eq!(restored.path.len(), 0);
        assert_eq!(restored.is_unique, 0);
    }
}