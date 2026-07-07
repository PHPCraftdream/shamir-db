בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Ревью клиентской поверхности S.H.A.M.I.R. — контракты, parity, error-surface, DX

_Агент: @fxx (max effort), 2026-07-06. Часть панели из 5 агентов ревью проекта после завершения векторной кампании._

Источник истины: `crates/shamir-query-types` (75 вариантов `BatchOp` + транспортные `DbRequest`/`DbResponse`). Сверены четыре поверхности: Rust-билдер (`shamir-query-builder`), Rust-клиент (`shamir-client`), TS SDK (`shamir-client-ts`), napi-биндинг (`shamir-client-node`), плюс `tests/e2e` как потребитель контракта.

## Таблица покрытия op'ов

Обозначения: ✓ полно · ⚠ drift (детали в секциях) · ✗ отсутствует. Колонка node-typings: биндинг не типизирует op'ы вообще — `execute(db, batch: object)` (`index.d.ts:67`), поэтому «○» = проходит как нетипизированный object.

| Op / область (wire) | wire | rust-builder | ts-builder | node-typings | e2e |
|---|---|---|---|---|---|
| Read: select/where/group_by/order_by | ✓ | ✓ | ✓ | ○ | ✓ (02,05,06,07) |
| Read: pagination LimitOffset | ✓ | ✓ | ✓ | ○ | ✓ (07) |
| Read: pagination Page | ✓ | ✓ | ✓ | ○ | ✗ |
| Read: pagination After (keyset) | ✓ | ✓ | ✓ | ○ | ✗ |
| Read: temporal as_of / history | ✓ | ✓ | ✓ | ○ | ✗ |
| Read: with_version | ✓ | ✓ | ✓ | ○ | ✗ |
| Read: **explain** (+ ExplainPlan в ответе) | ✓ | ✓ (`query.rs:291`) | **✗ drift** | ○ | ✗ |
| Insert (values, select-projection) | ✓ | ✓ | ✓ | ○ | ✓ (02) |
| Insert: **records_idmsgpack** (v2) | ✓ | ✓ (`insert.rs:63`) | **⚠ поле объявлено не там** | ○ | ✗ (только lib-тесты Rust) |
| Update / Set(upsert) / Delete (+returning) | ✓ | ✓ | ✓ | ○ | ✓ (02) |
| Batch: transactional/isolation | ✓ | ✓ | ✓ | ○ | ✓ (15) |
| Batch: **durability** | ✓ (`buffered/synced/async_index`) | ✓ | **⚠ нет `async_index`** | ○ | ✗ |
| Batch: **limits** | ✓ (5 полей) | ✓ | **✗ ломает запрос** (4 поля) | ○ | ✗ |
| Batch: return_all/return_only/return_result/after | ✓ | ✓ | ✓ | ○ | ⚠ частично (04) |
| Batch: interner_epochs / result_encoding | ✓ | ✓ | ✓ | ✗ (нет в биндинге) | ✗ |
| SubBatch (bind/$param) | ✓ | ✓ | ✓ | ○ | ✗ |
| Filters: eq..lte/like/in/between/exists/and/or/not | ✓ | ✓ | ✓ | ○ | ✓ (05) |
| Filters: fts | ✓ | ✓ | ✓ | ○ | ✓ (14) |
| Filters: vector_similarity (k/ef_search/oversample) | ✓ | ✓ | ✓ | ○ | ✓ (18) + **parity-фикстура** |
| Filters: computed / $ref/$query/$fn/$expr/$cond | ✓ | ✓ | ✓ | ○ | ⚠ (04,14 частично) |
| DDL: db/repo/table (+if_not_exists/cascade/schema/retention) | ✓ | ✓ | ✓ | ○ | ✓ (08,12) |
| DDL: create_index (vector_dim/metric/quantization/include) | ✓ | ✓ | ✓ | ○ | ✓ (14,18) |
| DDL: rename_db/repo/table/index | ✓ | ✓ | ✓ | ○ | ⚠ частично |
| Buffer config (set/get/alter) | ✓ | ✓ | ✓ | ○ | ✓ (11) |
| List (9 вариантов) | ✓ | ✓ | ✓ | ○ | ✓ (08) |
| Migration (start/commit/rollback/status) | ✓ | ✓ | ✓ | ○ | ✓ (13) |
| Auth RBAC (create_user/role, grant/revoke, drop+hmac) | ✓ | ✓ | ✓ | ○ | ✗ |
| ACL (chmod/chown/chgrp/groups/access_tree) | ✓ | ✓ | ✓ | ○ | ⚠ (chmod только как фикстура в 16) |
| Functions/validators/folders | ✓ | ✓ | ✓ | ○ | ✗ |
| Schema DDL (set/add/remove/get, describe_table) | ✓ | ✓ | ✓ | ○ | ✗ |
| Temporal admin (set_retention/purge_history/changes_since) | ✓ | ✓ | ✓ | ○ | ✗ |
| Interner (dump/touch) | ✓ | ✓ | ✓ | ○ | ✗ |
| Call | ✓ | ✓ (`batch.rs:555`) | ✓ | ○ | ✗ |
| Subscribe/Unsubscribe + push-роутинг | ✓ | ✓ (`batch.rs:628`; `subscribe_push`) | ✓ (router+handle) | **✗ push невозможен** | ✗ (нет e2e подписок) |
| Replication DDL (10 op'ов) | ✓ | ✓ | ✓, **но не входит в `BatchOpInput`** | ○ | ✓ (16,17) + **parity-фикстура** |
| Транспорт: Execute / Ping | ✓ | ✓ | ✓ (**query_version=1**) | ✓ | ✓ |
| Транспорт: TxBegin/TxExecute/TxCommit/TxRollback | ✓ | **✗ нет в shamir-client** | ✓ | **✗** | ✓ (15 — raw) |
| Транспорт: CreateScramUser | ✓ | ✓ | ✓ (`{name,user_id}`) | ⚠ (`Buffer` — другая форма) | ✓ (01) |
| Транспорт: resume (ticket) | ✓ | ✓ (читает server_query_version) | **⚠ не читает server_query_version** | **✗ resume отсутствует** | ✗ |
| Транспорт: Repl (pull-API) | ✓ | ✓ | ✗ (нет в ws-клиенте) | ✓ (Buffer↔Buffer) | ✓ (16,17) |

Parity-тесты байтовой идентичности Rust↔TS: **только 2 набора фикстур** — `crates/shamir-query-builder/tests/fixtures/vector_filter_msgpack.json` (3 кейса) и `repl_ddl_msgpack.json` (10 кейсов). CRUD, batch-конверт, DDL, фильтры (кроме vector), подписки, temporal, ACL — без кросс-языковой parity. Отдельно: **e2e гоняет только node-биндинг с raw-JSON объектами** (`tests/e2e/e2e.test.js:21`) — TS SDK (ws-транспорт, SCRAM на JS, билдеры, executeWithTouch) не имеет ни одного теста против живого сервера.

---

## 1. ОБЯЗАНЫ УЛУЧШИТЬ — расхождения контрактов

**1.1 [CRITICAL] TS `BatchLimits` без `max_nesting_depth` — `.limits()` гарантированно роняет запрос.**
Rust: `crates/shamir-query-types/src/batch/batch_limits.rs:47` — 5 обязательных полей, ни одного `#[serde(default)]` на полях. TS: `crates/shamir-client-ts/src/core/types/batch.ts:89-94` и `builders/batch.ts:34-39` (`DEFAULT_LIMITS`) — 4 поля. Сценарий: любой вызов `Batch.limits({max_queries: 20})` отправляет объект без `max_nesting_depth` → сервер (`shamir-server/src/db_handler/handler.rs:251`) отвечает `invalid_request: missing field 'max_nesting_depth'` — падает весь батч, с невнятной protocol-ошибкой. Юнит-тест `builders/__tests__/batch.test.ts:143` **закрепляет неправильную форму как эталон**. Фикс: добавить поле в тип+дефолты (4) и в parity-фикстуру; долгосрочно — `#[serde(default)]` на каждое поле `BatchLimits` (клиент шлёт только то, что сузил).

**1.2 [HIGH] `query_version` — TS клиент лжёт о версии и молча даунгрейдится после resume.**
Wire: `CURRENT_QUERY_LANG_VERSION = 2` (`shamir-query-types/src/wire/db_message.rs:19`). TS шлёт захардкоженный `query_version: 1` в `execute` (`client.ts:484`), `txBegin` (`:736`), `txExecute` (`:763`) — при этом **реально использует v2-фичи** (`records_idmsgpack`, `result_encoding:'id'`). Сегодня сервер принимает обе версии и не гейтит поля по версии (`shamir-server/src/version.rs:45-54`), но при первом же реальном гейте v1-декларация сломает TS. Плюс: `ShamirClient.resume()` (`client.ts:194-227`) не читает `server_query_version` из resume_ok (в wire он есть: `shamir-client/src/wire_frames.rs:63`) → после реконнекта `_serverQueryVersion = 0` и клиент **молча теряет весь id-on-wire путь**. Rust-клиент делает правильно (`client.rs:701` — шлёт CURRENT; resume читает поле). При mismatch старый клиент получает `DbResponse::Error{code:"unsupported_query_version"}` — но в TS это просто строка (см. 2.1). Фикс: слать `CURRENT_QUERY_LANG_VERSION`, экспортировать константу из одного места, читать поле в resume, e2e-тест на resume→v2.

**1.3 [HIGH] Дрейф wire-полей в TS-типах.**
- `ReadQuery.explain` есть в Rust (`read_query.rs:44-45`), отсутствует в TS `ReadQuery` (`types/query.ts:152-162`) и в билдере (`builders/query.ts` — нет `.explain()`); `QueryResult.explain: ExplainPlan` (`query_result.rs:80-82`) отсутствует в TS `QueryResult` (`types/batch.ts:168-173`). EXPLAIN недоступен TS-пользователю вообще.
- `DurabilityLevel` в TS = `'buffered'|'synced'` (`types/batch.ts:84`) — нет `'async_index'` (`batch_request.rs:76-79`, Rust-билдер `durability.rs` имеет `AsyncIndex`).
- `records_idmsgpack` объявлен в TS **на уровне `BatchRequest`** (`types/batch.ts:131`) — в Rust это поле `InsertOp` (`write/types.rs:93`); собственный jsdoc-коммент прямо противоречит объявлению («Present per query-entry, not at batch level»). Runtime-код (`client.ts:703`) кладёт правильно — врёт только тип.
- `DbResponse::Error` doc-список кодов (`db_message.rs:146-148`) не содержит фактически отправляемых `read_only_replica`, `access_denied`, `nesting_too_deep`, `tx_*`, `bad_role`, `unsupported_query_version` (см. `handler.rs:456-483`).

**1.4 [HIGH] Поведенческое расхождение Rust vs TS `executeWithTouch`.**
Rust всегда ставит `result_encoding = Id` на v2 (`shamir-client/src/interner_cache_ops.rs:412`); TS пропускает id-encoding, если в батче есть `$query`/`$param`/sub-batch (`client.ts:707-712`, комментарий: «those rely on server-side intermediate results staying name-keyed»). Одно из двух неверно: либо у Rust-клиента латентный баг (батч с `$query`-ref + id-кодирование ломает резолв путей), либо TS зря деградирует. Кросс-клиентского теста нет. Фикс: server-side тест «$query ref + result_encoding=Id», затем выровнять клиентов.

**1.5 [MED] `ReplicationOp` не входит в TS `BatchOpInput`** (`types/batch.ts:52-63`) — `batch.add('p', replication.publication(...))` не компилируется без `as`. Rust-билдер принимает их через `IntoBatchOp`. Фикс: добавить `| ReplicationOp` в union.

**1.6 [MED] Node-биндинг — усечённая поверхность при полном Rust-ядре.**
`lib.rs` не экспортирует: `resume` (при этом `resumptionTicket()` отдаёт тикет, использовать который **некуда** — API-тупик, `index.d.ts:55`), `server_query_version`, `execute_with_touch`/interner, `subscribe_push` (подписку можно оформить, но push-фреймы получить нельзя — подписки на node мертвы), интерактивные tx (e2e 15 делает их… через что? — raw `execute` с `transactional:true`, интерактивный TxBegin недоступен), `hmacTagHex` (e2e вынужден дублировать HMAC-дериватор на JS — `tests/e2e/helpers/hmac.js:26-31`). `index.d.ts` относительно честен для того, что есть (`execute(object)` описывает msgpack-обёртку из `index.js:61-67`, `repl: Buffer→Buffer` честно), но: сигнатура `createScramUser → Promise<Buffer>` расходится с TS SDK (`{name, user_id}`) — два JS-клиента с разными формами одного ответа. Паттерн `class ShamirClient extends native.ShamirClient` поверх `#[napi(factory)]` работает на текущем napi-rs, но это документированно-хрупкая зона (factory-инстанцирование подкласса) — закрепить smoke-тестом на прототип.

**1.7 [MED] node `connect` не принимает DNS-имена.** `lib.rs:100` — `format!("{}:{}", host, port).parse::<SocketAddr>()` принимает только IP; `index.d.ts:13` обещает `"db.example.com"`. Фикс: `tokio::net::lookup_host`.

**1.8 [MED] Дыры parity/e2e** (см. таблицу): нет байтовых фикстур для CRUD/batch-конверта/temporal/pagination/subscribe/ACL/schema-DDL; e2e не покрывает subscribe/unsubscribe как фичу, RBAC/ACL, schema DDL, temporal, Page/After, call/validators/functions, result_encoding, durability. Фикс-эскиз: один манифест `wire_fixtures/*.json` на **все** op'ы (генерится Rust-тестом, потребляется vitest'ом — как уже сделано для vector/repl), плюс e2e-прогон TS SDK (ws) рядом с node.

