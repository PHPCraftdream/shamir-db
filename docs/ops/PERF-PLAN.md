# План оптимизации S.H.A.M.I.R. DB

Актуально на 2026-05-11.

Этот план заменяет старую версию `PERF-PLAN.md`. Он учитывает уже сделанные
ускорения в последних коммитах и текущем worktree: `Store::insert_many`,
`set_many/remove_many`, amortised durability, key-per-record postings,
sorted-index range/min/first_k, reverse range для `sled/redb`, `lookup_last_k`,
`MAX`, `ORDER BY ... LIMIT` fast path.

Главный принцип: сначала убирать асимптотические и I/O bottlenecks, потом CPU
clone/alloc overhead. Микрооптимизации нужны, но не должны вытеснять batch I/O.

## Уже сделано, не планировать заново

- Release profile: `opt-level = 3` вместо `z`.
- Data bulk insert: `Store::insert_many` и native batch implementations для
  основных backends.
- `Store::set_many`, `remove_many`, `flush` contract.
- `redb` amortised durability через `Durability::None` плюс explicit `flush`.
- Key-per-record posting layout для обычных индексов.
- Sorted-index `lookup_range`, `lookup_min`, `lookup_first_k`.
- Reverse range API в `Store`: `iter_range_stream_reverse`.
- Native reverse range уже есть для `sled` и `redb`.
- `lookup_last_k`, `lookup_max`.
- `ORDER BY field ASC/DESC LIMIT/OFFSET` fast path через sorted index.
- Within-batch unique validation в `insert_many`.
- Self-delimiting sorted-index codec.
- `CachedStore::flush` доступен через trait dispatch.

## P0 - следующие сильные ускорения

### P0-1. `Store::get_many` для index scan и sorted-order fast paths

Сейчас после lookup по индексу движок делает N последовательных чтений:

- `read_sorted_index_scan`: `for id in &record_ids { self.get(*id).await ... }`
- `read_order_limit_fast`: `for id in ids.into_iter().skip(skip) { self.get(id).await ... }`
- `read_index_scan`: `for id in &record_ids { self.get(*id).await ... }`

Это особенно дорого на disk backends: много отдельных async calls,
`spawn_blocking`, read transactions или cursor setup.

Что сделать:

- Добавить в `Store`:

```rust
async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>>;
```

- Default implementation: простой loop через `get`, `NotFound` превращать в
  `None`, остальные ошибки пробрасывать.
- `InMemoryStore`: прямой DashMap lookup по всем ключам, порядок ответа как у
  входа.
- `CachedStore`: читать из cache, misses собирать и добирать через
  `inner.get_many`, затем обновлять cache.
- `sled`: один `spawn_blocking`, внутри loop `tree.get`.
- `redb`: одна read transaction на весь batch.
- `nebari/fjall/canopy/persy`: минимум один `spawn_blocking` на batch; где есть
  read transaction или tree handle, использовать один на весь batch.
- Использовать `get_many` в `read_index_scan`, `read_sorted_index_scan`,
  `read_order_limit_fast`.

Acceptance criteria:

- Backend-agnostic test в `types.rs`: порядок сохраняется, missing key дает
  `None`, non-NotFound error не маскируется.
- Tests для stale index entries: stale id пропускается так же, как сейчас.
- Benchmarks:
  - equality index scan: 10K rows, selectivity 10/100/1000;
  - sorted range scan: 10K rows, range 100/1000;
  - `ORDER BY field LIMIT 10/100/1000`.

Ожидаемый эффект:

- 2-5x на index scans с disk backend.
- Больше на backends, где текущий `get` открывает read transaction или запускает
  отдельный blocking task.
- Малый эффект на pure in-memory и tiny result sets.

### P0-2. Batch index writes в `TableManager::insert_many`

Data-store batch insert уже сделан, но индексы все еще обновляются per record:

```rust
for (id, value) in ids.iter().zip(values.iter()) {
    self.index_manager.on_record_created(id, value).await?;
    self.index_manager.on_record_created_unique(id, value).await?;
    self.sorted_indexes.on_record_created(id, value).await?;
}
```

