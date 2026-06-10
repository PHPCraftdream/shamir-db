//! Pure key-encoding helpers for the index subsystem.
//!
//! These are free functions shared by `index_manager` and
//! `index_manager_unique`. They do not access any `IndexManager` fields.

use crate::index::index_info_item::IndexInfoItem;
use crate::index::index_record_key::IndexRecordKey;
use bytes::Bytes;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// Извлекает значение из InnerValue по пути (список интернированных ключей).
///
/// Путь представляет собой последовательность ключей для навигации
/// по вложенным структурам данных (Map).
///
/// # Аргументы
///
/// * `value` — исходное значение (обычно Map)
/// * `path` — путь к искомому полю (интернированные ключи)
///
/// # Пример
///
/// Для JSON `{"user": {"name": "John"}}` путь к имени будет `[key_user, key_name]`.
pub(super) fn extract_value_by_path(value: &InnerValue, path: &[u64]) -> Option<InnerValue> {
    extract_value_by_path_ref(value, path).cloned()
}

/// Borrowing variant of `extract_value_by_path` — walks the
/// record in-place without cloning the leaf. Hot batch write
/// paths (build_index_key needs only `&InnerValue` to feed into
/// FxHash) use this; the owned variant survives for the few
/// callers that genuinely consume the value.
pub(super) fn extract_value_by_path_ref<'a>(
    value: &'a InnerValue,
    path: &[u64],
) -> Option<&'a InnerValue> {
    let mut cur = value;
    for &id in path {
        match cur {
            InnerValue::Map(map) => {
                let key = InternerKey::new(id);
                cur = map.get(&key)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Извлекает значения для составного индекса из записи.
///
/// Составной индекс может включать несколько полей (например, [name, age]).
/// Этот метод извлекает все указанные поля и возвращает их как вектор.
///
/// # Возвращает
///
/// - `Some(Vec<InnerValue>)` — все поля успешно извлечены
/// - `None` — хотя бы одно поле отсутствует
pub(super) fn extract_index_values(
    value: &InnerValue,
    paths: &[IndexInfoItem],
) -> Option<Vec<InnerValue>> {
    let mut values = Vec::with_capacity(paths.len());

    // Извлекаем значение для каждого поля индекса
    for item in paths {
        match extract_value_by_path(value, &item.path) {
            Some(v) => values.push(v),
            None => return None, // Если хоть одно поле отсутствует — индекс не применим
        }
    }
    Some(values)
}

/// Borrowing variant of `extract_index_values` — returns
/// `Vec<&InnerValue>` (no per-leaf clone). Used by batch write
/// hot paths that immediately feed the values into
/// `IndexRecordKey::with_values` (which takes `&[&InnerValue]`
/// already) and into `build_posting_key`.
pub(super) fn extract_index_values_ref<'a>(
    value: &'a InnerValue,
    paths: &[IndexInfoItem],
) -> Option<Vec<&'a InnerValue>> {
    let mut values = Vec::with_capacity(paths.len());
    for item in paths {
        match extract_value_by_path_ref(value, &item.path) {
            Some(v) => values.push(v),
            None => return None,
        }
    }
    Some(values)
}

/// Build the 25-byte index key from already-borrowed value refs.
/// Used by the batch write paths so they don't allocate an owned
/// `Vec<InnerValue>` just to feed the hash function.
pub(super) fn build_index_key_from_refs(
    is_unique: bool,
    name_interned: u64,
    value_refs: &[&InnerValue],
) -> IndexRecordKey {
    IndexRecordKey::new(is_unique, name_interned).with_values(value_refs)
}

/// Строит ключ записи индекса.
///
/// Ключ индекса состоит из:
/// - Флага is_unique (1 байт)
/// - Идентификатора индекса (интернированное имя, 8 байт)
/// - Хешей значений проиндексированных полей (16 байт)
///
/// Это позволяет быстро находить записи по значению индексируемых полей.
pub(super) fn build_index_key(
    is_unique: bool,
    name_interned: u64,
    values: &[InnerValue],
) -> IndexRecordKey {
    let value_refs: Vec<&InnerValue> = values.iter().collect();
    IndexRecordKey::new(is_unique, name_interned).with_values(&value_refs)
}

/// Compose the physical posting key:
/// `index_key (25b) || record_id (16b)` = 41 bytes.
pub(super) fn build_posting_key(index_key: &Bytes, record_id: &RecordId) -> Bytes {
    let mut k = Vec::with_capacity(index_key.len() + 16);
    k.extend_from_slice(index_key);
    k.extend_from_slice(record_id.as_bytes());
    Bytes::from(k)
}
