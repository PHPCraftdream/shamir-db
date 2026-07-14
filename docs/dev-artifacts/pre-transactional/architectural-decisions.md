# Архитектурные решения — decision log

Пять решений, которые надо зафиксировать **до** старта Этапа 0.
Каждое сформулировано как **проблема → выбранное решение → почему
не иначе → как проверяется** (тесты + бенчмарки). Если решение
пересматривается — pull request меняет этот документ + перечисляет
последствия.

## Test + bench coverage matrix

| # | Decision | Correctness tests | Performance benchmarks |
|---|---|---|---|
| D1 | HNSW staging | `tx_upsert_then_search_sees_staged`, `tx_rollback_does_not_pollute_graph`, `rollback_staged_drops_without_graph_mutation`, `commit_staged_inserts_all_into_graph`, recall@10 stability after 1000 aborts | `hnsw_search_with_staged_size_{100,1k,10k}` (overhead vs no-staging), `hnsw_commit_staged_batch` (commit throughput) |
| D2 | IndexWriteOp planner | `plan_insert_writes_expected_postings` (Functional), `plan_insert_writes_postings_per_token` (FTS), `plan_insert_emits_bump_stats` (FtsRanked), `plan_apply_round_trip` per backend | `index_plan_vs_direct_overhead` на 100k inserts (target < 1% regression), `index_apply_batch_throughput` |
| D3 | MemBuffer / tx staging separation | `non_tx_writes_go_through_membuffer`, `tx_commit_bypasses_membuffer`, `tx_writes_invisible_to_concurrent_non_tx_read` | `non_tx_write_throughput_unchanged`, `tx_commit_throughput_with_membuffer_bypass` |
| D4 | Repo-scoped WAL marker | `cross_table_tx_atomic`, `recovery_after_crash_mid_phase_{4,5,6,7}`, `mixed_v1_v2_recovery` | `repo_wal_append_latency`, `recovery_scan_with_n_inflight_tx` |
| D5 | `Option<&TxContext>` not generic | `non_tx_call_paths_zero_overhead` (branch coverage), `tx_call_paths_through_pipeline` | `engine_perf_non_tx_regression` (target < 2%), `read_pipeline_with_tx_context` (compare None / Some) |
| D6 | `version_codec` separator = `0xFF` | `round_trip`, `sort_order_matches_version`, `different_keys_dont_interleave`, `missing_separator_decodes_to_none` | (тривиальная функция, не требует bench) |

**Правило:** ни одно из D1-D6 не считается реализованным, пока **обе
колонки** имеют commit'нутые tests + бенчмарки с измеренными числами
в комментарии или task summary.

Benchmark suite landed in `crates/shamir-tx/benches/tx_overhead.rs` (4 baseline
measures — zero-overhead write, fast-path read, staging buffer, version counter).
Stage 4+ benchmarks land alongside their implementation.

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

**Test + bench coverage.**

Correctness tests (`crates/shamir-engine/src/index2/vector/hnsw_adapter.rs::tests`):

- `tx_upsert_then_search_sees_staged` — upsert в tx, search в той же
  tx → видит staged vector с правильным distance.
- `tx_rollback_does_not_pollute_graph` — 1000 staged inserts,
  `rollback_staged`, затем 1000 committed inserts; assert recall@10 ≥
  0.95 (граф чист).
- `tx_commit_staged_makes_committed` — staged → commit → outside-tx
  search видит как обычные точки.
- `concurrent_tx_isolation` — две параллельные tx с overlapping
  upserts, каждая видит только свои staged + committed (не чужие
  staged).
- `staged_size_cap_enforced` — попытка staged > 10k → `tx_too_large`.

Benchmarks (`crates/shamir-engine/benches/hnsw_staging.rs`):

- `hnsw_search_with_staged_{0,100,1k,10k}` — overhead search при
  разных размерах staged buffer. Acceptance: 100 staged < 2× от 0.
