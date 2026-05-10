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

**Важно:** Поле `id` обязательно в каждом `BatchRequest`. Поля `field` в фильтрах — это массивы строк (не точечные пути).

```json
{
  "id": "req-001",
  "queries": {
    "users": {
      "from": "users",
      "where": { "op": "eq", "field": ["status"], "value": "active" },
      "pagination": { "mode": "LimitOffset", "limit": 10 }
    }
  }
}
```

### Таблица из другого репо

Поле `from` может быть строкой (repo по умолчанию `"main"`) или массивом `["repo", "table"]`:

```json
{
  "id": 1,
  "queries": {
    "sessions": {
      "from": ["hot", "sessions"],
      "where": { "op": "eq", "field": ["user_id"], "value": 123 }
    }
  }
}
```

### Запросы с зависимостями

```json
{
  "id": 2,
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": ["id"], "value": 123 }
    },
    "orders": {
      "from": "orders",
      "where": {
        "op": "eq",
        "field": ["user_id"],
        "value": { "$query": "user[0].id" }
      }
    },
    "items": {
      "from": "order_items",
      "where": {
        "op": "in",
        "field": ["order_id"],
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
| `id` | `any` | ✅ | ID запроса (эхо в ответе для корреляции) |
| `queries` | `Map<String, QueryEntry>` | ✅ | Map алиас → запрос |
| `name` | `string` | ❌ | Имя для логирования |
| `transactional` | `boolean` | ❌ | MVCC транзакция (default: `false`) |
| `return_all` | `boolean` | ❌ | Вернуть все результаты (default: `true`) |
| `return_only` | `string[]` | ❌ | Только указанные алиасы |
| `limits` | `BatchLimits` | ❌ | Лимиты безопасности |

### QueryEntry

Операция `BatchOp` встраивается напрямую (через `#[serde(flatten)]`). Тип операции определяется автоматически по уникальному ключу JSON.

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| *(BatchOp fields)* | BatchOp | ✅ | Операция (flatten, определяется по ключу) |
| `return_result` | `boolean` | ❌ | Включить в ответ (default: `true`) |

**Примечание:** Ключ map — это алиас запроса, используемый в `$query` ссылках.

### Формат QueryEntry

```json
{
  "id": 1,
  "queries": {
    "users": { "from": "users" },
    "orders": {
      "from": "orders",
      "return_result": false
    },
    "new_user": {
      "insert_into": "users",
      "values": [{ "name": "Alice" }]
    }
  }
}
```

### ReadQuery

| Поле | Тип | Описание |
|------|-----|----------|
| `from` | `string` или `["repo", "table"]` | Ссылка на таблицу (`TableRef`) |
| `where` | `Filter` | Условие фильтрации |
| `select` | `Select` | Выборка полей (объект, не массив строк) |
| `group_by` | `GroupBy` | Группировка |
| `order_by` | `OrderBy` | Сортировка |
| `pagination` | `Pagination` | Пагинация: `LimitOffset`, `Page`, или `None` |
| `count_total` | `boolean` | Подсчитать общее кол-во записей (default: `false`) |

**Важно:** Поля в фильтрах (`field`) — это массивы строк: `["user", "address", "city"]`, не точечные пути `"user.address.city"`.

**Пагинация:**
```json
// Limit/Offset
"pagination": { "mode": "LimitOffset", "limit": 10, "offset": 20 }

// Page-based (1-based page number)
"pagination": { "mode": "Page", "page": 3, "page_size": 10 }
```

## Операции записи (Write Operations)

Batch API поддерживает операции записи: `insert`, `update`, `set`, `delete`.

### BatchOp — именование операций

`BatchOp` использует **явный key-based dispatch** (не `#[serde(untagged)]`). Десериализация проверяет наличие уникального ключа в JSON-объекте:

