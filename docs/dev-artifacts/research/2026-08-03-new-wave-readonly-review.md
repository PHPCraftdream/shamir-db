# Read-only review новой волны перед первым релизом

**Дата:** 2026-08-03  
**Репозиторий:** `D:\dev\rust\shamir-db`  
**HEAD:** `72106a908fd817433170496bab376d2fba27f7ab` (`master`)  
**Предыдущая точка review:** `28d39f31ec76a54a467441dc54a1a6ddc086648e`  
**Диапазон:** 102 коммита, 162 файла, `+16 988 / -1 709`  
**Режим:** только чтение Git и файлов. Cargo-команды, тесты, бенчмарки и сервер не запускались. Единственное изменение — этот отчёт.

## Короткий вердикт

Новая волна качественная и существенно улучшила проект: закрыты прежние дефекты epoch для `AsOf`, planner-invisibility незавершённых индексов, fail-open в commit-time rederive, порядок bump/apply, прежняя lock inversion, часть памяти backfill, честность `Store::transact`, loom-gate и API-проверки `CreateIndex`.

Но **первый релиз сейчас — NO-GO**. Причина не в нехватке ещё одного оператора OQL, а в незавершённом протоколе жизненного цикла индексов. Найдены релиз-блокирующие сценарии:

1. два одновременных DDL одного класса могут преждевременно снять общий write-barrier;
2. транзакция, staged до `CREATE INDEX`, может committed после него без posting/unique guard;
3. generation-снимки index2/sorted имеют TOCTOU между snapshot и чтением generation;
4. `DROP INDEX` не защищён от старых readers/writers/tx и не crash-safe;
5. повреждённый unique posting трактуется как свободный ключ;
6. rename index2 не сохраняет новое имя в descriptor, поэтому rename теряется после restart;
7. релизный perf-gate физически не готов: нет `bench-baseline.json`, а workflow прямо сообщает, что runner не зарегистрирован.

До закрытия этих пунктов расширять OQL крупными функциями вроде JOIN не рекомендую: это увеличит площадь системы поверх ещё нестабильного DDL/index foundation.

## 1. Что изучено

- история `28d39f31..72106a90`, включая F69–F87 и последующие client/e2e/observability commits;
- `TableManager`, tx staging/pre-commit/materialize, четыре семейства индексов;
- create/drop/rename, persisted lifecycle state, mutation epochs;
- `Store::transact` и capability `supports_atomic_transact`;
- Rust query types, Rust query builder, TS/Node release surface;
- README, CHANGELOG, Known Limitations, security/contribution docs;
- CI, loom, stress/nightly, release и performance workflows;
- корневые tracked/untracked файлы.

Рабочая копия до записи отчёта содержала только пользовательские untracked-артефакты:

```text
?? devrust.cargo-target/
?? docs/checkpoints/2026-08-03-query-builder-migration-complete.md
```

Они не изменялись.

## 2. Оценка сделанной волны

| Направление | Статус | Комментарий |
|---|---:|---|
| F69: один packed atomic для barrier flags | Частично закрыто | Torn read устранён, hot path теперь делает один `SeqCst` load. Но bit не считает владельцев, что создаёт новую overlap-гонку. |
| F70: lock inversion | Закрыто локально | Канонический `raise → drain → lock` действительно удаляет старый цикл deadlock. Композиция двух DDL с одним bit не закрыта. |
| F71: `AsOf` mutation epoch | Закрыто | Ready floor задаётся текущим committed watermark, восстановление epoch покрыто тестовыми векторами. |
| F72: planner не видит partial build | Закрыто для новых readers | `Building` фильтруется у regular/sorted/index2. Уже получившие старый read handle и DROP — другой сценарий. |
| F73: rederive fail-closed | Закрыто | Ошибки read/decode/planner теперь прерывают commit через `Result`. |
| F74: bump epoch до posting apply | Закрыто | Удалено окно, в котором seek мог наблюдать старые postings с ещё разрешённым epoch. |
| F76: retire before sweep | Частично закрыто | Новый reader после retire fallback-ится. In-flight reader, старый writer/tx и durable resurrection не закрыты. |
| F77/F85: `Store::transact` truthfulness | Улучшено, не закрывает потребителей | Capability честный, но production callers его не проверяют; отдельные callers всё ещё описывают batch как atomic. |
| F78: bounded-memory regular backfill | Закрыто для regular | Regular и sorted stream-ят. Unique по-прежнему O(table) memory; главное — все create удерживают write lock на полный backfill. |
| F79: runtime mutex cleanup | В основном хорошо | Горячие registry/read paths стали лучше; остаётся явно обозначенный dead-scaffolding `pending_commits` под `std::sync::Mutex`. |
| F80/F84: perf bench + loom CI | Частично закрыто | Loom стал честнее и включён в PR CI. Общий release perf-gate всё ещё неоперационен. |
| F81/F87: `CreateIndex::try_build` | Полезно, но узко | Проверяет только три btree-комбинации; stringly typed options и infallible escape hatches остаются. |
| F83: corrupt `IndexInfo` | Закрыто в этом codec | Corrupt metadata больше не превращается в пустой валидный каталог. Corrupt unique posting остаётся fail-open в другом месте. |
| F86: stale docs/tests layout | Частично | Большой объём исправлен, но stale assertions/comments ещё есть, примеры ниже. |