- `hnsw_commit_staged_batch_{100,1k}` — commit_staged throughput.
- `hnsw_non_tx_upsert_throughput` — non-tx путь не регрессирует > 2%
  относительно baseline без staging field.

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

**Test + bench coverage.**

Correctness tests (per-backend `tests` submodule):

- `plan_then_apply_equals_direct_write` — для каждого из FTS,
  FtsRanked, Functional, SortedIndex: вызвать `plan_insert(rid, val)`
  + `apply_index_ops(ops, &store)` и сравнить state store с прямой
  записью через старый `on_insert`. Bit-for-bit equivalence.
- `apply_index_ops_atomic_batch` — `apply_index_ops` через
  `Store::transact(ops)` → concurrent observer видит либо все ops,
  либо ни одного.
- `plan_update_diff_minimization` — `plan_update(rid, old, new)`
  возвращает только diff (RemovePosting для исчезнувших токенов,
  SetPosting для появившихся), не полный re-build.
- `plan_delete_complete_cleanup` — после `apply_index_ops(plan_delete)`
  никаких остаточных postings для этого rid в store.

Benchmarks (`crates/shamir-engine/benches/index_plan_apply.rs`):

- `fts_insert_plan_vs_direct_100k` — overhead plan+apply относительно
  direct write. Acceptance: < 1% (SmallVec[IndexWriteOp; 8] покрывает
  99% случаев без heap).
- `fts_batch_apply_throughput` — `apply_index_ops` на batch из 1k ops
  через `transact`.
- `engine_perf` regression check — read + write pipeline на 100k
  records не теряет throughput.

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

**Test + bench coverage.**

Correctness tests:

- `non_tx_writes_go_through_membuffer` — non-tx `set(k, v)` оседает
  в MemBuffer's `dirty`, не виден в base backend до flush.
- `tx_commit_bypasses_membuffer` — tx commit пишет напрямую в base
  backend, обходя `dirty`. После commit `dirty` не содержит tx
  writes; они виден в base сразу.
- `tx_writes_invisible_to_concurrent_non_tx_read` — non-tx read во
  время open tx читает pre-tx state (через base) — не видит tx
  staging.
- `mixed_tx_and_non_tx_traffic` — параллельные tx commits + non-tx
  writes в одной таблице → state корректен, MemBuffer не «теряет»
  non-tx writes которые конкурировали с tx commit.

Benchmarks (`crates/shamir-engine/benches/membuffer_tx_separation.rs`):

- `non_tx_write_throughput_baseline_vs_after_separation` — non-tx
  путь не регрессирует > 1% (тот же код, MemBuffer не trogut).
- `tx_commit_throughput_with_membuffer_bypass_{redb,sled,fjall}` —
  measure cost commit с bypass per backend. Если на каком-то
  backend > 2× медленнее non-tx → flag для отдельной оптимизации.
- `tx_commit_amortized_latency` — для batch tx с N writes
  среднее latency на write при commit.

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

**Test + bench coverage.**

Correctness tests (`crates/shamir-engine/tests/transactions/repo_wal.rs`):

- `cross_table_tx_atomic` — tx batch с writes в две таблицы одного
  repo. На outside read обе таблицы либо изменены, либо нет (одна
  точка atomic publish через `publish_committed`).
- `recovery_after_crash_mid_phase_4` — симуляция crash после
  `begin(WalEntryV2)`, до physical writes. Recovery на open применяет
  ops, atomicity сохранена.
- `recovery_after_crash_mid_phase_5` — crash в середине physical
  writes. Recovery re-applies полный entry — idempotent.
- `recovery_after_crash_after_commit` — crash после
  `publish_committed` но до `wal.commit(txn_id)`. Tx видна, recovery
  cleanup'ит residual entry.
- `mixed_v1_v2_recovery` — recovery с per-table V1 entries И
  repo-level V2 entries одновременно. Оба применяются корректно.
- `inflight_listing` — `list_inflight` после нормального flow
  возвращает пусто.