| BatchOp Variant | JSON ключ | Описание |
|-----------------|-----------|----------|
| `Read(ReadQuery)` | `from` | SELECT запрос |
| `Insert(InsertOp)` | `insert_into` | Вставка записей |
| `Update(UpdateOp)` | `update` | Обновление записей |
| `Set(SetOp)` | `set` | Upsert по ключу (полностью работает) |
| `Delete(DeleteOp)` | `delete_from` | Удаление записей |
| `CreateDb(CreateDbOp)` | `create_db` | Создание базы данных |
| `DropDb(DropDbOp)` | `drop_db` | Удаление базы данных |
| `CreateRepo(CreateRepoOp)` | `create_repo` | Создание репозитория |
| `DropRepo(DropRepoOp)` | `drop_repo` | Удаление репозитория |
| `CreateTable(CreateTableOp)` | `create_table` | Создание таблицы |
| `DropTable(DropTableOp)` | `drop_table` | Удаление таблицы |
| `CreateIndex(CreateIndexOp)` | `create_index` | Создание индекса |
| `DropIndex(DropIndexOp)` | `drop_index` | Удаление индекса |
| `List(ListOp)` | `list` | Список (databases/repos/tables/indexes) |

**Порядок проверки:** `from` -> `insert_into` -> `update` -> `delete_from` -> admin ops -> `set` (последний, т.к. `UpdateOp` тоже имеет поле `set`).

**Методы:**
- `table_ref()` -> `Option<&TableRef>` — для data-операций (Read/Insert/Update/Set/Delete)
- `is_admin()` -> `bool` — true для DDL-операций

**Выполнение admin-операций** происходит через трейт `AdminExecutor`:
```rust
#[async_trait]
pub trait AdminExecutor: Send + Sync {
    async fn execute_admin(&self, op: &BatchOp) -> Result<QueryResult, BatchError>;
}
```

### Insert — вставка записей

```json
{
  "id": 1,
  "queries": {
    "new_user": {
      "insert_into": "users",
      "values": [
        { "name": "Alice", "email": "alice@example.com" },
        { "name": "Bob", "email": "bob@example.com" }
      ]
    }
  }
}
```

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `insert_into` | `TableRef` | ✅ | Ссылка на таблицу (`"table"` или `["repo", "table"]`) |
| `values` | `Value[]` | ✅ | Массив записей для вставки |

### Update — обновление записей

Обновляет записи, соответствующие фильтру. Если фильтр не указан — обновляет все записи.

```json
{
  "id": 1,
  "queries": {
    "activate_users": {
      "update": "users",
      "where": { "op": "eq", "field": ["status"], "value": "pending" },
      "set": { "status": "active", "activated_at": "2024-01-15" }
    }
  }
}
```

```json
// Обновление с использованием результата другого запроса
{
  "id": 2,
  "queries": {
    "user": { "from": "users", "where": { "op": "eq", "field": ["id"], "value": 1 } },
    "update_orders": {
      "update": "orders",
      "where": { "op": "eq", "field": ["user_id"], "value": { "$query": "user[0].id" } },
      "set": { "status": "processed" }
    }
  }
}
```

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `update` | `TableRef` | ✅ | Ссылка на таблицу (`"table"` или `["repo", "table"]`) |
| `where` | `Filter` | ❌ | Условие фильтрации (все если опущено) |
| `set` | `Value` | ✅ | Поля для обновления (частичное или полное) |
| `select` | `UpdateSelect` | ❌ | Вернуть обновлённые записи |

#### UpdateSelect — возврат обновлённых записей

Опциональное поле `select` возвращает записи, которые были обновлены:

```json
{
  "id": 1,
  "queries": {
    "update_vips": {
      "update": "users",
      "where": { "op": "gte", "field": ["login_count"], "value": 100 },
      "set": { "is_vip": true },
      "select": {
        "return_mode": "changed",
        "fields": ["id", "name", "is_vip"]
      }
    }
  }
}
```

| Поле | Тип | Описание |
|------|-----|----------|
| `return_mode` | `"all"` / `"changed"` / `"unchanged"` | Режим возврата (default: `"changed"`) |
| `fields` | `string[]` | Список полей для возврата (все если опущено) |

**Режимы возврата:**
- `"all"` — Все записи, попавшие под фильтр
- `"changed"` — Только записи с фактическими изменениями (default)
- `"unchanged"` — Записи под фильтром, но данные не изменились

### Set — upsert по ключу

Обновляет запись если существует, создаёт если нет. Работает по ключу — полностью реализовано.

```json
{
  "id": 1,
  "queries": {
    "upsert_user": {
      "set": "users",
      "key": { "id": 1 },
      "value": { "name": "Alice Updated", "email": "alice@new.com" }
    }
  }
}
```