При N=10K и нескольких индексах это снова превращается в десятки тысяч
`info_store.set()` и async I/O.

Что сделать:

- Добавить batch collectors, которые только строят записи индекса, но не пишут:
  - `IndexManager::collect_created_entries_batch`
  - `IndexManager::collect_created_unique_entries_batch`
  - `SortedIndexManager::collect_created_entries_batch`
- На выходе получать `Vec<(RecordKey, Bytes)>`.
- Писать через `info_store.set_many(entries)`, не через `insert_many`: ключи
  postings детерминированные.
- Инвалидацию posting cache делать batch-ом по затронутым index/value keys.
- Сохранить текущую unique validation:
  - persisted-state check;
  - within-batch duplicate check.
- Для sorted-index backfill при `create_sorted_index` тоже писать пачками через
  `set_many`.

Acceptance criteria:

- `insert_many` с normal + unique + sorted indexes.
- Тест на within-batch unique duplicate остается зеленым.
- Тест на partial failure: ошибка index write не должна молча оставлять
  неконсистентное состояние. Если полная atomicity невозможна для backend,
  это должно быть явно описано и покрыто тестом.
- Benchmarks:
  - bulk insert 10K rows without indexes;
  - with one normal index;
  - with normal + unique + sorted index.

Ожидаемый эффект:

- 2-8x на indexed bulk insert.
- На no-index insert эффекта почти нет.
- На backends с native transactional `set_many` эффект максимальный.

### P0-3. Закрыть native reverse/range gaps по backends

`ORDER BY DESC LIMIT`, `MAX` и `lookup_last_k` уже используют
`iter_range_stream_reverse`. Но native reverse сейчас есть только у `sled` и
`redb`. Остальные backends идут через default implementation, который собирает
range в память и разворачивает.

Что сделать:

- Проверить поддержку reverse/range у:
  - `fjall`
  - `nebari`
  - `persy`
  - `canopy`
- Где backend поддерживает ordered cursor, добавить native
  `iter_range_stream_reverse`.
- Где не поддерживает, явно задокументировать fallback и не обещать O(log N + K).

Acceptance criteria:

- Общий тест `iter_range_stream_reverse` уже есть; расширить backend-specific
  assertions при необходимости.
- Benchmarks `ORDER BY DESC LIMIT 10` на каждом backend.

Ожидаемый эффект:

- Большой для DESC/MAX на backends, которые сейчас fallback-ят в O(N).
- Нулевой для `sled/redb`, где native path уже есть.

## P1 - высокий эффект, но меньше I/O blast radius

### P1-1. Covering indexes

Даже после `get_many` index scan все равно ходит в data store за полными
record bytes. Для запросов вида:

```sql
SELECT indexed_field, other_small_field
WHERE indexed_field = ?
ORDER BY indexed_field
LIMIT K
```

можно не читать record вообще, если индекс хранит нужные projected fields.

Что сделать:

- Расширить definition индекса опциональным `stored_fields`.
- Для sorted/equality index entry хранить:
  - record id;
  - indexed value;
  - packed projected fields или компактный `InnerValue::Map`.
- Planner должен выбирать covering path, только если:
  - `SELECT` покрыт индексом;
  - `WHERE` residual отсутствует или тоже покрыт;
  - `ORDER BY` покрыт;
  - не нужны aggregates/grouping, кроме отдельно поддержанных случаев.
- Сначала реализовать для sorted indexes, потому что `ORDER BY LIMIT` уже дает
  малый K и понятный выигрыш.

Acceptance criteria:

- Query stats показывает `covering_sorted_idx_*` или аналогичный label.
- Тест: covered query не вызывает data-store `get/get_many` вообще.
- Тест: stale data/index mismatch не приводит к неверному record id в ответе.

Ожидаемый эффект:

- Сильный на read-heavy workloads: минус random reads после index lookup.
- Особенно важен после P0-1, потому что это следующий потолок index scans.

### P1-2. Borrowing lookup для `resolve_field` / `resolve_path`

Сейчас hot paths клонируют `InnerValue` и создают `InternerKey` при каждом
шаге навигации:

