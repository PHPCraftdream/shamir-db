# Типы данных модуля индексов (src/db/engine/index)

Модуль предоставляет систему индексации для таблиц базы данных S.H.A.M.I.R.

## Обзор архитектуры

| Уровень | Компонент | Поля | Описание |
|---------|-----------|------|----------|
| 1 | **TableIndexManager** | `indexes`, `indexes_unique` | Менеджер индексов таблицы |
| 2 | **IndexInfo** | `indexes: Vec<IndexDefinition>`, `status: AtomicU8` | Конфигурация индексов |
| 3 | **IndexDefinition** | `name: String`, `paths: Vec<IndexInfoItem>` | Определение индекса |
| 4 | **IndexInfoItem** | `path: Vec<u64>` | Путь к полю через ID |

**Иерархия:** TableIndexManager → IndexInfo → IndexDefinition → IndexInfoItem

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

| Путь в документе | Интернированный путь |
|------------------|---------------------|
| `name` | `[42]` |
| `user.email` | `[10, 55]` |
| `address.city.name` | `[5, 12, 42]` |

**Методы:**
- `new(path: Vec<u64>) -> Self` — создание определения индекса

**Трейты:** `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`, `Serialize`, `Deserialize`

---

### IndexDefinition

**Файл:** `index_definition.rs`

Определение индекса — простого (одно поле) или составного (несколько полей).

```rust
pub struct IndexDefinition {
    pub name: String,              // Уникальное имя индекса
    pub paths: Vec<IndexInfoItem>, // Список путей индекса
}
```

**Типы индексов:**

| paths.len() | Тип индекса | Пример |
|-------------|-------------|--------|
| 1 | Простой | Индекс по `email` |
| >1 | Составной | Индекс по `(city, street)` |

**Методы:**
- `new(name: &str, paths: Vec<IndexInfoItem>) -> Self`

**Трейты:** `Debug`, `Clone`, `PartialEq`, `Eq`, `Serialize`, `Deserialize`

---

### IndexInfo

**Файл:** `index_info.rs`

Конфигурация индексов таблицы со статусом синхронизации. Статус является runtime-состоянием и не сериализуется.

```rust
pub struct IndexInfo {
    indexes: Vec<IndexDefinition>,     // Определения индексов
    #[serde(skip)]
    status: StatusAtom,                // Runtime-статус (не сериализуется)
}
```

**Методы:**

| Метод | Описание |
|-------|----------|
| `new() -> Self` | Создание пустой конфигурации |
| `from_definitions(indexes) -> Self` | Создание с определениями |
| `is_enabled(&self) -> bool` | Проверка наличия индексов |
| `status(&self) -> IndexStatus` | Получение текущего статуса |
| `set_status(&self, status)` | Установка статуса |
| `mark_pending(&self)` | Пометить как требующий синхронизации |
| `add_index(&mut self, index_def)` | Добавить/обновить индекс |
| `remove_index(&mut self, name) -> bool` | Удалить индекс по имени |
| `definitions(&self) -> &[IndexDefinition]` | Получить все определения |
| `definitions_mut(&mut self) -> &mut Vec<IndexDefinition>` | Мутабельный доступ |

**Особенности:**
- `PartialEq` сравнивает только `indexes`, игнорируя `status`
- Статус хранится в `Arc<AtomicU8>` для потокобезопасности
- При изменении индексов автоматически устанавливается статус `Pending`

---

### IndexRecordKey

**Файл:** `index_record_key.rs`

Ключ записи индекса для B-Tree. Используется для хранения и поиска в индексе.

```rust
pub struct IndexRecordKey {
    pub is_unique: u8,           // Флаг уникальности (1 или 0)
    pub path: Vec<Vec<u64>>,     // Пути к индексируемым полям
    pub hash1: u64,              // Хеш индексируемых значений
    pub hash2: u64,              // Второй хеш (защита от коллизий)
}
```

**Формат хранения (байты):**