## 3. P0 — блокеры первого релиза

### P0-1. WriteBarrierGuard не поддерживает несколько владельцев одного bit

**Код:**

- `crates/shamir-engine/src/table/table_manager.rs:763-777`
- `crates/shamir-engine/src/table/table_manager.rs:1040-1050`
- `crates/shamir-index/src/legacy/write_barrier_flags.rs:191-200`

`begin_write_barrier(bit)` выполняет:

```text
flags.fetch_or(bit) → drain → unique_write_lock.lock()
```

а `Drop` каждого guard безусловно делает:

```text
flags.fetch_and(!bit)
```

Сценарий для двух параллельных `CREATE INDEX` одного семейства:

1. DDL-A устанавливает `REGULAR_INDEX_CREATE`, drain-ит и получает lock.
2. DDL-B устанавливает тот же bit; значение atomic не меняется, B drain-ит и ждёт lock.
3. A заканчивает и очищает bit.
4. B получает lock, но его guard не переустанавливает уже очищенный bit.
5. Новый writer читает `needs_write_barrier() == false`, идёт fast path и может пересечь snapshot/backfill B. B уже выполнил drain и повторно его не делает.

То же относится к одновременным schema activation, unique/sorted/index2 create одного класса. Текущие F70 tests моделируют lock inversion, но не два guard одного bit.

**Исправление:** не делать bit владельческим флагом. Наиболее простой безопасный вариант — отдельный per-table `ddl_admission: tokio::sync::Mutex<()>`, который берётся **до** `raise → drain → unique_write_lock`; writers этот mutex не берут, поэтому старый deadlock не возвращается. Альтернатива — per-bit reference counters с единым derived packed word, но это сложнее доказать. После получения `unique_write_lock` полезна защитная повторная проверка intent + drain.

**Обязательные тесты:** два deterministic paused DDL одного bit; A завершается, B остаётся активным, writer обязан блокироваться до завершения B. Отдельно schema, regular, unique, sorted, index2.

### P0-2. Tx plan устаревает относительно CREATE INDEX

**Код:**

- `crates/shamir-engine/src/table/table_manager_tx_ops.rs:383-413`
- `crates/shamir-engine/src/table/table_manager_tx_ops.rs:228-240`
- `crates/shamir-engine/src/tx/pre_commit.rs:764-1080`
- `crates/shamir-index/src/registry.rs:71-101,132-150`

Транзакция хранит готовые physical `index_write_set` и `unique_guards`, вычисленные при staging. Commit-time rederive существует только для index2 и sorted.

#### 2a. Regular и unique вообще не имеют lifecycle generation/rederive

Сценарий unique:

1. T1 начинает tx и stage-ит INSERT, пока unique index ещё отсутствует.
2. `has_any_index()` false; unique validation, guard и posting не создаются.
3. DDL создаёт первый unique index по committed snapshot и завершается.
4. T1 commit-ится после DDL. Текущий unique bit заставит взять lock, но `tx.unique_guards` всё ещё пуст, а posting plan не пересчитывается.
5. Строка появляется без unique posting; она также может дублировать уже существующее значение.

Для regular результат — permanently missing posting. F50 закрыл аналогичный случай для части index2/sorted, но не legacy regular/unique.

#### 2b. Index2/sorted generation считывается после snapshot

Сейчас staging сначала вызывает `all_backends()`/snapshot defs, затем отдельно читает `registry.generation()`. Между этими операциями DDL может зарегистрировать индекс:

