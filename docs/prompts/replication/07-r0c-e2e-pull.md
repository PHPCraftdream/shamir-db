בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# R0-c — e2e pull через loopback TLS + epoch-fencing + deny-по-умолчанию

> Контекст: `docs/roadmap/REPLICATION.md` §5. R0-a (wire-типы) и R0-b
> (repl_handler) уже в дереве. Это интеграционный тест поверх полного
> TLS+SCRAM стека — доказывает, что `DbRequest::Repl` проходит по реальному
> проводу и возвращает `DbResponse::Repl`.

## Задача

Новый файл `crates/shamir-server/tests/repl_pull_e2e.rs` по шаблону
`crates/shamir-server/tests/mvp_e2e.rs` (полный стек: `spawn_ephemeral`
из `tests/common/mod.rs` + TLS 1.3 + SCRAM-Argon2id handshake +
`RequestEnvelope`/`ResponseEnvelope`).

Харнесс: `mod common;` даёт `spawn_ephemeral(&temp, admin_password)`.
SCRAM-handshake wire-зеркала (`WireAuthInit`/`WireChallenge`/…) и функцию
connect+authenticate СКОПИРУЙ из `mvp_e2e.rs` в этот файл (Cargo даёт
каждому tests/*.rs свой крейт; дублирование connect-хелпера — существующий
паттерн, common их не держит). Можно вынести в отдельный модуль внутри
файла.

## Сценарии (обязательные)

### A. Deny-по-умолчанию (security-critical, БЕЗ grant)

1. Admin (bootstrap superuser) подключается, создаёт SCRAM-юзера `plain`
   БЕЗ роли `replicator` (`DbRequest::CreateScramUser { name: "plain",
   password, roles: vec![] }`).
2. `plain` подключается по TLS+SCRAM, шлёт
   `DbRequest::Repl(ReplRequest::Hello { proto_ver: 1, node_id: "n1" })`.
3. Ответ декодируется в `DbResponse::Repl(ReplResponse::Error { code, .. })`
   с `code == "bad_role"`. **Это ядро теста — deny-by-default по проводу.**

### B. Happy-path: Hello + Pull с реальными событиями

1. Admin создаёт db `app` + repo `main` + таблицу `items` (batch DDL через
   query-builder), пишет несколько строк (upsert/insert, `transactional()`
   — как в существующих e2e; changefeed эмитит на tx-commit).
2. Admin создаёт SCRAM-юзера `repl` с ролью `replicator`
   (`roles: vec!["replicator".into()]`).
3. **Доступ `repl` к `app/main`** — PREFERRED: admin выдаёт read по wire
   (например `chmod` репо в world-read, или `chown` репо на `repl`, или
   через группу — используй ту access-DDL, что реально работает в этом коде;
   сверься с `tests/permission_e2e.rs` / `tests/access_tree_e2e.rs` как это
   делается по проводу).
   FALLBACK (если grant по wire окажется непрактичным в объёме R0): создай
   `repl` с `roles: vec!["superuser", "replicator"]` — тогда доступ идёт
   через superuser-bypass; это всё равно доказывает wire-путь Hello/Pull, но
   per-repo-authz не проверяется на e2e (он покрыт unit-тестами R0-b). В этом
   случае ОБЯЗАТЕЛЬНО оставь комментарий `// R0-c FALLBACK: <причина>` и
   упомяни в финальном сообщении.
4. `repl` подключается, шлёт `ReplHello` → `ReplResponse::Hello {
   leader_epoch, repos }`: проверь `leader_epoch == 1` (дефолт хендлера,
   §5.2) и что `repos` содержит `{db:"app", repo:"main"}` с
   `current_version > 0`.
5. `repl` шлёт `ReplPull { db:"app", repo:"main", from_version: 0, limit:
   100, wait_ms: None }` → `ReplResponse::Pull { leader_epoch, events,
   current_version, .. }`: `leader_epoch == 1`, `current_version > 0`,
   декодируй `rmp_serde::from_slice::<Vec<shamir_engine::ChangelogEvent>>(
   &events)` и проверь непустой список с изменениями на таблице `items`.

### C. Long-poll не виснет (по проводу)

`repl` шлёт `ReplPull` с `from_version` = текущий `current_version` (пустой
хвост) и `wait_ms: Some(200)`. Ответ приходит за разумное время (< ~2s) с
пустым `events`. (Строгий тайминг не нужен — важно, что не виснет.)

## Замечания по реализации

- Repl-запрос — это `DbRequest::Repl(ReplRequest::…)`; сериализуй
  `rmp_serde::to_vec_named(&DbRequest::Repl(..))` в `RequestEnvelope.req`,
  ровно как mvp_e2e делает с `DbRequest::Execute`.
- Импорты: `shamir_server::db_handler::{DbRequest, DbResponse}` +
  `shamir_query_types::wire::repl::{ReplRequest, ReplResponse}` (или через
  реэкспорт `shamir_query_types::wire::{…}`).
- Запросы конструировать через query-builder (batch DDL/write), НЕ raw JSON.
- Держи `temp` живым до конца теста (drop удаляет data-dir).

## Гейт

- `./scripts/test.sh @e2e --full` (или `-p shamir-server --full`) зелёный —
  особенно новый `repl_pull_e2e`.
- `cargo fmt -p shamir-server -- --check` чистый.
- `cargo clippy -p shamir-server --all-targets -- -D warnings` чистый.

## Definition of done

- `repl_pull_e2e.rs` со сценариями A (обязателен), B, C — зелёный по
  полному стеку.
- Deny-по-умолчанию (A) проходит по реальному проводу.
- Тронут только новый тест-файл (и, если выносишь connect-хелпер в common —
  `tests/common/mod.rs`; но предпочти локальную копию).
- Финальное сообщение: какой доступ-путь выбран (grant vs fallback), вывод
  test.sh, тайминги long-poll.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
