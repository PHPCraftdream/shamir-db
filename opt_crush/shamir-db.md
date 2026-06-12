# shamir-db (facade) — оптимизация производительности

## Обзор
Фасад: собирает engine + server в единый `ShamirDb` API. CLI binary + integration tests.
Не содержит hot-path логики — делегирует в engine/server.

## Вывод
**Нет оптимизаций.** Фасадный крейт. Вся производительность определяется engine, storage, server.