```text
tx snapshots old backends ── DDL inserts new backend/gen ── tx records new gen
```

На commit `current_gen == staged_gen`, поэтому rederive не запускается, хотя новый backend отсутствовал в plan. Для index2 ситуация усиливается тем, что `insert()` повышает generation до публикации в `by_id` (`registry.rs:76-95`).

#### 2c. DROP/RENAME не удаляют старые planned ops

`rederive_index2_ops_post_stage` только `extend`-ит новые ops. Нет `retain/remove` для backend/index IDs, retired после staging. Sorted rederive также добавляет plan по current defs, сохраняя old physical ops. Старый tx после DROP/RENAME способен воскресить orphan postings.

**Исправление:** унифицировать lifecycle generation для всех четырёх семейств и перестать считать один scalar generation достаточным описанием snapshot.

Рекомендуемый контракт:

```text
stage: snapshot_with_generation() -> {defs/backends, generation, active_ids}
tx:    хранит planned index IDs + generation по таблице
commit под DDL/write lock:
       current snapshot
       remove ops/guards для retired IDs
       derive ops + unique guards для added/changed IDs
       validate regenerated unique guards
       serialize final plan into WAL
```

Snapshot и его generation должны исходить из одной RCU-публикации либо plan должен делать set-diff по стабильным index IDs. Для unique нужен commit-time rebuild guard из staged new values, а не только posting rederive.

**Обязательная матрица:** INSERT/UPDATE/DELETE × regular/unique/sorted/fts/functional/vector × stage-before-create/overlap/commit-after-create; аналогичная матрица для drop и rename. Для unique отдельно duplicate introduced after stage.

### P0-3. DROP INDEX не имеет безопасного reader/writer/durability protocol

**Код:**

- `crates/shamir-index/src/legacy/index_manager.rs:707-786`
- `crates/shamir-index/src/legacy/index_manager_unique.rs:455-513`
- `crates/shamir-index/src/legacy/sorted_index_manager.rs:452-497`
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:679-750`

F76 правильно переставил retire перед sweep, поэтому **новый** planner после retire не выберет индекс. Но три утверждения всё ещё неверны.

#### 3a. `Arc`/RCU не даёт snapshot физических postings

Уже начавшийся reader может держать old definition или `Arc<dyn IndexBackend>`, но postings находятся в общем mutable `info_store`. `drop_all/remove_many` удаляет данные из того же store. Такой reader может увидеть частично очищенный индекс. `Arc` сохраняет backend object, а не версию его keyspace.

#### 3b. Старый writer/tx может записать posting после sweep

Non-tx writer мог snapshot-нуть old definitions до retire; tx мог stage-ить physical ops задолго до DROP. После sweep они запишут ключи обратно. Для legacy physical namespace основан на `name_interned`, поэтому последующий CREATE с тем же именем переиспользует namespace и получает ghost postings. Для unique это даёт ложные conflicts или нарушение constraint; для sorted/regular — wrong candidates/storage leak.

#### 3c. Crash/error после sweep, но до metadata persist, resurrect-ит Ready index

Во всех legacy drop порядок фактически такой:

```text
retire live → sweep postings → persist reduced definitions
```

У index2 аналогично `remove registry → drop_all → save metadata`. Если process падает или последняя запись metadata возвращает ошибку, на диске остаётся старый `Ready` descriptor, а postings уже удалены. После restart planner снова загрузит индекс и будет возвращать неполные результаты. Это durable correctness defect, не косметическая неатомарность DDL.

**Исправление:** persisted state machine как минимум `Ready → Dropping(tombstone) → GC complete → descriptor removed`. Open path обязан считать `Dropping` planner-invisible и завершать sweep идемпотентно. Physical GC — только после reader grace period/epoch и после того, как old tx plans больше не могут примениться; либо commit обязан отфильтровывать retired IDs по P0-2.

Для alpha допустим stop-the-world table DDL, если он корректен: взять DDL admission + writer barrier + lock, persisted tombstone, дождаться readers/запретить новые index reads, sweep, final metadata. Но «retire RCU и сразу удалить общий keyspace» недостаточно.

### P0-4. Corrupt unique posting fail-open

**Код:** `crates/shamir-index/src/legacy/index_manager_unique.rs:169-183`.

Если value unique key имеет длину не 16 bytes, код логирует warning и возвращает `Ok(None)`, то есть «ключ свободен». Следующая запись может пройти validation и закрепить duplicate/corruption.

Это противоречит общей политике checksums/fail-closed и работе F83 для metadata. Нужно вернуть typed corruption error (`DbError::Codec`/новый `Corruption`) и запретить mutation до repair. Не использовать `unwrap()` даже после проверки длины — `try_into()` естественно маппится в typed error.

Тесты: длины 0/15/17/large; create/update/tx commit; ни один путь не должен считать key свободным.

### P0-5. RENAME INDEX не имеет надёжной persistence/error atomicity

#### 5a. Index2 rename детерминированно теряется после restart

**Код:**

- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1055-1071`
- `crates/shamir-index/src/registry.rs:216-256`
- `crates/shamir-index/src/persistence.rs:55-83`

