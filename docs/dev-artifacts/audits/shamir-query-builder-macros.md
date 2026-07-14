# shamir-query-builder-macros — оптимизация производительности

## Обзор
Proc-macro крейт (`filter_lower!`, query parse). Compile-time only, не участвует в runtime.

## Вывод
**Нет оптимизаций.** Proc-макросы работают только при компиляции. На runtime производительность не влияют.