```json
// Upsert по уникальному полю (email)
{
  "id": 2,
  "queries": {
    "upsert_by_email": {
      "set": "users",
      "key": { "email": "alice@example.com" },
      "value": { "name": "Alice", "status": "active" }
    }
  }
}
```

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `set` | `TableRef` | ✅ | Ссылка на таблицу (`"table"` или `["repo", "table"]`) |
| `key` | `Value` | ✅ | Ключ для поиска (id или уникальное поле) |
| `value` | `Value` | ✅ | Значения для установки |

### Delete — удаление записей

Удаляет записи по фильтру. **Фильтр обязателен** для безопасности.

```json
{
  "id": 1,
  "queries": {
    "delete_inactive": {
      "delete_from": "users",
      "where": { "op": "eq", "field": ["status"], "value": "inactive" }
    }
  }
}
```

```json
// Удаление с использованием результата запроса
{
  "id": 2,
  "queries": {
    "expired": {
      "from": "sessions",
      "where": { "op": "lt", "field": ["expires_at"], "value": "2024-01-01" }
    },
    "cleanup": {
      "delete_from": "sessions",
      "where": {
        "op": "in",
        "field": ["id"],
        "values": [{ "$query": "expired[].id" }]
      }
    }
  }
}
```

| Поле | Тип | Обязательно | Описание |
|------|-----|-------------|----------|
| `delete_from` | `TableRef` | ✅ | Ссылка на таблицу (`"table"` или `["repo", "table"]`) |
| `where` | `Filter` | ✅ | Условие фильтрации (обязательно!) |

### BatchOp — определение типа операции

Тип операции определяется по уникальному ключу в JSON-объекте (ручная десериализация, не `#[serde(untagged)]`):

| Ключ JSON | Операция |
|-----------|----------|
| `from` | Read (SELECT) |
| `insert_into` | Insert |
| `update` | Update |
| `delete_from` | Delete |
| `create_db` | CreateDb |
| `drop_db` | DropDb |
| `create_repo` | CreateRepo |
| `drop_repo` | DropRepo |
| `create_table` | CreateTable |
| `drop_table` | DropTable |
| `create_index` | CreateIndex |
| `drop_index` | DropIndex |
| `list` | List |
| `set` | Set (upsert) — проверяется последним |

### Admin (DDL) операции

Пример создания таблицы и индекса:

```json
{
  "id": "admin-001",
  "queries": {
    "new_table": {
      "create_table": "products",
      "repo": "main"
    },
    "new_index": {
      "create_index": "email_idx",
      "table": "users",
      "fields": [["email"]],
      "unique": true,
      "repo": "main"
    },
    "list_tables": {
      "list": "tables",
      "repo": "main"
    }
  }
}
```

Пример вывода `list`:

```json
{ "list": "databases" }
{ "list": "repos" }
{ "list": "tables", "repo": "main" }
{ "list": "indexes", "table": "users", "repo": "main" }
```

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

## Select from Alias (TODO: не реализовано)

Можно использовать результат другого запроса как источник данных.

### Синтаксис

Поле `from` начинается с `@` — это ссылка на алиас:

```json
{
  "queries": {
    "all_users": {
      "from": "users"
    },
    "active_only": {
      "from": "@all_users",
      "where": { "op": "eq", "field": ["active"], "value": true }
    }
  }
}
```

### Когда использовать

| Сценарий | Пример |
|----------|--------|
| Фильтрация подмножества | `from: "@users"` + WHERE |
| Каскадная обработка | chain → filter → transform |
| CTE-подобные запросы | WITH-like patterns |

### Зависимости

Алиас в `from` автоматически добавляется в зависимости:

```rust
// В planner.rs::extract_dependencies()
BatchOp::Read(query) => {
    // Если table начинается с '@' — это ссылка на алиас
    if query.from.table.starts_with('@') {
        let alias = &query.from.table[1..];  // убираем '@'
        deps.insert(alias.to_string());
    }
    // ... плюс существующая логика для $query в filter
}
```

**Примечание:** `from` — это `TableRef { repo, table }`. Для ссылок на алиас проверяется `from.table`.

### Примеры

#### Каскадная фильтрация

