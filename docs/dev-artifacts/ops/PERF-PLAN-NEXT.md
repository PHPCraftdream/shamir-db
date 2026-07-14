# PERF-PLAN-NEXT.md

Следующая итерация оптимизации. Структура каждого пункта строго:

1. **Bench (baseline)** — фиксируем текущие цифры в `target/criterion`.
2. **Test (RED)** — пишем тест, который падает на текущем коде.
3. **Optimization** — код.
4. **Test (GREEN)** — тот же тест проходит.
5. **Bench (after)** — фиксируем дельту.

Без 1 и 5 пункт не считается закрытым. Без 2 не начинаем 3.

Базовая отсылка: `docs/dev-artifacts/ops/PERF_BASELINE.md` (история), `docs/dev-artifacts/roadmap/PERF_OPPORTUNITIES.md` (исходный roadmap), code review результаты в комменте сессии.

---

## P0-1 — `Store::get_many` + проводка в read paths

**Проблема.** `read_exec.rs:612, 765, 824` делают `for id in &record_ids { self.get(*id).await }`. На disk бэкендах каждый `get` — отдельный `spawn_blocking` + (для redb/persy/nebari) свой read-transaction setup. После сорт-индекса и ORDER BY LIMIT это главный оставшийся bottleneck на чтении.

**Целевая дельта.** 3–10× на disk read paths с N matched ≥ 100.

### Шаги

1. **Bench (baseline)**
   - `read_by_city_with_index/10000` (in-memory, ~106 ms)
   - `range_query_with_index_sled/10000` (sled, ~48 ms)
   - `range_query_narrow_with_index_sled/10000` (sled, ~8 ms)
   - сохранить в `target/criterion`.

2. **Test (RED)**
   - `crates/shamir-storage/src/types.rs::run_batch_store_tests` — добавить блок: `get_many` по 10 ключам возвращает Vec в порядке input, отсутствующие — `Err(NotFound)` либо `Option::None` (выбрать контракт), пустой ввод → пустой Vec.
   - Per-backend test_*_batch_ops уже зовут run_batch_store_tests → автоматически распространяется.
   - **На текущем коде RED**: метода нет.

3. **Optimization**
   - Trait: `async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<DbResult<Bytes>>>` (внутренние ошибки per-key, не fail-whole). Default impl: loop.
   - **Семантика отсутствующих ключей:** возвращаем `Err(NotFound)` per-key, не None. Это сохранит ту же error-propagation что у текущих per-id `get`.
   - Native impls:
     - `redb`: один `begin_read()` + N `table.get`. Сортировать ключи для locality.
     - `sled`: один `spawn_blocking` + N `tree.get`. Опционально build range от min до max + filter.
     - `persy`: один read внутри одной транзакции / scan.
     - `nebari`: один `Roots::transaction` + N reads.
     - `fjall`, `canopy`: остаются на default loop, но в одном spawn_blocking (group_into_blocking helper).
   - Engine: `read_exec.rs` три call-сайта переводим на `get_many`.

4. **Test (GREEN)**
   - Тот же `run_batch_store_tests` зелёный.
   - **Дополнительно:** unit test в `read_exec_tests.rs` (или новый файл) что после индексного lookup'а records приходят в правильном порядке и без потерь.

5. **Bench (after)**
   - Те же три бенча. Ожидаем -50%…-90% на sled.

**Риски.** CachedStore: cache hit per-key, miss-fill через inner.get_many; race между cache.get и inner.get уже есть в single-`get`, не делаем хуже. Тест на miss-fill порядок.

---

## P0-2 — Batch index writes через `info_store.set_many`

**Проблема.** `table_manager.rs:279-285` после batched data write обновляет индексы `for (id, value) in ids.iter().zip(values.iter()) { on_record_created … }`. Каждое `add_index_entry` / `add_unique_entry` / `sorted_indexes.on_record_created` — отдельный `info_store.set`. На bulk_insert с индексами это N×commit, что съедает выигрыш от data-store batching.

**Целевая дельта.** 1.3–3× на `bulk_insert_with_index_*` на disk бэкендах.

### Атомарность — решаем ДО кода

Текущий контракт неявный: "если `insert_many` вернул Ok, данные + индексы + counter консистентны". На default-loop бэкендах он уже частично нарушен. План:

- **Tier 1 (redb / sled / nebari / persy)** — собрать data + все index writes в **одну** транзакцию (там где backend это даёт). Atomic.
- **Tier 2 (in_memory / cached / fjall / canopy)** — best-effort. Документировать в `Store::insert_many` doc-comment.
- **Тест атомарности:** failing-after-N test wrapper `Store` который ronжается на N-м write; убедиться что либо вся batch landed либо ничего.

### Шаги

1. **Bench (baseline)**
   - `bulk_insert_with_index_sled/1000` (121 ms сейчас)
   - Добавить новый: `bulk_insert_with_index_redb/1000`, `_nebari`, `_persy`.

