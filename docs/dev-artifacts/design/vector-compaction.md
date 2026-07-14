# Vector HNSW Compaction — Design Document (#408, V4.2)

## Контекст

`HnswAdapter` использует soft-delete: при удалении/замене вектора старый internal id добавляется в `deleted` (scc::HashMap), а граф HNSW продолжает хранить мёртвый узел. Со временем доля tombstone'ов растёт, ухудшая recall и раздувая memory footprint.

**Цель:** фоновая компакция — построить СВЕЖИЙ `HnswAdapter` БЕЗ tombstone'ов (rebuild-aside), атомарно подменить через `ArcSwap<AdapterSlot>` (RCU), форс-снапшот.

---

## 1. Анализ мутационных путей

### Не-транзакционный путь (hot-path, NO lock, NO delta-log)
- `plan_insert` / `plan_update` / `plan_delete` — вызывают `adapter.load_full().adapter.upsert/delete` НАПРЯМУЮ.
- Вызывается из `table_manager_crud.rs` (прямой CRUD) и `table_manager_replication.rs` (репликация).
- **НЕ пишет в delta-log.** `append_vector_delta` НЕ вызывается.
- **НЕ берёт commit-lock.**

### Транзакционный путь (commit-lock, delta-log)
- `apply_staged_vectors` / `apply_committed_vectors` (Phase 5d) — вызывается ПОД commit-lock.
- `append_vector_delta` вызывается ТОЛЬКО здесь (Phase 5d, `commit_phases.rs:458`).

### Критический вывод
Delta-log содержит ТОЛЬКО tx-мутации. Non-tx мутации (прямой CRUD, репликация) записываются ТОЛЬКО в live-адаптер, минуя delta-log. Любой механизм, опирающийся исключительно на delta-replay, ТЕРЯЕТ non-tx мутации при swap — недопустимо.

---

## 2. Выбор механизма: Double-Write + Backfill-If-Absent

### Отвергнутые альтернативы

**(A) Delta-replay HWM** — ОТВЕРГНУТ:
- Delta-log записывает ТОЛЬКО tx-мутации (Phase 5d).
- Non-tx путь (`plan_insert/update/delete` из CRUD/репликации) в delta-log НЕ пишет.
- После swap: все non-tx мутации, произошедшие во время rebuild, ПЕРМАНЕНТНО потеряны из нового адаптера.
- Потерянный DELETE = ghost-вектор в поиске = ре-интродукция gap#1/#416 (HIGH-6). Недопустимо.

**(C) Brief-exclusion через commit-lock** — ОТВЕРГНУТ:
- Non-tx путь НЕ берёт commit-lock. Эксклюзив через commit-lock оставляет non-tx мутации неприкрытыми. Та же проблема.

**(B) Mutation-tee (bounded buffer)** — ОТВЕРГНУТ в пользу double-write:
- Bounded buffer требует backpressure → latency spike на hot-path.
- Требует drain-loop / actor — лишняя сложность.
- Double-write (прямая запись в target) проще и не имеет backpressure.

### Выбранный механизм: Double-Write + Backfill-If-Absent

**Суть:** пока идёт компакция, каждая мутация hot-path'а дублируется в строящийся (new) адаптер. После сбора live-set из старого, backfill вносит в new только те rid'ы, которых там ещё нет (мутации из double-write всегда новее). Затем — атомарный swap.