```json
{
  "queries": {
    "users": {
      "from": "users"
    },
    "active": {
      "from": "@users",
      "where": { "op": "eq", "field": ["status"], "value": "active" }
    },
    "vip": {
      "from": "@active",
      "where": { "op": "gte", "field": ["score"], "value": 100 }
    }
  }
}
```

План выполнения:
```
Stage 1: [users]        // читаем из таблицы
Stage 2: [active]       // фильтруем users
Stage 3: [vip]          // фильтруем active
```

#### Комбинирование с $query

```json
{
  "queries": {
    "orders": {
      "from": "orders",
      "where": { "op": "gte", "field": ["total"], "value": 1000 }
    },
    "order_items": {
      "from": "order_items",
      "where": {
        "op": "in",
        "field": ["order_id"],
        "values": [{ "$query": "orders[].id" }]
      }
    },
    "expensive_items": {
      "from": "@order_items",
      "where": { "op": "gt", "field": ["price"], "value": 500 }
    }
  }
}
```

## Системные функции ($fn) (TODO: не реализовано)

Системные функции вычисляются на стороне БД во время выполнения запроса.

### Синтаксис

```json
// Без аргументов
{ "$fn": "NOW" }
{ "$fn": "UUID" }

// С аргументами
{ "$fn": { "name": "COALESCE", "args": [null, "default"] } }
{ "$fn": { "name": "SUBSTRING", "args": [{ "$ref": "name" }, 0, 10] } }
```

### Категории функций

#### Дата/время

| Функция | Описание | Пример |
|---------|----------|--------|
| `NOW` | Текущее время (UTC) | `2024-01-15T10:30:00Z` |
| `TODAY` | Начало текущего дня | `2024-01-15T00:00:00Z` |
| `UNIX_TIMESTAMP` | Unix timestamp | `1705315800` |

#### Генерация

| Функция | Описание | Пример |
|---------|----------|--------|
| `UUID` | Новый UUID v4 | `"550e8400-e29b-41d4-a716-446655440000"` |
| `RANDOM` | Случайное число [0, 1) | `0.7234...` |
| `RANDOM_INT` | Случайное число [min, max] | `42` |

#### Строки

| Функция | Описание | Пример |
|---------|----------|--------|
| `LENGTH(str)` | Длина строки | `5` |
| `UPPER(str)` | В верхний регистр | `"HELLO"` |
| `LOWER(str)` | В нижний регистр | `"hello"` |
| `TRIM(str)` | Удалить пробелы | `"hello"` |
| `SUBSTRING(str, start, len)` | Подстрока | `"hel"` |

#### Логические

| Функция | Описание | Пример |
|---------|----------|--------|
| `COALESCE(a, b, ...)` | Первый не-null | `"default"` |
| `IFNULL(a, b)` | Если null, то b | `"default"` |
| `NULLIF(a, b)` | null если a == b | `null` |

#### Хеширование

| Функция | Описание | Пример |
|---------|----------|--------|
| `MD5(str)` | MD5 хеш | `"5d41402abc4b2a76b9719d911017c592"` |
| `SHA256(str)` | SHA-256 хеш | `"2c26b46b68ffc68ff99b453c1d304134..."` |

#### Математика

| Функция | Описание | Пример |
|---------|----------|--------|
| `ABS(n)` | Модуль числа | `42` |
| `ROUND(n, precision)` | Округление | `3.14` |
| `FLOOR(n)` | Округление вниз | `3` |
| `CEIL(n)` | Округление вверх | `4` |

### Примеры использования

#### В WHERE

```json
{
  "from": "sessions",
  "where": {
    "op": "lt",
    "field": ["expires_at"],
    "value": { "$fn": "NOW" }
  }
}
```

#### В SET (INSERT/UPDATE)

```json
{
  "insert_into": "users",
  "values": [{
    "name": "Alice",
    "created_at": { "$fn": "NOW" },
    "api_key": { "$fn": "UUID" }
  }]
}
```

```json
{
  "update": "users",
  "where": { "op": "eq", "field": ["id"], "value": 1 },
  "set": { "last_login": { "$fn": "NOW" } }
}
```

#### В SET с аргументами

