# Common — общие утилиты парсинга запросов

Модуль содержит парсеры для компонентов запросов, которые используются
несколькими типами операций (SELECT, UPDATE, DELETE и т.д.).

## Зачем

`ReadQuery` парсится через `query_from_value()` в модуле `read/parser.rs`,
но составные части запроса (фильтры, группировка, сортировка, агрегаты,
пагинация) переиспользуются и в других контекстах. Общие парсеры вынесены
сюда, чтобы избежать дублирования.

## Ключевые функции

| Функция | Входные данные | Результат |
|---------|---------------|-----------|
| `filter_from_value` | `QueryValue` | `Filter` |
| `filter_value_from_value` | QueryValue | `FilterValue` |
| `group_by_from_value` | QueryValue | `GroupBy` |
| `order_by_from_value` | QueryValue | `OrderBy` |
| `order_by_item_from_value` | QueryValue | `OrderByItem` |
| `pagination_from_value` | QueryValue | `Pagination` |
| `agg_func_from_str` | `&str` ("sum", "avg", ...) | `AggFunc` |
| `aggregate_field_from_value` | QueryValue | `AggregateField` |
| `expr_from_value` | QueryValue | `SelectExpr` |
| `expr_value_from_value` | QueryValue | `SelectExprValue` |

## Парсинг фильтров

`filter_from_value` рекурсивно парсит `QueryValue`-объект с ключом `"op"`:

```
{"op": "eq", "field": ["status"], "value": "active"}

{"op": "and", "filters": [
  {"op": "gte", "field": ["age"], "value": 18},
  {"op": "lt", "field": ["age"], "value": 65}
]}
```

### Поддерживаемые операторы

Сравнение: `eq`, `ne`, `gt`, `gte`, `lt`, `lte`
Логические: `and`, `or`, `not`
Null: `is_null`, `is_not_null`
Существование: `exists`, `not_exists`
Вхождение: `in`, `not_in`
Паттерны: `like`, `ilike`, `regex`
Содержание: `contains`, `contains_any`, `contains_all`
Диапазон: `between`

### FieldPath

Пути к полям — массивы строк (без парсинга строк с точками):

```
"field": ["user", "address", "city"]
```

Парсер принимает и строку (авто-разбиение по `.` для обратной совместимости)
и массив (рекомендуемый формат).

### FilterValue

Значения в фильтрах поддерживают несколько форматов:

```
"value": "active"                              // литерал
"value": {"$ref": ["other_field"]}             // ссылка на поле записи
"value": {"$query": "alias", "path": "[0].id"} // ссылка на результат другого запроса
```

## QueryParseError

```rust
pub enum QueryParseError {
    NotAnObject,
    MissingField(&'static str),
    InvalidType(&'static str, &'static str),
    UnknownType(String),
    UnknownAggregateFunction(String),
    UnknownFilterOp(String),
    InvalidField(&'static str, &'static str),
}
```

## Файлы

| Файл | Содержимое |
|------|-----------|
| `parser.rs` | Все функции парсинга |
| `tests/common_parser_tests.rs` | Тесты парсинга фильтров, группировки, сортировки, агрегатов |
