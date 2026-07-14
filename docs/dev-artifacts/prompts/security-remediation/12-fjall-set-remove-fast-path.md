# Brief: Fjall set/remove flag-free fast path (taskId #613, partial HIGH, perf)

## Контекст

`crates/shamir-storage/src/storage_fjall.rs`, `set` (строки ~337-380) и
`remove` (строки ~529-559) — оба делают ЛИШНИЙ `keyspace.contains_key(...)`
LSM point-lookup ПЕРЕД самой записью/удалением, ТОЛЬКО чтобы вернуть
`bool` = "was created"/"existed and removed" — контракт `Store` trait'а
(`crates/shamir-storage/src/types.rs:38,66`). Уже задокументировано в
коде (обоих местах): "fjall 3.x `Keyspace::insert`/`remove` returns
`Result<(), Error>` — no prior-value — so `existed` CANNOT be derived
from the write op itself... A flag-free fast-path variant on `Store` is
the proper follow-up."

**Аудит вызывающих подтвердил: почти НИКТО реально не использует этот
флаг** (grep по всему workspace):

- `crates/shamir-engine/src/table/table_manager_crud.rs` — ДВА прямых
  вызова `.set(...)` (строки ~119-121, ~426-428) — ОБА полностью
  ИГНОРИРУЮТ возвращаемый `bool` (`.set(...).await?; 0` — результат не
  биндится вообще). Оба места уже делают СОБСТВЕННУЮ отдельную проверку
  существования через `self.get(id).await.ok()` (строки ~328, ~396) ДО
  вызова `set` — комментарий в `storage_fjall.rs` прямо это называет:
  "the storage-side flag is technically redundant for the engine".
- `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs` — ШЕСТЬ вызовов
  `.remove(...)` (строки ~111,246,247,344,347,460,461), ВСЕ через
  `let _ = self.history.remove(...).await;` — флаг ЯВНО отброшен.
- Единственный ЖИВОЙ потребитель флага: `crates/shamir-storage/src/storage_cached.rs:248`
  (`CachedStore::set`, `WriteMode::Sync` ветка) — но даже там, в
  СОСЕДНЕЙ `WriteMode::Async` ветке (строка ~254), `created` вычисляется
  НЕ через `self.inner.set(...)`'s флаг, а через `cache_upsert` —
  собственную, ДЁШЕВУЮ (in-memory, без LSM lookup) проверку по кэшу.

## Задача

### 1. Новые trait-методы с default-реализацией

