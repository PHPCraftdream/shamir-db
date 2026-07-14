# shamir-query-types — оптимизация производительности

## Обзор
Чистый DTO-крейт: Filter, ReadQuery, BatchRequest, BatchPlanner. Нет runtime-кода, нет I/O.
Почти всё — `#[derive(Serialize, Deserialize)]` типы. Оптимизации минимальны.

---

## 🟡 Значимые оптимизации

### 1. BatchPlanner: topological_sort — O(N²) per stage
**Файл:** `batch/planner.rs:452-486`
**Сейчас:** Внешний `while !remaining.is_empty()` + внутренний `.iter().filter()` + `is_subset()` — O(N²) на каждую стадию.
**Проблема:** Для batch из 100 queries — до 100×100=10k операций. При 10 stages = 100k.
**Решение:** Kahn's algorithm с in-degree counters — O(V+E):
```rust
let mut in_degree: HashMap<String, usize> = ...;
// queue = nodes with in_degree 0
// process queue, decrement neighbors
```
- **Ожидаемый эффект:** O(N²) → O(N+E). Для 100 queries: 100k → ~200 ops.

### 2. BatchPlanner: extract_base_alias — String alloc на каждый вызов
**Файл:** `batch/planner.rs:323-328`
**Сейчас:** `s[..pos].to_string()` — allocation на каждый dependency extraction.
**Решение:** Возвращать `&str` (borrow из input):
```rust
fn extract_base_alias<'a>(s: &'a str) -> &'a str {
    let s = s.strip_prefix('@').unwrap_or(s);
    s.find(['[', '.']).map_or(s, |pos| &s[..pos])
}
```
- **Ожидаемый эффект:** −N allocations на planning.

### 3. detect_cycle — clone strings в TSet
**Файл:** `batch/planner.rs:331-377`
**Сейчас:** `node.to_string()` при каждом DFS step — N строковых allocation.
**Решение:** Использовать `&str` borrow-based sets или `HashSet<&str>`. TSet (IndexSet) требует owned ключи — можно заменить на `HashSet` с borrowed lookups.

### 4. FilterValue: `#[serde(untagged)]` — slow deserialization
**Файл:** `filter/filter_value.rs:9`
**Сейчас:** `#[serde(untagged)]` означает что serde пробует каждый вариант по очереди — O(N) variants на deserialize.
**Проблема:** 12 вариантов → до 12 попыток десериализации. Для `$ref`/`$query`/`$fn` — probe-based.
**Решение:** Custom deserializer с early exit по ключу (`$ref`, `$query`, `$fn`, `$expr`, `$cond`, `$param`). Или `#[serde(tag = "type")]` external tagging.

---

## 🟢 Мелкие

### 5. `new_map()` / `new_set()` без capacity
**Файлы:** `batch/planner.rs` — множество вызовов `new_map()`, `new_set()` без capacity.
**Решение:** `_wc` variants где размер известен.

### 6. `default_fts_mode()` — `"and".to_string()` alloc
**Файл:** `filter/filter_enum.rs:195-197`
**Решение:** `const` или `Arc<str>`.

---

## Приоритет
| # | Улучшение | Ожидаемый эффект | Сложность |
|---|-----------|------------------|-----------|
| 1 | Kahn's algorithm topo sort | O(N²)→O(N+E) | Низкая |
| 2 | `extract_base_alias` → `&str` | −N alloc | Низкая |
| 4 | FilterValue custom deserializer | −12× probe on parse | Средняя |
| 3 | detect_cycle borrow-based | −N alloc | Низкая |
