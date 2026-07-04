# shamir-collections — оптимизация производительности

## Обзор
Листовой крейт-алиас: `TMap`/`TSet` = `IndexMap`/`IndexSet` + `FxHasher`.
Уже использует `fxhash`, что соответствует проектным стандартам.

## Что можно ускорить

### 1. 💛 Предвыделение емкости (batch-операции)
**Сейчас:** `new_map()` / `new_set()` без capacity.
**Проблема:** При массовом заполнении — множенные реаллокации (каждая ×2, копирование всех entries).
**Решение:** Везде где известен размер заранее — использовать `new_map_wc(cap)` / `new_set_wc(cap)`.
В вызывающем коде (engine, storage) добавить `with_capacity` при batch-insert.

### 2. 💛 Добавить `HashMap`/`HashSet` алиасы (unordered, для hot paths)
**Сейчас:** Только `IndexMap`/`IndexSet` — insertion-ordered, что медленнее `HashMap` на ~15-20% из-за overhead на порядок.
**Проблема:** Не всем нужен порядок. Большинство hot-path lookup'ов в engine/storage — обычный hash map.
**Решение:** Добавить:
```rust
pub type TFxMap<K, V> = std::collections::HashMap<K, V, THasher>;
pub type TFxSet<T> = std::collections::HashSet<T, THasher>;
```
Использовать `TMap`/`TSet` только там где порядок итерации критичен.

### 3. 💛 `entry` API helper для массового обновления
**Сейчас:** Нет хелперов — каждый вызов делает `lookup + insert` вручную.
**Решение:** Добавить thin wrappers вокруг `TMap::entry()` для паттернов:
- `upsert` (insert or update)
- `get_or_insert_with`

### 4. 🟡 Мелкие: `extend` вместо цикла insert
В вызывающем коде часто `for k in items { map.insert(k, v); }` — это O(n) rehash на каждом шаге.
Заменить на `map.extend(iter)` — `IndexMap` резервирует capacity за один вызов.

## Приоритет
| # | Улучшение | Ожидаемый эффект | Сложность |
|---|-----------|------------------|-----------|
| 2 | `TFxMap`/`TFxSet` алиасы | −15-20% lookup на hot paths | Низкая |
| 1 | `with_capacity` везде | −30-50% реаллокаций при batch | Низкая |
| 4 | `extend` вместо loop | −20% batch-insert | Низкая |
| 3 | `entry` helpers | Меньше alloc на upsert | Низкая |
