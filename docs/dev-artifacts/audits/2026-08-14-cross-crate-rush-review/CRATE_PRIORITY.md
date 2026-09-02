# Crate priority — сложность, важность, порядок разбора

Рабочий документ для похода по крейтам после cross-crate rush-review (23 крейта из
свипа 2026-08-14 + `shamir-client-node`/`shamir-transport-ipc`, добавленные позже).
У каждого крейта есть свой `SUMMARY.md` с планом правок (P0/P1/P2) — путь указан в
каждой строке. Три списка ниже: сложность (по данным ревью), важность
(архитектурная), и итоговый рекомендуемый порядок разбора.

## Список 1 — Сложность (по данным ревью)

Источник: `SUMMARY.md` (workspace-wide) — `Per-Crate Health Scorecard`, плюс
2 крейта, добавленные отдельным прогоном после свипа. Ранжирование: critical ↓,
затем high ↓, при равенстве — качественный тай-брейк (silent-data-loss /
memory-safety важнее style). "Итог" — из синтезированного `<крейт>/SUMMARY.md`
(дедуплицированные дефекты), "Raw" — сырые lens-tagged находки из исходного свипа
(только для 23 крейтов из свипа; для двух новых их не было).

| # | Крейт | Raw | Crit/High | Итог | Вердикт |
|---|---|---|---|---|---|
| 1 | [shamir-funclib](./shamir-funclib/SUMMARY.md) | 64 | 1c / 8h | 47 | **high-risk** — process-abort DoS на любом низкопривилегированном запросе (uncapped recursion + allocations в `validate`/`is_json`, `random_bytes`/`repeat`/`pad`) |
| 2 | [shamir-client-node](./shamir-client-node/SUMMARY.md) | — | 1c / 3h | 25 | **high-risk** — весь enrichment-слой обёртки мёртв на документированном пути (`connect()` — нативная factory, subclass не подключается); repl-ошибки читаются как успех |
| 3 | [shamir-server](./shamir-server/SUMMARY.md) | 18 | 1c / 1h | — | **high-risk** — единственный критикал (read-only-реплика принимает записи через interactive-tx путь); иначе самый чистый крейт из всех |
| 4 | [shamir-engine](./shamir-engine/SUMMARY.md) | 87 | 0c / 12h | 79 | **high-risk** — сквозные quadratic hot-паты + silent data loss на drain-пути |
| 5 | [shamir-db](./shamir-db/SUMMARY.md) | 71 | 0c / 10h | — | **needs focused remediation** — silent-порча каталога (DROP CASCADE не по адресу, фантомные записи, проглоченные rename) |
| 6 | [shamir-index](./shamir-index/SUMMARY.md) | 79 | 0c / 10h | — | **needs focused remediation** — silent wrong-results (hash-коллапс, баг токенайзера, потеря vector-персистентности) |
| 7 | [shamir-client](./shamir-client/SUMMARY.md) | 69 | 0c / 9h | — | **needs focused remediation** — класс permanent-hang (подписчики виснут навсегда) + MITM-экспозиция на resume-пути |
| 8 | [shamir-connect](./shamir-connect/SUMMARY.md) | 76 | 0c / 8h | 53 | **needs focused remediation** — TOCTOU-гонка в rate-limiter'е, TOFU до верификации подписи, `dispatch_request`-двойник без гейта |
| 9 | [shamir-storage](./shamir-storage/SUMMARY.md) | 53 | 0c / 7h | — | **needs focused remediation** — конкурентные гонки, молча маскирующие подтверждённые записи; зависание `flush()` |
| 10 | [shamir-tx](./shamir-tx/SUMMARY.md) | 65 | 0c / 7h | — | **needs focused remediation** — регрессии версий MVCC и durability-иллюзии без rollback |
| 11 | [shamir-wasm-host](./shamir-wasm-host/SUMMARY.md) | 69 | 0c / 7h | 46 | **needs focused remediation** — обход sandbox-границы, resource-лимиты не держат, vacuous security-тесты |
| 12 | [shamir-query-types](./shamir-query-types/SUMMARY.md) | 66 | 0c / 7h | — | **needs focused remediation** — decode-time stack-overflow DoS + silent wire coercion |
| 13 | [shamir-sdk](./shamir-sdk/SUMMARY.md) | 50 | 0c / 6h | — | **needs focused remediation** — fail-open decode, unbounded guest-память, spin-on-Pending executor |
| 14 | [shamir-wal](./shamir-wal/SUMMARY.md) | 59 | 0c / 5h | — | **needs focused remediation** — hang-класс на durability-спине (застрявшие committer'ы, заклиненное лидерство) |
| 15 | [shamir-types](./shamir-types/SUMMARY.md) | 64 | 0c / 5h | — | **needs focused remediation** — decode-abort DoS + silent-wrong-results в примитивах (Hash/Eq, lossy wire) |
| 16 | [shamir-numa](./shamir-numa/SUMMARY.md) | 38 | 0c / 3h | 21 | **moderate** — реальный concurrency-баг (гонка зеркала реплик, перманентная расходимость) |
| 17 | [shamir-transport-ws](./shamir-transport-ws/SUMMARY.md) | 49 | 0c / 3h | — | **moderate** — пробелы spec/interop + непротестированный security-контроль (anti-CSWSH) |
| 18 | [shamir-query-builder-macros](./shamir-query-builder-macros/SUMMARY.md) | 32 | 0c / 3h | — | **moderate** — silent write-мискомпиляция (пропадают поля); ноль error-path тестов |
| 19 | [shamir-query-builder](./shamir-query-builder/SUMMARY.md) | 42 | 0c / 3h | — | **moderate** — over-strict валидация выталкивает с проверенного пути; лишний per-field codec |
| 20 | [shamir-sdk-macros](./shamir-sdk-macros/SUMMARY.md) | 43 | 0c / 2h | 17 | **lean but untested** — ноль покрытия вообще; ложные отказы валидных сигнатур |
| 21 | [shamir-transport-ipc](./shamir-transport-ipc/SUMMARY.md) | — | 0c / 1h | 16 | **solid with isolated gaps** — `accept()` на Windows может запаниковать и тихо убить весь IPC-транспорт после первой ошибки |
| 22 | [shamir-transport-tcp](./shamir-transport-tcp/SUMMARY.md) | 49 | 0c / 1h | 30 | **solid with isolated gaps** — один латентный unsafe/UB-сайт (`set_len` над неинициализированной памятью); остальное хорошо протестировано |
| 23 | [shamir-collections](./shamir-collections/SUMMARY.md) | 18 | 0c / 1h | 11 | **solid with isolated gaps** — ноль тестов, но это pillar-4 anchor: поломки здесь всплывают по всему workspace |
| 24 | [shamir-bench-utils](./shamir-bench-utils/SUMMARY.md) | 36 | 0c / 1h | — | **solid with isolated gaps** — только hygiene и bench-fidelity находки |
| 25 | [shamir-tunables](./shamir-tunables/SUMMARY.md) | 23 | 0c / 1h | — | **mostly clean** — один невыполненный runtime API выдан за рабочий |

## Список 2 — Важность (архитектурная центральность)

Не зависит от находок ревью — по тому, что ломается у остальных при поломке
этого крейта (blast radius). Тир 0 — если сломан, ломается буквально всё.

**Тир 0 — Фундамент (зависят все крейты выше по стеку)**
- [shamir-types](./shamir-types/SUMMARY.md) — `Value`/`RecordId`/интернер полей; используется в каждом крейте без исключения.
- [shamir-collections](./shamir-collections/SUMMARY.md) — `TMap`/`TSet`/`THasher` (Fx-hash по умолчанию для всего workspace).

**Тир 1 — Durability-спина (потеря данных при поломке — не в одной фиче, а во всей БД)**
- [shamir-storage](./shamir-storage/SUMMARY.md) — абстракция бэкендов (Fjall/InMemory/Cached/Mirrored).
- [shamir-wal](./shamir-wal/SUMMARY.md) — crash recovery, group commit.

**Тир 2 — Транзакционное ядро (корректность каждой транзакции в базе)**
- [shamir-tx](./shamir-tx/SUMMARY.md) — MVCC/SSI/wound-wait локинг, Version Oracle.
- [shamir-engine](./shamir-engine/SUMMARY.md) — единственная точка, через которую проходит каждый read/write; оркестрация таблиц, drain.

**Тир 3 — Запросы и индексация**
- [shamir-query-types](./shamir-query-types/SUMMARY.md) — DTO/wire-типы запросов (Filter/ReadQuery/BatchRequest).
- [shamir-query-builder](./shamir-query-builder/SUMMARY.md) + [shamir-query-builder-macros](./shamir-query-builder-macros/SUMMARY.md) — типизированный билдер запросов (единственный разрешённый способ строить запрос).
- [shamir-index](./shamir-index/SUMMARY.md) — hash/sorted/vector/FTS индексы.
- [shamir-funclib](./shamir-funclib/SUMMARY.md) — библиотека функций/валидаторов, вызываемая из пользовательских WASM-модулей и движка.

**Тир 4 — Каталог/DDL**
- [shamir-db](./shamir-db/SUMMARY.md) — DDL, catalog, мультибаза, curl-gateway.

**Тир 5 — Сетевой/security-периметр (единственная граница между внешним миром и данными)**
- [shamir-connect](./shamir-connect/SUMMARY.md) — auth-протокол: SCRAM, resumption-тикеты, rate-limiting, ACL-гейты.
- [shamir-server](./shamir-server/SUMMARY.md) — listener/dispatch, единственная точка входа для сетевых клиентов.
- [shamir-transport-tcp](./shamir-transport-tcp/SUMMARY.md), [shamir-transport-ws](./shamir-transport-ws/SUMMARY.md), [shamir-transport-ipc](./shamir-transport-ipc/SUMMARY.md) — конкретные транспорты (TCP+TLS, WebSocket, Unix socket/Named Pipe).

**Тир 6 — Расширяемость (пользовательская логика)**
- [shamir-wasm-host](./shamir-wasm-host/SUMMARY.md) — sandboxing пользовательских WASM-модулей.

**Тир 7 — Клиентская сторона**
- [shamir-client](./shamir-client/SUMMARY.md) — Rust SDK (эталонная реализация протокола на клиенте).
- [shamir-client-node](./shamir-client-node/SUMMARY.md) — napi-биндинг поверх `shamir-client` (Node.js).
- [shamir-sdk](./shamir-sdk/SUMMARY.md) + [shamir-sdk-macros](./shamir-sdk-macros/SUMMARY.md) — SDK для написания функций/валидаторов, компилируемых в WASM.

**Тир 8 — Инфраструктура/поддержка (влияет на производительность и dev-опыт, не на корректность данных)**
- [shamir-numa](./shamir-numa/SUMMARY.md) — NUMA-aware репликация для многосокетных машин.
- [shamir-tunables](./shamir-tunables/SUMMARY.md) — runtime-тюнинг параметров.
- [shamir-bench-utils](./shamir-bench-utils/SUMMARY.md) — общая инфраструктура бенчмарков.

## Список 3 — Рекомендуемый порядок разбора (важность × риск)

Комбинация обоих списков: сначала архитектурно-центральные крейты с реальными
high/critical находками, затем периферийные/чистые — последними.

1. [shamir-server](./shamir-server/SUMMARY.md) — единственный critical, сетевой периметр, маленький и точечный фикс
2. [shamir-connect](./shamir-connect/SUMMARY.md) — auth-протокол, security-периметр, 8 high
3. [shamir-tx](./shamir-tx/SUMMARY.md) — MVCC-ядро, correctness всех транзакций, 7 high
4. [shamir-engine](./shamir-engine/SUMMARY.md) — центральный движок, 12 high, quadratic + silent data loss
5. [shamir-storage](./shamir-storage/SUMMARY.md) — durability, 7 high
6. [shamir-wal](./shamir-wal/SUMMARY.md) — durability, 5 high, hang-класс
7. [shamir-types](./shamir-types/SUMMARY.md) — фундамент, 5 high
8. [shamir-funclib](./shamir-funclib/SUMMARY.md) — критикал (DoS), но более изолирован от durability-спины
9. [shamir-db](./shamir-db/SUMMARY.md) — каталог/DDL, 10 high
10. [shamir-index](./shamir-index/SUMMARY.md) — индексация, 10 high
11. [shamir-query-types](./shamir-query-types/SUMMARY.md) — DTO-слой запросов, 7 high
12. [shamir-client-node](./shamir-client-node/SUMMARY.md) — critical, но опциональный биндинг (не влияет на сервер/данные)
13. [shamir-client](./shamir-client/SUMMARY.md) — Rust SDK, 9 high, permanent-hang класс
14. [shamir-wasm-host](./shamir-wasm-host/SUMMARY.md) — sandbox-граница пользовательского кода, 7 high
15. [shamir-sdk](./shamir-sdk/SUMMARY.md) — WASM-side SDK, 6 high
16. [shamir-collections](./shamir-collections/SUMMARY.md) — фундамент (pillar-4 anchor), находок мало, но blast radius большой
17. [shamir-transport-tcp](./shamir-transport-tcp/SUMMARY.md) — 1 high, но реальный UB (`unsafe set_len`)
18. [shamir-transport-ipc](./shamir-transport-ipc/SUMMARY.md) — 1 high, новый транспорт, тихий panic в accept-loop
19. [shamir-transport-ws](./shamir-transport-ws/SUMMARY.md) — 3 high, spec/interop + непротестированный anti-CSWSH
20. [shamir-numa](./shamir-numa/SUMMARY.md) — 3 high, гонка репликации (не на пути записи данных)
21. [shamir-query-builder](./shamir-query-builder/SUMMARY.md) — 3 high, DX/строгость валидации
22. [shamir-query-builder-macros](./shamir-query-builder-macros/SUMMARY.md) — 3 high, кодоген-баги
23. [shamir-sdk-macros](./shamir-sdk-macros/SUMMARY.md) — 2 high, ноль тестов
24. [shamir-tunables](./shamir-tunables/SUMMARY.md) — 1 high, невыполненный API
25. [shamir-bench-utils](./shamir-bench-utils/SUMMARY.md) — 1 high, только dev-tooling

---

Каждый `<крейт>/SUMMARY.md` уже содержит executive summary, находки по 7 линзам
с severity/file:line/failure-scenario, таблицу счётчиков и Fix Plan (P0/P1/P2) —
именно по нему и идти при разборе конкретного крейта из списка 3.
