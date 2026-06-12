# shamir-query-builder — оптимизация производительности

## Обзор
Fluent builder для batch-запросов. Тонкий слой над query-types — производит DTO, не выполняет.
Не является горячим путем — builder вызывается один раз на запрос, не в цикле.

## Вывод
**Нет критических оптимизаций.** Builder работает с DTO, нет горячих циклов, нет I/O.

## 🟢 Мелкие
- String allocations в builder methods (`format!`, `to_string()`) — незначительно на фоне I/O.
- Можно pre-allocate Vec где известен размер (`.with_capacity(N)`), но эффект минимальный — builder не hot path.