2. **Test (RED)**
   - `IndexManager::add_index_entries_batch(name, items: &[(values, record_id)])` — новый метод. Текущая реализация: none → compile-fail.
   - Атомарность test: мок-Store с lifecycle counter, set_many на N+1-м элементе возвращает Err. После insert_many: count == 0 (или N если уже flush'ну) — НИКОГДА partial-N.

3. **Optimization**
   - `IndexManager::add_index_entries_batch(name, items)` — собирает все posting keys, один `info_store.set_many`. Cache-invalidate batched.
   - Аналогично `SortedIndexManager::on_record_created_batch`.
   - `TableManager::insert_many` — после `table.insert_many`, для каждого индекса собрать `Vec<(values, record_id)>` и вызвать batch hooks.
   - Для tier 1 backends — обернуть data + info_store writes в одну транзакцию. Это требует ещё одного trait метода `Repo::transaction(stores)` ИЛИ перенос batching на уровень backend'а через специальный объект `BatchedWriter`. **Дизайн-решение нужно зафиксировать.** Для первой итерации — оставить tier 2 semantics везде, atomicity потом.

4. **Test (GREEN)**
   - Batch insert + immediate index lookup — все записи находятся.
   - Existing tests 1240/0 — не должно сломаться.

5. **Bench (after)**
   - bulk_insert_with_index_* на всех backends.

---

## P0-2b — Batch UPDATE/DELETE через `set_many`/`remove_many`

**Проблема.** `execute_update` / `execute_delete` идут через `lookup_records_via_index` → per-record set/remove. После P0-1 это станет следующей точкой амплификации.

**Целевая дельта.** 1.5–4× на bulk update/delete.

1. **Bench (baseline)**
   - Сейчас нет bulk-update/bulk-delete бенчей. Добавить:
     - `bulk_update_by_filter/1000` (UPDATE WHERE city = X)
     - `bulk_delete_by_filter/1000`
   - на in_memory + sled.

2. **Test (RED)**
   - Engine-level: `TableManager::update_many` / `delete_many` (или модификация существующего `execute_update`/`execute_delete` чтобы он использовал batch). RED — компилируется но идёт по старому пути; bench показывает baseline.
   - Не correctness regression — pure perf.

3. **Optimization**
   - В `execute_update`: после lookup собрать `Vec<(RecordKey, Bytes)>` обновлённых, вызвать `data_store.set_many` + index batch hooks.
   - Аналогично `execute_delete` → `data_store.remove_many`.

4. **Test (GREEN)**
   - Existing tests + новые bench-вспомогательные тесты на корректность.

5. **Bench (after)**
   - bulk_update_by_filter, bulk_delete_by_filter.

---

## P0-3 — Native `iter_range_stream_reverse` для fjall/nebari/persy/canopy

**Проблема.** Default impl делает forward scan + collect + reverse в памяти, O(N). На больших range этого хватает чтобы убить wallclock даже когда K маленький. sled и redb уже native; четыре остальных — нет.

**Целевая дельта.** На fjall/nebari/persy/canopy DESC LIMIT K должен стать константным по K (как sled: ~600 µs независимо от N).

### Шаги

1. **Bench (baseline)**
   - Добавить `order_limit_top10_desc_sorted_{fjall,nebari,persy,canopy}` бенчи.

2. **Test (RED)**
   - `run_batch_store_tests::iter_range_stream_reverse` уже есть — он correctness-зеленый на default impl. Тест останется зелёным после native. Но добавим **performance assertion test** который проверяет асимптотику: тайминг на N=100 и N=10000 должен быть в пределах 5×, не 100×.
   - Альтернатива: смотреть на cache-state / iter-count метрику (если есть).
   - Если performance assertion слишком хрупкий — пропустить шаг 2 для этого пункта; correctness уже покрыт.

3. **Optimization**
   - `fjall`: `partition.range(...).rev()` если API даёт; иначе оставить default.
   - `nebari`: `tree.scan(range, forward=false, ...)` — у nebari есть параметр направления в `scan`.
   - `persy`: `tx.range::<K, V, _>(name, ..)` поддерживает только forward; для reverse — buffered approach. Возможно остаётся default.
   - `canopy`: проверить API.

4. **Test (GREEN)**
   - Все backend `test_*_batch_ops` зелёные.

5. **Bench (after)**
   - DESC LIMIT 10 на каждом backend.

---

## P0-4 — `flush()` audit для всех backends

**Проблема.** Только sled и redb имеют осмысленные `flush()` override. fjall (journal-buffered), nebari, persy, canopy не реализуют trait метод → через `Arc<dyn Store>` идёт в default no-op. Callers могут считать что fsync произошёл.

### Шаги

1. **Bench (baseline)** — не релевантно (correctness item, не perf).

2. **Test (RED)**
   - Добавить в `run_batch_store_tests`: после write вызвать `flush()` через dyn, затем (для disk бэкендов) reopen репозитория и проверить что данные на диске.
   - Для backend без durability gap (fjall: journal write-ahead, all writes committed) тест может быть слабее — проверить что flush возвращает Ok.

3. **Optimization**
   - `fjall`: `keyspace.persist(PersistMode::SyncAll)` если API доступен.
   - `nebari`: явный sync. Если нет API — документировать.
   - `persy`: его commits уже fsync'ятся, можно noop.
   - `canopy`: проверить.

4. **Test (GREEN)** — после implementation.

5. **Bench (after)** — не релевантно.

---

## P1-3 — `InternerKey(u64)` вместо `Bytes`

**Проблема.** `InternerKey` обёртывает variable-width `Bytes` (varint), что дороже по CPU и памяти чем `u64`. Hash, Eq, Ord идут через сравнение байтов.

**Целевая дельта.** 2–10% на CPU-bound фильтрации/группировке/проекции.

### ВАЖНЫЙ риск — `Ord` change

Текущий `Ord` на `Bytes` — это лексикографический byte-order varint-кодированного u64. Это **НЕ** то же самое что numeric Ord на u64. Проверить:

- `BTreeSet<InternerKey>` в posting lists — есть ли on-disk persisted?
- Sorted-index physical key — там используются raw u64 BE-bytes для name_interned, нашего InternerKey там НЕТ напрямую.
- Order of fields в `BTreeMap<InternerKey, Value>` для record serialization — если bincode сериализует BTreeMap в порядке итерации, смена Ord меняет байт-порядок serialized record.

**Перед началом работы** — grep `InternerKey.*Ord`, `BTreeMap<InternerKey`, `BTreeSet<InternerKey>` и составить список persisted структур.

### Шаги

1. **Bench (baseline)**
   - `complex_filter/10000`, `read_by_city_with_index/10000`, `bulk_insert/1000`.

2. **Test (RED)**
   - **Compat test:** записать N records под старым InternerKey(Bytes), переоткрыть с новым InternerKey(u64), все records читаемы.
   - Если шага migration не делаем — на текущем коде compat-тест зелёный (формат не меняется); нет настоящего RED.
   - Альтернатива: написать perf-assertion test — `complex_filter/10000` < X µs. Хрупко.

3. **Optimization**
   - `interned_key.rs`: `pub struct InternerKey(pub u64)`.
   - `Hash`/`Eq`/`Ord`/`Clone`/`Copy`/`Debug` derive automatically.
   - На границе serialization: `bincode` encoding должно остаться backward-compat. Либо сохранить varint encoding в codec, либо bump format-версию.

4. **Test (GREEN)**
   - Все 1240 тестов + новые compat тесты.

5. **Bench (after)**
   - Сравниваем с baseline.

---

## P1-5 — Diff-aware index update

**Проблема.** Каждый `on_record_updated` сейчас делает remove + add для всех индексов, даже если индексное поле не изменилось.

**Целевая дельта.** Зависит от workload. На update'ах где меняется НЕ индексное поле — экономим N×info_store I/O. До 5× на update-heavy workload.

### Шаги

1. **Bench (baseline)**
   - `update_by_id_with_index/10000` (~42 µs сейчас, но это touch только id field). Добавить `update_non_indexed_field_with_index/10000`.

2. **Test (RED)**
   - Тест: записать record, обновить только non-indexed поле, проверить что index entries НЕ менялись (через info_store stats или мок).
   - Сейчас тест RED потому что код всё равно делает remove + add.

3. **Optimization**
   - `IndexManager::on_record_updated`: для каждого индекса извлечь old_values, new_values, если равны — skip writes.
   - Уже есть код для этого в `table_manager.rs::set()` — переиспользовать.

4. **Test (GREEN)**

5. **Bench (after)**

---

## Что НЕ берём в эту итерацию

- **P1-1 covering indexes** — defer. После P0-1 ROI меньше, и нужен rebuild story.
- **P1-2 borrowing resolve_field** — defer. CPU constant-factor, мелкий win.
- **P1-4 ordered iter in CachedStore** — defer, риск регрессии writes.
- **P2 (всё)** — defer. По профилированию подберём что реально нужно.
- **Параллельные batch stages** через tokio::spawn — defer, без доказанного workload'а.
- **Persistable trait** (Opt H₂) — defer, architectural cleanup без перф-spike.

---

## Порядок исполнения

```
1. P0-1 Store::get_many         [3-4 дня] — высший ROI, низкий риск
2. P0-2 batch index writes      [2-3 дня] — ПОСЛЕ зафиксированной atomicity-spec
3. P0-2b bulk UPDATE/DELETE     [1 день]
4. P0-3 native reverse iter     [1-2 дня]
5. P0-4 flush() audit           [4 часа]
6. P1-3 InternerKey(u64)        [1 день] — после grep'а Ord usage
7. P1-5 diff-aware index update [4 часа]
```

Итого: 8–12 рабочих дней.

После всех шагов — re-bench всей суиты, обновление `PERF_BASELINE.md` с финальной таблицей "до проекта vs после" по всем backend'ам.