```json
{
  "update": "products",
  "where": { "op": "eq", "field": ["id"], "value": 1 },
  "set": {
    "display_name": { "$fn": { "name": "COALESCE", "args": [{ "$ref": "name" }, "Unnamed"] } }
  }
}
```

### Комбинация с $query

```json
{
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": ["id"], "value": 1 }
    },
    "update_session": {
      "update": "sessions",
      "where": { "op": "eq", "field": ["user_id"], "value": { "$query": "user[0].id" } },
      "set": {
        "last_active": { "$fn": "NOW" },
        "token": { "$fn": "UUID" }
      }
    }
  }
}
```

### Безопасность

- Функции выполняются на сервере
- Нет доступа к файловой системе
- Нет выполнения произвольного кода
- Детерминированные функции кешируются в рамках запроса

## Фильтры (Filter)

### Операторы сравнения

**Важно:** `field` — это массив строк (FieldPath = `Vec<String>`), не строка.

```json
{ "op": "eq", "field": ["status"], "value": "active" }
{ "op": "ne", "field": ["status"], "value": "deleted" }
{ "op": "gt", "field": ["age"], "value": 18 }
{ "op": "gte", "field": ["age"], "value": 18 }
{ "op": "lt", "field": ["price"], "value": 100 }
{ "op": "lte", "field": ["price"], "value": 100 }
```

Вложенные пути:
```json
{ "op": "eq", "field": ["user", "address", "city"], "value": "Moscow" }
```

### Операторы массивов

```json
{ "op": "in", "field": ["status"], "values": ["active", "pending"] }
{ "op": "not_in", "field": ["status"], "values": ["deleted"] }
{ "op": "contains", "field": ["tags"], "value": "rust" }
{ "op": "contains_any", "field": ["tags"], "values": ["rust", "go"] }
{ "op": "contains_all", "field": ["tags"], "values": ["rust", "async"] }
```

### Операторы соответствия

```json
{ "op": "like", "field": ["name"], "pattern": "%john%" }
{ "op": "i_like", "field": ["email"], "pattern": "%@gmail.com" }
{ "op": "regex", "field": ["phone"], "pattern": "^\\+7" }
```

### Null-проверки

```json
{ "op": "is_null", "field": ["deleted_at"] }
{ "op": "is_not_null", "field": ["email"] }
```

### Логические операторы

```json
{
  "op": "and",
  "filters": [
    { "op": "eq", "field": ["status"], "value": "active" },
    { "op": "gt", "field": ["age"], "value": 18 }
  ]
}
```

```json
{
  "op": "or",
  "filters": [
    { "op": "eq", "field": ["role"], "value": "admin" },
    { "op": "eq", "field": ["role"], "value": "moderator" }
  ]
}
```

```json
{
  "op": "not",
  "filter": { "op": "eq", "field": ["banned"], "value": true }
}
```

### Диапазон

```json
{ "op": "between", "field": ["price"], "from": 10, "to": 100 }
```

### Существование поля

```json
{ "op": "exists", "field": ["profile", "avatar"] }
{ "op": "not_exists", "field": ["deleted_at"] }
```

### Сокращённая форма

```json
{ "op": "field", "field": ["user_id"], "value": { "$query": "users[0].id" } }
```

## Валидация ссылок $query

### Стратегия: Строгая валидация

S.H.A.M.I.R. использует **строгую валидацию** ссылок `$query` на этапе планирования.

**Правило:** Все `$query` ссылки должны указывать на алиасы, существующие в том же батче.

```json
// ❌ ОШИБКА: "usres" - опечатка, алиас не существует
{
  "queries": {
    "users": { "from": "users" },
    "orders": {
      "from": "orders",
      "where": { "op": "eq", "field": ["user_id"], "value": { "$query": "usres[0].id" } }
    }
  }
}
// Результат: BatchError::UnknownAlias { alias: "usres", referenced_by: "orders" }
```

### Почему строгая валидация?

| Подход | Поведение | Результат |
|--------|-----------|-----------|
| **Строгий** (наш) | Ошибка на этапе планирования | Явная проблема, легко исправить |
| Graceful | Пустой результат или `null` | Молчаливая ошибка, сложная отладка |

**Причины выбора строгой валидации:**

1. **Опечатки ловятся сразу**
   - `$query: "usres"` вместо `"users"` → немедленная ошибка
   - При graceful подходе запрос вернул бы пустой результат

