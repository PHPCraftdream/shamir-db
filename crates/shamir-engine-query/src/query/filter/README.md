# Filter System

## Обзор

Модуль `filter` реализует систему фильтрации для WHERE, HAVING, UPDATE и DELETE.
Фильтры описываются как AST из `QueryValue`/MessagePack, затем компилируются в дерево `FilterCallback`
для быстрого вычисления на InnerValue записях.

## Ключевые типы

### FieldPath

```rust
pub type FieldPath = Vec<String>;
// ["name"] — простое поле
// ["user", "address", "city"] — вложенное
```

### Filter (enum)

Tagged enum (по полю `op`) с 22 вариантами:

| Категория | Операторы |
|-----------|-----------|
| Сравнение | `Eq`, `Ne`, `Gt`, `Gte`, `Lt`, `Lte` |
| Паттерны | `Like`, `ILike`, `Regex` |
| Null | `IsNull`, `IsNotNull` |
| Множества | `In`, `NotIn`, `Contains`, `ContainsAny`, `ContainsAll` |
| Диапазон | `Between` |
| Существование | `Exists`, `NotExists` |
| Логические | `And`, `Or`, `Not` |
| Сокращение | `FieldEq` (field = value) |

### FilterValue (enum, untagged)

Значения в фильтрах. Поддерживает литералы и ссылки:

| Вариант | Пример (QueryValue) | Описание |
|---------|------|----------|
| `Null` | `null` | Null значение |
| `Bool` | `true` | Булево |
| `Int` | `42` | Целое число |
| `Float` | `3.14` | Дробное число |
| `String` | `"hello"` | Строка |
| `Array` | `[1, 2]` | Массив |
| `FieldRef` | `{"$ref": ["salary"]}` | Ссылка на поле записи |
| `QueryRef` | `{"$query": "users", "path": "[0].id"}` | Ссылка на результат запроса |
| `FnCall` | `{"$fn": "NOW"}` | Системная функция |
| `Expr` | `{"$expr": {"op": "add", "args": [1, 2]}}` | Выражение |
| `Cond` | `{"$cond": {"if": ..., "then": ..., "else": ...}}` | Условие |

### FilterCallback (trait)

```rust
pub trait FilterCallback: Send + Sync {
    fn matches(&self, record: &InnerValue, ctx: &FilterContext) -> bool;
}
```

### FilterContext

Контекст для вычисления фильтров: содержит `Interner` и ссылки на результаты
других запросов в batch (`$query`).

## Компиляция фильтров

```rust
let callback = compile_filter(&filter, interner);
let matches = callback.matches(&record, &ctx);
```

`compile_filter()` рекурсивно обходит AST `Filter` и создаёт дерево реализаций
`FilterCallback`. Каждый узел хранит пред-интернированные пути для O(1) доступа.

## Примеры фильтров

Filter-операции задаются через `QueryValue` (строится query builder'ом);
на проводе они кодируются в MessagePack.

```
{"op": "eq", "field": ["status"], "value": "active"}
{"op": "between", "field": ["age"], "from": 18, "to": 65}
{"op": "in", "field": ["role"], "values": ["admin", "moderator"]}
{"op": "like", "field": ["name"], "pattern": "%alice%"}
{"op": "and", "filters": [
  {"op": "gt", "field": ["score"], "value": 100},
  {"op": "ne", "field": ["banned"], "value": true}
]}
{"op": "gt", "field": ["salary"], "value": {"$ref": ["min_salary"]}}
{"op": "eq", "field": ["user_id"], "value": {"$query": "users", "path": "[0].id"}}
```

## Файлы

DTO-типы (`Filter`, `FilterValue`, `Cond`, `FilterExpr`, `FilterExprOp`,
`FnCall`, `FieldPath`) живут в крейте **`shamir-query-types::filter`**.
В `shamir-engine::query::filter` остаётся только runtime-логика, которой
нужен интернер:

| Файл | Описание |
|------|----------|
| `mod.rs` | Re-export DTO + публикация runtime-функций |
| `eval.rs` | `compile_filter()`, `FilterCallback`, `compare_values()`, `resolve_field()`, `intern_field_path`, `filter_value_to_inner` |
| `eval_context.rs` | `FilterContext` — контекст выполнения (Interner + результаты других запросов) |

## Архитектура

```
QueryValue/MessagePack → Filter (AST) → compile_filter() → FilterCallback tree
                                                                    │
                                                                    ▼
                                                       record.matches(ctx) → bool
```

Фильтры используются в: ReadQuery (WHERE), GroupBy (HAVING), UpdateOp, DeleteOp,
а также в row-level security (permissions).
