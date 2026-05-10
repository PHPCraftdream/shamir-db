# Index System

## Обзор

Модуль `index` реализует систему индексов для быстрого поиска записей.
Поддерживает два типа индексов: обычные (regular) и уникальные (unique).
Индексы hash-based, оптимизированы для операций `Eq` и `In`.

## IndexManager

Центральный компонент — менеджер индексов одной таблицы.

```rust
pub struct IndexManager {
    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,
    indexes: Arc<IndexInfo>,           // обычные индексы
    indexes_unique: Arc<IndexInfo>,    // уникальные индексы
    has_indexes: Arc<AtomicBool>,      // O(1) проверка наличия
    has_indexes_unique: Arc<AtomicBool>,
}
```

### Оптимизация: атомарные флаги

`has_indexes` / `has_indexes_unique` — `AtomicBool` для мгновенной проверки
наличия индексов без блокировок. Когда индексов нет (типичный случай для
многих таблиц), все проверки пропускаются за O(1).

## Ключевые типы

### IndexDefinition

Определение индекса: имя (interned u64) + список путей.

```rust
pub struct IndexDefinition {
    pub name_interned: u64,
    pub paths: Vec<IndexInfoItem>,
}
```

### IndexInfoItem

Один путь индекса (interned): `["user", "email"]` → `vec![2, 5]` (u64).

```rust
pub struct IndexInfoItem {
    pub path: Vec<u64>,
}
```

### IndexRecordKey

Ключ записи в B-Tree индекса (25 байт: `is_unique` + `name_interned` +
двойной FxHash значений). Двойной хеш — защита от коллизий; при попадании
на коллизию данные обязательно перепроверяются по фактическому значению.

```rust
pub struct IndexRecordKey {
    pub is_unique: u8,        // 1 = unique, 0 = regular
    pub name_interned: u64,   // interned ID имени индекса
    pub hash1: u64,           // FxHash значений
    pub hash2: u64,           // FxHash с другим seed (XOR name_interned)
}
```

### IndexInfo

Контейнер для определений индексов. Использует `DashMap` — thread-safe
без дополнительной синхронизации.

### IndexStatus

Статус синхронизации индекса:

| Статус | Описание |
|--------|----------|
| `Actual` | Индекс актуален |
| `Pending` | Ожидает построения |
| `Saving` | Сохраняется на диск |

## Event-обработчики

IndexManager реагирует на изменения данных:

| Метод | Когда вызывается |
|-------|-----------------|
| `on_record_created(id, value)` | После insert |
| `on_record_updated(id, old, new)` | После update |
| `on_record_deleted(id, value)` | После delete |

При уникальном индексе: валидация **до** записи (BEFORE write),
обновление индекса **после** записи (AFTER write).

## Использование в read-запросах

При выполнении `ReadQuery` с `WHERE: Eq` или `WHERE: In`:

1. `TableManager` проверяет наличие индекса на поле
2. Если индекс есть — index scan (O(1) lookup по hash)
3. Если нет — full table scan

Информация об использованном индексе возвращается в `QueryStats::index_used`.

## Файлы

| Файл | Описание |
|------|----------|
| `index_manager.rs` | `IndexManager` — основная логика |
| `index_definition.rs` | `IndexDefinition` — определение индекса |
| `index_info.rs` | `IndexInfo` — контейнер определений (DashMap) |
| `index_info_item.rs` | `IndexInfoItem` — один путь индекса |
| `index_record_key.rs` | `IndexRecordKey` — ключ в B-Tree |
| `index_status.rs` | `IndexStatus` — статус синхронизации |
| `index_types.md` | Подробная документация типов индексов |
