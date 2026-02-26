# Batch Query System

**Batch** — единая точка входа для всех запросов к S.H.A.M.I.R. Database.

## Почему Batch?

Batch API предоставляет унифицированный интерфейс для выполнения запросов:

| Подход | Описание |
|--------|----------|
| **Один запрос** | `queries: { "q": { "from": "table" } }` |
| **Несколько запросов** | Map с автоматическим параллелизмом |
| **С зависимостями** | Ссылки на результаты через `$query` |

### Преимущества

1. **Единый формат** — все запросы через JSON
2. **Автоматический параллелизм** — независимые запросы выполняются одновременно
3. **Ссылки на результаты** — используй результаты одного запроса в другом
4. **Валидация** — проверка зависимостей, циклов, лимитов
5. **Транзакции** — опциональная MVCC изоляция

## Quick Start

### Простой запрос

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

### Запросы с зависимостями

```json
{
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": "id", "value": 123 }
    },
    "orders": {
      "from": "orders",
      "where": {
        "op": "eq",
        "field": "user_id",
        "value": { "$query": "user[0].id" }
      }
    },
    "items": {
      "from": "order_items",
      "where": {
        "op": "in",
        "field": "order_id",
        "values": [{ "$query": "orders[].id" }]
      }
    }
  }
}
```

## Формат запроса

### BatchRequest

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `queries` | `Map<String, QueryEntry>` | ✅ | Map алиас → запрос |
| `name` | `string` | ❌ | Имя для логирования |
| `transactional` | `boolean` | ❌ | MVCC транзакция (default: `false`) |
| `return_all` | `boolean` | ❌ | Вернуть все результаты (default: `true`) |
| `return_only` | `string[]` | ❌ | Только указанные алиасы |
| `limits` | `BatchLimits` | ❌ | Лимиты безопасности |

### QueryEntry

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `query` | `Query` | ✅ | Запрос к БД (или сам Query как значение) |
| `return_result` | `boolean` | ❌ | Включить в ответ (default: `true`) |

**Примечание:** Ключ map — это алиас запроса, используемый в `$query` ссылках.

### Два формата QueryEntry

```json
{
  "queries": {
    "users": { "from": "users" },
    "orders": {
      "query": { "from": "orders" },
      "return_result": false
    }
  }
}
```

### Query

| Поле | Тип | Описание |
|------|-----|----------|
| `from` | `string` | Имя таблицы |
| `where` | `Filter` | Условие фильтрации |
| `select` | `string[]` | Поля для выборки |
| `order_by` | `string` | Поле сортировки |
| `order_dir` | `"asc" \| "desc"` | Направление |
| `limit` | `number` | Лимит записей |
| `offset` | `number` | Смещение |

## Ссылки на результаты ($query)

### Синтаксис

```
@alias           — весь массив результатов
@alias[n]        — n-й элемент (0-based)
@alias[]         — все элементы (для извлечения)
@alias.field     — поле из первой записи
@alias[n].field  — поле из n-й записи
@alias[].field   — массив значений поля
@alias.count     — количество записей
@alias.length    — синоним для count
```

### Примеры

```json
// Весь результат
{ "$query": "@users" }

// Первый пользователь
{ "$query": "@users[0]" }

// ID первого пользователя
{ "$query": "@users[0].id" }

// Массив всех ID
{ "$query": "@users[].id" }

// Количество пользователей
{ "$query": "@users.count" }

// С вложенным путём
{ "$query": "@users[0].address.city" }
```

### Альтернативный формат

```json
{
  "$query": "users",
  "path": "[0].id"
}
```

## Фильтры (Filter)

### Операторы сравнения

```json
{ "op": "eq", "field": "status", "value": "active" }
{ "op": "ne", "field": "status", "value": "deleted" }
{ "op": "gt", "field": "age", "value": 18 }
{ "op": "gte", "field": "age", "value": 18 }
{ "op": "lt", "field": "price", "value": 100 }
{ "op": "lte", "field": "price", "value": 100 }
```

### Операторы массивов

```json
{ "op": "in", "field": "status", "values": ["active", "pending"] }
{ "op": "not_in", "field": "status", "values": ["deleted"] }
{ "op": "contains", "field": "tags", "value": "rust" }
{ "op": "contains_any", "field": "tags", "values": ["rust", "go"] }
{ "op": "contains_all", "field": "tags", "values": ["rust", "async"] }
```

### Операторы соответствия

```json
{ "op": "like", "field": "name", "pattern": "%john%" }
{ "op": "i_like", "field": "email", "pattern": "%@gmail.com" }
{ "op": "regex", "field": "phone", "pattern": "^\\+7" }
```

### Null-проверки

```json
{ "op": "is_null", "field": "deleted_at" }
{ "op": "is_not_null", "field": "email" }
```

### Логические операторы

```json
{
  "op": "and",
  "filters": [
    { "op": "eq", "field": "status", "value": "active" },
    { "op": "gt", "field": "age", "value": 18 }
  ]
}
```

```json
{
  "op": "or",
  "filters": [
    { "op": "eq", "field": "role", "value": "admin" },
    { "op": "eq", "field": "role", "value": "moderator" }
  ]
}
```

```json
{
  "op": "not",
  "filter": { "op": "eq", "field": "banned", "value": true }
}
```