2. **Безопасность операций записи**
   - `DELETE FROM orders WHERE user_id = $query`
   - Если ссылка битая → лучше ошибка, чем удаление 0 строк

3. **Защита от атак**
   - Злоумышленник не может "сломать" запрос, удалив зависимость
   - Все связи явно проверяются

4. **Отсутствие "лжи" о зависимостях**
   - Зависимости извлекаются автоматически из содержимого запроса
   - Нет поля `depends_on`, которое можно указать неверно
   - Планировщик видит реальные ссылки

### Когда нужна гибкость?

Если нужна "опциональная" зависимость, используйте отдельные батчи или условную логику на уровне приложения:

```json
// Батч 1: Получить пользователя (может не существовать)
{ "id": 1, "queries": { "user": { "from": "users", "where": { "op": "eq", "field": ["id"], "value": 999 } } } }

// Приложение проверяет: если user пустой → не выполнять батч 2
// Если user существует → выполнить батч 2 с реальным ID
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
  "id": "page-3",
  "queries": {
    "products": {
      "from": "products",
      "where": { "op": "eq", "field": ["category"], "value": "electronics" },
      "order_by": { "items": [{ "field": ["created_at"], "direction": "desc" }] },
      "pagination": { "mode": "Page", "page": 3, "page_size": 20 }
    }
  }
}
```

### E-commerce Dashboard

```json
{
  "id": "dashboard-001",
  "name": "dashboard",
  "queries": {
    "user": {
      "from": "users",
      "where": { "op": "eq", "field": ["id"], "value": 123 }
    },
    "recent_orders": {
      "from": "orders",
      "where": {
        "op": "and",
        "filters": [
          { "op": "eq", "field": ["user_id"], "value": { "$query": "user[0].id" } },
          { "op": "gte", "field": ["created_at"], "value": "2024-01-01" }
        ]
      },
      "order_by": { "items": [{ "field": ["created_at"], "direction": "desc" }] },
      "pagination": { "mode": "LimitOffset", "limit": 10 }
    },
    "order_count": {
      "from": "orders",
      "where": { "op": "eq", "field": ["user_id"], "value": { "$query": "user[0].id" } },
      "select": { "items": [{ "type": "count_all" }] }
    },
    "notifications": {
      "from": "notifications",
      "where": {
        "op": "and",
        "filters": [
          { "op": "eq", "field": ["user_id"], "value": { "$query": "user[0].id" } },
          { "op": "eq", "field": ["read"], "value": false }
        ]
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
  "id": "transfer-001",
  "name": "transfer",
  "transactional": true,
  "queries": {
    "from_account": {
      "from": "accounts",
      "where": { "op": "eq", "field": ["id"], "value": 1 }
    },
    "to_account": {
      "from": "accounts",
      "where": { "op": "eq", "field": ["id"], "value": 2 }
    },
    "check_balance": {
      "from": "accounts",
      "where": {
        "op": "and",
        "filters": [
          { "op": "eq", "field": ["id"], "value": 1 },
          { "op": "gte", "field": ["balance"], "value": { "$query": "from_account[0].balance" } }
        ]
      }
    }
  }
}
```

## Выражения ($expr)

Выражения для арифметических и строковых операций.

### Синтаксис

```json
{ "$expr": { "op": "add", "args": [10, 20] } }
{ "$expr": { "op": "mul", "args": [{ "$ref": "price" }, 1.1] } }
{ "$expr": { "op": "concat", "args": [{ "$ref": "first" }, " ", { "$ref": "last" }] } }
```

### Операторы

#### Математика

| Оператор | Описание | Пример | Результат |
|----------|----------|--------|-----------|
| `add` | Сложение | `{ "op": "add", "args": [10, 5] }` | `15` |
| `sub` | Вычитание | `{ "op": "sub", "args": [10, 5] }` | `5` |
| `mul` | Умножение | `{ "op": "mul", "args": [10, 5] }` | `50` |
| `div` | Деление | `{ "op": "div", "args": [10, 5] }` | `2` |
| `mod` | Остаток | `{ "op": "mod", "args": [10, 3] }` | `1` |
| `neg` | Унарный минус | `{ "op": "neg", "args": [5] }` | `-5` |

#### Строки