`crates/shamir-storage/src/types.rs`, `trait Store` — добавь рядом с
`set`/`remove` (не убирай существующие, они остаются для потребителей,
которым флаг реально нужен — `CachedStore::set`'s Sync-ветка):

```rust
/// Like [`Self::set`] but for callers that don't need the "was created"
/// flag — skips the extra existence-check lookup some backends (Fjall)
/// would otherwise need to compute it (task #613). Default impl just
/// discards `set`'s flag; override where the backend can genuinely skip
/// the check.
async fn set_no_flag(&self, key: RecordKey, value: Bytes) -> DbResult<()> {
    self.set(key, value).await.map(|_| ())
}

/// Like [`Self::remove`] but for callers that don't need the "existed"
/// flag (task #613). Default impl just discards `remove`'s flag.
async fn remove_no_flag(&self, key: RecordKey) -> DbResult<()> {
    self.remove(key).await.map(|_| ())
}
```

(Проверь точное имя типов/сигнатур по актуальному коду `types.rs` —
`RecordKey`/`Bytes`/`DbResult` уже импортированы там для `set`/`remove`.
`#[async_trait]` уже применён к трейту — новые методы наследуют то же
макрос-разворачивание, default-тело должно компилироваться без проблем.)

### 2. `FjallStore` — реализация без лишнего lookup

`crates/shamir-storage/src/storage_fjall.rs` — переопредели оба метода,
УБРАВ `contains_key`:

```rust
async fn set_no_flag(&self, key: RecordKey, value: Bytes) -> DbResult<()> {
    let keyspace = self.keyspace.clone();
    task::spawn_blocking(move || -> DbResult<()> {
        keyspace
            .insert(&key[..], &value[..]) // подставь точный метод/сигнатуру insert, как в существующем `set`
            .map_err(|e| DbError::Storage(e.to_string()))
    })
    .await
    .map_err(|e| DbError::Internal(e.to_string()))?
}

async fn remove_no_flag(&self, key: RecordKey) -> DbResult<()> {
    let keyspace = self.keyspace.clone();
    task::spawn_blocking(move || -> DbResult<()> {
        keyspace
            .remove(&key[..])
            .map_err(|e| DbError::Storage(e.to_string()))
    })
    .await
    .map_err(|e| DbError::Internal(e.to_string()))?
}
```

(Посмотри существующий `set`/`remove` реализации строка-в-строку —
скопируй ТОЧНО тот же способ вызова `keyspace.insert`/`.remove`, просто
без предшествующего `contains_key` и без построения `existed`/`Ok(bool)`.
НЕ трогай существующий routing-комментарий про write worker (task #536)
— эти fast-path методы ТОЖЕ НЕ должны идти через write worker, по той же
причине, что и `set`/`remove` — если сомневаешься, спроси себя "does
this still embed a read before the write?" — ответ теперь "нет", так что
формально ограничение task #536 может не применяться, но НЕ меняй
routing без явной перепроверки бенчем — оставь на `spawn_blocking`, как
сейчас, не мигрируй на worker в рамках этой задачи.)

Добавь короткий doc-комментарий над каждым методом объясняющий разницу
с `set`/`remove` (без lookup, для callers без надобности во флаге).

### 3. Переключи confirmed-safe вызывающих

- `crates/shamir-engine/src/table/table_manager_crud.rs`, строки
  ~119-121 и ~426-428 — замени
  `self.table.data_store().set(RecordKey::from_slice(id.as_bytes()), bytes).await?;`
  на `.set_no_flag(...)` (тот же `RecordKey`/`bytes`, просто другой метод
  имени). Убедись, что `0` (dummy version placeholder) после этой строки
  не менялся по смыслу — только имя вызова метода.
- `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs`, все 6 мест вида
  `let _ = self.history.remove(...).await;` — замени на
  `let _ = self.history.remove_no_flag(...).await;` (или, если `remove_no_flag`
  возвращает `DbResult<()>` вместо `DbResult<bool>`, паттерн `let _ = ...await;`
  остаётся синтаксически корректным без изменений кроме имени метода).

### 4. НЕ трогай `CachedStore::set`

`crates/shamir-storage/src/storage_cached.rs:248` — ЕДИНСТВЕННЫЙ
подтверждённый живой потребитель `set`'s флага (`Sync`-ветка). НЕ меняй
эту ветку в рамках этой задачи — рассмотрение "можно ли и здесь заменить
на `cache_upsert`-based check вместо inner store флага" требует отдельного
анализа cache-coldness корректности (кэш может не содержать все ключи из
backing store, значит `cache_upsert`'s "existed" не эквивалентен
настоящему "существовал в backing store") — вне scope этой surgical
perf-задачи. Оставь `CachedStore::set`/`remove` полностью без изменений.

## Тесты

- `crates/shamir-storage` уже имеет тесты на `set`/`remove` (сам
  Fjall-backend тестовый файл — найди его, вероятно
  `storage_fjall_tests.rs`/аналог, посмотри как структурированы
  существующие тесты) — добавь симметричные тесты для
  `set_no_flag`/`remove_no_flag`: запись нового ключа, обновление
  существующего, удаление существующего/несуществующего — проверь, что
  данные реально записываются/удаляются (через последующий `get`), не
  просто что метод не паникует.
- Если есть benchmark-файл на `storage_fjall`'s `set`/`remove` (grep
  `benches/` для `storage_fjall`) — НЕ обязательно добавлять новый
  бенч на fast-path методы, но если легко — плюс.

## Прогон проверок

- `cargo fmt -p shamir-storage -p shamir-engine -p shamir-tx -- --check`
- `cargo clippy -p shamir-storage -p shamir-engine -p shamir-tx --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-storage -p shamir-engine -p shamir-tx --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ убирай существующие `set`/`remove` из трейта — они остаются,
  `set_no_flag`/`remove_no_flag` ДОБАВЛЯЮТСЯ рядом, не заменяют.
- НЕ трогай `CachedStore`'s код (ни `set`, ни `remove`) — вне scope,
  пункт 4 выше объясняет почему.
- НЕ мигрируй эти новые методы на write worker (task #536) — оставь на
  `spawn_blocking`, как существующие `set`/`remove`.
- НЕ меняй сигнатуру существующих `set`/`remove` (по-прежнему
  `DbResult<bool>`) — только ДОБАВЬ новые методы.
- Если найдёшь ЕЩЁ каких-то вызывающих `.set(`/`.remove(` за пределами
  перечисленных, которые ТОЖЕ явно игнорируют bool (`let _ = ...`/
  без биндинга результата) — можешь переключить и их на fast-path
  вариант, но СНАЧАЛА покажи в отчёте, что нашёл, прежде чем менять
  что-то за пределами явно перечисленного в этом брифе списка (табличный
  учёт для оркестратора).

## Проверка (сделает оркестратор)

- Диф ограничен `types.rs`, `storage_fjall.rs` (оба shamir-storage),
  `table_manager_crud.rs` (shamir-engine), `mvcc_gc.rs` (shamir-tx),
  плюс новые тесты.
- fmt/clippy по трём крейтам чисты.
- `./scripts/test.sh -p shamir-storage -p shamir-engine -p shamir-tx --full`
  зелёный.
- `CachedStore` не тронут вообще (`git diff` не должен показывать
  изменений в `storage_cached.rs`).
