<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# Приоритеты после второго независимого цикла

Снимок: `e3765c935fc71655ee1ec0160cf180607935d89b`; дата: 2026-10-08. Все 25 свежих XS-ревью завершены; приняты все 187 модульных документов и обновлены 189 отчётов с двумя сводками. [Методика, ограничения и текущие числа](SUMMARY.md).

Это порядок последующей работы, не выполненные исправления и не разрешение менять код, запускать тесты, повышать версии, делать коммит или пуш. В этом цикле ничего из перечисленного не выполнялось. Внешние изменения Cargo/toolchain/CI/Docker и более поздние Rust-правки сохранены и не сертифицированы; ревью и ссылки на исходниковые доказательства относятся к замороженному снимку, не к изменённому рабочему дереву.

2635 утверждений, 447 пунктов планов и 94 строки наблюдений включают повторы и общие причины. Из наблюдений 76 добавлены в этом цикле; это не 76 уникальных багов. Старые P0/P1/P2 и исторические суммы не задают текущий релизный порог. Закрытая исходная причина не доказывает полную семантическую корректность нового решения.

## Порядок и условия приёмки

| Очерёдность | Модуль | Подтверждённая причина приоритета | Безопасная область следующей работы |
|---:|---|---|---|
| 1 | [shamir-storage](shamir-storage/SUMMARY.md) | Outstanding drain order can regress successfully flushed data; exact native batch visibility and journal-error propagation violate advertised guarantees. | Value/order preservation, honest atomicity and errors, backing-plus-eviction and journal replay oracles. |
| 2 | [shamir-tx](shamir-tx/SUMMARY.md) | History deferral is accepted as persistence, one-table drain can finalize a shared multi-table version, and live-snapshot floors can race. | Repository-wide completion, retention/GC, lock-mode preservation, gap and timestamp contracts. |
| 3 | [shamir-wal](shamir-wal/SUMMARY.md) | Failure/cancellation leadership and sealed-corruption/truncation eligibility remain integrity-sensitive. | Settle every waiter, own blocking work, detect sealed corruption and retain every unsafe segment. |
| 4 | [shamir-engine](shamir-engine/SUMMARY.md) | A8 coverage, malformed-body screening and attach rollback remain open; new membership and registry consistency regressions qualify earlier optimizations. | Retain specific source-fixed mechanisms, repair normal executable-value guards and register discriminating production-seam oracles. |
| 5 | [shamir-index](shamir-index/SUMMARY.md) | Snapshot absorption/pruning and restart delta reseeding can omit committed mutations; functional lookups rely on colliding fingerprints. | Applied/recovered watermarks, ordered replay, lawful keys, complete temp-file ownership and failure outcomes. |
| 6 | [shamir-client](shamir-client/SUMMARY.md) | Resume exposes bearer material before endpoint identity verification; lifecycle admission, sparse cache epochs and raw-decode boundaries remain open. | Authenticate before credentials, coordinate write cancellation/close/pending settlement and complete dictionary knowledge. |
| 7 | [shamir-db](shamir-db/SUMMARY.md) | Catalogue recovery/durability and successful schema-preserving rename are incomplete; concrete conditional ACL grants and cross-target/cross-owner operations remain. | Recover before serving-set construction, preserve bindings/metadata, fail closed on real lookup/corruption errors and inspect recursive wire paths. |
| 8 | [shamir-client-node](shamir-client-node/SUMMARY.md) | Mutable Buffer aliases cross async workers and valid integer data loses fidelity; availability and error-marker discrimination need real wrapper oracles. | Synchronous ownership snapshots, exact numeric encoding, terminal lifecycle and documented subset/builder compatibility. |
| 9 | [shamir-server](shamir-server/SUMMARY.md) | Response reservation ownership ends before write handoff, proof-stage admission has no timeout, and shutdown does not quiesce accepted work. | Keep guards through writes, bound the complete auth lifecycle, refresh security decision time and drain/cancel with defined commit outcomes. |
| 10 | [shamir-transport-tcp](shamir-transport-tcp/SUMMARY.md) | The safe pooled-read API violates initialization contracts; TLS proof callbacks and caller-specific handshake limits/oracles remain incomplete. | Initialize soundly, preserve terminal framing/cancellation, enforce phase caps and verify exact rejection causes. |
| 11 | [shamir-wasm-host](shamir-wasm-host/SUMMARY.md) | Aggregate fuel and compiler/macro authority/lifecycle gaps persist; mixed async-import getters fail and host/guest value contracts diverge. | Reentrant/cancel-safe accounting, async allocator reentry, bounded process ownership and coordinated codec semantics. |
| 12 | [shamir-funclib](shamir-funclib/SUMMARY.md) | A source-proven semaphore lost notification can strand work; scalar replacement/comparison and canonical value semantics need preservation. | Predicate/wait protocol, lawful equivalence and versioned index semantics; qualify structural-only optimizations. |
| 13 | [shamir-types](shamir-types/SUMMARY.md) | Interner tuple-count progress can skip durable mappings; publication generations, repeated-ID views, signed-zero hashing and codec boundaries diverge. | Complete ID-based progress, trait laws, strict accepted-input policy and fallible/bounded materialization. |
| 14 | [shamir-connect](shamir-connect/SUMMARY.md) | Audit retention and source-proven session/ticket policy gaps remain; published canonical/HMAC and normalization statements need independent oracles. | Honest persistence outcomes, active-session admission, cache-key lifecycle and interoperable durable bytes. |
| 15 | [shamir-transport-ipc](shamir-transport-ipc/SUMMARY.md) | Windows accept ownership is lost on errors/cancellation; Unix cleanup can leak or remove replacement endpoints. | Reusable listener state, endpoint ownership, logical addressing and effective ACL/final-handle tests. |
| 16 | [shamir-transport-ws](shamir-transport-ws/SUMMARY.md) | Configured paths/subprotocol negotiation and logical prefix headroom diverge; control buffering and close/Origin/heartbeat obligations are incomplete. | Complete auth deadline, bounded output/teardown, exact upgrade and close-code oracles; no reserved 1006 transmission. |
| 17 | [shamir-query-types](shamir-query-types/SUMMARY.md) | Current DTO semantics and new trait/budget/oracle witnesses remain; earlier unsupported schema/style mandates were removed. | Sibling virtual budgets, exact wire fixtures, lawful opaque-row equality and scoped security fields. |
| 18 | [shamir-query-builder](shamir-query-builder/SUMMARY.md) | Scoped alias/marker validation, silent registration/mutator loss and production SELECT/HAVING reference supply disagree. | Preserve actual execution scopes and result projections, hygienic exports, fallible local helpers and real integration oracles. |
| 19 | [shamir-sdk](shamir-sdk/SUMMARY.md) | Malformed/codec fallback, invocation-local retention and raw-filter selection remain; mixed-import and value-mirroring boundaries are source-proven. | Preserve sentinels/errors/owned buffers and strict host boundaries; keep alpha/API enhancements optional. |
| 20 | [shamir-numa](shamir-numa/SUMMARY.md) | Library mirror convergence and fixed-mask Result guarantees fail; physical-locality and smoke/host-test assurances are too broad. | Publication-ordered mirrors, target-correct mask errors and distinct-node/eligible-host oracles; do not assume stock DDL races. |
| 21 | [shamir-query-builder-macros](shamir-query-builder-macros/SUMMARY.md) | Builder-only paths and raw identifier names are incorrect; exact backend prefix work is now source-confirmed. | Real consumer and negative fixtures, correct name/segment lowering and bounded diagnostics; measure speed separately. |
| 22 | [shamir-sdk-macros](shamir-sdk-macros/SUMMARY.md) | Pattern/type acceptance and raw-name/hygiene defects remain; deliberate guest errors and malformed ABI inputs lose their intended semantics. | Real generated guest/UI tests, spanned errors, identified result protocol and explicit pointer ownership. |
| 23 | [shamir-collections](shamir-collections/SUMMARY.md) | Direct contract/oracle documentation and caller allocation/removal obligations remain; string HashDoS amplification is still unverified. | Strict decode policy and lawful input trust, ordered/duplicate tests; optional façade/constructor/lint redesign is N/A. |
| 24 | [shamir-tunables](shamir-tunables/SUMMARY.md) | A nonexistent startup environment override and undocumented Duration conversion remain; production consumption is deliberately deferred. | Correct the promise, define precision/range and boundary tests; do not commission a new live cascade implicitly. |
| 25 | [shamir-bench-utils](shamir-bench-utils/SUMMARY.md) | Global measurement ownership/metric and allocator behavior can distort development results; float/helper and provenance oracles are incomplete. | Opt-in instrumentation, independent reset/math fixtures and local-input/provenance contracts; no fabricated measurements. |

