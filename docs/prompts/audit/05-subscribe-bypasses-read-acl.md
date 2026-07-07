בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# CRIT-5 — Subscribe обходит per-table read-ACL (#439)

> Ты — суб-агент в S.H.A.M.I.R. (Rust DB), D:\dev\rust\shamir-db. Задача #439 —
> CRITICAL находка панельного ревью (`docs/audits/2026-07-06-security-network-surface.md`,
> Топ-5 #1 + §1a). Область: `crates/shamir-server/src/subscriptions/bridge.rs`,
> `crates/shamir-server/src/db_handler/subscribe_handler.rs`.

## Дефект (подтверждён личным чтением кода оркестратором)

`BatchOp::Subscribe` классифицируется в `crates/shamir-engine/src/query/auth/session.rs:545`
как `(Action::Read, Resource::Global)` с комментарием "actual table checks
happen when the subscription is activated" — это ЛОЖЬ по факту: нигде в
`crates/shamir-server/src/subscriptions/` нет ни одного вызова
`authorize_access` для конкретных таблиц подписки.

`subscribe_handler.rs::activate_subscriptions` (строки 13-62) спавнит
`bridge::bridge_task` (`bridge.rs:52-...`) с полным списком
`sources: Vec<SubscriptionSource>` (каждый — `(repo, table, event_mask,
filter)`) без единой ACL-проверки. `bridge_task` подписывается на
changefeed каждого repo и пушит ЛЮБОЙ live insert/update/delete по имени
таблицы + фильтру — вообще не сверяясь с правами актора на конкретную
таблицу.

### Сценарий провала (из аудита, подтверждён)

1. Юзер логинится с глобальным `read`, но БЕЗ `read` на таблицу `secrets`.
2. `Execute{Subscribe(secrets, deliver=Records, initial=false)}` —
   классификатор пропускает как `(Read, Global)` (общий read есть).
3. Юзер получает каждый live insert/update/delete из `secrets` с полным
   содержимым записи. Read-ACL таблицы полностью обойдён.

Общий per-batch `authorize_access` в `db_execute.rs:61-67` проверяет
`entry.op.table_ref()` — но грепни `table_ref()` для `BatchOp::Subscribe`:
скорее всего возвращает `None` (подписка держит МНОЖЕСТВО таблиц через
`sources`, а не одну `table_ref`), так что этот общий цикл её тоже не
ловит.

## Задача

### Фикс — авторизовать каждый source в bridge_task, НЕ пушить неавторизованные таблицы

`db: Arc<ShamirDb>` уже доступен внутри `bridge_task` — у него есть
`authorize_access(&actor, &ResourcePath, Action) -> Result<(), AccessError>`
(см. `crates/shamir-db/src/shamir_db/execute/db_execute.rs:61-67` для
канонического паттерна вызова с `ResourcePath::Table { db, store, table }`).

В `bridge_task` (`bridge.rs:52`), ПЕРЕД тем как строить `targets`/
подписываться на changefeed (строки 82-114), для каждого `source` в
`sources` вызови:
```rust
let path = shamir_db::access::ResourcePath::Table {
    db: db_name.clone(),
    store: source.table.repo.clone(),
    table: source.table.table.clone(),
};
db.authorize_access(&actor, &path, shamir_db::access::Action::Read).await
```
(проверь точный путь импорта `ResourcePath`/`Action` — используется в
`bridge.rs` уже `use shamir_db::access::Actor;`, добавь соседние типы
тем же путём).

**Стратегия при отказе — не пушить неавторизованные таблицы** (дословно
из фикс-эскиза аудита), НЕ ронять всю подписку целиком: отфильтруй
`sources` до подмножества, на которое `authorize_access` вернул `Ok`,
залогируй `tracing::warn!` для каждого отклонённого source (repo, table,
sub_id), и продолжай `bridge_task` с УРЕЗАННЫМ списком (как если бы
пользователь никогда не подписывался на эту таблицу). Если ПОСЛЕ фильтра
`sources` пуст — веди себя как существующий `changefeed not available`
путь (`bridge.rs:109-113`): залогируй и `return` (никакого receiver'а не
создавай, никакого пуша).

Обнови `activate_subscriptions` в `subscribe_handler.rs`, если нужно
что-то передать доп. (скорее всего не нужно — `actor` и `db` уже
пробрасываются).

**Пересмотри значение ответа клиенту**: сейчас `activate_subscriptions`
всегда репортит `sub_id` клиенту синхронно ДО того как `bridge_task`
асинхронно стартует и может обнаружить ACL-отказ. Реши (и обоснуй в
докладе): либо (а) клиент получает `sub_id` как обычно, но реально
доставляемые таблицы — это подмножество после ACL-фильтра (тихий
частичный отказ, как changefeed-unavailable кейс сейчас уже делает), либо
(б) если ВСЕ sources отклонены ACL — здесь недостижимо синхронно
проверить до спавна таска (потому что `authorize_access` — async, а
`activate_subscriptions` — sync функция), так что синхронная ошибка
клиенту НЕ вариант без более широкого рефакторинга; **выбери вариант (а)
как минимальный точечный фикс** и явно задокументируй в коде почему
(комментарий у `activate_subscriptions`), что случай "запросил подписку
без единого разрешённого source" сейчас тихо не доставляет ничего (может
стать отдельной HIGH-задачей — упомяни это в докладе, не открывай новую
задачу сам).