## 2. ГДЕ ОСТОРОЖНОСТЬ УПУСКАЕТ

**2.1 [HIGH] Error-surface: типизация умирает на границе JS.**
Сервер выдаёт богатый словарь кодов (`permission_denied`, `access_denied`, `read_only_replica`, `limits`, `timeout`, `lock_timeout`, `tx_conflict`, `bad_hmac`, `fk_*`, `unsupported_query_version`, …). Rust-клиент сохраняет их типизированно (`ClientError::Db{code,message}`, `shamir-client/src/error.rs:24-25`). Но: node-биндинг схлопывает в строку `to_napi(e) = Error::from_reason(e.to_string())` (`lib.rs:264-266`); TS ws-клиент — `new Error(\`db error [${code}]: ${message}\`)` (`client.ts:353-360`). Программно отличить retry-able (`timeout`, `lock_timeout`, `tx_conflict`, `read_only_replica`→redirect) от фатальной (`validation`, `permission_denied`) можно только regex'ом по message — что e2e и делает (`tests/e2e/tests/09-errors.test.js:18,58` — тест сам документирует дефект). Фикс: `class ShamirDbError extends Error { code; retryable }` в TS; в napi — `Error` с `code`-property (napi-rs поддерживает reason+code), плюс экспорт таблицы кодов из одного источника.