## Сквозные зависимости

- Начать с полноты долговечности и границ памяти/владения: storage → tx/drainer → engine/index/catalogue. Возврат `Ok`, логирование и локальное очищение флага не равны сохранению всех данных или завершению репозиторного состояния.
- Проверять авторизацию на фактическом пути вызова и при ошибках. Верхний wire-admin gate, внутренний ForEach dispatcher, встроенные привилегированные API и обычные пользовательские полномочия — разные границы.
- Совместно менять host/SDK/клиентские кодеки, если меняется контракт. Сохранить успешный Null, сырые repl-ответы, точные целые, ключи/порядок, старые обещанные форматы и ошибки после уже состоявшегося коммита.
- Для каждой правки нужна различающая проверка реального механизма: управляемое чередование, повторное открытие после новой записи, независимые wire/математические ожидания, точные ошибки и исходы. Наличие файла, симметричный roundtrip, mock с неверной capability и bare `is_err` недостаточны.
- Только после исправления подтверждённых причин оценивать измеренную выгоду оптимизаций. Исторические проценты/тайминги, объявленная версия toolchain, регистрация тестов и исходниковые оценки сложности не являются выполненными измерениями или зелёным прогоном.
- Не вводить несовместимые ограничения вместо исправления: обязательный WHERE для разрешённого bulk UPDATE, запрет документированных ID/слотов, новый untagged envelope, изменение существующих имён/чисел/представления и удаление публичных путей требуют отдельного решения.

Каждый модульный SUMMARY.md содержит актуальный ledger, источники, контр-доказательства, ограничения и уточнённые рецепты. Брать задачи из текущей части, а не из свёрнутого исторического текста. Для реализации потребуется отдельная команда пользователя; неожиданные фактические падения тестов при разрешённом прогоне не откладываются.

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