Benchmarks (`crates/shamir-engine/benches/wal_recovery.rs`,
расширяем существующий):

- `repo_wal_append_latency_{small,large_payload}` — стоимость
  записи V2 entry. Acceptance: < 5ms для batch с 1k ops (inline
  body).
- `recovery_scan_with_{0,10,100}_inflight_tx` — стоимость recovery
  с разным количеством inflight tx. Должна быть линейная в кол-ве
  entries.
- `wal_size_amplification` — размер V2 entry для 10k inserts по 1KB.
  Acceptance: < 11MB (overhead < 10%).

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

**Test + bench coverage.**

Correctness tests:

- `non_tx_call_paths_zero_overhead` — для каждой read/write функции
  pipeline (TableManager::read_one, iter_stream, filter_stream,
  IndexBackend::lookup, HnswAdapter::search) вызвать с tx=None и
  проверить идентичность результата с pre-tx-refactor baseline.
- `tx_call_paths_through_pipeline` — full pipeline test: tx writes
  через executor → reads внутри той же tx видят свои writes; reads
  вне tx видят pre-tx snapshot.
- `option_branch_coverage` — coverage tool (через `cargo llvm-cov`)
  подтверждает что обе ветки `if let Some(tx) = ...` покрыты тестами.

Benchmarks (`crates/shamir-engine/benches/engine_perf.rs`,
расширяем):

- `non_tx_read_throughput_before_after` — regression check
  относительно pre-refactor commit. Acceptance: < 2% regression
  (предсказуемый branch predictor).
- `non_tx_write_throughput_before_after` — то же на write path.
- `tx_read_overhead_vs_non_tx` — measure overhead Some(&TxContext)
  относительно None. Acceptance: < 15% (working_set lookup + mvcc
  range scan vs direct get).
- `tx_write_overhead_vs_non_tx` — write через TxContext.write_set
  vs direct store.set. Acceptance: < 10% (один HashMap insert vs
  один store call).

---

## D6. `version_codec` separator = `0xFF`

**Проблема.** Физический ключ MVCC history имеет форму
`<original_key> || <separator> || <version_be_u64>`. Какой byte
использовать как separator?

**Выбранное решение.** Single `0xFF` byte.

**Почему не иначе.**
- `0x00` — встречается в `RecordId::system("name")` (4-byte zero
  prefix + ASCII tag). Высокий риск collision.
- `\\` или `:` (ASCII printable) — могут встретиться в
  interner-encoded keys.
- Length-prefix encoding (`varint(key_len) || key || version`) —
  semantically чище, но 2-5 байт overhead вместо 1. И требует
  две версии decoder (varint detection).

`0xFF` в RecordId встречается с вероятностью ~1/256 в каждом байте
(crypto-random); никогда не встречается в `system("tag")` RecordIds
(4 нулевых байта + ASCII tag, ASCII никогда не достигает `0xFF`).
History store **физически отделён** от main store — `decode_version_key`
вызывается только на ключах, которые мы сами туда положили.
Invariant контролируем.

**Реализовано в** commit `a9791b4`
(`crates/shamir-tx/src/version_codec.rs`).

**Test coverage** (`tests` module в том же файле):
- `round_trip` — encode → decode → original для 5 различных
  version значений (0, 1, 42, u64::MAX/2, u64::MAX).
- `empty_key_round_trip` — корректно работает для key = `b""`.
- `sort_order_matches_version` — BE encoding даёт естественный
  лексикографический порядок: `k::0 < k::1 < k::42`.
- `different_keys_dont_interleave` — `aaa::MAX < aab::0`
  (key-prefix dominates).
- `short_input_decodes_to_none` — < 9 bytes → `None`.
- `missing_separator_decodes_to_none` — 16-byte input без `0xFF`
  на правильной позиции → `None`.

**Bench coverage.** Не требуется — функция выполняет O(key.len())
`extend_from_slice` + два byte writes. Нет hot-path риска.

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