`rename_entry` меняет только `by_name`. Backend descriptor остаётся immutable со старыми `name`/`name_interned`. `save_index2_metadata()` вызывает `all_descriptors()`, который клонирует старый descriptor и заменяет только `state`. Следовательно, live lookup по новому имени работает, но persisted metadata снова содержит старое имя; после restart rename исчезает.

Нужно хранить authoritative mutable descriptor в registry tuple/RCU snapshot либо заменить backend entry новым descriptor-preserving backend. Rename обязан обновлять и string name, и interned ID, затем проверяться reopen-тестом для FTS/functional/vector.

#### 5b. Sorted rename полагается на atomic `transact`, которой может не быть

`rekey_sorted_prefix` сначала persist-ит definition под новым ID, затем делает `info_store.transact(Set(new), Remove(old), ...)` и комментирует это как atomic. Но trait после F77 прямо сообщает, что atomic visibility доступна только при `supports_atomic_transact() == true`; production caller flag не проверяет. При partial error rename уже не может быть штатно повторён: catalog считает source новым, а часть postings остаётся под старым prefix.

Нужны persisted `Renaming { old_id, new_id }`, resumable open-path settle и fault-injection на каждом op boundary. Capability flag сам по себе correctness не добавляет.

### P0-6. Release perf-gate не может пропустить релиз

**Код/конфиг:**

- `.github/workflows/perf-gate.yml:15-23,60-74`
- `.github/workflows/release.yml:461-471`
- `scripts/bench_gate.sh:13,38-43,338-340`

Проверено по дереву:

- `bench-iters.txt` tracked и содержит calibration counts;
- `bench-baseline.json` отсутствует и не tracked;
- workflow-комментарий прямо говорит, что runner с labels `[self-hosted, shamir-bench]` не зарегистрирован и job будет queued forever;
- tag release имеет `perf-gate` в обязательных `needs`.

Даже после регистрации runner script fail-closed завершится на отсутствии baseline. Перед тегом необходимо зарегистрировать/защитить runner, зафиксировать hardware/OS/power governor, capture baseline на этом же runner, commit-нуть baseline и сделать dry-run ровно tag workflow. Желательно artifact с fresh JSON и environment fingerprint.

## 4. P1 — сделать до релиза либо явно урезать обещания

### P1-1. Legacy Building не self-heal-ится автоматически

Regular/sorted create теперь безопасно остаётся `Building` при error/crash и не используется planner-ом — это хороший fail-closed. Но open path не продолжает/перестраивает build; требуется ручной `doctor::repair()`. Для self-contained DB это слабая эксплуатационная модель: после обычного restart индекс навсегда скрыт без заметного server health failure.

Добавить open-time job: удалить partial postings, rebuild from current snapshot, выставить epoch/Ready, persist. До этого doctor должен показывать prominent unhealthy state и server readiness/metrics должны сигнализировать degraded index.

### P1-2. CREATE/RENAME/DROP могут вернуть `Err` после live mutation

В нескольких путях live state публикуется до финальной metadata write. Клиент получает error, но операция фактически частично/полностью действует до restart; после restart состояние может отличаться. Нужен единый DDL result contract: operation ID + status, либо rollback, либо persisted state machine с идемпотентным recovery. Простого `Result<()>` недостаточно для многофазного DDL.

### P1-3. `Store::transact` contract всё ещё двусмыслен для callers

`types.rs:179` начинает описание словами «Atomic mixed-op batch — either ALL...», сразу после чего говорит, что default не atomic. Capability добавлен правильно, но API по имени и первому абзацу продолжает провоцировать неверные предположения. В comments production callers заявлен self-healing settle/re-scan, хотя это нужно доказать отдельно для каждого caller и error boundary.