### Диапазон

```json
{ "op": "between", "field": "price", "from": 10, "to": 100 }
```

### Сокращённая форма

```json
{ "op": "field", "field": "user_id", "value": { "$query": "users[0].id" } }
```

## План выполнения (BatchPlan)

Планировщик автоматически:

1. **Извлекает зависимости** — сканирует все `$query` ссылки
2. **Валидирует** — проверяет неизвестные алиасы, циклы
3. **Вычисляет глубину** — контролирует цепочку зависимостей
4. **Топологическая сортировка** — группирует в параллельные стадии

### Пример

```
Запросы: { users, products, orders, stats }

Зависимости:
  users    -> {}
  products -> {}
  orders   -> {users, products}
  stats    -> {orders}

Стадии: [[users, products], [orders], [stats]]
```

Стадия 1: `users` и `products` параллельно
Стадия 2: `orders` после завершения стадии 1
Стадия 3: `stats` после завершения стадии 2

## Лимиты безопасности (BatchLimits)

| Параметр | Default | Описание |
|----------|---------|----------|
| `max_queries` | 50 | Максимум запросов в батче |
| `max_dependency_depth` | 10 | Максимальная глубина зависимостей |
| `max_execution_time_secs` | 30 | Таймаут выполнения |
| `max_result_size` | 10MB | Максимальный размер результата |

```json
{
  "limits": {
    "max_queries": 20,
    "max_dependency_depth": 5,
    "max_execution_time_secs": 10,
    "max_result_size": 1000000
  }
}
```

## Ошибки (BatchError)

| Ошибка | Описание |
|--------|----------|
| `TooManyQueries` | Превышен `max_queries` |
| `UnknownAlias` | Ссылка на несуществующий алиас |
| `CircularDependency` | Циклическая зависимость |
| `TooDeep` | Превышена `max_dependency_depth` |
| `Timeout` | Превышено время выполнения |
| `QueryError` | Ошибка выполнения запроса |
| `LockTimeout` | Таймаут блокировки |

## Примеры использования

### Пагинация

```json
{
  "queries": {
    "products": {
      "from": "products",
      "where": { "op": "eq", "field": "category", "value": "electronics" },
      "order_by": "created_at",
      "order_dir": "desc",
      "limit": 20,
      "offset": 40
    }
  }
}
```

### E-commerce Dashboard

```json
{
  "name": "dashboard",
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": "id", "value": 123 }
    },
    "recent_orders": {
      "from": "orders",
      "where": {
        "op": "and",
        "filters": [
          { "op": "eq", "field": "user_id", "value": { "$query": "user[0].id" } },
          { "op": "gte", "field": "created_at", "value": "2024-01-01" }
        ]
      },
      "order_by": "created_at",
      "order_dir": "desc",
      "limit": 10
    },
    "order_count": {
      "from": "orders",
      "where": { "op": "eq", "field": "user_id", "value": { "$query": "user[0].id" } },
      "select": ["count(*)"]
    },
    "notifications": {
      "query": {
        "from": "notifications",
        "where": {
          "op": "and",
          "filters": [
            { "op": "eq", "field": "user_id", "value": { "$query": "user[0].id" } },
            { "op": "eq", "field": "read", "value": false }
          ]
        }
      },
      "return_result": false
    }
  },
  "return_only": ["user", "recent_orders", "order_count"]
}
```

### Транзакционный батч

```json
{
  "name": "transfer",
  "transactional": true,
  "queries": {
    "from_account": {
      "from": "accounts",
      "where": { "op": "eq", "field": "id", "value": 1 }
    },
    "to_account": {
      "from": "accounts",
      "where": { "op": "eq", "field": "id", "value": 2 }
    },
    "check_balance": {
      "from": "accounts",
      "where": {
        "op": "and",
        "filters": [
          { "op": "eq", "field": "id", "value": 1 },
          { "op": "gte", "field": "balance", "value": { "$query": "from_account[0].balance" } }
        ]
      }
    }
  }
}
```

## Типы данных

### FilterValue

| Тип | JSON пример |
|-----|-------------|
| `null` | `null` |
| `bool` | `true`, `false` |
| `int` | `42`, `-100` |
| `float` | `3.14`, `-0.5` |
| `string` | `"hello"` |
| `binary` | `[1, 2, 3]` (base64 в JSON) |
| `array` | `[1, 2, 3]` |
| `field_ref` | `{ "$ref": "other_field" }` |
| `query_ref` | `{ "$query": "@users[0].id" }` |

## Архитектура

```
BatchRequest
    │
    ▼
┌─────────────────┐
│  BatchPlanner   │
│  ┌───────────┐  │
│  │ Parse $query │──▶ Extract dependencies
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
│  ┌───────────┐  │
│  │ Stage 1   │──▶ Parallel execution
│  └───────────┘  │
│  ┌───────────┐  │
│  │ Stage 2   │──▶ Wait for deps, then parallel
│  └───────────┘  │
│  ...            │
└─────────────────┘
    │
    ▼
BatchResponse { results, execution_plan, execution_time_us }
```

## См. также

- [Query Reference](./reference.rs) — парсинг `$query` ссылок
- [Batch Types](./types.rs) — типы данных
- [Batch Planner](./planner.rs) — планировщик выполнения
