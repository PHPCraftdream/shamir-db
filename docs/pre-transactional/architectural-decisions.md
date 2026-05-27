# Архитектурные решения — decision log

Пять решений, которые надо зафиксировать **до** старта Этапа 0.
Каждое сформулировано как **проблема → выбранное решение → почему
не иначе**. Если решение пересматривается — pull request меняет этот
документ + перечисляет последствия.

---

## D1. HNSW: stage-on-insert, apply-on-commit

**Проблема.** `hnsw_rs::Hnsw::insert` нативно необратим. На abort
транзакции вставленные точки остаются в графе. Soft-delete
tombstones отфильтровывают их при search, но граф постоянно растёт
+ recall деградирует (overscan ×2 не спасает при многих aborts).

**Выбранное решение.** В `HnswAdapter::upsert` точка попадает в
**staging** (`scc::HashMap<TxId, Vec<StagedVector>>`). Реальный
`hnsw.insert` происходит **только** на commit (`commit_staged(tx_id)`).
На abort (`rollback_staged(tx_id)`) staging entry дропается.

**Почему не tombstone-on-abort.** Текущая schema у нас именно
такая, и видна её цена: при `recall_at_10_on_1k_vectors` тесте мы
вынуждены поднимать `ef_search` до 400 чтобы компенсировать
потенциальное «грязное» состояние графа. На production workload с
realistic abort rate (5-10%) граф будет ухудшаться монотонно — не
sustainable.

**Цена решения.** Search внутри tx должен искать **и** в committed
graph, **и** в staged vectors (brute-force scan). Если staging
большой (тысячи точек) — search замедлится. Mitigation: max staged
size cap (например, 10k) — beyond этого tx force-aborted с
`tx_too_large`.

**Влияние на trait.** `VectorAdapter::upsert / delete / search`
принимают `tx: Option<TxId>`. Non-tx путь (`None`) — как сейчас.

---

## D2. Index writes: planner-возвращает-ops, executor-применяет

