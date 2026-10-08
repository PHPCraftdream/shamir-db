<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# Приоритет крейтов — актуальная повторная проверка

Срез: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`, 2026-10-08. Все 25 ранее рассмотренных модулей и 189 Markdown-файлов ревью обновлены после read-only XS-проверок и принятия результатов. Это порядок последующей инженерной работы, а не разрешение на изменения кода или заявление о пройденных тестах. Сборки, компиляции, тесты, бенчмарки и воспроизведения не запускались; source, версии, commit и push не менялись.

[Общий свод и статусы](./SUMMARY.md#status-definitions) — основа этого документа. Старые raw-счётчики, style Critical/High и общие вердикты «не поставлять» не используются как формула ранжирования. Актуальный census уникальных багов не заявляется: строки линз, summary и групп часто повторяют один механизм.

## 1. Сложность последующей работы

Качественная оценка связанных состояний/контрактов и необходимости согласования. Она не равна числу исторических находок и не измеряет время.

| Крейт | Сложность | Причина |
|---|---|---|
| [shamir-engine](./shamir-engine/SUMMARY.md) | Высокая | Связанные commit/drain/A8/migration состояния и частично закрытые группы. |
| [shamir-index](./shamir-index/SUMMARY.md) | Высокая | Graph/delta/snapshot/compaction concurrency и durable форматы. |
| [shamir-tx](./shamir-tx/SUMMARY.md) | Высокая | MVCC publication, sparse versions, rollback/GC и lock lifecycle. |
| [shamir-wal](./shamir-wal/SUMMARY.md) | Высокая | Leader/cancellation/fsync/append/replay и метаданные сегментов. |
| [shamir-storage](./shamir-storage/SUMMARY.md) | Высокая | Несколько wrapper/cache backend контрактов, batch visibility и worker lifecycle. |
| [shamir-wasm-host](./shamir-wasm-host/SUMMARY.md) | Высокая | Реентерабельный fuel, host/guest ABI, compiler и teardown; координация SDK. |
| [shamir-db](./shamir-db/SUMMARY.md) | Высокая | Многошаговый каталог/DDL, durability и actor права. |
| [shamir-client](./shamir-client/SUMMARY.md) | Средняя–высокая | Resume/auth и конкурентное завершение запросов/подписок. |
| [shamir-connect](./shamir-connect/SUMMARY.md) | Средняя–высокая | Protocol/session/counter/audit контракты; часть old claims опровергнута. |
| [shamir-types](./shamir-types/SUMMARY.md) | Средняя–высокая | Общие codec/value/hash и legacy/typed wire контракты. |
| [shamir-query-types](./shamir-query-types/SUMMARY.md) | Средняя | Planner графы и nested confirmation gates; post-parse walk bounds. |
| [shamir-sdk](./shamir-sdk/SUMMARY.md) | Средняя | Guest ABI ownership/decode defaults; согласование с host/macros. |
| [shamir-server](./shamir-server/SUMMARY.md) | Средняя | Узкие gates/lifecycle, но важны handler mode, actor ACL и supervisor states. |
| [shamir-query-builder](./shamir-query-builder/SUMMARY.md) | Средняя | Nested alias scopes, mutator intent, exported macros и variant semantics. |
| [shamir-client-node](./shamir-client-node/SUMMARY.md) | Средняя | Адреса/таймауты/close ownership; alleged dead wrapper не требует fix. |
| [shamir-numa](./shamir-numa/SUMMARY.md) | Средняя | Mirror linearization и Linux mask, с учётом сериализованных live DDL. |
| [shamir-transport-ws](./shamir-transport-ws/SUMMARY.md) | Средняя | Config/upgrade/control backpressure и caller deadlines. |
| [shamir-transport-tcp](./shamir-transport-tcp/SUMMARY.md) | Средняя | Малый unsafe участок; обязательны initialization/cancellation контракты. |
| [shamir-transport-ipc](./shamir-transport-ipc/SUMMARY.md) | Средняя | Windows accept state и platform security/cleanup. |
| [shamir-sdk-macros](./shamir-sdk-macros/SUMMARY.md) | Средняя | Pattern/signature/diagnostics и guest error ABI. |
| [shamir-query-builder-macros](./shamir-query-builder-macros/SUMMARY.md) | Средняя | Call paths/grammar/diagnostics; performance backend-dependent. |
| [shamir-funclib](./shamir-funclib/SUMMARY.md) | Средняя | Input/output caps и semaphore predicate/wait; метрики не измерялись. |
| [shamir-collections](./shamir-collections/SUMMARY.md) | Низкая–средняя | Delegated contracts, прямые oracle тесты и qualified hashing boundary. |
| [shamir-tunables](./shamir-tunables/SUMMARY.md) | Низкая–средняя | Честный inert/live контракт, domain policy и sampling semantics. |
| [shamir-bench-utils](./shamir-bench-utils/SUMMARY.md) | Низкая–средняя | Dev measurement ownership, parameters/reproducibility и layout. |

## 2. Архитектурная важность

Ориентир по радиусу воздействия, а не утверждение, что каждый лист зависит от каждого фундаментального крейта. Опциональные транспорты/клиенты и прямые library API имеют отдельную область применимости; инфраструктурный крейт тоже может влиять на корректность.

- Фундамент: [shamir-types](./shamir-types/SUMMARY.md), [shamir-collections](./shamir-collections/SUMMARY.md).
- Хранение и durability: [shamir-storage](./shamir-storage/SUMMARY.md), [shamir-wal](./shamir-wal/SUMMARY.md).
- Транзакционное ядро: [shamir-tx](./shamir-tx/SUMMARY.md), [shamir-engine](./shamir-engine/SUMMARY.md).
- Запросы/индексы/скаляры: [shamir-query-types](./shamir-query-types/SUMMARY.md), [shamir-query-builder](./shamir-query-builder/SUMMARY.md), [shamir-query-builder-macros](./shamir-query-builder-macros/SUMMARY.md), [shamir-index](./shamir-index/SUMMARY.md), [shamir-funclib](./shamir-funclib/SUMMARY.md).
- Каталог/DDL: [shamir-db](./shamir-db/SUMMARY.md).
- Сетевой и auth периметр: [shamir-connect](./shamir-connect/SUMMARY.md), [shamir-server](./shamir-server/SUMMARY.md), [shamir-transport-tcp](./shamir-transport-tcp/SUMMARY.md), [shamir-transport-ws](./shamir-transport-ws/SUMMARY.md), [shamir-transport-ipc](./shamir-transport-ipc/SUMMARY.md).
- WASM host: [shamir-wasm-host](./shamir-wasm-host/SUMMARY.md).
- Клиенты и guest SDK: [shamir-client](./shamir-client/SUMMARY.md), [shamir-client-node](./shamir-client-node/SUMMARY.md), [shamir-sdk](./shamir-sdk/SUMMARY.md), [shamir-sdk-macros](./shamir-sdk-macros/SUMMARY.md).
- Поддержка: [shamir-numa](./shamir-numa/SUMMARY.md), [shamir-tunables](./shamir-tunables/SUMMARY.md), [shamir-bench-utils](./shamir-bench-utils/SUMMARY.md).

## 3. Рекомендуемый порядок — механизм и риск, не raw totals

Первые позиции объединяют видимость/сохранность данных, незавершающиеся операции, раскрытие bearer credential и unsafe boundary. Условные API/конфигурации, legacy policy и отсутствие измерений остаются частью оценки. Пункты можно вести параллельно, если изменения не делят контракт или файлы.

| # | Крейт | Актуальная причина | Open-H строк summary | Дополнительных строк |
|---:|---|---|---:|---:|
| 1 | [shamir-wal](./shamir-wal/SUMMARY.md) | Commit liveness and failed-append/replay semantics remain open; stale sidecar unlink failures can invalidate truncation safety. | 4 | 1 |
| 2 | [shamir-storage](./shamir-storage/SUMMARY.md) | Read-fill/cache-publication races and worker-death flush hangs remain; version envelopes are partial compatibility remediation. | 4 | 2 |
| 3 | [shamir-tx](./shamir-tx/SUMMARY.md) | Live cell-version regression, failed history writes masking older values, journal error handling and GC costs remain. | 4 | 0 |
| 4 | [shamir-engine](./shamir-engine/SUMMARY.md) | Many August mechanisms are source-fixed; pre-read, A8, migration replay/page, orphan-test and initialization-lifecycle residuals remain. | 3 | 8 |
| 5 | [shamir-index](./shamir-index/SUMMARY.md) | Functional hash collapse, Unicode tokenization, metadata error swallowing and vector persistence/compaction remain; snapshot absorption lacks an applied-graph barrier. | 6 | 1 |
| 6 | [shamir-client](./shamir-client/SUMMARY.md) | Resume discloses a bearer ticket before peer identity verification; disconnect/request/subscription and reference-walk defects remain. | 5 | 2 |
| 7 | [shamir-funclib](./shamir-funclib/SUMMARY.md) | Authorized scalar paths retain uncapped recursion/output allocation and a semaphore lost wakeup; universal deployment and numerical claims are qualified. | 8 | 1 |
| 8 | [shamir-transport-tcp](./shamir-transport-tcp/SUMMARY.md) | Fresh/grown receive-buffer initialization is unsound; cancelled production buffers are dropped, so remote disclosure is not proven. | 1 | 0 |
| 9 | [shamir-db](./shamir-db/SUMMARY.md) | Wrong-database cascade and catalogue error/order defects remain; ACL dedup is fixed, while arbitrary curl-directive injection is unverified. | 7 | 0 |
| 10 | [shamir-wasm-host](./shamir-wasm-host/SUMMARY.md) | Aggregate fuel, compiler scanning/offloading and duplicate HTTP headers remain. Async imports already suspend fibers; boxed handlers are required by the pin. | 8 | 0 |
| 11 | [shamir-types](./shamir-types/SUMMARY.md) | Signed-zero Hash/Eq and header-driven allocation remain. Lossy string/list projection is deliberate; typed-RHS/docs concerns are not blanket wire corruption. | 5 | 0 |
| 12 | [shamir-server](./shamir-server/SUMMARY.md) | ReadOnly handler transaction writes lack the gate, conditional on explicit mode/write ACLs; stock launcher is ReadWrite. Supervisor scenarios are narrower. | 2 | 1 |
| 13 | [shamir-connect](./shamir-connect/SUMMARY.md) | N-racer fetch_max refill allegation is refuted. Subnet watermark, login scans, resume/session/audit retention and API-gate discrepancies remain. | 3 | 2 |
| 14 | [shamir-transport-ipc](./shamir-transport-ipc/SUMMARY.md) | Windows retry panics after accept leaves next=None; the supposed 255-instance cap is false. Platform cleanup/security guarantees need qualification. | 1 | 0 |
| 15 | [shamir-transport-ws](./shamir-transport-ws/SUMMARY.md) | Configured paths are not wired; subprotocol/control-frame/backpressure gaps remain. Browser coverage exists; exporter fallback is not a browser-mode downgrade. | 1 | 0 |
| 16 | [shamir-query-builder](./shamir-query-builder/SUMMARY.md) | Nested validation, alias-state loss and macro hygiene remain. Zeroize-disabled framing is refuted; deep-input codec-to-panic paths are source-proven. | 2 | 0 |
| 17 | [shamir-query-types](./shamir-query-types/SUMMARY.md) | Combined filter/value depth checks are fixed; unlimited decoding and trailing TableRef acceptance are refuted. Nested gates, IDs and API gaps remain. | 0 | 0 |
| 18 | [shamir-sdk](./shamir-sdk/SUMMARY.md) | Encoded buffers persist within an invocation, but fresh host Stores reclaim them between calls. Decode defaults and polling need narrower contracts/oracles. | 1 | 0 |
| 19 | [shamir-client-node](./shamir-client-node/SUMMARY.md) | Dead-wrapper critical is refuted by exact N-API source. Address, timeout, request/close locking and typed API gaps remain; raw repl errors are intentional. | 2 | 0 |
| 20 | [shamir-numa](./shamir-numa/SUMMARY.md) | Concurrent library mirror writers can diverge; inspected normal engine DDL is serialized. Exact CPU_SET bounds panic is proven; workflow/doctest false positives are removed. | 3 | 0 |
| 21 | [shamir-query-builder-macros](./shamir-query-builder-macros/SUMMARY.md) | Silent malformed-group truncation is refuted by syn. Call lowering and diagnostic/grammar gaps remain; compiler-backend timing is unverified. | 0 | 0 |
| 22 | [shamir-sdk-macros](./shamir-sdk-macros/SUMMARY.md) | Patterns, qualified returns, decode/error classification and UI/boundary gaps remain. Existing function ABI coverage and host metering are acknowledged. | 0 | 0 |
| 23 | [shamir-collections](./shamir-collections/SUMMARY.md) | Contract/docs/removal coverage remains. Practical FxHash collision amplification is unverified; constructor/test-oracle explanations are corrected. | 0 | 0 |
| 24 | [shamir-tunables](./shamir-tunables/SUMMARY.md) | Runtime foundation remains unwired and the environment promise phantom. Domain/docs need work; zero-cap deadlock and mandatory layout/symmetry claims are refuted. | 0 | 0 |
| 25 | [shamir-bench-utils](./shamir-bench-utils/SUMMARY.md) | Dev allocator/fixture/metadata/test gaps remain. Exact reset race is proven; cancellation carry-over and mandatory coupled-export splitting are refuted. | 0 | 0 |

Open-H — подтверждённые/частично исправленные High/Critical строки утверждений; повторы не становятся новыми багами, unverified не входит в счётчик. Дополнительные строки тоже могут повторять cross-module root. Число строк не определяет порядок, сложность или одинаковую production-экспозицию.

## Ограничения и изменения к старому порядку

- Node больше не ранжируется по alleged critical «мёртвая обёртка»: точные N-API исходники создают через JS receiver конструктора. Raw repl Error — существующий контракт, а не обязательный exception fix.
- Silent token-drop в macros опровергнут pinned syn. Negative diagnostics и call lowering актуальны, но это другой риск.
- rmp-serde decoder имеет 1024-container depth counter, без универсальной гарантии stack safety. Encoder depth поле не активно: глубокие локально построенные значения могут дать decode-error → panic в builder roundtrip.
- Session fetch_max — атомарный RMW; N-racer refill multiplication неверен. Отдельные subnet watermark, audit retention и session-cap scan этим не закрываются.
- Engine: 14 из 31 групп source-fixed, 16 partial, 1 open. Historical verification notes не означают актуальные прогоны; два добавленных regression файла не зарегистрированы.
- ReadOnly bypass требует handler mode и write ACL; обычный launcher остаётся ReadWrite. Journal-gap обычно чистится periodic reconciliation, но active-state exit/early resume требуют отдельного решения.
- Legacy buffer-config rejection — выбранное breaking fail-closed поведение, а не случайное silent corruption. Для обновления существующих данных нужен migration/notice контракт; envelope сам его не доказывает.
- Guest leaks ограничены жизнью свежего Store, finite Pending может завершиться, async imports уже используют fiber suspension. Это не закрывает intra-call growth и polling/fuel accounting.
- Boxed future нужен Wasmtime 46.0.2; простой unboxed closure несовместим. Не планировать этот fix без альтернативного доказанного API.
- NUMA library race подтверждена, но inspected normal engine DDL сериализован. libc CPU_SET вне 1024 storage bits вызывает Rust bounds panic, не заявленный C macro stack overwrite.
- IndexMap remove/swap/shift, QueryRecord batching, restore_cell и parent fuel reservation должны сохранять свои семантики. Исторические рецепты в свёрнутых разделах не являются актуальными инструкциями.
- Style/coverage откалиброваны: closely-coupled groups и downstream тесты учтены. Версии проекта/библиотек не повышались и не подразумеваются этим планом.

Каждый модульный SUMMARY.md содержит актуальный ledger утверждений и планов, доказательства, контр-доказательства и ограничения. Для исправлений берётся текущая часть, не старые P0/P1/P2 рецепты.

---

<details>
<summary>Исторический порядок — сохранён для происхождения; не текущий приоритет</summary>

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


</details>
