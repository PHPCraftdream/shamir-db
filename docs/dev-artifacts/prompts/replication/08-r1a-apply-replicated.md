בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# R1-a — apply_replicated engine-ядро (применение ChangelogEvent на follower)

> Контекст: `docs/dev-artifacts/roadmap/REPLICATION.md` §4.1 (раскладка apply), §4.2,
> §4 (идемпотентность). PR3 `finalize_sync_post_publish`
> (`crates/shamir-engine/src/tx/finalize.rs`) СПЕЦИАЛЬНО выделен под этот
> шаг — см. его docstring.

## Задача

Новый engine-путь: применить один `shamir_tx::ChangelogEvent`, вытянутый
follower'ом с лидера, в локальный repo. Это фундамент R1.

## Форма события (готова)

`ChangelogEvent { repo, commit_version, tx_id, actor, timestamp_ns,
changes: Vec<RecordChange> }`, где `RecordChange { table: String, key:
Bytes (raw 16-byte RecordId), op: ChangeOp::{Put,Delete}, value:
Option<Bytes> }`. Ключи и байты — УЖЕ сырые (leader их спроецировал);
интернер-деривация НЕ нужна.

## Модель применения (пинится здесь — не угадывать)

1. **Raw-apply, не high-level ops.** НЕ использовать `execute_set_tx`/
   `execute_delete_tx` (они деривят ключи через interner из high-level
   Set/Delete op'ов). Нужно записать `value` bytes по raw `key` в таблицу
   `table`, или удалить по `key`. Изучи как это делает WAL-replay при
   восстановлении: `crates/shamir-engine/src/tx/recovery.rs`,
   `crates/shamir-engine/src/tx/drainer.rs` (`apply_interner_delta`,
   `recover_v2_inflight`), и `RepoInstance` low-level put/delete в
   `crates/shamir-engine/src/repo/repo_instance.rs`. Переиспользуй
   существующий низкоуровневый store-put/delete по (table_token, RecordId,
   bytes) — table→token резолвится через существующий реестр таблиц repo.
2. **Версии.** Follower аллоцирует ЛОКАЛЬНЫЕ commit-версии через свой gate
   (single-hop R1 — данные сходятся; follower-локальная версия ≠ leader
   версия, это ОК и даже нужно для chain-репликации: follower эмитит
   собственный changefeed downstream). Bookmark (R1-b, отдельная таска)
   хранит LEADER commit_version — для идемпотентности.
3. **Идемпотентность (V4, §4).** Сигнатура принимает текущий applied
   leader-watermark (параметром — durable-хранение в R1-b). Если
   `event.commit_version <= applied_watermark` → **skip (no-op)**, вернуть
   без записи. O(1) сравнение, без пер-записного скана.
4. **Переиспользовать finalize-хвост.** Нижняя половина коммита (publish →
   emit changefeed → promote) — через `finalize_sync_post_publish`
   (finalize.rs). Верхняя половина apply НЕ содержит SSI-валидации/WAL-
   begin как обычный клиентский commit — событие УЖЕ закоммичено на лидере;
   apply доверяет ему. Если finalize-хвост не переиспользуется чисто
   (сигнатура требует PostPublishState, которого у raw-apply нет) —
   опиши в финальном сообщении, что именно мешает, и переиспользуй
   максимально возможную часть (как минимум emit_changefeed_event +
   watermark-publish), НЕ дублируй слепо.

## Где живёт

Новый файл `crates/shamir-engine/src/tx/apply_replicated.rs` (один primary
export: `apply_replicated`), подключить в `tx/mod.rs`. Публичный метод-
обёртка на `RepoInstance` (напр. `pub async fn apply_replicated(&self,
event: &ChangelogEvent, applied_watermark: u64) -> Result<ApplyOutcome,
...>`), возвращающий что применено / skipped + новую follower-версию.

## Тесты (tx/tests/ или apply_replicated_tests.rs по layout)

`#[tokio::test]`, in-memory repo с таблицей `items`:
1. **Put-конвергенция:** сконструировать ChangelogEvent с Put на items →
   apply → запись читается на follower'е с теми же байтами.
2. **Delete-конвергенция:** apply Put затем Delete → запись отсутствует.
3. **Идемпотентность:** apply одного event дважды с watermark =
   event.commit_version после первого → второй = skip (no-op), состояние
   не меняется.
4. **Порядок/watermark:** apply события v=5 при watermark=3 применяется;
   при watermark=5 — skip.
5. **finalize-хвост:** после apply changefeed follower'а содержит событие
   (downstream chain работает) — если хвост переиспользован.
   (Как сконструировать ChangelogEvent для теста — спроецируй из реального
   локального коммита на leader-repo через `shamir_tx::project_event`, или
   собери структуру вручную; предпочти реальную проекцию.)

## Гейт

- `./scripts/test.sh @oracle` зелёный (shamir-tx + shamir-engine).
- `cargo fmt -p shamir-engine -p shamir-tx -- --check` чистый.
- `cargo clippy -p shamir-engine --all-targets -- -D warnings` чистый.

## Definition of done

- `apply_replicated.rs` + метод на RepoInstance + подключение в mod.rs.
- Raw-apply Put/Delete, идемпотентность по watermark, finalize-хвост
  переиспользован (или задокументировано почему частично).
- Тесты 1-5 зелёные.
- Тронуты: apply_replicated.rs, tx/mod.rs, repo_instance.rs (метод-обёртка),
  tests. Hot commit-путь (commit.rs/group_commit.rs) НЕ трогать.
- Финальное сообщение: как устроен raw-apply (какой store-primitive),
  переиспользован ли finalize-хвост целиком, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