**Обоснование:**
- Покрывает ОБА пути (tx и non-tx) — double-write инструментирует все 4 мутационных сайта.
- Lock-free на hot-path: одна проверка `ArcSwapOption::load()` (Relaxed nullptr check ~0 ns когда компакция не идёт).
- Не требует delta-log для корректности.
- Минусы: инвазивность (4 сайта), 2x memory peak. Приемлемо: сайты тривиальны (условный second op), memory peak — временный (старый Arc drop'ается после swap).

---

## 3. Протокол по шагам

### Новое поле в VectorBackend
```rust
/// Target adapter for double-write during compaction.
/// None = no compaction in progress; Some = duplicate every mutation here.
/// Lock-free: readers do ArcSwapOption::load() (one atomic, ~0 ns for None).
compaction_target: Arc<ArcSwapOption<AdapterSlot>>,
```

### Background task (tokio::spawn)

```
STEP 1: Создать пустой new_adapter
    new_adapter = HnswAdapter::new(dim, metric, config)  // config из old

STEP 2: Взвести double-write
    compaction_target.store(Some(Arc::new(AdapterSlot { adapter: Arc::new(new_adapter_clone) })))
    // С этого момента КАЖДАЯ мутация (tx и non-tx) дублируется в new.

STEP 3: Собрать live-set S0 из СТАРОГО адаптера
    old_adapter = arc_swap.load_full().adapter.as_hnsw_adapter()
    live_pairs = old_adapter.collect_live_vectors()
    // S0 — снимок текущего состояния old. Мутации, пришедшие ПОСЛЕ
    // Step 2 и ДО конца scan, уже в new (double-write). Мутации,
    // пришедшие ДО Step 2, в old И в S0.

STEP 4a: Backfill S0 в new с семантикой INSERT-IF-ABSENT
    new_adapter.backfill_if_absent(&live_pairs)
    // Для каждого (rid, vec) в S0:
    //   - если rid УЖЕ в new (из double-write) → SKIP (свежее значение сохраняется)
    //   - если rid в compaction_deleted_rids (delete через double-write) → SKIP
    //   - иначе → insert (этот rid не менялся после взвода, S0-значение актуально)

STEP 4b: Reconcile-deletes (ЗАКРЫВАЕТ backfill↔delete гонку)
    for rid in compaction_deleted_rids.snapshot():
        new_adapter.delete(rid)   // идемпотентно; убирает любой resurrect
    // ОБЯЗАТЕЛЬНО. Проверка compaction_deleted_rids + insert в Step 4a — ДВЕ
    // отдельные операции, не атомарные вместе. Конкурентный double-write
    // delete для rid R может: (1) добавить R в compaction_deleted_rids ПОСЛЕ
    // того как backfill прочитал его как absent, но (2) сам delete на new —
    // no-op (R ещё не вставлен), а затем backfill вставляет R из S0 →
    // ПЕРМАНЕНТНЫЙ resurrect (ghost). Reconcile-pass переигрывает ВСЕ
    // накопленные к этому моменту compaction_deleted_rids как delete на new,
    // затирая любой такой resurrect. Delete'ы, пришедшие ПОСЛЕ reconcile и до
    // Step 6, покрыты живым double-write (target ещё взведён). Delete
    // идемпотентен (rid отсутствует → no-op), так что двойное применение
    // безопасно.

STEP 5: Атомарный swap
    arc_swap.store(Arc::new(AdapterSlot { adapter: new_adapter_arc }))
    // С этого момента load_full() возвращает new.
    // Мутации ПОСЛЕ swap: load_full()=new → пишут в new (как primary).
    // Double-write ещё взведён → дублируют на new (тот же адаптер) → идемпотентно.

STEP 6: Снять double-write
    compaction_target.store(None)
    // Порядок Step5→Step6 критичен: см. доказательство ниже.

STEP 7: Форс снапшота
    run_background_snapshot от НОВОГО адаптера.
    delta_count.store(0)  // новая durable-база
```

### Hot-path double-write (изменения в 4 сайтах)

```rust
// После основного upsert/delete на load_full().adapter:
if let Some(target) = self.compaction_target.load().as_ref() {
    let _ = target.adapter.upsert(rid, &vec).await;  // или .delete(rid)
}
```

**Сайты инструментирования:**
1. `plan_insert` (vector_backend.rs:248) — после `adapter.upsert(rid, &v)`: дублировать `target.adapter.upsert(rid, &v)`
2. `plan_update` (vector_backend.rs:264) — после upsert или delete на old: дублировать тот же op на target
3. `plan_delete` (vector_backend.rs:289) — после `adapter.delete(rid)`: дублировать `target.adapter.delete(rid)`
4. `apply_staged_vectors` (vector_backend.rs:399) — после `adapter.apply_committed_vectors(vecs)`: дублировать `target.adapter.apply_committed_vectors(vecs)`

**Latency на hot-path когда компакция НЕ идёт:**
Один `ArcSwapOption::load()` — single atomic load, возвращает None → ветка не берётся. ~0 ns overhead.

**Latency на hot-path когда компакция идёт:**
Один дополнительный upsert/delete на new_adapter. Это O(1) per-rid (scc entry_async + graph insert via spawn_blocking). Допустимо: компакция — редкое событие, и операция та же что и основная.

**Guard через await:** `ArcSwapOption::load()` возвращает `arc_swap::Guard` — маленький stack-local тип. Мы извлекаем `Arc` через `.as_ref()` / clone и DROP guard до await. Корректно — guard НЕ удерживается через await.

---

## 4. Доказательство корректности

### Инвариант: мутации после взвода (Step 2) ВСЕГДА новее любого значения в S0

**Обоснование:** S0 собирается в Step 3, ПОСЛЕ Step 2. Любая мутация M, попавшая в double-write (Step 2+), произошла ПОСЛЕ (или одновременно с) чтением S0 для данного rid. Следовательно, значение M для rid новее или равно значению в S0.

### Теорема: ни одна мутация не потеряна

**Случай A: rid мутирован ДО Step 2 и НЕ мутирован после.**
Его значение в old стабильно. `collect_live_vectors` (Step 3) увидит его. `backfill_if_absent` (Step 4) вставит в new (rid отсутствует — double-write его не трогал). Корректно.

**Случай B: rid мутирован ПОСЛЕ Step 2 (double-write активен).**
Мутация применена к old (load_full) И к new (double-write). В new — свежее значение. Если rid также есть в S0 — `backfill_if_absent` видит rid уже в new → SKIP. Свежее значение сохраняется. Корректно.

**Случай C: rid удалён ПОСЛЕ Step 2.**
Delete применён к old И к new (double-write). В new rid в `deleted`. Если rid есть в S0 — `backfill_if_absent` видит rid в deleted → SKIP (не воскрешает). Нет ghost. Корректно.

**Случай D: rid мутирован ВО ВРЕМЯ scan S0 (Step 3).**
- D1: мутация успела ДО чтения rid в scan → S0 содержит новое значение. Double-write тоже записал в new. Backfill видит rid в new → SKIP. OK (new имеет свежее).
- D2: мутация произошла ПОСЛЕ чтения rid в scan → S0 содержит старое значение. Double-write записал свежее в new. Backfill видит rid в new → SKIP. OK (new имеет свежее).
- D3: мутация = delete, произошла ПОСЛЕ чтения → S0 содержит rid (live). Double-write записал delete в new. Backfill видит rid в deleted → SKIP. OK (нет ghost).

### Теорема: нет ghost-записей

Ghost = удалённый rid возвращается поиском. Единственный способ ghost: backfill воскрешает удалённый rid. Но `backfill_if_absent` проверяет `deleted` набор в new — если rid там (из double-write delete), backfill НЕ вставляет. Ghost невозможен.

### Теорема: backfill НЕ может откатить более новую мутацию старым значением

Доказательство: backfill работает с семантикой INSERT-IF-ABSENT. Для каждого rid:
- Если rid уже в `rid_to_internal` (из double-write upsert) → SKIP. Новое значение не перезаписывается.
- Если rid в `deleted` (из double-write delete) → SKIP. Удаление не отменяется.
- Иначе (rid absent) → вставка S0-значения. Это корректно: отсутствие rid в new означает, что ни один double-write не трогал этот rid, значит его значение в old стабильно с момента до Step 2, и S0 отражает текущее.

### Теорема: нет дубликатов

`upsert` — last-write-wins по rid (D12 invariant через `entry_async`). Повторный upsert того же rid tombstone'ит предыдущий internal. Поиск фильтрует deleted. Один rid — ровно один live-результат.

### Закрытие хвостовой гонки (Step 5 → Step 6)

Окно: после swap (Step 5), до очистки double-write (Step 6).
- `load_full()` возвращает new (swap уже сделан).
- Мутация M в этом окне: применяется к new (load_full()=new). Double-write ещё взведён → дублируется на new (compaction_target=new). Результат: `upsert(rid, vec)` вызван на new ДВАЖДЫ. По D12 (entry_async last-write-wins): второй upsert tombstone'ит internal первого и вставляет новый с тем же vec. Функционально идемпотентно (один live-результат с тем же вектором). Overhead: один лишний graph insert per op в этом окне. Допустимо (окно — наносекунды, единичные операции).

После Step 6: compaction_target=None → double-write не срабатывает. Мутации идут только в new через load_full(). Стационарный режим.

---

## 5. Триггер компакции

### Условия
```rust
fn should_compact(adapter: &HnswAdapter) -> bool {
    let ratio = adapter.deleted_ratio();
    let live = adapter.live_count();
    ratio >= VECTOR_COMPACTION_RATIO_THRESHOLD  // tunable, default 0.3
        && live >= VECTOR_COMPACTION_MIN_LIVE    // tunable, default 1000
}
```

### Где вызывается
На ack-пути после `append_vector_delta` (аналогично `trigger_snapshot_check`). Также после non-tx `plan_delete` (единственный non-tx путь, увеличивающий tombstone ratio).

### Single-flight
```rust
compaction_in_flight: Arc<AtomicBool>  // аналог snapshot_in_flight
```
`compare_exchange(false, true)` перед spawn; `CompactionFlightGuard` (drop-guard) сбрасывает на drop.

### Координация с фоновым снапшотом

Компакция и snapshot НЕ ДОЛЖНЫ работать одновременно (обе работают с ArcSwap):
```rust
// В trigger_compaction_check:
if self.snapshot_in_flight.load(Acquire) { return; }  // snapshot работает — skip
if self.compaction_in_flight.compare_exchange(...).is_err() { return; }

// В trigger_snapshot_check (добавить):
if self.compaction_in_flight.load(Acquire) { return; }  // компакция — skip
```

Обе проверки — Acquire load, ~0 ns. Пропущенный trigger перепроверяется на следующем ack.

---

## 6. Форс снапшота после swap

После swap нового адаптера:
1. Сбросить `delta_count` в 0 (новый адаптер = новая база, старые delta невалидны для него).
2. Запустить `run_background_snapshot` от НОВОГО адаптера (gen+1).
3. `flip_generation` пишет новый manifest с `delta_applied_upto = next_delta_idx`.
4. Prune: старые chunks + delta chunks < delta_applied_upto.

**Delta-log после swap:** Phase 5d продолжает append_vector_delta для tx-мутаций. Новый снапшот (от нового адаптера) абсорбирует все delta до текущего HWM. Non-tx мутации по-прежнему не в delta — но это OK: они IN-MEMORY в новом адаптере, а при crash → restart rebuild from data-store (полный scan, как fallback).

**Crash-safety:** если crash между swap и форс-снапшотом: restart загружает СТАРЫЙ снапшот + replay delta. Non-tx мутации, попавшие в new через double-write, потеряны из snapshot (были only in-memory). Но restart делает полный rebuild from data-store (план B) если snapshot + delta не покрывают текущее состояние. Это существующее поведение — компакция НЕ ухудшает crash-safety.

---

## 7. Deletes и #416 (HIGH-6)

**Текущее состояние #416:** tx-committed vector deletes НЕ доходят до графа (known gap). `apply_committed_vectors` / `apply_staged_vectors` стейджит только upsert'ы. Delete проходит через `plan_delete_tx` → при `tx_id.is_some()` = no-op (не трогает адаптер).

**Влияние double-write на #416:** double-write покрывает те delete-пути, которые РЕАЛЬНО доходят до адаптера:
- `plan_delete` (non-tx, `tx_id == None`) → вызывает `adapter.delete(rid)` → double-write дублирует delete на new. Покрыто.
- `plan_delete_tx` с `tx_id == Some` → no-op на адаптере → double-write не нужен (нечего дублировать). #416 не ухудшен.

Компакция НЕ пытается чинить #416. Она корректно зеркалит текущее поведение: если delete не доходил до старого адаптера, он не дойдёт и до нового.

---

## 8. Аксессоры и поля для добавления

### VectorBackend — новые поля
```rust
/// Single-flight guard for background compaction.
compaction_in_flight: Arc<AtomicBool>,

/// Double-write target during compaction. None = idle.
/// Lock-free: ArcSwapOption load is one atomic (Relaxed).
compaction_target: Arc<ArcSwapOption<AdapterSlot>>,
```

### VectorBackend — новые методы
```rust
/// Check if compaction should trigger; spawn background task if so.
fn trigger_compaction_check(&self, info_store: &Arc<dyn Store>);
```

### HnswAdapter — новые методы
```rust
/// Collect all live (non-tombstoned) (rid, vector) pairs for rebuild-aside.
/// O(N) scan — called once per compaction, NOT on hot path.
pub(crate) fn collect_live_vectors(&self) -> Vec<(RecordId, Vec<f32>)>;

/// Insert (rid, vec) ONLY if rid is absent from rid_to_internal AND absent
/// from deleted. Used by compaction backfill: double-write values are always
/// newer than S0, so existing entries must not be overwritten.
/// Uses entry_async per rid for atomic check-and-insert (no race with
/// concurrent double-write on the same rid).
pub(crate) async fn backfill_if_absent(&self, items: &[(RecordId, Vec<f32>)]) -> Result<(), VectorError>;

/// Clone the build config so the compaction task can construct a fresh
/// HnswAdapter with identical parameters.
pub(crate) fn build_config(&self) -> HnswConfig;
```

### Tunables (shamir-tunables)
```rust
/// Tombstone ratio threshold above which compaction triggers.
pub const VECTOR_COMPACTION_RATIO_THRESHOLD: f64 = 0.3;

/// Minimum live vector count to trigger compaction (skip tiny indexes).
pub const VECTOR_COMPACTION_MIN_LIVE: usize = 1000;
```

### Drop-guard
```rust
struct CompactionFlightGuard(Arc<AtomicBool>);
impl Drop for CompactionFlightGuard { fn drop(&mut self) { self.0.store(false, Release); } }
```

---

## 9. Точки под тесты

### Unit-тесты
1. `collect_live_vectors` возвращает ТОЛЬКО не-tombstone'нные пары.
2. `backfill_if_absent` — rid уже в new → skip; rid в deleted → skip; rid absent → insert.
3. `deleted_ratio` / `live_count` корректны после серии upsert/delete.
4. `should_compact` логика порогов.

### Integration-тесты
5. Compaction rebuild-aside: после компакции `deleted_count == 0` (для backfill'ных), `live_count` == ожидаемый.
6. Double-write: мутации во время rebuild видны в new_adapter.
7. Single-flight: второй trigger при compacting==true — no-op.
8. Координация: snapshot trigger при compacting==true — skip (и наоборот).
9. Backfill ordering: upsert в double-write → backfill с другим vec для того же rid → new содержит double-write значение.
10. Delete в double-write → backfill с rid → new НЕ содержит rid (не воскрешает).

### Stress-тест (#408 critical)
11. **Concurrent stress:** N потоков выполняют upsert/delete (mix tx и non-tx CRUD) В ПАРАЛЛЕЛЬ с компакцией.
    - После завершения (swap + clear): `live_set(new_adapter) == expected_live_set` (ни потерь, ни ghost).
    - Все rid'ы в `rid_to_internal(new)` — живые (не в deleted).
    - Все rid'ы в `deleted(new)` — НЕ в `rid_to_internal`.
12. **Search consistency during compaction:** поиск во время компакции никогда не паникует; результаты содержат только валидные rid'ы (существующие в текущем load_full() адаптере).
13. **Tail-race (Step 5→6):** мутации в окне swap→clear дублируются на тот же адаптер — assert: один live internal per rid (no leak of orphan internals).

---

## 10. Риски и острые углы для impl-агента

1. **Memory pressure.** Два полных адаптера в памяти одновременно (old + new). Для индексов > RAM/2 нужен streaming. В V4.2 документируем лимит: индекс должен помещаться в RAM/2.

2. **`collect_live_vectors` consistency.** Между scan rid_to_internal и проверкой deleted может прийти мутация. Безопасно: double-write гарантирует что любая мутация после Step 2 УЖЕ в new. Backfill не перезапишет.

3. **`backfill_if_absent` и D12.** Race возможен если double-write upsert для rid прибывает ОДНОВРЕМЕННО с backfill check. Решение: `backfill_if_absent` использует `rid_to_internal.entry_async(rid)` — Vacant → insert; Occupied → skip. Атомарно per-rid. Также проверить `deleted.contains_async(&rid_internal)` ПЕРЕД entry — но since it's a new rid (not yet in rid_to_internal), deleted check is on the rid itself, not internal. Impl note: check `rid_to_internal.contains(rid)` || check deleted-by-rid (нужен reverse lookup или set<RecordId> в deleted).

4. **Deleted set keyed by internal, not rid.** `deleted: scc::HashMap<usize, ()>` — keyed by INTERNAL id. Backfill для rid, удалённого через double-write: delete вызывает `rid_to_internal.remove_async(&rid)` + `deleted.insert(internal)`. После remove, rid ОТСУТСТВУЕТ в `rid_to_internal`. Значит `entry_async(rid)` = Vacant. НО insert тут некорректен (rid был удалён!). Решение: backfill должен вести ДОПОЛНИТЕЛЬНЫЙ `deleted_rids: scc::HashMap<RecordId, ()>` в new_adapter, populated при каждом double-write delete. Backfill проверяет: `if deleted_rids.contains(rid) → SKIP`. Альтернатива: НЕ делать `rid_to_internal.remove` в delete (оставить rid в rid_to_internal, пометить internal в deleted). Тогда `entry_async(rid)` = Occupied → backfill SKIP. Это текущее поведение? ПРОВЕРИТЬ: в `HnswAdapter::delete` строка 603: `rid_to_internal.remove_async(&rid)` — ДА, rid УДАЛЯЕТСЯ из rid_to_internal. Значит нужен `deleted_rids` set или изменение семантики delete в new_adapter. **Рекомендация impl-агенту:** добавить `compaction_deleted_rids: Option<Arc<scc::HashMap<RecordId, (), THasher>>>` в HnswAdapter (Some только для compaction-target адаптера). Double-write delete добавляет rid туда. `backfill_if_absent` проверяет этот set.

5. **hnsw_rs graph capacity.** Новый адаптер создаётся с `max_elements = live_count_old + buffer` (buffer = 10% или tunable). `hnsw_rs` 0.3.4 supports dynamic growth, но лучше pre-size.

6. **Double-write await.** `target.adapter.upsert(rid, &v).await` — это await НА hot-path. Допустимо: тот же cost что и primary upsert (spawn_blocking для graph insert). НЕ блокирует async executor. Guard от `ArcSwapOption::load()` дропнут ДО await.

7. **Компакция и delta-log.** После swap, Phase 5d продолжает `append_vector_delta`. Эти delta относятся к НОВОМУ адаптеру. При crash+restart: load snapshot (нового адаптера, если форс-снапшот успел) + replay delta. Если форс-снапшот НЕ успел — restart загружает старый snapshot + replay delta (от старого адаптера) + full rebuild. Всё корректно.

8. **#416 gap.** Tx-committed deletes не доходят до адаптера — это known issue. Компакция НЕ чинит и НЕ ухудшает: если ghost-вектор от #416 в old (потому что delete не прошёл), он попадёт в S0 и будет backfill'нут в new. Ghost остаётся — ровно как до компакции. Починка #416 — отдельная задача.

9. **Порядок Step 5 → Step 6.** НЕЛЬЗЯ менять: если clear double-write (Step 6) ДО swap (Step 5), то мутации между clear и swap идут ТОЛЬКО в old (load_full ещё старый) и НЕ в new. После swap эти мутации потеряны. Текущий порядок (swap first) гарантирует: после swap load_full()=new, мутации идут в new напрямую; double-write дублирует туда же (идемпотентно).

10. **`backfill_if_absent` batch insert и parallel_insert.** Для производительности: сначала filter (skip existing rids), затем один `upsert_batch` на отфильтрованный набор. Но upsert_batch использует `entry_async` per-rid (D12) — если double-write concurrent, entry race разрешается атомарно. Можно вызывать `upsert_batch` с filtered items — D12 гарантирует корректность.