Предлагается разделить API:

- `apply_batch_ordered` — последовательная/error semantics;
- `atomic_transact` — только для backend с гарантией, иначе `UnsupportedCapability`;
- state-machine callers — resumable независимо от backend atomicity.

### P1-4. Долгий CREATE INDEX блокирует все writes на время полного scan

F78 уменьшил peak memory regular backfill, но комментарий честно фиксирует, что write lock удерживается на весь scan. На средней таблице DDL становится write outage; для unique ещё и O(table) memory.

Перед alpha-релизом минимум: documented operational warning, progress metrics, timeout prohibition, health visibility и benchmark writer queue p50/p95/p99 на 100k/1m rows. Следующий этап — online build:

```text
persist Building → snapshot version → bulk scan without global writer lock
→ collect/replay delta log → short barrier/drain cutover → Ready
```

### P1-5. Query Builder остаётся panic-prone через основной Batch API

**Код:**

- `crates/shamir-query-builder/src/batch/into_batch_op.rs`
- `crates/shamir-query-builder/src/ddl/replication.rs:275-298`
- `crates/shamir-query-builder/src/query/query.rs:325-385`

`Update`, `Upsert`, `Delete`, `AlterSubscriptionBuilder` имеют fallible `build()`, но `IntoBatchOp`/`From` вызывают `.expect(...)`. `Batch::insert/update/op` принимает infallible `IntoBatchOp`, поэтому malformed builder паникует **до** `Batch::try_build()` и typed error не может быть возвращён. `Query` и `CreateIndex` аналогично имеют permissive `build`, а common conversions обходят `try_build`.

До API freeze следует ввести `TryIntoBatchOp` и fallible `Batch::try_add/try_op`, либо typestate builders, где обязательные terminal fields закодированы типом. Публичная библиотека builder не должна падать из-за пользовательского пропуска поля.

### P1-6. Индексный builder stringly typed и валидирует только малую часть shape

`CreateIndexOp` сочетает `unique`, `sorted`, `index_type: Option<String>` и набор nullable options для FTS/functional/vector. Получается множество бессмысленных состояний: tokenizer на vector, vector_dim на btree, unknown metric/quantization, empty fields, zero dimension и т.д. F81 ловит только три btree-комбинации.

Лучший Rust surface:

```rust
enum IndexSpec {
    Hash { fields, unique },
    Sorted { field, include },
    Fts { field, tokenizer, language, ranking },
    Functional { field, expression },
    Vector { field, dim: NonZeroU32, metric, quantization },
}
```

Wire DTO можно оставить backward-compatible, но builders Rust/TS должны строиться из одной declarative schema/fixture matrix, чтобы parity не поддерживалась вручную.

## 5. DDL: что развивать

### До первого релиза

1. Unified persisted lifecycle для индексов: `Building`, `Ready`, `Dropping`, `Renaming`, `Failed`.
2. Per-table DDL admission, исключающий overlapping same-bit operations.
3. Commit-time reconciliation всех index families.
4. `ALTER INDEX ... REBUILD/VALIDATE` и machine-readable status, хотя бы как admin DTO.
5. `DESCRIBE/LIST INDEXES` должны показывать state, progress, last error, ready_at_version, storage bytes.
6. Unified `DROP INDEX name`: текущий `unique: bool` заставляет клиента заранее знать внутреннее семейство. Catalog уже умеет классифицировать имя.
7. Исправить contract `RenameIndexOp`: doc говорит об `if_exists`, но поля и builder method нет.
8. DDL audit: документация честно сообщает, что durable HMAC audit покрывает только authentication, не DDL/ACL/admin. Для публичного secure-positioning это P1.

### После alpha.1

- composite sorted indexes и composite unique constraints;
- partial indexes (`WHERE predicate`) — обычно полезнее TTL/geo для малых проектов;
- online/resumable build + cancel;
- schema migrations как schema evolution, а не только смена storage engine;
- nested defaults/auto timestamps и multi-field FK;
- declarative CHECK constraints поверх существующей validator/function infrastructure;
- optional transactional metadata DDL внутри одного repo, после стабилизации state machine.

## 6. OQL: что развивать

Текущая OQL уже сильнее «простого document CRUD»: filters, aggregates/group/having, batch DAG, references, conditional execution, loops, temporal reads, FTS/vector/functional filters, EXPLAIN, cursors. Для alpha это достаточный объём.

