בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Репл-DDL ops в shamir-query-types (BatchOp + is_write/is_admin + serde)

> Контекст: `docs/dev-artifacts/roadmap/REPLICATION-CLIENT-SURFACE.md` §2-3 п.1,
> `docs/dev-artifacts/roadmap/REPLICATION.md` §5.5 (publication/subscription-модель).

## Задача

Добавить admin-op DTO + BatchOp-варианты для декларативной конфигурации
репликации. Это ТИПЫ (wire/serde) — исполнение op'ов на сервере НЕ входит
в эту таску (отдельный трек интеграции с R1-loop).

## Набор ops (§5.5, форма — не финальный контракт, но фиксируем сейчас)

Discriminator-ключ (unique key в wire-map) → op:
- `create_replication_profile` → `CreateReplicationProfileOp { name, streams: Vec<ReplStream> }`
  где `ReplStream { scope: ReplScope, direction: ReplDirection, mode: ReplMode }`,
  `ReplScope { db, repo: Option<String>, table: Option<String> }` (repo=None → вся db),
  `ReplDirection` enum {Pull, Push, Both}, `ReplMode` enum {ReadOnly, ReadWrite}.
- `drop_replication_profile` → `DropReplicationProfileOp { name }`
- `create_publication` → `CreatePublicationOp { name, scopes: Vec<ReplScope> }`
- `drop_publication` → `DropPublicationOp { name }`
- `create_subscription` → `CreateSubscriptionOp { name, upstream: String, publication: String, profile: String }`
  (`upstream` — адрес/идентификатор лидера)
- `drop_subscription` → `DropSubscriptionOp { name }`
- `alter_subscription` → `AlterSubscriptionOp { name, action: SubAction }`
  где `SubAction` enum {Pause, Resume, SetProfile(String)}
- `list_publications` → `ListPublicationsOp {}` (read-only)
- `list_subscriptions` → `ListSubscriptionsOp {}` (read-only)
- `replication_status` → `ReplicationStatusOp {}` (read-only)

Node-account роль — это `create_user roles(["replicator"])` (уже есть),
ОТДЕЛЬНОГО op НЕ добавлять.

## Файлы (следуй существующему паттерну)

1. **`crates/shamir-query-types/src/admin/types/repl_ops.rs`** (новый) — все
   DTO выше + вспомогательные enum'ы (`ReplScope`, `ReplStream`,
   `ReplDirection`, `ReplMode`, `SubAction`), все `#[derive(Debug, Clone,
   PartialEq, Serialize, Deserialize)]`. Подключить в `admin/types/mod.rs`
   и реэкспортнуть через `admin/mod.rs` по существующей цепочке (сверь как
   реэкспортятся, напр., `CreateDbOp`).
2. **`crates/shamir-query-types/src/batch/batch_op.rs`**:
   - `use` новых типов из `crate::admin`.
   - Добавить 10 вариантов `BatchOp` (`CreateReplicationProfile(...)`, …,
     `ReplicationStatus(...)`).
   - Добавить в `serialize` match (каждый `op.serialize(serializer)`).
   - Добавить в `from_query_value` dispatch (`else if has("create_publication")
     { qv_to::<CreatePublicationOp,_>(&bytes).map(BatchOp::CreatePublication) }`
     и т.д. — discriminator = snake_case имя op'а выше).
   - **is_admin():** добавить ВСЕ 10 в `matches!` (репл-DDL — admin).
   - **is_write():** мутирующие (create/drop/alter *) → `true`; read-only
     (`list_publications`, `list_subscriptions`, `replication_status`) →
     `false`. is_write — exhaustive match БЕЗ wildcard, так что новые
     варианты СЛОМАЮТ компиляцию, пока не классифицируешь — это и нужно.

## Тесты (batch/tests/batch_types_tests.rs + admin/types/tests/)

- Serde round-trip каждого op'а через `roundtrip_op(mpack!({...}))` (как
  существующие is_write-тесты) — Create/Drop/Alter/List — все 10.
- `is_admin()==true` для всех 10; `is_write()`: create/drop/alter→true,
  list_*/status→false.
- round-trip вложенных enum'ов (`ReplDirection::Both`, `ReplMode::ReadOnly`,
  `SubAction::SetProfile`).

## Гейт

- `./scripts/test.sh -p shamir-query-types` зелёный.
- `cargo fmt -p shamir-query-types -- --check` чистый.
- `cargo clippy -p shamir-query-types --all-targets -- -D warnings` чистый.
- Убедись, что весь воркспейс компилируется (новые BatchOp-варианты — в
  shared enum; проверь `cargo clippy --workspace --all-targets` на предмет
  не-exhaustive match'ей у ПОТРЕБИТЕЛЕЙ BatchOp — если где-то есть match без
  wildcard по BatchOp, его придётся дополнить ветками; сделай это минимально
  — просто пробрось в тот же путь, что прочий admin-DDL, или добавь в
  `not_supported`-ветку с TODO, НЕ реализуя исполнение).

## Definition of done

- repl_ops.rs + 10 BatchOp-вариантов + serialize/from_query_value/is_admin/
  is_write + реэкспорты.
- Все потребители BatchOp снова компилируются (exhaustive match'и дополнены
  минимально, исполнение НЕ реализуем — только классификация/маршрут).
- Тесты serde + классификации зелёные.
- Финальное сообщение: тронутые файлы, где пришлось дополнить match'и
  потребителей, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
