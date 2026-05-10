# Table Layer

## Обзор

Модуль `table` реализует табличный слой ShamirDB. Разделён на два уровня:
- `Table` — низкоуровневая работа с InnerValue и bytes (CRUD + streaming)
- `TableManager` — высокоуровневый фасад с индексами, интернером и query pipeline

## Архитектура

```
TableManager (фасад)
  ├── Table (low-level CRUD)
  ├── InternerManager (string ↔ u64)
  ├── RecordCounter (подсчёт записей)
  └── IndexManager (regular + unique индексы)
```

Каждая таблица использует два хранилища:
```
__data__{table_name}  → данные (InnerValue с u64 ключами)
__info__{table_name}  → interner state + метаданные индексов
```

## Table (low-level)

```rust
pub struct Table {
    data_store: Arc<dyn Store>,
}
```

Операции: `get`, `set`, `remove`, `list_stream(batch_size)`.
Работает напрямую с `InnerValue` и `RecordId`.
`list_stream` — async streaming для обхода всех записей без OOM.

## TableManager (high-level)

```rust
pub struct TableManager {
    name: String,
    table: Arc<Table>,
    interner: InternerManager,
    counter: Arc<RecordCounter>,
    index_manager: IndexManager,
}
```

### CRUD операции

| Метод | Описание |
|-------|----------|
| `execute_insert(&InsertOp)` | Вставка (JSON → InnerValue → store) |
| `execute_update(&UpdateOp, &FilterContext)` | Обновление по фильтру |
| `execute_delete(&DeleteOp, &FilterContext)` | Удаление по фильтру |
| `execute_set(&SetOp)` | Upsert по ключу |
| `read(&ReadQuery, &FilterContext)` | SELECT с полным pipeline |

### Индексы

| Метод | Описание |
|-------|----------|
| `create_index(name, paths)` | Создать обычный индекс |
| `create_unique_index(name, paths)` | Создать уникальный индекс |
| `drop_index(name)` | Удалить обычный индекс |
| `drop_unique_index(name)` | Удалить уникальный индекс |
| `lookup_by_index(name, values)` | Поиск по индексу |

### Доступ к компонентам

| Метод | Описание |
|-------|----------|
| `interner()` | `&InternerManager` |
| `index_manager_ref()` | `&IndexManager` |
| `name()` | Имя таблицы |

## InternerManager

Ленивая загрузка interner из `__info__` store через `OnceCell`:

```rust
pub struct InternerManager {
    info_store: Arc<dyn Store>,
    interner: OnceCell<Interner>,
}
```

| Метод | Описание |
|-------|----------|
| `get()` | Получить (загрузить при первом вызове) |
| `persist()` | Сохранить на диск |

## RecordCounter

Подсчёт записей с mutex для атомарности:

```rust
pub struct RecordCounter {
    info_store: Arc<dyn Store>,
    counter_mutex: Mutex<()>,
}
```

## Разделение read/write

Логика выполнения вынесена в отдельные файлы:
- `read_exec.rs` — pipeline чтения (index scan → filter → select → order → pagination)
- `write_exec.rs` — логика записи (insert/update/delete/set + index update + interner persist)

## Файлы

| Файл | Описание |
|------|----------|
| `table.rs` | `Table` — low-level CRUD |
| `table_manager.rs` | `TableManager` — фасад |
| `table_config.rs` | `TableConfig` |
| `interner_manager.rs` | `InternerManager` — lazy interner |
| `record_counter.rs` | `RecordCounter` |
| `read_exec.rs` | Pipeline чтения |
| `write_exec.rs` | Логика записи |

## Thread Safety

Все компоненты thread-safe. Клонирование дешёвое (Arc).
Concurrent reads и writes поддерживаются. Interner синхронизирован через DashMap.