Приоритеты развития:

1. **Computed SELECT expressions.** DTO `SelectItem::Expression` существует, но Known Limitations сообщает, что executor их отвергает. Это меньшая и более естественная ступень, чем JOIN.
2. **From/query alias или explicit semi-join.** В старом batch README отмечен `Select from Alias` как TODO. Сначала полезны `EXISTS`, lookup по результату предыдущего alias и bounded equi-lookup, а не универсальный SQL JOIN planner.
3. **True streaming result protocol.** Сейчас обычный query result полностью materialize-ится; cursor тоже имеет ограничения (`Latest`, без `with_version`, сложные key types). Это важнее для средних проектов, чем новый синтаксис.
4. **Composite index-aware ORDER/RANGE.** Composite sorted + planner prefix rules дают больше практической пользы, чем широкий набор новых filter operators.
5. **EXPLAIN ANALYZE.** Есть dry-run EXPLAIN и stats, но нужен actual-vs-estimated rows, fallback reason, post-filter count, heap/materialization bytes.
6. **Bulk mutation returning/projection** и typed affected/returned records с лимитами.
7. **Window functions / joins / set operations** — только после bounded execution, spill-to-disk и cost model; иначе они усилят O(N) memory риски.

Не стоит обещать SQL parity. README сейчас правильно и честно говорит, что ShamirDB не drop-in replacement PostgreSQL/MySQL/MongoDB/Redis/Memcached; это позиционирование следует сохранить.

## 7. Производительность: реальные приоритеты

### Уже сделано хорошо

- one-load `needs_write_barrier` вместо шести atomics;
- regular/sorted streamed backfill;
- top-K heap для `ORDER BY + LIMIT` вместо предварительного O(N) accumulator;
- O(1) counter shortcut и indexed count paths;
- RecordView/lens path уменьшает decode/materialization;
- bounded caches/RCU/CAS registries в основных engine paths;
- отдельный benchmark F80 и loom PR gate.

### Следующие измеримые задачи

1. **DDL write stall**, не nanoseconds одного atomic. Измерять writer p50/p95/p99/max при create regular/sorted/unique/index2 на 100k/1m rows.
2. **Unique build memory.** Global duplicate knowledge не требует обязательно RAM O(table): partitioned hash files, external sort или временный durable keyspace с collision detection дают bounded memory.
3. **DROP memory.** Legacy drop собирает все keys в `Vec<RecordKey>` перед `remove_many`; это O(index size). Удалять bounded batches либо backend-native prefix delete.
4. **GROUP BY/DISTINCT spill.** Сейчас эти формы остаются full materialization; добавить memory budget и spill/error-before-OOM.
5. **Result streaming/backpressure.** Полный `Vec` результата остаётся главным memory/latency ceiling.
6. **Centralize backfill batch size.** В нескольких production местах жёстко `list_stream(1000)` вместо tunable/profile value.
7. **Unique validation batching.** Сейчас проверки по indexes/rows делают отдельные async gets; использовать `get_many`/dedup keys для batched insert и commit reconciliation.
8. **Perf gate quality.** Текущий 25% threshold слишком широк для hot-path regressions, а commit «percentile» — один average proxy. После стабилизации runner добавить distributions и несколько repetitions с median/MAD.

Не оптимизировать `SeqCst` barrier load вслепую до появления working baseline. Correctness protocol важнее экономии нескольких ns.

## 8. Документация, public repo и release engineering

### Хорошо

- README имеет честный alpha disclaimer и не обещает drop-in replacement зрелых СУБД.
- Есть MIT/Apache-2.0, CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, issue/PR templates.
- Toolchain pinned, actions pinned SHA, supply-chain checks, SBOM и cosign release artifacts.
- CI покрывает Linux/Windows/macOS; loom на PR; stress и TS/Node e2e имеют nightly workflows.
- Все Cargo crates явно `publish = false`, что согласуется с binary-first GitHub release.
- Корень tracked-дерева в целом чистый; `bench-iters.txt` теперь является намеренным входом perf harness, а не случайным логом.

### Перед тегом