| Оператор | Описание | Пример | Результат |
|----------|----------|--------|-----------|
| `concat` | Конкатенация | `{ "op": "concat", "args": ["a", "b", "c"] }` | `"abc"` |
| `lower` | Нижний регистр | `{ "op": "lower", "args": ["HELLO"] }` | `"hello"` |
| `upper` | Верхний регистр | `{ "op": "upper", "args": ["hello"] }` | `"HELLO"` |
| `trim` | Удалить пробелы | `{ "op": "trim", "args": ["  hi  "] }` | `"hi"` |
| `length` | Длина строки | `{ "op": "length", "args": ["hello"] }` | `5` |

#### Логика

| Оператор | Описание | Пример | Результат |
|----------|----------|--------|-----------|
| `and` | Логическое И | `{ "op": "and", "args": [true, false] }` | `false` |
| `or` | Логическое ИЛИ | `{ "op": "or", "args": [true, false] }` | `true` |
| `not` | Логическое НЕ | `{ "op": "not", "args": [true] }` | `false` |

#### Сравнение (возвращает bool)

| Оператор | Описание | Пример | Результат |
|----------|----------|--------|-----------|
| `eq` | Равно | `{ "op": "eq", "args": [1, 1] }` | `true` |
| `ne` | Не равно | `{ "op": "ne", "args": [1, 2] }` | `true` |
| `gt` | Больше | `{ "op": "gt", "args": [5, 3] }` | `true` |
| `gte` | Больше или равно | `{ "op": "gte", "args": [5, 5] }` | `true` |
| `lt` | Меньше | `{ "op": "lt", "args": [3, 5] }` | `true` |
| `lte` | Меньше или равно | `{ "op": "lte", "args": [5, 5] }` | `true` |

### Использование

#### В WHERE

```json
{
  "from": "products",
  "where": {
    "op": "gt",
    "field": ["price"],
    "value": { "$expr": { "op": "mul", "args": [{ "$ref": "base_price" }, 1.2] } }
  }
}
```

#### В SET (Update)

```json
{
  "update": "products",
  "where": { "op": "eq", "field": ["id"], "value": 1 },
  "set": {
    "price": { "$expr": { "op": "mul", "args": [{ "$ref": "price" }, 1.1] } },
    "full_name": { "$expr": { "op": "concat", "args": [{ "$ref": "first" }, " ", { "$ref": "last" }] } }
  }
}
```

#### В INSERT

```json
{
  "insert_into": "orders",
  "values": [{
    "total": { "$expr": { "op": "mul", "args": [{ "$ref": "price" }, { "$ref": "quantity" }] } },
    "created_at": { "$fn": "NOW" }
  }]
}
```

### Вложенные выражения

```json
{
  "set": {
    "final_price": {
      "$expr": {
        "op": "mul",
        "args": [
          { "$expr": { "op": "sub", "args": [{ "$ref": "price" }, { "$ref": "discount" }] } },
          { "$expr": { "op": "add", "args": [1, { "$ref": "tax_rate" }] } }
        ]
      }
    }
  }
}
```

## Условия ($cond)

Условный оператор (тернарный) — возвращает `then` или `else` в зависимости от условия.

### Синтаксис

```json
{
  "$cond": {
    "if": { "op": "eq", "field": ["active"], "value": true },
    "then": "yes",
    "else": "no"
  }
}
```

### Условие (if)

Поле `if` использует **существующий синтаксис Filter**:

```json
// Простое сравнение
"if": { "op": "eq", "field": ["status"], "value": "active" }

// Сравнение с $ref
"if": { "op": "gt", "field": ["score"], "value": { "$ref": "threshold" } }

// Логические операторы
"if": { "op": "and", "filters": [
  { "op": "eq", "field": ["active"], "value": true },
  { "op": "gte", "field": ["level"], "value": 5 }
]}

// С выражением $expr
"if": { "op": "gt", "field": ["total"], "value": { "$expr": { "op": "mul", "args": [100, 2] } } }
```

### Примеры

#### Простой статус

```json
{
  "update": "users",
  "where": { "op": "eq", "field": ["id"], "value": 1 },
  "set": {
    "label": {
      "$cond": {
        "if": { "op": "gte", "field": ["score"], "value": 100 },
        "then": "vip",
        "else": "regular"
      }
    }
  }
}
```

