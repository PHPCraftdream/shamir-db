# Типы данных модуля индексов (src/index)

Модуль предоставляет систему индексации для таблиц базы данных S.H.A.M.I.R.

**Обновлено:** 2026-02-22

## Обзор архитектуры

|| Уровень | Компонент | Поля | Описание |
||---------|-----------|------|----------|
|| 1 | **IndexManager** | `indexes`, `indexes_unique` | Менеджер индексов таблицы |
|| 2 | **IndexInfo** | `indexes: TDashMap<u64, IndexDefinition>`, `status: AtomicU8` | Конфигурация индексов |
|| 3 | **IndexDefinition** | `name_interned: u64`, `paths: Vec<IndexInfoItem>` | Определение индекса |
|| 4 | **IndexInfoItem** | `path: Vec<u64>` | Путь к полю через ID |

**Иерархия:** IndexManager → IndexInfo → IndexDefinition → IndexInfoItem

---

## Типы индексов

### Обычные индексы (`indexes`)
- Позволяют быстро находить записи по значению
- Значение в хранилище: `BTreeSet<RecordId>` (множество ID)
- Используются для оптимизации запросов

### Уникальные индексы (`indexes_unique`)
- Гарантируют уникальность значения
- Значение в хранилище: `RecordId` (16 байт, один ID)
- Проверка **ДО** записи, обновление **ПОСЛЕ** записи
- При создании сканирует таблицу и возвращает ошибку с количеством дубликатов

---

## Типы данных

### IndexStatus

**Файл:** `index_status.rs`

Статус синхронизации индекса с диском. Используется для отслеживания состояния индекса в runtime.

```rust
pub enum IndexStatus {
    Actual = 0,   // Индекс синхронизирован с диском
    Pending = 1,  // Индекс изменён, требует сохранения
    Saving = 2,   // Индекс сохраняется на диск
}
```

**Методы:**
- `from_u8(value: u8) -> Self` — преобразование из байта
- `as_u8(self) -> u8` — преобразование в байт

**Особенности:**
- Представлен как `#[repr(u8)]` для эффективного хранения
- Используется с `AtomicU8` для потокобезопасного доступа

---

### IndexInfoItem

**Файл:** `index_info_item.rs`

Определение пути к индексируемому полю. Компоненты пути хранятся как интернированные ID (`u64`).

```rust
pub struct IndexInfoItem {
    pub path: Vec<u64>,  // Путь к индексируемому полю
}
```

**Примеры путей:**

|| Путь в документе | Интернированный путь |
||------------------|---------------------|
|| `name` | `[42]` |
|| `user.email` | `[10, 55]` |
|| `address.city.name` | `[5, 12, 42]` |

**Методы:**
- `new(path: Vec<u64>) -> Self` — создание определения индекса

**Трейты:** `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`, `Serialize`, `Deserialize`

---

### IndexDefinition

**Файл:** `index_definition.rs`

Определение индекса — простого (одно поле) или составного (несколько полей).

```rust
pub struct IndexDefinition {
    pub name_interned: u64,        // Интернированный ID имени индекса
    pub paths: Vec<IndexInfoItem>, // Список путей индекса
}
```

**Типы индексов:**

|| paths.len() | Тип индекса | Пример |
||-------------|-------------|--------|
|| 1 | Простой | Индекс по `email` |
|| >1 | Составной | Индекс по `(city, street)` |

**Методы:**
- `new(name_interned: u64, paths: Vec<IndexInfoItem>) -> Self`

**Трейты:** `Debug`, `Clone`, `PartialEq`, `Eq`, `Serialize`, `Deserialize`

---

### IndexInfo

**Файл:** `index_info.rs`

Конфигурация индексов таблицы со статусом синхронизации. Статус является runtime-состоянием и не сериализуется.

```rust
pub struct IndexInfo {
    indexes: TDashMap<u64, IndexDefinition>,  // name_interned → определение
    status: StatusAtom,                       // runtime-статус (не сериализуется)
}
```

Сериализуется как `BTreeMap<u64, IndexDefinition>` (для детерминированного
порядка байт на диске); десериализуется обратно в DashMap. `status` всегда
восстанавливается как `Actual` и затем переводится в `Pending` при первой
правке.

**Методы:**

|| Метод | Описание |
||-------|----------|
|| `new() -> Self` | Создание пустой конфигурации |
|| `from_definitions(iter) -> Self` | Создание из итератора `IndexDefinition` |
|| `is_enabled(&self) -> bool` | Проверка наличия индексов |
|| `status(&self) -> IndexStatus` | Получение текущего статуса |
|| `set_status(&self, status)` | Установка статуса |
|| `mark_pending(&self)` | Пометить как требующий синхронизации |
|| `add_index(&self, index_def)` | Добавить/обновить индекс (`&self`, не `&mut`) |
|| `remove_index(&self, name_interned) -> bool` | Удалить по interned-имени |
|| `get_index(&self, name_interned) -> Option<IndexDefinition>` | Получить по имени |
|| `iter(&self) -> impl Iterator<…>` | Итерация по копиям определений |
|| `contains(&self, name_interned) -> bool` | Проверить наличие |
|| `len(&self) -> usize` / `is_empty(&self) -> bool` | Размер карты |