### НЕ трогай

- Классификатор `BatchOp::Subscribe → (Read, Global)` в `session.rs:545`
  — оставь как есть (уровень пропуска "может ли вообще подписываться"
  корректен, дыра именно в отсутствии per-table проверки при активации).
- `db_execute.rs` общий per-op auth-цикл — не расширяй его под Subscribe,
  фикс живёт в `bridge_task` (там, где реально известен полный список
  таблиц подписки).
- Остальные CRITICAL/HIGH находки того же файла (WASM db_execute
  scope — #440, TLS/WS timeout — #444 HIGH-security кластер) — не
  твоя задача.

## Тесты

1. **Regression**: юзер БЕЗ read на таблицу `secrets`, глобальный read
   есть → `Subscribe(secrets, deliver=Records)` → коммить insert в
   `secrets` от другого актора/System → assert НИКАКОГО push-события не
   долетело до неавторизованного подписчика (используй существующий
   тестовый харнесс `crates/shamir-server/src/subscriptions/tests/` —
   грепни `bridge_tests.rs`/аналог для образца мока `PushSink`/ACL
   setup). Проверь и позитивный кейс: юзер С read на `secrets` —
   события долетают как раньше (не сломай легитимный путь).
2. **Смешанная подписка**: subscribe на ДВЕ таблицы одновременно —
   одна разрешена, другая нет → assert события только с разрешённой
   таблицы долетают, с неразрешённой — нет (docs describes "не пушить
   неавторизованные таблицы", не "провалить всю подписку").
3. Существующие `bridge_tests.rs`/`registry_tests.rs`/
   `target_match_tests.rs` остаются зелёными (System actor / уже
   покрытые ACL-кейсы не регрессируют).

## Гейт

- `./scripts/test.sh -p shamir-server --full` 1×, целевые новые тесты
  10× повторно (concurrency/async delivery — единичный зелёный прогон
  не доказывает достаточно);
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt -p shamir-server -- --check`.

## Дисциплина

Тесты ТОЛЬКО через ./scripts/test.sh. Хирургично: `bridge.rs` +
возможно `subscribe_handler.rs` (комментарий про partial-reject case) +
новые regression-тесты в `crates/shamir-server/src/subscriptions/tests/`.
НЕ трогай WASM/db_execute (#440) и HMAC/timeout находки (другие задачи).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Definition of done

`bridge_task` авторизует каждый source per-table через `authorize_access`
ПЕРЕД подпиской на changefeed/доставкой событий; неавторизованные таблицы
тихо исключаются из доставки (не роняют всю подписку); если ни одного
разрешённого source не осталось — подписка не создаёт ресурсов
(зеркалирует существующий "changefeed not available" путь). Regression-
тесты доказывают: неавторизованный подписчик больше НЕ получает
live-данные закрытой таблицы; смешанная подписка доставляет только
разрешённую часть; легитимные (авторизованные) подписки не регрессируют.
Гейт зелёный. Финал доклада: точный diff, вывод тестов (включая 10×
повторы), вывод гейта, явное обоснование выбора стратегии (а) из раздела
про синхронный ответ клиенту.