1. Закрыть P0 и выполнить полный gate на clean checkout.
2. Реально выполнить release workflow через временный pre-release tag/dry-run, включая self-hosted perf runner.
3. Создать `## [0.1.0-alpha.1] - YYYY-MM-DD`, перенести релизные пункты из огромного `[Unreleased]`; сейчас workflow иначе использует fallback notes.
4. Обновить SECURITY после появления первого tag: таблица сейчас говорит, что tags/releases отсутствуют.
5. Зафиксировать upgrade/export/import и backup-restore drill конкретными командами и checksum verification.
6. Опубликовать reproducible release manifest: git SHA, rustc, target, features, archive SHA256, SBOM, signature verification command.
7. Если одновременно заявляется клиентский релиз: решить Node package `0.1.0` vs workspace/TS `0.1.0-alpha.1` и отсутствие npm publish workflow. Документированный gap не заменяет release decision.
8. Исправить stale comments, в частности:
   - `table_manager_index_mgmt.rs:63-69` всё ещё говорит, что tx commit gap «not implemented», хотя F50 добавлен;
   - `rename_index` regular comments описывают старую register-first модель без актуального `Building` Ready-gate;
   - `RenameIndexOp` обещает `if_exists`, которого нет;
   - `rekey_sorted_prefix` безусловно называет `Store::transact` atomic, противореча F77.

## 9. Предлагаемый release plan

### Wave R0 — correctness foundation

- P0-1 DDL admission / barrier ownership.
- P0-2 atomic lifecycle snapshot + commit reconciliation для всех index families.
- P0-3 `Dropping` state, old-reader/old-writer protocol, reopen recovery.
- P0-4 fail-closed unique corruption.
- P0-5 durable/resumable rename, включая descriptor update index2.

Gate: deterministic concurrency/fault/restart matrix, затем обязательные repo checks.

### Wave R1 — operational release readiness

- auto-heal `Building/Dropping/Renaming` на open;
- doctor/status/metrics/progress;
- bounded drop и unique build memory либо строгие alpha limits;
- working self-hosted perf runner + committed baseline;
- tagged changelog, security/release docs, backup/restore drill.

### Wave R2 — API freeze

- `TryIntoBatchOp`/typestate, убрать user-triggered panics;
- typed `IndexSpec` и Rust/TS parity fixtures;
- unified DROP/ALTER/STATUS index DDL;
- review wire compatibility/version negotiation.

### Wave R3 — после alpha.1

- computed projections;
- streaming results/cursors parity;
- composite/partial indexes;
- online delta-catchup index builds;
- затем bounded joins/window functions.

## 10. Минимальный exit checklist для `v0.1.0-alpha.1`

- [ ] Два concurrent DDL одного barrier bit не открывают fast writer path.
- [ ] Tx staged до CREATE корректно reconciles regular/unique/sorted/index2 ops.
- [ ] Новый unique index проверяет staged-before-create duplicate на commit.
- [ ] Snapshot+generation TOCTOU устранён API-конструкцией, не timing test-only workaround.
- [ ] DROP не даёт partial result in-flight reader-у.
- [ ] Old tx/writer не воскресит posting после DROP/RENAME.
- [ ] Crash/error в каждой фазе CREATE/DROP/RENAME безопасно восстанавливается.
- [ ] Corrupt unique posting abort-ит write/commit.
- [ ] Index2 rename сохраняется после reopen.
- [ ] Sorted rename корректен на backend без atomic transact или явно запрещён там.
- [ ] Legacy `Building` имеет auto-heal либо server health явно degraded и документирован repair drill.
- [ ] `cargo fmt --all -- --check` green.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` green.
- [ ] `./scripts/test.sh` green.
- [ ] integration/full, loom, stress subset, TS e2e и Node e2e green на final SHA.
- [ ] `bench-baseline.json` committed с fingerprint целевого runner.
- [ ] Release workflow dry-run завершён, не queued.
- [ ] Backup → destructive mutation → restore → checksum/data verification пройден.
- [ ] CHANGELOG tagged section, SECURITY и release notes обновлены.

## Итог

Архитектурное направление новой волны верное: state-gated publication, mutation epochs, fail-closed decoding, RCU/CAS и explicit capability metadata — именно те примитивы, которые нужны production-grade engine. Проблема в том, что они пока применены по отдельным семействам и отдельным фазам, а не образуют один доказуемый lifecycle protocol.

Главная следующая задача — не «добавить больше DDL/OQL», а унифицировать индекс как durable state machine, согласованную с tx plan, readers, writers и recovery. После этого проект будет гораздо ближе к честному alpha-релизу; до этого tag создаст риск тихо неверных результатов именно в тех сценариях, которые база данных обязана выдерживать лучше всего.
