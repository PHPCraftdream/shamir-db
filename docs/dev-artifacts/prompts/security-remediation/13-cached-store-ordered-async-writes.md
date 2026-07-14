# Brief: CachedStore ordered async writes (taskId #616 pt.2, low severity)

## Контекст

`crates/shamir-storage/src/storage_cached.rs`, `CachedStore::set`/`remove`
в `WriteMode::Async` (строки ~252-276 и ~304-320) — КАЖДЫЙ вызов спавнит
НЕЗАВИСИМУЮ `tokio::spawn(async move { inner.set(...)... })` таску
("fire-and-forget by design", §B8).

**Реальный баг**: если один и тот же `RecordKey` записывается ДВАЖДЫ
подряд (например `set(k, v1)` затем `set(k, v2)`), кэш обновляется
СИНХРОННО и в правильном порядке (`cache_upsert` вызывается на каждом
`set` до спавна фоновой таски), но ДВЕ независимо заспавненные таски,
пишущие в `inner` (backing store), НЕ гарантируют порядок исполнения —
tokio-scheduler может выполнить таску для `v2` раньше таски для `v1`.
Если это произойдёт, backing store останется с УСТАРЕВШИМ `v1`, пока
кэш верно хранит `v2` — расхождение кэш/диск. `flush()` (строки ~205-216)
ждёт `pending_writes == 0`, но это НЕ гарантирует порядок — только
что все таски ЗАВЕРШИЛИСЬ, в любом порядке.

## Задача

Замени "spawn таску на каждую запись" на ОДИН фоновый tokio-worker per
`CachedStore` instance, обрабатывающий очередь job'ов СТРОГО FIFO —
мирроря уже существующий паттерн `WriteWorker` в
`crates/shamir-storage/src/storage_fjall.rs:55-131` (тот же файл,
`enum WriteJob` / `struct WriteWorker` / `fn submit`), НО:

- Fjall's `WriteWorker` — выделенный OS-thread + sync `mpsc::sync_channel`,
  потому что fjall's операции СИНХРОННЫЕ (blocking). `CachedStore`'s
  `inner: Arc<dyn Store>` — АСИНХРОННЫЙ trait (`async fn set`/`remove`),
  так что для этого случая нужен НЕ OS-thread, а один ЛЁГКИЙ фоновый
  `tokio::spawn`-таск (создаётся ОДИН РАЗ в конструкторе, живёт всё время
  жизни `CachedStore`), потребляющий `tokio::sync::mpsc::UnboundedReceiver`
  (или bounded, на твоё усмотрение — unbounded проще, т.к. отправитель
  никогда не блокируется, что важно раз `set`/`remove` остаются async fn
  без await на send).

### 1. Новый тип job'а + worker-таск

```rust
enum CacheWriteJob {
    Set { key: RecordKey, value: Bytes },
    Remove { key: RecordKey },
}
```

В конструкторе (`new_with_mode`, строка ~115) — если `mode == WriteMode::Async`,
заведи `tokio::sync::mpsc::unbounded_channel::<CacheWriteJob>()`, сохрани
`Sender` в `CachedStore` (новое поле, например
`async_write_tx: Option<mpsc::UnboundedSender<CacheWriteJob>>`, `None`
для `WriteMode::Sync`), и заспавни ОДИН фоновый `tokio::spawn`, который
`while let Some(job) = rx.recv().await { ... apply to inner, decrement pending_writes on completion ... }` —
СТРОГО последовательно (один `.await` за раз, не спавня вложенные таски
внутри цикла) — это и даёт гарантию порядка.

### 2. `set`/`remove` в Async-ветке — отправляют job вместо spawn

Замени:
```rust
tokio::spawn(async move {
    if let Err(e) = inner.set(key, value).await { ... }
    pending.fetch_sub(1, Ordering::Relaxed);
});
```
на что-то вроде:
```rust
self.pending_writes.fetch_add(1, Ordering::Relaxed);
let _ = self
    .async_write_tx
    .as_ref()
    .expect("async_write_tx set for WriteMode::Async")
    .send(CacheWriteJob::Set { key, value });
```
(`send` на `UnboundedSender` синхронный и не блокирует — не требует
`.await`; ошибка `send` означает "worker остановлен/канал закрыт",
логируй через `log::error!`, аналогично текущему `if let Err(e) = inner.set(...)`
логированию — не молчаливо игнорируй.)