**Особенности:**
- Внутри — `TDashMap` (DashMap с FxHasher), все мутации через `&self`
- Статус хранится в `Arc<AtomicU8>` для потокобезопасности
- При изменении индексов автоматически устанавливается статус `Pending`

---

### IndexRecordKey

**Файл:** `index_record_key.rs`

Ключ записи индекса для B-Tree. Используется для хранения и поиска в индексе.

```rust
pub struct IndexRecordKey {
    pub is_unique: u8,           // Флаг уникальности (1 или 0)
    pub name_interned: u64,      // ID индекса (интернированное имя)
    pub hash1: u64,              // Хеш индексируемых значений
    pub hash2: u64,              // Второй хеш (защита от коллизий)
}
```

**Формат хранения (байты):**

|| Поле | Размер | Описание |
||------|--------|----------|
|| `is_unique` | 1 byte | Флаг уникальности (0=обычный, 1=уникальный) |
|| `name_interned` | 8 bytes | ID индекса (интернированное имя) |
|| `hash1` | 8 bytes | Хеш значений |
|| `hash2` | 8 bytes | Хеш для коллизий |

**Разделение ключей:**
- Обычные индексы: `is_unique=0` → значение = `BTreeSet<RecordId>`
- Уникальные индексы: `is_unique=1` → значение = `RecordId`

**Методы:**

|| Метод | Описание |
||-------|----------|
|| `new(is_unique: bool, name_interned: u64) -> Self` | Создание префикса ключа |
|| `with_values<T: Hash>(self, values: &[&T]) -> Self` | Вычисление хешей |
|| `to_bytes(&self) -> Bytes` | Сериализация в байты |
|| `from_bytes(bytes: Bytes) -> Result<Self, String>` | Десериализация |
|| `to_prefix_bytes(&self) -> Bytes` | Префикс для сканирования |
|| `name_interned(&self) -> u64` | Получить ID индекса |

**Вычисление хешей:**
- `hash1` — FxHasher с seed 0
- `hash2` — FxHasher с seed `0x9E3779B97F4A7C15` (golden ratio) XOR `name_interned`

**⚠️ ВАЖНО: Проверка данных при коллизиях хешей**

Хеш имеет фиксированный размер (128 бит = hash1 + hash2), поэтому возможны коллизии.
При извлечении данных по ключу **ОБЯЗАТЕЛЬНО** проверять фактическое значение:

```rust
// ❌ НЕПРАВИЛЬНО: доверяем хешу без проверки
let key = IndexRecordKey::new(false, name_interned).with_values(&[&value]);
if let Some(record_ids) = store.get(key.to_bytes())? {
    return Ok(record_ids);  // Может вернуть чужие записи!
}

// ✅ ПРАВИЛЬНО: проверяем фактическое значение
let key = IndexRecordKey::new(false, name_interned).with_values(&[&value]);
if let Some(record_ids) = store.get(key.to_bytes())? {
    let mut result = BTreeSet::new();
    for record_id in record_ids {
        let record = data_store.get(record_id.to_bytes())?;
        if extract_field(&record, &path) == value {
            result.insert(record_id);  // Точное совпадение
        }
        // Иначе: коллизия хеша, пропускаем
    }
    return Ok(result);
}
```

**Почему это важно:**
- FxHasher — быстрый, но не криптографический хеш
- Вероятность коллизии: ~1/2^128 (низкая, но не нулевая)
- При больших объёмах данных (миллиарды записей) коллизии неизбежны
- Без проверки можно вернуть или перезаписать чужую запись

---

### IndexManager

**Файл:** `index_manager.rs` (~1020 строк)

Менеджер индексов таблицы. Управляет обычными и уникальными индексами.

```rust
pub struct IndexManager {
    data_store: Arc<dyn Store>,             // Хранилище данных таблицы
    info_store: Arc<dyn Store>,             // Хранилище метаданных
    indexes: Arc<IndexInfo>,                // Обычные индексы (DashMap внутри)
    indexes_unique: Arc<IndexInfo>,         // Уникальные индексы
    has_indexes: Arc<AtomicBool>,           // Флаг наличия обычных
    has_indexes_unique: Arc<AtomicBool>,    // Флаг наличия уникальных
}
```

