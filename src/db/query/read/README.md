# Read Queries (SELECT)

Модуль для построения запросов на чтение данных.

## Основные типы

| Тип | Описание |
|-----|----------|
| `ReadQuery` | Полный SELECT запрос |
| `Select` | Список полей и агрегаций |
| `Filter` | WHERE условие |
| `GroupBy` | GROUP BY выражение |
| `OrderBy` | Сортировка результатов |
| `Pagination` | Пагинация (limit/offset или page-based) |
| `PaginationInfo` | Метаданные пагинации в ответе |

## ReadQuery Structure

```rust
pub struct ReadQuery {
    pub from: TableName,             // Имя таблицы
    pub select: Select,              // SELECT clause
    pub r#where: Option<Filter>,     // WHERE clause
    pub group_by: Option<GroupBy>,   // GROUP BY
    pub order_by: Option<OrderBy>,   // ORDER BY
    pub pagination: Pagination,      // Пагинация
    pub count_total: bool,           // Запрашивать ли total count (дорого)
}
```

## Pagination

Два режима пагинации:

```rust
pub enum Pagination {
    /// Классический limit + offset
    LimitOffset { limit: Option<u64>, offset: u64 },
    /// Постраничный: номер страницы (1-based) + размер страницы
    Page { page: u64, page_size: u64 },
    /// Без пагинации
    None,
}
```

### JSON: Limit/Offset (обратная совместимость)

```json
{
  "from": "users",
  "limit": { "limit": 10, "offset": 20 }
}
```

### JSON: Page-based

```json
{
  "from": "users",
  "limit": { "page": 2, "page_size": 10 },
  "count_total": true
}
```

### count_total

`count_total: true` — запрашивает подсчёт общего количества записей.
По умолчанию `false` (не считаем — это дорогая операция).

Парсер определяет режим по ключам в `limit`:
- есть `page` → `Pagination::Page`
- есть `limit` или `offset` → `Pagination::LimitOffset`
- нет секции `limit` → `Pagination::None`

## Query Result

```rust
pub struct QueryResult {
    pub records: Vec<Value>,                // Результаты
    pub stats: Option<QueryStats>,          // Статистика выполнения
    pub pagination: Option<PaginationInfo>, // Метаданные пагинации
}
```

### PaginationInfo

```rust
pub struct PaginationInfo {
    pub total_count: Option<u64>,   // Только если count_total = true
    pub total_pages: Option<u64>,   // Если есть total_count и page_size
    pub current_page: Option<u64>,  // Только для page-based
    pub page_size: Option<u64>,     // Размер страницы / limit
    pub has_next: bool,             // Есть ли следующая страница
    pub has_prev: bool,             // Есть ли предыдущая страница
}
```

```rust
pub struct QueryStats {
    pub index_used: Option<String>,  // Использованный индекс
    pub records_scanned: u64,        // Сканировано записей
    pub records_returned: u64,       // Возвращено записей
    pub execution_time_us: u64,      // Время выполнения (мкс)
}
```

## JSON Examples

### Basic Query

```json
{
  "from": "users",
  "where": { "op": "eq", "field": "status", "value": "active" },
  "limit": { "limit": 10 }
}
```

### Query with Aggregation

```json
{
  "from": "orders",
  "select": {
    "items": [
      { "type": "field", "path": "user_id" },
      { "type": "aggregate", "func": "sum", "field": "total", "alias": "total_spent" },
      { "type": "aggregate", "func": "count", "field": { "type": "all" }, "alias": "order_count" }
    ]
  },
  "group_by": { "fields": ["user_id"] },
  "order_by": { "items": [{ "field": "total_spent", "order": "desc" }] },
  "limit": { "limit": 10 }
}
```

### Query with Page-based Pagination

```json
{
  "from": "products",
  "where": { "op": "gt", "field": "price", "value": 100 },
  "order_by": { "items": [{ "field": "created_at", "order": "desc", "nulls": "last" }] },
  "limit": { "page": 3, "page_size": 20 },
  "count_total": true
}
```

## Select Clause

### Select All (default)

```json
{ "from": "users" }
```

### Select Fields

```json
{
  "from": "users",
  "select": {
    "items": [
      { "type": "field", "path": "id" },
      { "type": "field", "path": "name" },
      { "type": "field", "path": "email" }
    ]
  }
}
```

### Aggregations

```json
{
  "from": "orders",
  "select": {
    "items": [
      { "type": "field", "path": "status" },
      { "type": "count_all", "alias": "count" },
      { "type": "aggregate", "func": "sum", "field": "total", "alias": "revenue" },
      { "type": "aggregate", "func": "avg", "field": "total", "alias": "avg_order" }
    ]
  },
  "group_by": { "fields": ["status"] }
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

## Order By

### Multiple Fields

```json
{
  "order_by": {
    "items": [
      { "field": "status", "order": "asc" },
      { "field": "created_at", "order": "desc" }
    ]
  }
}
```

### Null Handling

```json
{
  "order_by": {
    "items": [
      { "field": "deleted_at", "order": "asc", "nulls": "last" }
    ]
  }
}
```

| Nulls Option | Описание |
|--------------|----------|
| `first` | NULL значения первыми |
| `last` | NULL значения последними |

## Builder Pattern

```rust
let query = ReadQuery::new("users")
    .filter(Filter::eq("status", "active"))
    .order_by(OrderBy::desc("created_at"))
    .limit(10);

// Page-based
let query = ReadQuery::new("products")
    .pagination(Pagination::page(2, 20))
    .count_total(true);
```

## См. также

- [Filter](../filter/) — WHERE условия
- [Batch](../batch/) — выполнение запросов