**2.2 [HIGH] Таймауты/реконнект: их нет — клиент может висеть вечно.**
Rust: `roundtrip` ждёт oneshot без таймаута (`client.rs:827`); TS: `sendDbRequest` регистрирует pending без дедлайна (`client.ts:404-430`), `readLoop` реджектит только при закрытии сокета. Server-side `max_execution_time_secs` спасает только `Execute`; `Ping`/`CreateScramUser`/`TxCommit` и потерянный сервером rid → вечный await. Connect-таймаута нет (`platform/node.ts:107-125` — `ws.once('open'|'error')`, TCP-hang = hang), heartbeat нет, авто-resume нет (тикет есть, политика — на пользователе). WS URL захардкожен `wss://…/shamir/v1/browser` (`client.ts:119`) — ни порт-путь, ни ws:// для dev. Фикс-эскиз: `opts.requestTimeoutMs` (default ~35s > серверного max_execution_time), `connectTimeoutMs`, таймер в pending-слоте с reject `ShamirTimeoutError{retryable:true}`.

**2.3 [MED] Молчаливые дефолты/валидация — всё уезжает на сервер.**
Ни один билдер (Rust `create_index.rs`, TS `ddl.ts:154-201`) не проверяет: `index_type:'vector'` без `vector_dim`, `unique+sorted` (запрещено — коммент `index_ops.rs:21`), `k=0`/пустой `query` в `vectorSimilarity`, `ef_search > MAX_EF_SEARCH` (клэмпится молча сервером), dim-mismatch вектора (ловится глубоко в `shamir-index/vector/adapter.rs:11`). Цена — round-trip и неструктурная ошибка. `Batch.tryBuild()` (TS) — хороший прецедент; естественно добавить туда/в `build()` дешёвые инварианты. Также: TS `Batch.transactional()` повторным вызовом без аргумента затирает isolation (`builders/batch.ts:162-166`); `fts` в TS дефолтит mode='and' — Rust-билдер требует явно (`leaf.rs:208`) — расхождение дефолт-поведения билдеров при одинаковом wire-дефолте.

