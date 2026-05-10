# Write Operations

## Обзор

Модуль `write` определяет типы операций записи: insert, update, delete, set (upsert).
Каждая операция — отдельная структура, десериализуемая из JSON batch запроса.
Выполнение происходит через методы `TableManager` (`execute_insert`, `execute_update`,
`execute_delete`, `execute_set`).

## Типы операций

### InsertOp

Вставка новых записей в таблицу.

```rust
pub struct InsertOp {
    pub insert_into: TableRef,  // таблица
    pub values: Vec<Value>,     // массив записей
}
```

```json
{"insert_into": "users", "values": [{"name": "Alice"}, {"name": "Bob"}]}
```

Возвращает вставленные записи с присвоенным `_id`.

### UpdateOp

Обновление записей по фильтру.

```rust
pub struct UpdateOp {
    pub update: TableRef,
    pub where_clause: Option<Filter>,  // все записи если None
    pub set: Value,                     // поля для обновления
    pub select: Option<UpdateSelect>,   // какие записи вернуть
}
```

```json
{
  "update": "users",
  "where": {"op": "eq", "field": ["status"], "value": "inactive"},
  "set": {"status": "active"},
  "select": {"return_mode": "changed"}
}
```

### UpdateSelect и UpdateReturnMode

| return_mode | Описание |
|-------------|----------|
| `changed` (default) | Только фактически изменённые записи |
| `unchanged` | Совпавшие по фильтру, но не изменённые |
| `all` | Все совпавшие записи |

### DeleteOp

Удаление записей по фильтру. Фильтр **обязателен** (защита от случайного удаления).

```rust
pub struct DeleteOp {
    pub delete_from: TableRef,
    pub where_clause: Filter,  // обязательно!
}
```

```json
{"delete_from": "users", "where": {"op": "eq", "field": ["status"], "value": "deleted"}}
```

### SetOp (Upsert)

Upsert по ключу: обновляет если запись существует, вставляет если нет.

```rust
pub struct SetOp {
    pub set: TableRef,
    pub key: Value,    // ключ для поиска
    pub value: Value,  // значения для установки
}
```

```json
{
  "set": "users",
  "key": {"email": "alice@example.com"},
  "value": {"email": "alice@example.com", "name": "Alice", "status": "active"}
}
```

При update — merge `value` в существующую запись. Результат содержит `_created: true/false`.

## WriteResult

```rust
pub struct WriteResult {
    pub affected: u64,             // количество затронутых записей
    pub records: Vec<Value>,       // возвращённые записи
    pub execution_time_us: u64,    // время выполнения (мкс)
}
```

## Выполнение

Все write операции выполняются через `TableManager`:

1. Интернирование ключей/значений через `InternerManager`
2. Компиляция фильтра (для update/delete) через `compile_filter()`
3. Модификация записей в storage
4. Обновление индексов (`on_record_created/updated/deleted`)
5. Автоматический persist interner после записи

Реализация разделена между `table_manager.rs` (публичный API) и `write_exec.rs`
(логика выполнения).

## Файлы

| Файл | Описание |
|------|----------|
| `types.rs` | `InsertOp`, `UpdateOp`, `SetOp`, `DeleteOp`, `UpdateSelect`, `UpdateReturnMode` |
| `write_result.rs` | `WriteResult` |
| `mod.rs` | Re-exports |