- `query/filter/eval.rs::resolve_field`
- `index/sorted_index_manager.rs::resolve_path`
- `index/index_manager.rs::extract_value_by_path`

Что сделать:

- Добавить borrowing варианты:

```rust
fn resolve_field_ref<'a>(record: &'a InnerValue, path: &[u64]) -> Option<&'a InnerValue>;
fn resolve_path_ref<'a>(record: &'a InnerValue, path: &[u64]) -> Option<&'a InnerValue>;
```

- Перевести filter callbacks на refs там, где возможно.
- `resolve_filter_value` сделать через borrowed/Cow path для string/binary
  literals, не клонировать без нужды.
- Для index extraction клонировать только финальные values, которые реально
  нужны для key encoding/posting construction.
- Отдельно решить проблему `InternerKey::new(path[i])` в lookup: либо после
  P1-3 ключ станет cheap, либо добавить lookup по raw `u64`.

Acceptance criteria:

- Tests для всех filter callbacks остаются зелеными.
- Benchmarks:
  - full scan with 1/3/5 predicates;
  - wide records;
  - sorted-index insert/update.

Ожидаемый эффект:

- Хороший CPU constant-factor на scans и index maintenance.
- Не стоит ожидать 3-5x на полный query, если bottleneck уже storage decode или
  JSON projection.

### P1-3. `InternerKey(u64)` вместо `InternerKey(Bytes)` в памяти

Текущий `InternerKey(Bytes)` аллоцирует маленький `Bytes` при `new(id)`, а
`Hash/Eq/Ord` каждый раз декодируют bytes обратно в `u64`.

Что сделать:

- Представление в памяти:

```rust
pub struct InternerKey(u64);
```

- `new`, `id`, `Hash`, `Eq`, `Ord` работают напрямую по `u64`.
- `Serialize` продолжает писать variable-length bytes, как сейчас.
- `Deserialize` принимает старые bytes и создает `InternerKey(u64)`.
- `as_bytes()` заменить на:
  - `encode_to_vec()`;
  - или `write_bytes(&mut Vec<u8>)`;
  - избегать API, который требует хранить bytes внутри struct.

Важная поправка к старому плану: это не обязано ломать on-disk format. Breaking
change нужен только если поменять serialization. Менять serialization не надо.

Acceptance criteria:

- Existing MessagePack/bincode codec tests проходят без изменения fixtures.
- Тест на разные byte widths: ids 1, 255, 256, 65536, u32+1.
- Benchmarks:
  - map lookup by interned key;
  - filter scan;
  - JSON/MessagePack decode/encode.

Ожидаемый эффект:

- 1.2-2x на CPU-heavy map/key hot paths.
- Особенно помогает после P1-2, если `InternerKey::new` все еще часто вызывается.

### P1-4. Ordered in-memory/cached iteration без full materialization

`InMemoryStore::iter_stream` сейчас собирает все ключи, сортирует, потом
батчит. `CachedStore::iter_stream` собирает все key/value pairs и сортирует.
Для `LIMIT 10` это все равно O(N log N) и лишняя память.

Что сделать аккуратно:

- Не делать механическую замену `DashMap -> RwLock<BTreeMap>` без benchmark:
  это может ухудшить concurrent writes.
- Рассмотреть отдельный ordered backend для tests/benchmarks:
  - `BTreeMap<RecordKey, Bytes>` под `RwLock`;
  - batch clone только текущей страницы;
  - range/reverse native через `BTreeMap::range`.
- Для `CachedStore` можно держать вторичную ordered structure:
  - DashMap для point lookup/write concurrency;
  - BTree index of keys для ordered iteration.
- Если двойная структура слишком сложна, оставить как P2 для in-memory only.

Acceptance criteria:

- Benchmarks на scan, range, reverse range, concurrent set/get.
- Не должно быть read lock held across async yield.

Ожидаемый эффект:

- 2-5x на in-memory/cached scan/range workloads.
- Может регрессировать write-heavy workloads, поэтому нужен benchmark до merge.

## P2 - средний эффект и нишевые пути

### P2-1. `deintern_key` без двойной String allocation

Сейчас `get_str` возвращает owned `UserKey`, затем `deintern_key` делает
`.as_ref().to_string()`.

