# Read Queries (SELECT)

Модуль для построения запросов на чтение данных.

## Основные типы

| Тип | Описание |
|-----|----------|
| `Query` / `ReadQuery` | Полный SELECT запрос |
| `Select` | Список полей и агрегаций |
| `Filter` | WHERE условие |
| `GroupBy` | GROUP BY выражение |
| `OrderBy` | Сортировка результатов |
| `LimitOffset` | Пагинация |

## Query Structure

```rust
pub struct Query {
    pub from: TableName,        // Имя таблицы
    pub select: Select,         // SELECT clause
    pub r#where: Option<Filter>, // WHERE clause
    pub group_by: Option<GroupBy>, // GROUP BY
    pub order_by: Option<OrderBy>, // ORDER BY
    pub limit: LimitOffset,     // LIMIT/OFFSET
}
```

## JSON Examples

### Basic Query

```json
{
  "from": "users",
  "where": { "op": "eq", "field": "status", "value": "active" },
  "limit": 10
}
```

### Query with Aggregation

```json
{
  "from": "orders",
  "select": [
    "user_id",
    { "agg": "sum", "field": "total", "as": "total_spent" },
    { "agg": "count", "as": "order_count" }
  ],
  "group_by": ["user_id"],
  "order_by": [{ "field": "total_spent", "dir": "desc" }],
  "limit": 10
}
```

### Query with Sorting and Pagination

```json
{
  "from": "products",
  "where": { "op": "gt", "field": "price", "value": 100 },
  "order_by": [{ "field": "created_at", "dir": "desc", "nulls": "last" }],
  "limit": 20,
  "offset": 40
}
```

## ReadQuery Alias

`ReadQuery` — это type alias для `Query`:

```rust
pub type ReadQuery = Query;
```

Используется для ясности API, когда нужно отличить read от write операций:

```rust
// В BatchOp
pub enum BatchOp {
    Read(ReadQuery),  // или Query
    Insert(InsertOp),
    Update(UpdateOp),
    Set(SetOp),
    Delete(DeleteOp),
}
```

## Select Clause

### Select All (default)

```json
{ "from": "users" }
// Эквивалентно SELECT *
```

### Select Fields

```json
{
  "from": "users",
  "select": ["id", "name", "email"]
}
```

### Aggregations

```json
{
  "from": "orders",
  "select": [
    "status",
    { "agg": "count", "as": "count" },
    { "agg": "sum", "field": "total", "as": "revenue" },
    { "agg": "avg", "field": "total", "as": "avg_order" }
  ],
  "group_by": ["status"]
}
```

### Supported Aggregations

| Function | Description |
|----------|-------------|
| `count` | Количество записей |
| `sum` | Сумма значений |
| `avg` | Среднее значение |
| `min` | Минимум |
| `max` | Максимум |
| `first` | Первое значение |
| `last` | Последнее значение |

## Order By

### Single Field

```json
{ "order_by": "created_at" }
// По умолчанию ASC
```

### Multiple Fields

```json
{
  "order_by": [
    { "field": "status", "dir": "asc" },
    { "field": "created_at", "dir": "desc" }
  ]
}
```

### Null Handling

```json
{
  "order_by": [
    { "field": "deleted_at", "dir": "asc", "nulls": "last" }
  ]
}
```

| Nulls Option | Описание |
|--------------|----------|
| `first` | NULL значения первыми |
| `last` | NULL значения последними |

## Limit/Offset

```json
{
  "limit": 20,
  "offset": 40
}
```

Для пагинации: страница 3 при 20 записях на странице.

## Query Result

```rust
pub struct QueryResult {
    pub records: Vec<Value>,      // Результаты
    pub stats: Option<QueryStats>, // Статистика выполнения
    pub has_more: bool,           // Есть ли ещё данные
}

pub struct QueryStats {
    pub index_used: Option<String>,  // Использованный индекс
    pub records_scanned: u64,        // Сканировано записей
    pub records_returned: u64,       // Возвращено записей
    pub execution_time_us: u64,      // Время выполнения (мкс)
}
```

## Builder Pattern

```rust
let query = Query::new("users")
    .filter(Filter::eq("status", "active"))
    .order_by(OrderBy::desc("created_at"))
    .limit(10);
```

## См. также

- [Filter](../filter/) — WHERE условия
- [Batch](../batch/) — выполнение запросов
- [Select Examples](../examples/select.md) — примеры SELECT
- [Aggregate Examples](../examples/aggregate.md) — примеры агрегаций