| Поле | Размер | Описание |
|------|--------|----------|
| `is_unique` | 1 byte | Флаг уникальности |
| `num_paths` | 1 byte | Количество путей |
| `path data` | variable | `len(4) + ids(8*n)` на каждый путь |
| `hash1` | 8 bytes | Хеш значений |
| `hash2` | 8 bytes | Хеш для коллизий |

**Методы:**

| Метод | Описание |
|-------|----------|
| `new(is_unique: bool, path: Vec<Vec<u64>>) -> Self` | Создание ключа |
| `with_values<T: Hash>(self, values: &[&T]) -> Self` | Вычисление хешей |
| `to_bytes(&self) -> Bytes` | Сериализация в байты |
| `from_bytes(bytes: Bytes) -> Result<Self, String>` | Десериализация |
| `paths(&self) -> &[Vec<u64>]` | Получить пути |
| `matches_paths(&self, paths: &[Vec<u64>]) -> bool` | Проверка соответствия путей |

**Вычисление хешей:**
- `hash1` — FxHasher с seed 0
- `hash2` — FxHasher с seed `0x9E3779B97F4A7C15` (golden ratio) XOR `index_name_interned`

---

### TableIndexManager

**Файл:** `table_index_manager.rs`

Менеджер индексов таблицы. Управляет обычными и уникальными индексами.

```rust
pub struct TableIndexManager {
    interner: Arc<OnceCell<Interner>>,     // Интернатор строк
    data_store: Arc<dyn Store>,             // Хранилище данных
    info_store: Arc<dyn Store>,             // Хранилище метаданных
    indexes: Arc<RwLock<IndexInfo>>,        // Обычные индексы
    indexes_unique: Arc<RwLock<IndexInfo>>, // Уникальные индексы
    has_indexes: AtomicBool,                // Флаг наличия обычных
    has_indexes_unique: AtomicBool,         // Флаг наличия уникальных
}
```

**Ключи хранения:**
- Обычные индексы: `RecordId::system("indexes").to_bytes()`
- Уникальные индексы: `RecordId::system("indexes_unique").to_bytes()`

**Методы:**

| Метод | Описание |
|-------|----------|
| `new(data_store, info_store, interner) -> Result<Self, DbError>` | Создание менеджера |
| `has_indexes(&self) -> bool` | Проверка наличия обычных индексов |
| `has_unique_indexes(&self) -> bool` | Проверка наличия уникальных индексов |

**Особенности:**
- `Clone` создаёт новые `Arc` ссылки и атомарные флаги
- При инициализации загружает индексы из `info_store`
- Сериализация через `bincode`

---

## Потокобезопасность

| Компонент | Механизм синхронизации |
|-----------|----------------------|
| `IndexStatus` | `AtomicU8` через `StatusAtom` |
| `IndexInfo` | `Arc<RwLock<IndexInfo>>` в `TableIndexManager` |
| `has_indexes` | `AtomicBool` |
| `Interner` | `Arc<OnceCell<Interner>>` |

---

## Зависимости

### TableIndexManager

| Зависимость | Модуль |
|-------------|--------|
| `Interner` | `core::interner` |
| `Store` | `db::storage::types` |
| `RecordId` | `types::record_id` |
| `IndexInfo` | `db::engine::index` |

### Цепочка IndexInfo

| Компонент | Содержит |
|-----------|----------|
| `IndexInfo` | `Vec<IndexDefinition>` |
| `IndexDefinition` | `Vec<IndexInfoItem>` |
| `IndexInfoItem` | `Vec<u64>` |

### IndexRecordKey (независимый)

| Зависимость | Crate |
|-------------|-------|
| `Bytes` | `bytes` |
| `FxHasher` | `fxhash` |

---

## Пример использования

```rust
// Создание определения индекса
let email_path = IndexInfoItem::new(vec![42]); // ID для "email"
let index_def = IndexDefinition::new("email_idx", vec![email_path]);

// Добавление в конфигурацию
let mut index_info = IndexInfo::new();
index_info.add_index(index_def);

// Проверка статуса
assert_eq!(index_info.status(), IndexStatus::Pending);

// Создание ключа для поиска
let key = IndexRecordKey::new(true, vec![vec![42]])
    .with_values(&[&"user@example.com"]);
```
