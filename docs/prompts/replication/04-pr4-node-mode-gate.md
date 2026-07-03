בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# PR4 — точка read-only гейта в `ShamirDbHandler::execute()`

> Контекст: `docs/research/REPLICATION-PRE-REFACTOR-2026-06-30.md` §Б PR4,
> `docs/roadmap/REPLICATION.md` §4.3. Зависит от PR1 (`BatchOp::is_write()`
> уже в `crates/shamir-query-types/src/batch/batch_op.rs`).

## Задача

Добавить понятие «режим ноды» в `ShamirDbHandler` и точку read-only гейта
в единственной точке входа всех батчей — `execute()`. До R1 режим всегда
`ReadWrite` ⇒ НУЛЕВОЕ изменение поведения, но точка врезки готова и покрыта
тестом.

## Шаги

1. **`crates/shamir-server/src/db_handler/config.rs`** — добавить рядом с
   `QueryLimitsCap`/`SlowQueryConfig`:
   ```rust
   /// Read/write mode of this node. A replica follower runs ReadOnly and
   /// rejects client writes (they must go to the leader). Default ReadWrite.
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
   pub enum NodeMode {
       #[default]
       ReadWrite,
       ReadOnly,
   }
   ```
   (одна публичная единица на файл — если правило проекта требует отдельный
   файл `node_mode.rs`, вынеси туда; но config.rs уже держит несколько
   related config-типов, так что добавление сюда допустимо — реши по
   существующему стилю файла.)

2. **`handler.rs`** — поле `pub(super) node_mode: NodeMode` в структуре
   `ShamirDbHandler`; проинициализировать `NodeMode::ReadWrite` (или
   `NodeMode::default()`) в обоих конструкторах `new` и `with_admin`;
   добавить билдер `pub fn with_node_mode(mut self, mode: NodeMode) -> Self`
   рядом с `with_query_limits`.

3. **Гейт в `execute()`** — после version-check и admin/auth-гейта (там где
   уже итерируется `batch.queries` для `is_admin()`), добавить:
   ```rust
   if self.node_mode == NodeMode::ReadOnly {
       for (alias, entry) in &batch.queries {
           if entry.op.is_write() {
               return DbResponse::Error {
                   code: "read_only_replica".into(),
                   message: format!(
                       "query '{}' is a write; this node is a read-only replica",
                       alias
                   ),
               };
           }
       }
   }
   ```
   (В R1 сюда добавится `leader_addr` в тело ошибки — НЕ сейчас, оставь TODO-
   комментарий `// R1: include leader_addr for client redirect`.)

## Тесты

- Новый файл `crates/shamir-server/src/db_handler/tests/node_mode_tests.rs`,
  зарегистрировать в `tests/mod.rs` (`pub mod node_mode_tests;`).
- Кейсы (`#[tokio::test]`, in-memory `ShamirDb`, builder-only запросы):
  1. ReadWrite (default) + write-батч (upsert) → успех (нулевое изменение
     поведения).
  2. ReadOnly + write-батч (upsert/insert) → `DbResponse::Error { code:
     "read_only_replica", .. }`.
  3. ReadOnly + read-only батч (select/from) → успех (чтения проходят).
  4. ReadOnly + смешанный батч (read + insert) → отказ (любой write валит).
- Запросы строить ТОЛЬКО через `shamir-query-builder` (builder-only, CLAUDE.md).
  Пример fixture-хендлера — см. `crates/shamir-server/benches/wire_pipelining.rs`
  (build_handler: `ShamirDb::init_memory` + `create_db_as`/`add_repo_as`
  под `Actor::User(principal_id("alice"))`, т.к. System-owned ресурсы 0o700).

## Гейт

- `./scripts/test.sh @server` зелёный.
- `cargo fmt -p shamir-server -- --check` чистый.
- `cargo clippy -p shamir-server --all-targets -- -D warnings` чистый.

## Definition of done

- `NodeMode` enum + поле + builder + гейт в `execute()`.
- 4 теста зелёные; поведение при ReadWrite (default) не изменилось.
- Тронуты только config.rs, handler.rs (+ возможный node_mode.rs) и тесты.
- Финальное сообщение: список тронутых файлов, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