Что сделать:

- Добавить `Interner::with_str(id, |s| ...)` для zero-copy callback.
- Или вернуть owned `UserKey` и конвертировать в `String` без второго clone.

Эффект: заметен на JSON/MessagePack encode большого числа records, но это не
главный bottleneck index scans.

### P2-2. HAVING без JSON roundtrip

`apply_group_by` строит JSON aggregate object, потом для HAVING делает
`serde_json::to_vec` и `json_to_inner`.

Что сделать:

- Строить aggregate object как `InnerValue::Map`.
- HAVING применять к `InnerValue`.
- JSON строить после HAVING.

Эффект: только GROUP BY/HAVING workloads.

### P2-3. `merge_inner_maps` in-place для UPDATE/SET

Сейчас `merge_inner_maps` клонирует всю map, даже если меняется одно поле.

Что сделать:

- Для update path клонировать record один раз и мутировать map in-place.
- Сравнение changed делать до/после только по touched keys, где возможно.
- Не ломать semantics для non-map values.

Эффект: хороший на UPDATE больших records, слабый на обычных inserts/reads.

### P2-4. `CachedStore` async mode без spawn-per-write

Сейчас async write mode делает `tokio::spawn` на каждый `set/remove`.

Что сделать:

- Один background worker + `mpsc` queue.
- Coalesce writes по key.
- `flush()` ждет drain queue и inner flush.

Эффект: только если `CachedStore::new_async` реально используется на горячем
write path.

### P2-5. `InternerManager::get_sync` fast path

`InternerManager::get()` уже проверяет `OnceCell::get()` перед async init, но
call sites все равно проходят через async function.

Что сделать:

- Добавить `fn get_sync(&self) -> Option<&Interner>`.
- В hot call sites: сначала `if let Some(interner) = get_sync()`, и только на
  cold path вызывать `get().await`.

Эффект: небольшой. Делать после P0/P1, если benchmark показывает overhead.

## Рекомендуемый порядок работ

1. P0-1 `Store::get_many`.
2. P0-2 batch index writes через `set_many`.
3. P0-3 native reverse gaps по backends.
4. P1-2 borrowing `resolve_field/resolve_path`.
5. P1-3 `InternerKey(u64)` с совместимой serialization.
6. P1-1 covering indexes.
7. P1-4 ordered in-memory/cached iteration.
8. P2 по фактическим benchmark bottlenecks.

Почему `InternerKey(u64)` после borrowing: borrowing refactor уменьшит число
ненужных clones сразу и даст более ясную картину, сколько `InternerKey::new`
осталось в hot path. Если окажется, что `InternerKey::new` все еще доминирует,
P1-3 можно поднять перед P1-2.

## Что обязательно мерить

Минимальный bench set перед и после каждого P0/P1 изменения:

- full scan, 10K/100K rows, 1/3/5 predicates;
- equality index scan, selectivity 10/100/1000;
- sorted range scan, result 10/100/1000;
- `ORDER BY ASC LIMIT 10/100/1000`;
- `ORDER BY DESC LIMIT 10/100/1000`;
- bulk insert 10K without indexes;
- bulk insert 10K with normal + unique + sorted indexes;
- UPDATE 10K rows, small patch over wide records;
- JSON/MessagePack encode/decode roundtrip for projected records.

Для каждого результата фиксировать:

- backend;
- debug/release;
- wall time;
- records scanned;
- records returned;
- index label в `QueryStats`;
- memory peak, если изменение связано с iteration/materialization.

## Чего не делать

- Не обещать 3-5x от clone removal на весь query без benchmark. Это CPU
  constant-factor, не асимптотическое ускорение.
- Не менять `IndexMap -> HashMap` для `InnerValue::Map`: порядок полей важен.
- Не менять `BTreeSet -> HashSet` в posting lists без отдельного плана:
  deterministic order полезен и для tests, и для predictable scans.
- Не заменять все `DashMap` на `RwLock<HashMap>` механически.
- Не оптимизировать error-path allocations до закрытия P0/P1.
- Не менять storage serialization format для `InternerKey`, если можно сохранить
  совместимость через custom serde.
