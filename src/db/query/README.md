# Query System

Единая система запросов S.H.A.M.I.R. Database.

## Модули

| Модуль | Описание |
|--------|----------|
| [read](./read/) | SELECT запросы (Query, ReadQuery) |
| [write](./write/) | Операции записи (Insert, Update, Set, Delete) |
| [batch](./batch/) | Batch API — выполнение нескольких запросов |
| [filter](./filter/) | WHERE условия и фильтры |
| [common](./common/) | Общие утилиты парсинга |

## Основные типы

### Read Operations

```rust
pub use read::{
    Query,        // Основной тип SELECT запроса
    ReadQuery,    // Alias для Query (для ясности API)
    Select,       // SELECT clause
    GroupBy,      // GROUP BY
    OrderBy,      // ORDER BY
    LimitOffset,  // Пагинация
    QueryResult,  // Результат запроса
    QueryStats,   // Статистика выполнения
};
```

### Write Operations

```rust
pub use write::{
    InsertOp,     // INSERT INTO
    UpdateOp,     // UPDATE ... SET
    SetOp,        // UPSERT по ключу
    DeleteOp,     // DELETE FROM
    UpdateSelect, // Возврат обновлённых записей
};
```

### Batch API

```rust
pub use batch::{
    BatchOp,       // Enum: Read, Insert, Update, Set, Delete
    BatchRequest,  // Запрос с несколькими операциями
    BatchResponse, // Результат выполнения
    BatchPlan,     // План выполнения (стадии)
    BatchPlanner,  // Планировщик
    BatchError,    // Ошибки
    QueryEntry,    // Запись в батче (операция + return_result)
    QueryReference,// Ссылка $query
};
```

### Filters

```rust
pub use filter::{
    Filter,       // WHERE условие
    FilterValue,  // Значение в фильтре
    FieldPath,    // Путь к полю (nested)
};
```

## BatchOp Enum

`BatchOp` — универсальный тип для всех операций:

```rust
pub enum BatchOp {
    Read(Query),      // SELECT (определяется по полю "from")
    Insert(InsertOp), // INSERT (определяется по "insert_into")
    Update(UpdateOp), // UPDATE (определяется по "update")
    Set(SetOp),       // UPSERT (определяется по "set")
    Delete(DeleteOp), // DELETE (определяется по "delete_from")
}
```

### Автоматическое определение типа

Serde автоматически определяет тип операции по уникальным полям:

```json
{ "from": "users" }                          // → Read
{ "insert_into": "users", "values": [...] }  // → Insert
{ "update": "users", "set": {...} }          // → Update
{ "set": "users", "key": {...}, "value": {...} } // → Set
{ "delete_from": "users", "where": {...} }   // → Delete
```

## Quick Examples

### Single Read Query

```json
{
  "queries": {
    "users": {
      "from": "users",
      "where": { "op": "eq", "field": "status", "value": "active" },
      "limit": 10
    }
  }
}
```

### Mixed Batch (Read + Write)

```json
{
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": "id", "value": 1 }
    },
    "update_orders": {
      "update": "orders",
      "where": { "op": "eq", "field": "user_id", "value": { "$query": "user[0].id" } },
      "set": { "status": "processed" }
    }
  }
}
```

### Insert with Reference

```json
{
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": "id", "value": 1 }
    },
    "new_order": {
      "insert_into": "orders",
      "values": [{
        "user_id": { "$query": "user[0].id" },
        "status": "pending",
        "created_at": "2024-01-15"
      }]
    }
  }
}
```

## Архитектура

```
BatchRequest
    │
    ▼
┌─────────────────┐
│  BatchPlanner   │
│  ┌───────────┐  │
│  │ Parse     │──▶ Extract dependencies from $query
│  └───────────┘  │
│  ┌───────────┐  │
│  │ Validate  │──▶ Check aliases, cycles, depth
│  └───────────┘  │
│  ┌───────────┐  │
│  │ Topo Sort │──▶ Create parallel stages
│  └───────────┘  │
└─────────────────┘
    │
    ▼
BatchPlan { stages, aliases, dependencies }
    │
    ▼
┌─────────────────┐
│   Executor      │
│  Stage 1: parallel execution
│  Stage 2: wait for deps, then parallel
│  ...
└─────────────────┘
    │
    ▼
BatchResponse { results, execution_plan, execution_time_us }
```

## См. также

- [Batch README](./batch/README.md) — полная документация Batch API
- [Write README](./write/README.md) — операции записи
- [Examples](./examples/) — примеры JSON