**Проблема.** Все три index2 backend и старый IndexManager пишут в
storage **внутри** `on_insert/update/delete` хуков. Откатить нечего
(нет журнала plan'а). Atomicity multi-record batch'а невозможна.

**Выбранное решение.** Trait меняется: `on_insert → plan_insert`
возвращает `Vec<IndexWriteOp>`. Reserved-ops применяет executor —
немедленно для non-tx, в commit transact()-е для tx.

**Почему не «add tx-aware variant внутри backends».** Дублирование
кода (две версии каждого хука), risk of drift. Лучше один путь
build-ops, две точки apply.

**Цена решения.** Один extra heap allocation `Vec<IndexWriteOp>` на
каждый mutation. `SmallVec<[IndexWriteOp; 8]>` покрывает 99% случаев
без heap → measurable overhead < 1%. Bench order_by_pipeline и
engine_perf верифицируют.

**Влияние на trait.** `IndexBackend::on_insert` → `plan_insert` (и
аналогично update/delete). `apply_index_ops(ops, store)` — top-level
function, не trait method.

---

## D3. MemBuffer и tx staging — разные слои

**Проблема.** MemBuffer — write-back cache (perf). Tx staging — buffer
для atomicity. Соблазнительно объединить (один буфер с tx_id-тегами).
Это сделает обе системы сложнее и взаимозависимыми.

**Выбранное решение.** Оставляем **разными** слоями:
- MemBuffer над raw storage — оптимизирует non-tx writes.
- StagingStore над MvccStore (поверх MemBuffer) — обслуживает tx.
- На commit: tx writes **минуют** MemBuffer (bypass, см. этап 5.1).

**Почему не объединить.** Cross-cutting concerns:
- MemBuffer flushes по таймеру / по размеру, безотносительно tx.
- Tx commit — explicit durability point, нельзя задержать.
- MemBuffer не знает про versioning, MvccStore не знает про
  write-back.

Объединение требует unified buffer manager — большой проект сам по
себе, без явной выгоды.

**Цена решения.** Tx commits пишут напрямую в backend, теряя
write-back. На бэкендах где fsync дорогой это medium hit. Acceptable:
commit IS the durability point. Если в production это окажется
проблемой — отдельный optimization (например, commit batching на
уровне executor).

---

## D4. Repo-scoped WAL marker, не table-scoped

**Проблема.** Сейчас `WalManager` per-table. Batch с queries на 2+
таблиц одного repo сгенерирует 2 независимых WAL marker'а — нет
**одной** точки atomic publish.

**Выбранное решение.** Новый `RepoWalManager` рядом с per-table.
Хранит WAL entries в info_store самого repo (под
`SysKey::RepoWalEntry(txn_id)`). Tx commit пишет **один** entry,
включающий ops по всем таблицам. Per-table WAL остаётся для
non-tx ops (back-compat).

**Почему не убрать per-table WAL.** Existing recovery flow зависит
от него. Trying to migrate everything to repo-scope = большой scope
creep. Сосуществование стабильнее.

**Recovery на open repo.**
1. Per-table WAL → forward-fix как сейчас (V1 entries).
2. Repo WAL → forward-fix V2 entries (apply ops через MvccStore).
3. Version cache rebuild.

**Цена решения.** Recovery scan по двум WAL вместо одного. На clean
shutdown — это пустые scans, дёшево. На dirty — оба прогоняются по
очереди, не параллельно (избегаем гонок).

---

## D5. `Option<&TxContext>` параметр, не generic dispatch

**Проблема.** Каждая mutation/read функция должна знать про tx
context. Два варианта:
- A. `Option<&TxContext>` параметр.
- B. Generic dispatch: `fn execute<TX: TxLayer>(...)` где `NoTx`
     и `WithTx` — два типа.

**Выбранное решение.** A — `Option<&TxContext>`. None ветка идёт
через if-let и предсказывается branch predictor'ом.

**Почему не generic.**
- Compile-time binary bloat: каждая instantiation создаёт две версии
  функции. На размерах нашего read pipeline (десятки функций) —
  существенный рост binary size.
- Test coverage: каждый branch теста надо запускать 2× (NoTx +
  WithTx) для полного покрытия.
- Type system pressure: generic параметр пробрасывается через **все**
  signatures. `IndexBackend`, `TableManager`, executor — везде
  `<TX: TxLayer>`. Trait object'ы становятся невозможными
  (`Arc<dyn IndexBackend>` не может быть generic).
- Read pipeline уже использует `&dyn` через всё — runtime dispatch
  не новая стоимость.

**Цена решения.** Один branch на каждый call. На non-tx path —
если ветка не taken (None), branch predictor handles it cheap.
Measure через `engine_perf.rs` — ожидаем < 1% regression.

---

## Открытые вопросы (требуют отдельного обсуждения до Этапа 0)

### Q1. Default isolation: Snapshot или Serializable?

Текущий план: **Snapshot** (last-writer-wins, lost-update possible).
Argued: SI быстрее, не требует read-set tracking; SSI за флагом.

**Альтернатива:** Default SSI (always validate read-set). Безопаснее
по умолчанию, но overhead per-read.

**Решение to defer:** запускаем Phase A с SI default. Если в реальном
production lost update приведёт к bug'у — даём guidance в docs
включить SSI для критичных tx (счётчики, балансы).

### Q2. Wire-flag `transactional: true` без `isolation` field

Старые clients шлют `transactional: true` без `isolation`. Что им
дать? **Решение:** Default = Snapshot (matches SI default).

### Q3. Phase B interactive transactions

Out of scope этого подготовительного фронта. Появится после
production stabilization Phase A.

### Q4. 2PC for cross-repo

Out of scope **forever** (нет use case). Cross-repo batches с
`transactional: true` возвращают `tx_cross_repo_not_supported`.

### Q5. WAL inline body size cap

V2 WAL entries содержат inline bytes. Большой batch → большой entry.
Cap: запретить tx с total writes > 16MB? Force smaller batches?

**Решение to defer:** добавить config `max_tx_size_bytes` (default
64MB). Превышение → tx aborts с `tx_too_large`.

---

## Стиль внесения изменений

Любое решение пересматривается через PR с:

1. Описание новой проблемы (что обнаружили).
2. Сравнение со старым решением.
3. Перечень последствий (что меняется в коде).
4. Update архитектурного решения в этом документе.

Никаких silent reversions.