**2.4 [MED] TS ConnectOptions обещает безопасность, которой нет.**
`acceptNewHost`/`trustedPin` объявлены (`types/connection.ts:41-42`) и **нигде не используются**; `identity_sig` из auth_ok читается по индексу, но не проверяется (`protocol.ts:149-184`, `client.ts:114-154`). Rust/node клиенты делают TOFU-пиннинг Ed25519; TS — нет. Мёртвые опции = ложное чувство защиты. Фикс: либо реализовать пин-проверку, либо убрать опции из типа с явным комментарием.

## 3. УЛУЧШИТЬ — DX

- **Ломаная конвенция имён внутри TS SDK**: `ddl.ts` opts — snake_case (`if_not_exists`, `vector_dim`, `fts_tokenizer` — `ddl.ts:158-173`), `write/subscribe/batch` — camelCase (`returningFields`, `fromVersion`, `returnResult`), фильтры — camelCase→snake маппинг (`efSearch` → `ef_search`, `filter.ts:181-215`). Выбрать одну (camelCase в opts, snake — только wire) и дать alias-период.
- **Нет typed-ошибок** (см. 2.1) — главный DX-долг.
- **Тройное дублирование типов**: Rust `shamir-query-types` ↔ рукописные `client-ts/src/core/types/*` ↔ рукописный `client-node/index.d.ts`; HMAC-канонизация в трёх копиях (`hmac.rs`, TS `hmac.ts`, `tests/e2e/helpers/hmac.js`). Дрейф уже материализовался трижды (limits/durability/explain). Эскиз: codegen TS-типов из Rust (schemars→json-schema→ts, или хотя бы CI-тест, диффающий список полей serde против TS-интерфейсов).
- `WireValue` в TS не включает `Uint8Array` (`types/write.ts:33-39`) — бинарь нельзя типобезопасно вставить, хотя рантайм и `FilterValue` его поддерживают.
- `vs()` (fluent vector-билдер) не включён в агрегат `filter` namespace (`filter.ts:435-470`) — обнаруживаемость.
- e2e написан raw-JSON'ом мимо билдеров — по CLAUDE.md исключений для e2e нет; перевод e2e на TS-билдеры закрыл бы одновременно норму и e2e-дыру TS SDK.
- Недокументированное: `max_nesting_depth` (нет в TS-доках вовсе), `async_index` (нет в TS), `explain` (нет в TS), «node не умеет subscriptions/tx/resume» — не сказано в `index.d.ts` заголовке, который обещает «full …» и «Mirrors the Rust SDK 1:1» (`lib.rs:3`) — уже неправда.

