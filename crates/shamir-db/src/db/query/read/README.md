# Read Query (SELECT)

## Обзор

Модуль `read` определяет типы и пайплайн выполнения SELECT-запросов.
`ReadQuery` — основной тип запроса с поддержкой select, where, group_by, order_by,
pagination и count_total.

## ReadQuery

```rust
pub struct ReadQuery {
    pub from: TableRef,           // таблица (опционально с repo)
    pub select: Select,           // поля, агрегации, выражения
    pub r#where: Option<Filter>,  // WHERE фильтр
    pub group_by: Option<GroupBy>,// GROUP BY + HAVING
    pub order_by: Option<OrderBy>,// ORDER BY
    pub pagination: Pagination,   // LIMIT/OFFSET или Page
    pub count_total: bool,        // подсчитать общее количество
}
```

## Ключевые типы

| Тип | Описание |
|-----|----------|
| `ReadQuery` | Полное определение запроса |
| `Select` | Список полей/агрегаций + distinct |
| `SelectItem` | All / Field / Aggregate / CountAll / Expression |
| `AggFunc` | Count, Sum, Avg, Min, Max |
| `GroupBy` | fields + having (Filter) |
| `OrderBy` | items: Vec\<OrderByItem\> |
| `OrderByItem` | field + direction (asc/desc) + nulls (first/last) |
| `Pagination` | LimitOffset / Page / None |
| `PaginationInfo` | Информация о пагинации в ответе |
| `QueryResult` | records + stats + pagination |
| `QueryStats` | index_used, records_scanned, records_returned, execution_time_us |
| `SelectProjection` | Пред-разрешённая проекция для streaming |

## Pipeline выполнения

### Без GROUP BY

```
index scan / full scan → WHERE (filter) → SELECT → DISTINCT → ORDER BY → PAGINATION
```

### С GROUP BY

```
index scan / full scan → WHERE → GROUP BY → AGG per group → HAVING → SELECT → DISTINCT → ORDER BY → PAGINATION
```

## Ключевые функции (exec.rs)

| Функция | Описание |
|---------|----------|
| `apply_select()` | Проекция полей (InnerValue → JSON) |
| `apply_group_by()` | Группировка + агрегации |
| `apply_order_by()` | Сортировка по полям |
| `apply_pagination()` | LIMIT/OFFSET и Page |
| `apply_distinct()` | Удаление дубликатов |
| `SelectProjection::new()` | Компиляция проекции из Select |
| `SelectProjection::apply()` | Применение проекции к записи |

## JSON примеры

### Простой запрос

```json
{
  "from": "users",
  "where": {"op": "eq", "field": ["status"], "value": "active"},
  "select": {"items": [{"type": "field", "path": ["name"]}, {"type": "field", "path": ["email"]}]},
  "order_by": {"items": [{"field": ["name"], "direction": "asc"}]},
  "pagination": {"mode": "LimitOffset", "limit": 10, "offset": 0}
}
```

### С агрегациями и GROUP BY

```json
{
  "from": "orders",
  "select": {
    "items": [
      {"type": "field", "path": ["city"]},
      {"type": "aggregate", "func": "sum", "field": {"path": ["amount"]}, "alias": "total"},
      {"type": "count_all", "alias": "count"}
    ]
  },
  "group_by": {
    "fields": [["city"]],
    "having": {"op": "gt", "field": ["count"], "value": 5}
  }
}
```

### Page-based пагинация

```json
{
  "from": "products",
  "pagination": {"mode": "Page", "page": 3, "page_size": 25},
  "count_total": true
}
```

## Файлы

| Файл | Описание |
|------|----------|
| `read_query.rs` | `ReadQuery` — определение запроса |
| `select.rs` | `Select`, `SelectItem` |
| `agg.rs` | `AggFunc`, `AggregateField` |
| `group_by.rs` | `GroupBy` (fields + having) |
| `order_by.rs` | `OrderBy`, `OrderByItem`, `OrderDirection`, `NullsOrder` |
| `limit.rs` | `Pagination` (LimitOffset/Page/None), `PaginationInfo` |
| `query_result.rs` | `QueryResult`, `QueryStats` |
| `select_expr.rs` | `SelectExpr`, `SelectExprValue` |
| `parser.rs` | `query_from_value()` — парсинг JSON → ReadQuery |
| `exec.rs` | Пайплайн выполнения: apply_select, apply_group_by и т.д. |

## Индексы

Если `where` содержит `Eq` или `In` на индексированном поле, выполняется
index scan вместо full scan. Информация об использованном индексе возвращается
в `QueryStats::index_used`.
