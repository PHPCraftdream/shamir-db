בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Rust query builder для репл-DDL + юнит-тесты

> Контекст: `docs/roadmap/REPLICATION-CLIENT-SURFACE.md` §3 п.2. Op-типы уже
> в `crates/shamir-query-types/src/admin/types/repl_ops.rs` (коммит 143dc060):
> `CreateReplicationProfileOp`, `DropReplicationProfileOp`,
> `CreatePublicationOp`, `DropPublicationOp`, `CreateSubscriptionOp`,
> `DropSubscriptionOp`, `AlterSubscriptionOp`, `ListPublicationsOp`,
> `ListSubscriptionsOp`, `ReplicationStatusOp` + enum'ы `ReplScope`,
> `ReplStream`, `ReplDirection`, `ReplMode`, `SubAction`.

## Задача

Fluent-билдеры для всех 10 репл-DDL ops — единственный санкционированный
путь конструирования (CLAUDE.md «builder only»). Паттерн — как в
`crates/shamir-query-builder/src/ddl/create_repo.rs` / `auth.rs`: free fn →
builder struct → fluent setters → `build() -> BatchOp`.

## Файл

Новый `crates/shamir-query-builder/src/ddl/replication.rs` (связная группа —
один файл). Подключить в `ddl/mod.rs` (`pub mod replication;` +
`pub use replication::*;`). Реэкспортнуть нужные типы аргументов из
`shamir_query_types::admin` (`ReplScope`/`ReplStream`/`ReplDirection`/
`ReplMode`/`SubAction`) через `ddl/mod.rs` `pub use`, как уже сделано для
`Retention`/`ResourceRef`.

## Билдеры (сигнатуры — по смыслу op'ов)

- `replication_profile(name) -> ReplicationProfileBuilder` с
  `.stream(scope, direction, mode)` (аккумулирует `Vec<ReplStream>`) →
  `.build() -> BatchOp::CreateReplicationProfile`.
- `drop_replication_profile(name) -> BatchOp` (без опций — можно сразу
  BatchOp или тонкий builder с build()).
- `publication(name) -> PublicationBuilder` с `.scope(ReplScope)` /
  `.scopes(iter)` → build → `CreatePublication`.
- `drop_publication(name)`.
- `subscription(name) -> SubscriptionBuilder` с обязательными
  `.upstream(addr)`, `.publication(name)`, `.profile(name)` → build →
  `CreateSubscription` (если обязательные не заданы — разумный дефолт пустой
  строкой ИЛИ отметь в доке; предпочти явные аргументы free-fn:
  `subscription(name, upstream, publication, profile)` если так чище).
- `drop_subscription(name)`.
- `alter_subscription(name)` с `.pause()` / `.resume()` /
  `.set_profile(name)` → build → `AlterSubscription { action: SubAction }`.
- `list_publications() -> BatchOp`, `list_subscriptions() -> BatchOp`,
  `replication_status() -> BatchOp` (read-only, без аргументов; собрать
  соответствующий `*Op::default()`).
- Хелпер для `ReplScope`: удобный конструктор
  `repl_scope(db).repo(r).table(t)` или свободные функции — на твоё
  усмотрение, но эргономично (db обязательна, repo/table опциональны).

Согласуй имена полей op'ов с их РЕАЛЬНОЙ формой в repl_ops.rs (сверься —
напр. `create_replication_profile: String` presence/name-поле, `streams`,
и т.д.; не угадывай — открой файл).

## Тесты (ddl/tests/, зарегистрируй в tests/mod.rs)

Для каждого билдера: построить → проверить, что `build()` даёт правильный
`BatchOp`-вариант с ожидаемыми полями. Например
`assert!(matches!(publication("p").scope(...).build(),
BatchOp::CreatePublication(op) if op.name == "p" && ...))`. Покрыть:
профиль с несколькими streams, publication с несколькими scopes,
subscription со всеми обязательными, alter c каждым SubAction, все 3
read-only.

## Гейт

- `./scripts/test.sh -p shamir-query-builder` зелёный.
- `cargo fmt -p shamir-query-builder -- --check` чистый.
- `cargo clippy -p shamir-query-builder --all-targets -- -D warnings` чистый.

## Definition of done

- `ddl/replication.rs` со всеми 10 билдерами + реэкспорты + хелпер scope.
- Тесты на каждый билдер зелёные.
- Тронуты: replication.rs, ddl/mod.rs, ddl/tests/*.
- Финальное сообщение: список билдеров, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