## 4. УСКОРИТЬ (по дороге)

- **Node-путь — 3 сериализации в каждую сторону.** Запрос: JS `encode(batchObj)` (`index.js:63`) → napi Buffer → `rmp_serde::from_slice::<BatchRequest>` (`lib.rs:200`) → повторный encode внутри `DbRequest::Execute` (`client.rs:795`). Ответ: decode в `BatchResponse` → `rmp_serde::to_vec_named` (`lib.rs:207`) → JS `decode`. Эскиз: `execute_raw(db, batch_bytes) -> response_bytes` в `core::Client` (envelope принимает готовые msgpack-байты батча; `DbRequest`-конверт можно собрать вручную вокруг сырых байтов), биндинг перестаёт декодировать/кодировать вовсе — минус 2 полных прохода по каждому батчу и ответу.
- **Сервер, но платит каждый клиентский op:** `BatchOp::deserialize` буферизует op в `QueryValue`, re-encode'ит в msgpack и декодирует второй раз (`batch_op.rs:257-285`) — двойной decode+alloc на каждый op каждого запроса; диспетчер по первому ключу без буферизации убрал бы это.
- TS `execute()` копирует объект батча ради `interner_epochs` (`client.ts:456-459`) и энкодит конверт двухслойно (`encode({...req: encode(req)})`, `client.ts:415`) — второе неустранимо (serde_bytes-конверт), первое — мутировать по месту, как это уже делает `executeWithTouch`.
- `executeWithTouch` (TS) на каждый вызов делает `collectFieldNames` + `JSON.stringify`-сравнения deliver-mode'ов (`subscribe.ts:135-141`) — микро, но на горячем пути вставок стоит кешировать по форме батча.

