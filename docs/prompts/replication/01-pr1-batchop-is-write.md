בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# PR1 — `BatchOp::is_write()` (симметрия к `is_admin`)

> Контекст: `docs/research/REPLICATION-PRE-REFACTOR-2026-06-30.md` §Б PR1.
> Назначение: read-only гейт follower-ноды (PR4 / §4.3 REPLICATION.md) —
> классификация «мутирует ли op данные или состояние сервера».

## Задача

В `crates/shamir-query-types/src/batch/batch_op.rs` рядом с `is_admin()`
(строка ~412) добавить `pub fn is_write(&self) -> bool`.

**Ключевое требование:** исчерпывающий `match` по ВСЕМ вариантам enum'а
БЕЗ wildcard (`_`) — чтобы каждый новый вариант `BatchOp` ломал компиляцию
и заставлял автора явно классифицировать его. (`is_admin` использует
`matches!` — is_write сознательно строже.)

## Классификация

- **write = true**: всё, что мутирует данные или персистентное состояние —
  `Insert`, `Update`, `Set`, `Delete`, весь DDL (Create*/Drop*/Rename*),
  `SetBufferConfig`, `AlterBufferConfig`, миграции (Start/Commit/Rollback),
  auth-мутации (CreateUser/DropUser/CreateRole/DropRole/RenameRole/
  GrantRole/RevokeRole), access-DDL (Chmod/Chown/Chgrp/группы),
  function/validator DDL и Bind/Unbind, schema DDL (Set/Add/Remove),
  `CreateFunctionFolder`/`RenameFunctionFolder`, `InternerTouch`,
  `PurgeHistory`, `SetRetention`, `Call` (WASM-функция может писать —
  консервативно write).
- **write = false**: чистые чтения/интроспекция — `Read`, `List`,
  `GetBufferConfig`, `MigrationStatus`, `AccessTree`, `ListValidators`,
  `GetTableSchema`, `DescribeTable`, `InternerDump`, `ChangesSince`,
  `Subscribe`, `Unsubscribe`.
- **`Batch(SubBatchOp)`** — рекурсивно: write, если ЛЮБОЙ вложенный op —
  write. Посмотри структуру `sub_batch_op.rs`, чтобы правильно добраться
  до вложенных ops.

Если при чтении кода обнаружишь вариант, чья классификация спорна —
классифицируй консервативно (write=true) и оставь однострочный комментарий
у ветки с причиной.

## Тесты

- В layout `crates/shamir-query-types/src/batch/tests/` (если тестов у
  модуля нет — создать `tests/` каталог по правилам проекта: `tests/mod.rs`
  манифест + `batch_op_tests.rs`; подключение через `#[cfg(test)] mod tests;`
  в `mod.rs` модуля). Если tests-каталог уже есть — добавить туда.
- Кейсы: Read→false, Insert/Set/Delete→true, CreateUser→true,
  DescribeTable→false, Call→true, SubBatch с одним read→false,
  SubBatch с read+insert→true, вложенный SubBatch (глубина 2)→true.
- Запросы строить ТОЛЬКО через `shamir-query-builder` где применимо; если
  билдер в dev-dependencies недоступен для этого крейта — конструировать
  ops прямо структурами (это типы этого крейта, не raw JSON).

## Гейт

- `./scripts/test.sh -p shamir-query-types` — зелёный.
- `cargo fmt -p shamir-query-types -- --check` чистый.
- `cargo clippy -p shamir-query-types --all-targets -- -D warnings` чистый.

## Definition of done

- `is_write()` с исчерпывающим match без wildcard + doc-комментарий.
- Тесты по кейсам выше зелёные.
- Ничего кроме batch_op.rs, sub_batch_op.rs (если нужен хелпер) и тестов
  не тронуто.
- Финальное сообщение: список веток write=false (для ревью), вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
