# Write Operations

Модуль операций записи для S.H.A.M.I.R. Database.

## Операции

| Операция | Ключевое поле | Описание |
|----------|---------------|----------|
| **Insert** | `insert_into` | Вставка новых записей |
| **Update** | `update` | Обновление записей по фильтру |
| **Set** | `set` | Upsert по ключу (создание или обновление) |
| **Delete** | `delete_from` | Удаление записей по фильтру |

## Insert — Вставка записей

Вставляет одну или несколько записей в таблицу.

### JSON Формат

```json
{
  "insert_into": "users",
  "values": [
    { "name": "Alice", "email": "alice@example.com" },
    { "name": "Bob", "email": "bob@example.com" }
  ]
}
```

### Поля

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `insert_into` | `string` | ✅ | Имя таблицы |
| `values` | `Value[]` | ✅ | Массив записей для вставки |

### Примеры

**Одна запись:**
```json
{
  "insert_into": "users",
  "values": [{ "name": "Alice" }]
}
```

**Несколько записей:**
```json
{
  "insert_into": "products",
  "values": [
    { "name": "Product A", "price": 100 },
    { "name": "Product B", "price": 200 },
    { "name": "Product C", "price": 300 }
  ]
}
```

**Вложенные данные:**
```json
{
  "insert_into": "users",
  "values": [{
    "name": "Alice",
    "profile": {
      "age": 30,
      "city": "Moscow"
    },
    "tags": ["admin", "developer"]
  }]
}
```

---

## Update — Обновление записей

Обновляет записи, соответствующие фильтру.

### JSON Формат

```json
{
  "update": "users",
  "where": { "op": "eq", "field": "status", "value": "pending" },
  "set": { "status": "active", "updated_at": "2024-01-15" }
}
```

### Поля

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `update` | `string` | ✅ | Имя таблицы |
| `where` | `Filter` | ❌ | Условие фильтрации (все записи если опущено) |
| `set` | `Value` | ✅ | Поля для обновления |
| `select` | `UpdateSelect` | ❌ | Вернуть обновлённые записи |

### UpdateSelect — Возврат обновлённых записей

Опциональное поле `select` позволяет вернуть записи, которые были обновлены:

```json
{
  "update": "users",
  "where": { "op": "eq", "field": "status", "value": "inactive" },
  "set": { "status": "active" },
  "select": {
    "return_mode": "changed",
    "fields": ["id", "name", "status"]
  }
}
```

#### UpdateReturnMode — Режим возврата

| Режим | Описание |
|-------|----------|
| `"all"` | Все записи, попавшие под фильтр |
| `"changed"` | Только записи с фактическими изменениями (default) |
| `"unchanged"` | Записи, попавшие под фильтр, но данные не изменились |

#### Поля UpdateSelect

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `return_mode` | `UpdateReturnMode` | ❌ | Режим возврата (default: `"changed"`) |
| `fields` | `string[]` | ❌ | Список полей для возврата (все если опущено) |

### Примеры Update

**Обновление без фильтра (все записи):**
```json
{
  "update": "products",
  "set": { "currency": "USD" }
}
```

**Обновление с фильтром:**
```json
{
  "update": "users",
  "where": { "op": "eq", "field": "status", "value": "pending" },
  "set": { "status": "active" }
}
```

**Возврат всех обновлённых записей:**
```json
{
  "update": "orders",
  "where": { "op": "eq", "field": "processed", "value": false },
  "set": { "processed": true },
  "select": {
    "return_mode": "all"
  }
}
```

**Возврат только изменённых записей с выборкой полей:**
```json
{
  "update": "users",
  "where": { "op": "gte", "field": "login_count", "value": 100 },
  "set": { "is_vip": true },
  "select": {
    "return_mode": "changed",
    "fields": ["id", "name", "is_vip"]
  }
}
```

**Возврат записей, которые не изменились:**
```json
{
  "update": "products",
  "where": { "op": "eq", "field": "category", "value": "electronics" },
  "set": { "category": "electronics" },
  "select": {
    "return_mode": "unchanged",
    "fields": ["id", "name"]
  }
}
```

**Сложный фильтр:**
```json
{
  "update": "orders",
  "where": {
    "op": "and",
    "filters": [
      { "op": "eq", "field": "status", "value": "pending" },
      { "op": "lt", "field": "created_at", "value": "2024-01-01" }
    ]
  },
  "set": { "status": "expired" }
}
```

---

## Set — Upsert по ключу

Обновляет запись если существует, создаёт если нет. Работает с первичным ключом (`id`) или уникальными полями.

### JSON Формат

```json
{
  "set": "users",
  "key": { "id": 1 },
  "value": { "name": "Alice Updated", "email": "alice@new.com" }
}
```

### Поля

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `set` | `string` | ✅ | Имя таблицы |
| `key` | `Value` | ✅ | Ключ для поиска (id или уникальное поле) |
| `value` | `Value` | ✅ | Значения для установки |

### Примеры Set

**По первичному ключу (id):**
```json
{
  "set": "users",
  "key": { "id": 1 },
  "value": { "name": "Alice Updated" }
}
```

**По уникальному полю (email):**
```json
{
  "set": "users",
  "key": { "email": "alice@example.com" },
  "value": { "name": "Alice", "status": "active" }
}
```

**По составному ключу:**
```json
{
  "set": "order_items",
  "key": { "order_id": 100, "product_id": 500 },
  "value": { "quantity": 5, "price": 99.99 }
}
```

---

## Delete — Удаление записей

Удаляет записи по фильтру. **Фильтр обязателен** для безопасности.

### JSON Формат

```json
{
  "delete_from": "users",
  "where": { "op": "eq", "field": "status", "value": "inactive" }
}
```

### Поля

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `delete_from` | `string` | ✅ | Имя таблицы |
| `where` | `Filter` | ✅ | Условие фильтрации (обязательно!) |

### Примеры Delete

**Простой фильтр:**
```json
{
  "delete_from": "sessions",
  "where": { "op": "lt", "field": "expires_at", "value": "2024-01-01" }
}
```

**Удаление по ID:**
```json
{
  "delete_from": "users",
  "where": { "op": "eq", "field": "id", "value": 123 }
}
```

**Сложный фильтр:**
```json
{
  "delete_from": "logs",
  "where": {
    "op": "and",
    "filters": [
      { "op": "lt", "field": "created_at", "value": "2023-01-01" },
      { "op": "eq", "field": "archived", "value": true }
    ]
  }
}
```

**С использованием IN:**
```json
{
  "delete_from": "products",
  "where": {
    "op": "in",
    "field": "category_id",
    "values": [10, 20, 30]
  }
}
```

---

## Использование в Batch API

Все операции записи могут использоваться в Batch API вместе с операциями чтения:

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
      "set": { "status": "processed" },
      "select": {
        "return_mode": "changed",
        "fields": ["id", "status"]
      }
    },
    "cleanup": {
      "delete_from": "temp_data",
      "where": { "op": "lt", "field": "expires_at", "value": "2024-01-01" }
    }
  }
}
```

## Автоматическое определение типа

Serde автоматически определяет тип операции по уникальным полям:

| Ключевое поле | Операция |
|---------------|----------|
| `from` | Query (чтение) |
| `insert_into` | Insert (вставка) |
| `update` | Update (обновление) |
| `set` | Set (upsert) |
| `delete_from` | Delete (удаление) |

## См. также

- [Batch Query System](../batch/README.md) — Batch API документация
- [Filter Examples](../examples/filter.md) — Примеры фильтров
- [Types](./types.rs) — Rust типы операций