## Топ-5 «обязаны»

| # | Что | Серьёзность | Где | Фикс |
|---|---|---|---|---|
| 1 | `BatchLimits` в TS без `max_nesting_depth` — любой `.limits()` → серверный decode-fail; юнит-тест закрепляет баг | CRITICAL | `client-ts/src/core/types/batch.ts:89`; `builders/batch.ts:34`; `batch.test.ts:143`; wire: `batch_limits.rs:47` | добавить поле+дефолт 4; serde-default на поля BatchLimits; parity-фикстура batch-конверта |
| 2 | `query_version:1` захардкожен при использовании v2-фич; `resume()` не читает `server_query_version` → тихий даунгрейд | HIGH | `client-ts/src/core/client.ts:484,736,763,194-227`; wire: `db_message.rs:19`, `wire_frames.rs:63` | экспортировать CURRENT=2, слать её; парсить поле в resume; e2e resume→v2 |
| 3 | Дрейф wire-типов TS: нет `explain`/`ExplainPlan`, нет `'async_index'`, `records_idmsgpack` объявлен на уровне батча | HIGH | `types/query.ts:152`; `types/batch.ts:84,131,168`; wire: `read_query.rs:44`, `batch_request.rs:76`, `write/types.rs:93` | добавить поля + `.explain()` в Query; перенести объявление в `InsertOp`; CI-дифф типов |
| 4 | Ошибки без кода в обоих JS-клиентах (retry-able неотличима от фатальной), нулевые таймауты запросов/коннекта | HIGH | `client.ts:353-360,404-430`; `client-node/src/lib.rs:264`; e2e-регексы `09-errors.test.js:18` | `ShamirDbError{code,retryable}`; napi Error с code; `requestTimeoutMs`/`connectTimeoutMs` |
| 5 | Parity закреплена только для vector-filter и repl-DDL; e2e не тестирует TS SDK вовсе и написан мимо билдеров; расхождение `result_encoding`-логики Rust↔TS | HIGH | `query-builder/tests/fixtures/*`; `tests/e2e/e2e.test.js:21`; `interner_cache_ops.rs:412` vs `client.ts:707` | общий манифест msgpack-фикстур на все op'ы; e2e-прогон ws-TS-клиента; серверный тест `$query`+`result_encoding=Id` и выравнивание клиентов |

Отдельной строкой (не влезло в топ, но обязательно): node-биндинг — тупиковый `resumptionTicket()` без `resume()`, отсутствие подписок/tx/interner при заявленном «Mirrors the Rust SDK 1:1» (`client-node/src/lib.rs:3`), и `connect` не принимает hostname (`lib.rs:100`).
