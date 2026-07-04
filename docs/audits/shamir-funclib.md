# shamir-funclib — оптимизация производительности

## Обзор
Встроенная библиотека функций: scalar (math, strings, datetime, crypto, cast, compare, json), aggregate (sum, count, avg, min, max).

## Вывод
Функции вызываются один раз на значение в SELECT/filter — не в tight loop. Основной overhead — от String allocations в string functions.

## 🟡 Значимые
### 1. Registry lookup — HashMap<String, Fn> на каждый вызов
**Решение:** Enum dispatch + массив по FnId (index-based) вместо HashMap lookup.

### 2. String functions alloc
`trim`, `lower`, `upper`, `concat` — каждое создаёт новую String.
**Решение:** Использовать `Cow<str>` для avoidable alloc (trim и lower без изменений → Borrowed).

## 🟢 Мелкие
- Aggregate functions — уже O(1) per row. ✅
- Math — inline. ✅