**Ключи хранения:**
- Обычные индексы: `RecordId::system("indexes").to_bytes()`
- Уникальные индексы: `RecordId::system("indexes_unique").to_bytes()`

**Основные методы:**

|| Метод | Описание |
||-------|----------|
|| `new(data_store, info_store) -> Result<Self, DbError>` | Создание менеджера |
|| `has_indexes(&self) -> bool` | O(1) проверка обычных индексов |
|| `has_unique_indexes(&self) -> bool` | O(1) проверка уникальных индексов |

**Обычные индексы:**

|| Метод | Описание |
||-------|----------|
|| `create_index(&self, index_def) -> DbResult<()>` | Создать индекс |
|| `drop_index(&self, name_interned) -> DbResult<bool>` | Удалить индекс |
|| `lookup_by_index(&self, name, values) -> DbResult<BTreeSet<RecordId>>` | Поиск по индексу |
|| `on_record_created(&self, record_id, value)` | Обновление после вставки |
|| `on_record_updated(&self, record_id, old, new)` | Обновление после изменения |
|| `on_record_deleted(&self, record_id, old_value)` | Обновление после удаления |

**Уникальные индексы:**

|| Метод | Описание |
||-------|----------|
|| `create_unique_index(&self, index_def) -> DbResult<()>` | Создать уникальный индекс |
|| `drop_unique_index(&self, name_interned) -> DbResult<bool>` | Удалить уникальный индекс |
|| `lookup_by_unique_index(&self, name, values) -> DbResult<Option<RecordId>>` | Поиск |
|| `validate_unique_for_create(&self, value) -> DbResult<()>` | Валидация ДО вставки |
|| `validate_unique_for_update(&self, record_id, old, new) -> DbResult<()>` | Валидация ДО изменения |
|| `on_record_created_unique(&self, record_id, value)` | Обновление ПОСЛЕ вставки |
|| `on_record_updated_unique(&self, record_id, old, new)` | Обновление ПОСЛЕ изменения |
|| `on_record_deleted_unique(&self, record_id, old_value)` | Обновление ПОСЛЕ удаления |

**Особенности:**
- `Clone` создаёт новые `Arc` ссылки (дешёвая операция)
- При инициализации загружает индексы из `info_store`
- Сериализация через `bincode`
- Атомарные флаги для O(1) проверки наличия индексов

---

## Ошибки

### UniqueIndexCreationFailed

```rust
pub enum DbError {
    // ...
    UniqueIndexCreationFailed(String, usize, String),
    // (index_name, duplicate_count, sample_value)
}
```

Возвращается при попытке создать уникальный индекс на таблице с дубликатами.
Содержит имя индекса, количество записей с дублирующимися значениями и пример значения.

---

## Потокобезопасность

|| Компонент | Механизм синхронизации |
||-----------|----------------------|
|| `IndexStatus` | `AtomicU8` через `StatusAtom` |
|| `IndexInfo` | `Arc<IndexInfo>` (DashMap внутри) |
|| `has_indexes` | `Arc<AtomicBool>` |
|| `has_indexes_unique` | `Arc<AtomicBool>` |

---

## Зависимости

### IndexManager

|| Зависимость | Crate / модуль |
||-------------|----------------|
|| `Store` | `shamir-storage::types` |
|| `RecordId` | `shamir-types::types::record_id` |
|| `InnerValue` | `shamir-types::types::value` |
|| `IndexInfo` | `shamir-engine::index` (этот модуль) |

### Цепочка IndexInfo

|| Компонент | Содержит |
||-----------|----------|
|| `IndexInfo` | `Vec<IndexDefinition>` |
|| `IndexDefinition` | `name_interned: u64`, `Vec<IndexInfoItem>` |
|| `IndexInfoItem` | `Vec<u64>` |

### IndexRecordKey (независимый)

|| Зависимость | Crate |
||-------------|-------|
|| `Bytes` | `bytes` |
|| `FxHasher` | `fxhash` |

---

## Пример использования

```rust
// Создание определения индекса
let email_path = IndexInfoItem::new(vec![42]); // ID для "email" (interned)
let index_def = IndexDefinition::new(1001, vec![email_path]);

// Создание обычного индекса
manager.create_index(index_def).await?;

// Создание уникального индекса
manager.create_unique_index(unique_index_def).await?;

// Поиск по индексу
let records = manager.lookup_by_index(1001, &[InnerValue::Str("test".into())]).await?;

// Поиск по уникальному индексу
let record = manager.lookup_by_unique_index(1002, &[InnerValue::Str("user@example.com".into())]).await?;

// Валидация перед вставкой (для уникальных индексов)
manager.validate_unique_for_create(&new_value).await?;
```

---

## Статистика

- **Файлов:** 7 (+ tests/)
- **Тестов:** 56 (index_manager)
- **Строк кода:** ~1020 (index_manager.rs)