#### С $expr в then/else

```json
{
  "set": {
    "price": {
      "$cond": {
        "if": { "op": "eq", "field": ["is_vip"], "value": true },
        "then": { "$expr": { "op": "mul", "args": [{ "$ref": "base_price" }, 0.9] } },
        "else": { "$ref": "base_price" }
      }
    }
  }
}
```

#### Вложенные $cond

```json
{
  "set": {
    "tier": {
      "$cond": {
        "if": { "op": "gte", "field": ["score"], "value": 1000 },
        "then": "platinum",
        "else": {
          "$cond": {
            "if": { "op": "gte", "field": ["score"], "value": 500 },
            "then": "gold",
            "else": {
              "$cond": {
                "if": { "op": "gte", "field": ["score"], "value": 100 },
                "then": "silver",
                "else": "bronze"
              }
            }
          }
        }
      }
    }
  }
}
```

#### В WHERE

```json
{
  "from": "products",
  "where": {
    "op": "eq",
    "field": ["category"],
    "value": {
      "$cond": {
        "if": { "op": "gt", "field": ["price"], "value": 1000 },
        "then": "premium",
        "else": "standard"
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
| `fn_call` | `{ "$fn": "NOW" }` |
| `expr` | `{ "$expr": { "op": "add", "args": [1, 2] } }` |
| `cond` | `{ "$cond": { "if": {...}, "then": "a", "else": "b" } }` |

## Архитектура

```
BatchRequest { id, queries, limits, ... }
    │
    ▼
┌──────────────────────────┐
│  BatchPlanner            │
│  ┌────────────────────┐  │
│  │ Parse $query refs  │──▶ Extract dependencies
│  └────────────────────┘  │
│  ┌────────────────────┐  │
│  │ Validate           │──▶ Check aliases, cycles, depth
│  └────────────────────┘  │
│  ┌────────────────────┐  │
│  │ Topo Sort          │──▶ Create parallel stages
│  └────────────────────┘  │
└──────────────────────────┘
    │
    ▼
BatchPlan { stages, aliases, dependencies }
    │
    ▼
┌──────────────────────────┐
│  execute_batch()         │
│  ├─ TableResolver trait  │──▶ Resolve TableRef → TableManager
│  ├─ AdminExecutor trait  │──▶ Execute DDL (optional)
│  ├─ FilterContext        │──▶ Resolved $query refs
│  │                       │
│  │  ┌─────────────────┐  │
│  │  │ Stage 1         │──▶ Parallel (read/write/admin)
│  │  └─────────────────┘  │
│  │  ┌─────────────────┐  │
│  │  │ Stage 2         │──▶ Wait for deps, then parallel
│  │  └─────────────────┘  │
│  │  ...                  │
└──────────────────────────┘
    │
    ▼
BatchResponse { id, results, execution_plan, execution_time_us }
```

## См. также

- [Write Operations](../write/README.md) — операции записи (Insert, Update, Set, Delete)
- [Admin Operations](../admin/types.rs) — DDL операции (Create/Drop/List)
- [Write Examples](../examples/write.md) — примеры JSON для операций записи
- [Filter Examples](../examples/filter.md) — примеры фильтров WHERE
- [Query Reference](./reference.rs) — парсинг `$query` ссылок
- [Batch Types](./types.rs) — BatchRequest (id обязательно), BatchOp, QueryEntry
- [Batch Planner](./planner.rs) — планировщик выполнения
- [Batch Executor](./executor.rs) — execute_batch, TableResolver, AdminExecutor

## Performance Notes

### Валидация зависимостей

Проверка `$query` ссылок происходит **один раз** на этапе планирования:

```
O(n) где n = количество $query ссылок
+ O(1) HashSet lookup для каждой проверки aliases.contains(dep)
```

### Runtime разрешение (TODO: Executor)

При реализации executor'а добавить кеширование извлечённых значений:

```rust
// Кеш: "alias.path" -> Value
let cache: TMap<String, Value> = new_map();

// Первый запрос к "users[0].id" — извлекаем и кешируем
// Последующие запросы — O(1) lookup из кеша
```

Это ускорит случаи когда одна и та же ссылка используется многократно в фильтрах.