Внутри worker-цикла — на каждый `CacheWriteJob`, ПОСЛЕ `inner.set/remove(...).await`,
`pending_writes.fetch_sub(1, ...)` (перенеси эту логику из старого
per-spawn кода в worker-цикл, сохрани точно то же поведение
логирования ошибок — "§B8: WriteMode::Async is fire-and-forget by
design, but a swallowed Err silently loses durability... Log so an
operator gets a signal").

### 3. Завершение worker'а при `Drop`

`CachedStore` сейчас не реализует `Drop` явно — проверь, нужна ли
явная остановка worker-таска (если `Sender` просто дропается вместе с
`CachedStore`, `Receiver`'s `recv()` вернёт `None` и цикл естественно
завершится — Tokio таск не нуждается в join как OS-thread, но убедись,
что нет утечки/зависшего таска в тестах — возможно понадобится
`JoinHandle` + `Drop` impl, мирроря `WriteWorker`'s Drop, ЕСЛИ тесты
показывают проблему; если естественного завершения через drop канала
достаточно — не усложняй).

## Тесты

Добавь тест (в `crates/shamir-storage/src/tests/` — найди файл с
существующими `CachedStore`/`storage_cached` тестами) специально
демонстрирующий ИСПРАВЛЕННЫЙ баг:
- Создай `CachedStore` в `WriteMode::Async` с `inner`, который
  ИСКУССТВЕННО задерживает более РАННИЕ записи дольше, чем более
  ПОЗДНИЕ (например, обёртка-мок над `InMemoryStore`, вставляющая
  `tokio::time::sleep` пропорционально номеру вызова — первый вызов
  спит дольше второго) — под СТАРЫМ кодом (независимые spawn) это бы
  привело к тому, что второй (более новый) write долетел бы до `inner`
  РАНЬШЕ первого, оставив `inner` с устаревшим значением после
  `flush()`. Под НОВЫМ кодом (упорядоченная очередь) порядок
  гарантированно FIFO независимо от относительной задержки экзекуции.
- `set(k, v1)` затем СРАЗУ `set(k, v2)`, `flush().await`, затем
  `inner.get(k)` (не через кэш!) должен вернуть `v2`, не `v1`.
- Если конструирование "искусственно медленного mock Store" слишком
  трудоёмко — альтернативный, более простой тест: много быстрых
  последовательных `set`/`remove` на ОДИН ключ (например 50 раз,
  чередуя set/remove), `flush()`, потом `inner.get()`/`inner`-level
  проверка совпадает с ПОСЛЕДНИМ логическим состоянием — если тест
  иногда флейкует под старым кодом (доказывая гонку) и всегда зелёный
  под новым — это тоже приемлемое доказательство фикса.

## Прогон проверок

- `cargo fmt -p shamir-storage -- --check`
- `cargo clippy -p shamir-storage --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-storage --full`
- Также `./scripts/test.sh -p shamir-engine -p shamir-tx --full` —
  `CachedStore` используется как обёртка в паре мест выше по стеку,
  косвенная регрессия должна быть исключена.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `WriteMode::Sync` ветку — она уже строго последовательна
  (await прямо на месте вызова), не в scope.
- НЕ копируй Fjall's OS-thread `WriteWorker` буквально — `CachedStore`'s
  `inner` асинхронный, легковесный tokio-таск подходит лучше и не
  требует блокирующего thread'а.
- НЕ меняй публичный API `CachedStore` (`new_sync`/`new_async`/
  `mode()`/`cache_size()` и т.д.) — только внутреннюю реализацию
  Async-веток `set`/`remove` + конструктор.

## Проверка (сделает оркестратор)

- Диф ограничен `storage_cached.rs` + новый(е) тест(ы) в
  `crates/shamir-storage/src/tests/`.
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-storage -p shamir-engine -p shamir-tx --full`
  зелёный.
- Новый тест реально демонстрирует упорядоченность (не тавтологичен) —
  желательно с обоснованием, что он ловил бы регресс на старом коде.
