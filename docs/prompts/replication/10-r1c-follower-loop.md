בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# R1-c — follower pull-loop (фоновая задача ReplPull→apply→advance)

> Контекст: `docs/roadmap/REPLICATION.md` §4/§5.2/§5.3/§5.6. Опирается на:
> R0-b `handle_repl` (leader-side), R1-a `RepoInstance::apply_replicated`,
> R1-b `RepoInstance::replication_bookmark`/`advance_replication_bookmark`.

## Задача

Фоновая задача follower'а, которая тянет события с лидера и применяет их
локально, продвигая durable bookmark. Ключ к тестируемости — отделить
loop-логику от транспорта через абстракцию источника.

## Архитектура (пинится здесь)

### 1. `ReplSource` trait (абстракция лидера)

Новый модуль в shamir-server (напр. `src/replication/mod.rs` +
`source.rs`, `follower_loop.rs` — новый каталог `replication/`):

```rust
#[async_trait::async_trait]
pub trait ReplSource: Send + Sync {
    async fn hello(&self, node_id: &str) -> Result<ReplResponse, ReplError>;
    async fn pull(&self, db: &str, repo: &str, from_version: u64,
                  limit: u32, wait_ms: Option<u32>) -> Result<ReplResponse, ReplError>;
}
```
(тип ошибки — свой `ReplError` enum thiserror, или reuse существующего).

### 2. Wire-импл через shamir-client

- Сначала в `crates/shamir-client/src/client.rs` добавить публичный тонкий
  метод: `pub async fn repl(&self, req: ReplRequest) -> Result<ReplResponse,
  ClientError>`, который вызывает приватный `roundtrip(&DbRequest::Repl(req))`
  и разбирает `DbResponse::Repl(r) => Ok(r)` (иначе ошибка). Реэкспорт
  `ReplRequest`/`ReplResponse` уже есть через `shamir_query_types::wire`.
- Wire-`ReplSource` импл оборачивает `shamir_client::Client` (держит
  подключённого как replicator-аккаунт клиента). Живёт в shamir-server
  (shamir-server уже зависит от shamir-client? если нет — НЕ добавляй
  тяжёлую зависимость; тогда wire-импл оставь на R1-d/e2e, а в R1-c поставь
  только trait + in-process-импл + loop, и отметь это в финальном сообщении).

### 3. `follower_loop` — движок

```rust
pub async fn run_follower_loop(
    db: Arc<ShamirDb>, source: Arc<dyn ReplSource>,
    node_id: String, db_name: String, repo: String,
    poll_wait_ms: u32, /* + cancellation: tokio_util::sync::CancellationToken или shutdown-rx */
) -> Result<(), ReplError>
```
Логика:
1. `hello(node_id)` → извлечь `leader_epoch`; запомнить как max seen.
2. Цикл (до отмены):
   - `from = repo.replication_bookmark()? + 1`.
   - `pull(db, repo, from, limit, wait_ms=poll_wait_ms)`.
   - **Epoch-fencing (§5.2):** если ответ несёт `leader_epoch <` max seen →
     вернуть `ReplError::StaleLeaderEpoch` (закрыть loop). Обновить max.
   - `gap_at: Some(g)` → лог + reseed: продолжить с `from = g` (в R1 просто
     сдвиг; полный snapshot-reseed — R2, отметить TODO).
   - декодировать `events` (`rmp_serde::from_slice::<Vec<ChangelogEvent>>`);
     для каждого по порядку:
     `apply_replicated(event, applied_watermark = bookmark)`; при
     `Applied` → `advance_replication_bookmark(event.commit_version)`
     (LEADER-версия!); при `Skipped` → пропустить.
   - если событий не было → `pull` уже поллил `wait_ms` на лидере; коротко
     `tokio::time::sleep` перед следующей итерацией НЕ обязателен (лидер уже
     ждал), но допустим маленький backoff.
   - §5.6: это фоновая задача, она НЕ держит локальных локов follower'а и
     НЕ блокирует его коммиты; ошибки транспорта — лог + backoff-повтор, а
     НЕ падение (кроме StaleLeaderEpoch, которое терминирует loop).

Запуск: `tokio::spawn(run_follower_loop(...))` — задача владельца.

## Тесты (shamir-server, in-process ReplSource)

In-process `ReplSource` импл, который держит Arc<ShamirDb> ЛИДЕРА +
leader-session и вызывает `handler.handle_repl(...)` напрямую (либо строит
ReplResponse из `read_changelog_from_journal` лидера + фиксированного
epoch). Follower — отдельный Arc<ShamirDb>.

1. **Применение N событий:** записать N строк на лидере; прогнать несколько
   итераций loop (или loop до догоняния) → follower сходится (те же записи
   читаются на follower'е), bookmark == leader current_version.
2. **Идемпотентность рестарта:** после догоняния запустить loop снова с тем
   же bookmark → apply_replicated возвращает Skipped, состояние не меняется,
   bookmark не откатывается.
3. **Epoch-регресс:** источник, возвращающий `leader_epoch` меньше ранее
   виденного → loop терминирует с `StaleLeaderEpoch`.

(Чтобы loop не крутился вечно в тесте — параметризуй «max iterations» или
используй cancellation-token, отменяемый после догоняния; НЕ полагайся на
бесконечный sleep.)

## Гейт

- `./scripts/test.sh @server` (+ `-p shamir-client` если тронул client)
  зелёный.
- `cargo fmt` по тронутым крейтам — чистый.
- `cargo clippy -p shamir-server --all-targets -- -D warnings` чистый
  (+ shamir-client если тронут).

## Definition of done

- `ReplSource` trait + `follower_loop` движок + (если shamir-client
  доступен) `Client::repl()` + wire-импл.
- Epoch-fencing, gap→reseed(shift), идемпотентность через bookmark, §5.6
  неблокирующая фоновая задача.
- 3 теста зелёные (in-process source).
- Финальное сообщение: тронутые файлы, включён ли wire-импл или отложен на
  R1-d, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
