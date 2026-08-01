# S.H.A.M.I.R. Database — readonly review новой волны перед первым релизом

Дата review: 2026-07-30  
Ветка: `master`  
Проверенный HEAD: `28d39f31ec76a54a467441dc54a1a6ddc086648e`  
Предыдущая точка review: `e145b1d36477391abae30d374c2cea6dfd07ed6d`  
Диапазон: 23 коммита, 52 файла, `+5227/-394`

## 1. Итоговый вердикт

Новая волна — существенное улучшение. F55/F56/F59/F60/F61/F62/F63/F65/F66
закрывают именно те классы дефектов, на которые были направлены. F67 правильно
сужает mutation high-water с table-wide до per-index. Особенно хорошо сделаны:

- fail-closed FK discovery и indexed FK re-read;
- единый writer drain для non-tx и tx commit writers;
- `SeqCst`-доказательство cross-atomic writer/drain protocol;
- защита persistent self-hosted runner от автоматического выполнения кода fork PR;
- fail-closed проверка полноты вывода perf benchmark;
- фактическая зависимость release DAG от perf gate;
- замена `ri_barrier_tokens` на `scc::HashSet`.

Но **тег пока NO-GO**. Остались четыре correctness blocker и один
release-operational blocker:

1. regular и sorted indexes становятся видимыми planner до окончания backfill;
   DROP INDEX у части семейств имеет симметричное окно частичной видимости;
2. tx commit изменяет sorted postings до подъёма AsOf mutation epoch;
3. AsOf epoch равен `0` после CREATE INDEX и после restart, хотя current index
   уже содержит состояние более новых версий;
4. commit-time index-plan re-derivation fail-open проглатывает storage/planner/
   decode errors и может закоммитить row без posting нового индекса;
5. release perf gate структурно подключён, но не может пройти: в репозитории нет
   `bench-baseline.json`, а runbook прямо фиксирует отсутствие runner.

Для закрытой technical alpha можно либо исправить эти пункты, либо временно
выключить опасные fast paths/online DDL. Для публичного первого релиза
оставлять их включёнными нельзя.

## 2. Границы review

Исследование выполнено только чтением Git и файлов. Код, конфигурация и
существующие документы не изменялись; создан только этот отчёт.

Проверено:

- каждый commit F55–F67 и его diff;
- FK discovery/action paths;
- writer-drain protocol и все index-create barrier flags;
- legacy regular/unique/sorted и index2 CREATE/DROP/RENAME lifecycle;
- AsOf sorted-index seek entry/post gates и mutation high-water wiring;
- tx commit-time re-derivation;
- `MirroredStore::transact`;
- perf script, parser test, standalone workflow и tag release DAG;
- DDL/OQL/query-builder surfaces;
- changelog, versions, tags и корневые release-файлы.

Не запускались `cargo fmt`, `clippy`, tests и benchmarks: пользователь задал
строгий readonly-режим, а Cargo создаёт/изменяет `target` и cache artifacts.
`git diff --check e145b1d3..HEAD` прошёл без whitespace errors. Утверждения в
commit messages о зелёных тестах считаются заявлением авторов, а не независимо
перепроверенным результатом этого review.

## 3. Матрица новой волны

| Изменение | Оценка | Что реально закрыто | Что осталось |
|---|---|---|---|
| F55 FK discovery fail-closed | Закрыто | Ошибка resolve любого child table прерывает весь repo scan | Нужны race/fault tests на drop/rename table, но fail-open больше нет |
| F56 writer drain | Core закрыт | Все четыре cross-atomic операции `SeqCst`; index2 вызывает drain | Loom запускается только вручную; hot-path цена не измерена новым baseline |
| F57 unified CREATE INDEX | Частично | Все writer classes сериализуются с create; in-flight fast writers дренируются | Нет единого reader-visible `Building/Ready`; regular/sorted публикуются раньше готовности |
| F58 AsOf post-check | Частично | Добавлен симметричный post-scan epoch check | Tx postings применяются раньше epoch bump; остаётся реальное окно |
| F59 mirrored mixed batch | Error atomicity закрыта | `Err` mirror не оставляет изменения ни durable, ни ephemeral primary subset | Concurrent readers всё ещё видят пооперационное применение primary batch |
| F60 fail-closed perf parser | Закрыто на уровне parser | Missing/duplicate/stray cells и missing baseline entries fail closed | Сам baseline отсутствует; нет нескольких samples/dispersion/percentiles |
| F61 self-hosted PR isolation | Закрыто | Workflow только `workflow_dispatch`, fork PR не запускается автоматически | Нужен trusted approval flow, если захотите automatic PR gate |
| F62 release DAG | Структурно закрыто | Все release jobs зависят от inline perf-gate | Без runner/baseline любой tag зависнет/упадёт |
| F63 action SHA pin | Закрыто | `dtolnay/rust-toolchain` переведён на immutable SHA в затронутых workflows | Поддерживать automated pin update policy |
| F65 FK candidate read errors | Закрыто | Ошибки re-read больше не превращаются в `continue` | Test seam аккуратно `cfg(test)`; production overhead отсутствует |
| F66 RI token mutex | Закрыто в заявленном scope | `ri_barrier_tokens` теперь lock-free `scc::HashSet` | В runtime остаются другие `std::sync::Mutex`, см. P1 |
| F67 per-index epoch | Хорошая perf-идея, correctness не завершён | Независимый index больше не инвалидирует чужой AsOf seek | Наследует ordering/init/restart проблемы epoch |

F64 в диапазоне отсутствует; нумерация переходит с F63 на F65.

## 4. Release blockers — P0

### P0-1. CREATE/DROP INDEX всё ещё может показать читателю частичный индекс

F57 корректно закрывает потерю concurrent writes: DDL поднимает barrier,
дренирует fast writers и держит `unique_write_lock`. Но этот lock используется
писателями, не читателями. Поэтому он не делает частично построенный индекс
невидимым planner.

#### Regular hash index

`TableManager::create_index` сначала материализует всю таблицу, затем вызывает
`create_index_from_records`:

- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:507-543`;
- `crates/shamir-engine/src/table/table_manager_streaming.rs:443-472`.

Внутри manager определение публикуется **до** записи postings:

- `crates/shamir-index/src/legacy/index_manager.rs:317-350`.

После `indexes.add_index` и `has_indexes.store(true)` concurrent SELECT уже
может выбрать этот index через `read_planner.rs:153-160`, пока `set_many`
postings ещё не завершён. Результат equality query может быть неполным.

Отдельно `save_index_info().await?` выполняется после live publication. Ошибка
persist оставляет опубликованный in-memory index и возвращает caller `Err`.

#### Sorted index

Sorted path ещё очевиднее:

- `register(def)` публикует RCU definition и сохраняет metadata;
- только затем streamed loop вызывает `on_record_created` для каждой row.

См.:

- `crates/shamir-engine/src/table/table_manager_sorted_index.rs:106-135`;
- `crates/shamir-index/src/legacy/sorted_index_manager.rs:306-331`;
- planner немедленно читает definition в
  `crates/shamir-engine/src/table/read_planner.rs:291-347`.

Во время успешного, не отменённого CREATE concurrent range/ORDER BY query
может выбрать частичный sorted index и вернуть не все строки. Комментарий
`cancel-safe: NO` описывает cancellation residual, но normal concurrent-read
residual опаснее и не требует cancellation.

#### DROP INDEX

Index2 drop сначала выполняет `backend.drop_all()`, пока backend ещё находится
в live registry, и лишь затем удаляет registry entry:

- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:662-683`.

Комментарий на строках 672–674 утверждает, что такой порядок не позволяет
читателю увидеть registered backend без postings, но фактически именно это
окно и создаётся: `drop_all().await` может быть длительным, а planner всё ещё
видит backend. Аналогично следует проверить legacy drop paths.

#### Что сделать

Нужен не только writer barrier, а единый lifecycle для **всех** index families:

```text
Absent -> Building (planner-invisible)
       -> Ready    (одна atomic publication)
       -> Dropping (planner-invisible; old readers держат snapshot/Arc)
       -> Absent
       -> Failed   (diagnostics + retry/drop)
```

Практический minimum перед alpha:

1. regular/sorted backfill вести в private backend/namespace;
2. definition публиковать planner только после полного успешного backfill;
3. DROP сначала атомарно убрать index из planner-visible snapshot, затем
   асинхронно удалить postings;
4. при ошибке/cancellation гарантировать rollback либо оставить явный
   `Failed/Building`, но никогда не queryable partial state;
5. добавить pause-seam tests: CREATE/DROP паркуется в середине, concurrent
   SELECT обязан использовать full scan/старый complete snapshot;
6. до реализации — объявить эти DDL offline-only и блокировать reads+writes
   table-wide либо отключить regular/sorted online CREATE.

### P0-2. Tx sorted postings применяются раньше AsOf epoch bump

F58 добавил правильный post-scan re-check. F67 сделал epoch per-index. Но tx
wire point расположен в неверном порядке:

```text
apply_index_ops_at_commit(...).await
invalidate_posting_cache_for_ops(...)
bump_touched_indexes(..., commit_version)
```

См. `crates/shamir-engine/src/tx/commit_phases.rs:596-617`.

Между завершением posting apply и bump другой runtime thread может:

1. пройти/уже пройти entry gate;
2. просканировать index без удалённого или перемещённого posting;
3. выполнить post-check до bump;
4. вернуть неполный AsOf result.

Отсутствие `.await` между apply и bump не создаёт атомарность между OS threads.
Для direct non-tx path порядок, наоборот, conservative-safe: bump выполняется
до `apply_ops` (`sorted_index_manager.rs:688-712`, `772-781`).

Комментарии на `commit_phases.rs:613-615` и
`sorted_index_manager.rs:658-661` также неверно называют missed bump
«только лишним fallback». Missed bump означает, что gate остаётся открытым;
для posting, исчезнувшего из scan, classifier ничего не увидит и fallback не
случится.

Исправление:

- минимальное: bump touched sorted indexes **до** posting apply; если apply
  завершится ошибкой, ложный future fallback безопасен;
- более строгий вариант: per-index seqlock (`odd = mutation in progress`,
  `even = complete`) и entry/post validation одного generation;
- долгосрочно: versioned/immutable index snapshots.

Нужен deterministic test seam строго между posting apply и epoch publication
для UPDATE indexed field и DELETE.

### P0-3. AsOf epoch не отражает возраст current index после CREATE и restart

`last_mutation_version` — только in-memory map. Новый manager создаёт его
пустым, а отсутствие entry читается как `0`:

- `crates/shamir-index/src/legacy/sorted_index_manager.rs:181-194`;
- `crates/shamir-index/src/legacy/sorted_index_manager.rs:206-231`.

CREATE sorted index backfill вызывает:

```rust
on_record_created(&id, &record, 0)
```

См. `crates/shamir-engine/src/table/table_manager_sorted_index.rs:126-134`.

Это ломает доказательство `epoch <= pinned => current postings mirror pinned`:

- index, созданный при current version 100, получает epoch 0;
- `AsOf(version=10)` с keyset pagination считает его безопасным;
- row, существовавшая в v10 и удалённая до build, отсутствует в current index;
- scan никогда не увидит её, classifier и post-check не помогут.

После process restart проблема повторяется для **всех** загруженных sorted
indexes: map снова пустой/0, хотя current postings включают историю изменений
до restart. Старый AsOf read может ошибочно воспользоваться current index.

Исправление:

1. при переходе index в `Ready` установить epoch минимум в current
   `last_committed_version`;
2. при open/restart инициализировать каждый loaded index conservative floor,
   равным repo/table open watermark;
3. либо persist `ready_at_version`/`last_mutation_version` в descriptor;
4. gate должен требовать одновременно
   `pinned >= ready_at_version` и отсутствие более новой/in-progress mutation;
5. до исправления отключить AsOf index seek после restart и для arbitrary
   historical `AsOf`; cursor, pinned после build, не оправдывает безопасность
   общего API.

Тесты: create index после delete/update, затем old AsOf; restart и old AsOf;
empty/non-empty table; index create concurrent с cursor pin.

### P0-4. Commit-time re-derivation нового индекса fail-open

Когда index generation изменился между stage и commit,
`rederive_index2_ops_post_stage` восстанавливает posting ops. Это правильная
архитектурная идея, но функция возвращает `()` и проглатывает ошибки:

- storage error: `Err(_) => {}` — `pre_commit.rs:719-720`, `820-821`;
- backend `plan_insert/update/delete_tx` errors — `if let Ok(ops)`;
- decode old/new record failures — `if let Ok(...)`;
- malformed RecordId — `continue`;
- sorted planning errors — `if let Ok(ops)`.

См. весь блок `crates/shamir-engine/src/tx/pre_commit.rs:606-843`.

При transient read error сценарий такой:

1. tx staged до CREATE INDEX;
2. новый index backfill завершился;
3. tx commit видит generation change;
4. re-derive не смог прочитать old row и silently skip;
5. data op попадает в WAL/Phase 5a, posting op отсутствует;
6. commit успешен, index навсегда расходится с table data.

Это тот же fail-open класс, который F55 и F65 правильно закрыли для FK.

Исправление:

- `rederive_index2_ops_post_stage(...) -> Result<(), TxError>`;
- propagate все storage/backend/planner/decode errors до WAL begin;
- `NotFound` трактовать как insert только там, где это доказанная семантика;
- malformed staged key/value считать internal invariant error, не `continue`;
- тестировать fault injection отдельно для index2 insert/update/delete/vector
  и sorted insert/update/delete;
- проверить, что при ошибке tx cleanly abort и никакой data/index/WAL partial
  state не публикуется.

### P0-R1. Perf gate пока невозможно пройти

F60–F62 хорошо сделали gate fail-closed и release-blocking. Но:

- `bench-baseline.json` отсутствует и не tracked;
- `CI_PERF_GATE_RUNBOOK.md` прямо говорит, что runner ещё не зарегистрирован;
- release job использует `[self-hosted, shamir-bench]`;
- `scripts/bench_gate.sh` без baseline завершится ошибкой;
- tag release поэтому останется queued либо red.

Это безопасное поведение pipeline, но operational **NO-GO** для тега.

До тега:

1. выделить изолированный runner;
2. harden host: ephemeral workspace, no reusable secrets, restricted network,
   automatic cleanup, pinned runner image/toolchain;
3. capture baseline на этом же host;
4. commit `bench-baseline.json` с machine fingerprint;
5. выполнить manual perf workflow на frozen release SHA;
6. затем создать tag; не ослаблять `needs`/`continue-on-error`.

## 5. Высокий приоритет — P1

### P1-1. `MirroredStore::transact` всё ещё не даёт visibility atomicity

F59 правильно обеспечивает whole-batch **error atomicity**: mirror commit
происходит до любого primary mutation. Однако primary меняется двумя
пооперационными loops:

- ephemeral subset — `storage_mirrored.rs:585-598`;
- durable subset — `storage_mirrored.rs:600-613`.

Concurrent reader может увидеть половину batch. Это прямо признано в
комментарии, но конфликтует с общим contract override:
`Store::transact` для backend override обещает, что partial state не observable
(`crates/shamir-storage/src/types.rs:189-193`).

Нужно либо:

- реализовать RCU root/snapshot swap для `InMemoryStore`;
- либо добавить capability `supports_atomic_transact` и не выдавать
  `MirroredStore` за atomic backend;
- либо переименовать/разделить API на `apply_batch` и `transact`;
- как минимум проверить, что никакой metadata caller не полагается на
  multi-key read atomicity.

### P1-2. DDL error/cancellation semantics различаются по index family

- regular: live definition до postings и persist/backfill errors;
- unique: postings до live definition, затем live publication до metadata
  persist success;
- sorted: live definition до streamed backfill, `cancel-safe: NO`;
- index2: durable `Building`, private backfill, live `Ready`, затем final
  metadata persist; failure последнего persist возвращает `Err` при live Ready;
- DROP/RENAME используют разные, местами best-effort sequences.

Единый state machine из P0-1 должен определить:

- что означает `Ok`, `Err`, cancellation и crash в каждой точке;
- можно ли retry той же operation idempotently;
- когда index становится planner-visible;
- кто и как удаляет orphan postings;
- что увидит caller после timeout, reconnect и restart.

### P1-3. WriterDrainBarrier добавляет заметную цену каждому write

Теперь каждый non-tx writer и каждая tx-written table платит:

- `SeqCst fetch_add` при входе;
- проверку `has_unique_indexes` и до пяти `SeqCst` barrier loads;
- `SeqCst fetch_sub` при выходе.

См.:

- `writer_drain_barrier.rs:148-182`, `207-221`;
- `table_manager.rs:820-837`;
- четыре CRUD entry points в `table_manager_crud.rs`.

Correctness важнее, и ослаблять ordering без нового доказательства нельзя.
Но F56 попал до появления рабочего baseline, поэтому реальная regression
неизвестна.

Нужно измерить:

- single-thread point set;
- 2/8/32 concurrent writers на одной и разных tables;
- tx commit 1/10/100 rows;
- no-index table и unique-index table;
- NUMA cross-socket cache-line contention.

После measurement можно рассмотреть один packed atomic state
(`gate bit + active count/generation`) вместо зависимости нескольких atomics,
что упростит доказательство и сократит flag loads.

### P1-4. Legacy regular/unique CREATE INDEX материализует всю таблицу

`collect_all_current_records` строит `Vec<(RecordId, InnerValue)>` всей table,
а затем index manager строит ещё vectors/maps postings:

- `table_manager_streaming.rs:443-472`;
- regular: `table_manager_index_mgmt.rs:537-543`;
- unique: `table_manager_index_mgmt.rs:610-617`;
- unique duplicate map: `index_manager_unique.rs:378-416`.

На medium dataset это O(table) decoded heap под длительно удерживаемым write
lock. При недостатке памяти process может abort, а latency writers растёт на
полный build.

Нужен streamed build через MVCC-aware `list_stream`:

- regular — batch postings с O(batch) memory;
- unique — external sort/partitioned duplicate detection или temporary
  unique backend с bounded memory;
- progress/checkpoint/cancel;
- write delta catch-up вместо блокировки writers на весь scan.

### P1-5. Loom model не входит в обязательный CI

Loom feature и model добавлены, но комментарии Cargo прямо говорят, что
`./scripts/test.sh` их не запускает. Ни один workflow не вызывает
`--features loom`.

Добавить отдельный trusted CI/nightly job с точной model-командой. Loom не
доказывает weak-memory correctness production atomics автоматически, но
защищает protocol state machine от будущих interleaving regressions.

### P1-6. F66 убрал один mutex, но нормативный invariant ещё не выполнен

Runtime всё ещё содержит:

- `PredicateSet.inner: std::sync::Mutex<Vec<_>>`
  (`shamir-tx/src/predicate_set.rs:44-100`);
- `RepoTxGate.pending_commits: std::sync::Mutex<Vec<_>>`
  (`shamir-tx/src/repo_tx_gate.rs:137-140`).

Комментарии считают их допустимыми short critical sections, но актуальный
`AGENTS.md` требует избегать `std::sync::Mutex`/`RwLock` в runtime hot paths.
Нужно либо мигрировать, либо формально сузить invariant и документировать
разрешённые исключения в одном authoritative файле. Сейчас правила и код
противоречат друг другу.

### P1-7. CHANGELOG не соответствует новой волне

В `[Unreleased]` нет F55–F67. Более того, описание F53d всё ещё утверждает,
что perf gate запускается на PR в `master`, хотя F61 сделал его
`workflow_dispatch`-only.

До тега:

- добавить release notes новой волны;
- исправить trigger/status perf gate;
- явно отметить закрытые и оставшиеся online DDL/AsOf limitations;
- не обещать «full unified lifecycle», пока P0-1 не закрыт;
- убедиться, что tag/version/date согласованы.

## 6. DDL — что развивать

### До первого публичного alpha

1. Единый index lifecycle для regular/unique/sorted/index2.
2. Planner-visible только `Ready`.
3. `DESCRIBE INDEX`/`SHOW INDEX BUILDS`:
   state, operation id, progress, started/ready version, last error.
4. Явные modes:
   - `CREATE INDEX ... OFFLINE`;
   - `CREATE INDEX ... CONCURRENTLY`;
   - reject unsupported safe-concurrent combinations.
5. Idempotency key и retry semantics долгих DDL.
6. Structured DDL errors вместо `Internal(String)`.
7. Dependency-aware `DROP ... RESTRICT/CASCADE`.
8. Crash/cancel tests для каждой state transition.

### После alpha

- versioned `ALTER TABLE`: add/drop/rename field rule, default, nullability;
- `NOT VALID` + отдельный `VALIDATE CONSTRAINT`;
- partial indexes;
- composite sorted indexes и composite keyset bookmark;
- generated fields с immutable expression contract;
- transactional catalogue mutation либо честно non-transactional DDL;
- resumable/checkpointed index builds;
- export/import как обязательный upgrade bridge между alpha formats.

До завершения lifecycle не следует расширять количество index shapes:
каждый новый type умножает уже существующие failure states.

## 7. OQL — приоритет развития

Новая волна не меняла `shamir-query-types`, `shamir-query-builder` или macros,
поэтому прежние функциональные gaps остаются.

### Сначала semantics и operability

1. Выключить/исправить небезопасный AsOf index seek из P0-2/P0-3.
2. Формализовать `Null`/absent/NaN/mixed numeric ordering и three-valued logic.
3. Гарантировать deterministic `(order tuple, RecordId)` tie-breaker во всех
   pagination modes.
4. Добавить query deadline/cancellation, включая WASM/index/backend work.
5. `EXPLAIN ANALYZE`: estimated/actual rows, chosen/rejected index, fallback
   reason, scanned candidates, memory, spill, elapsed stages.
6. Server-side storage streaming для plans без global sort/group materialization.

### Наибольшая продуктовая ценность после этого

1. equi `INNER JOIN` и `LEFT JOIN`;
2. `EXISTS`/semi-join с index nested-loop и hash join;
3. `UNION ALL`, затем UNION/INTERSECT/EXCEPT;
4. scalar/EXISTS subqueries с ограниченной decorrelation;
5. window functions после появления bounded sort/spill;
6. predicate-aware UPSERT/MERGE с явной concurrency semantics.

Для позиционирования рядом с SQLite/PostgreSQL именно JOIN/EXISTS и
предсказуемый planner важнее очередного специализированного index type.

## 8. Query builders — что расширить

### API hardening до публичной фиксации

`CreateIndex` сейчас stringly typed и infallible:

- `index_type`, tokenizer, functional op, metric, quantization — `String`;
- `.build()` не валидирует empty fields, incompatible flags, dimension,
  tokenizer/metric/type combinations;
- DTO документирует, что unknown quantization silently означает unquantized.

См.:

- `shamir-query-builder/src/ddl/create_index.rs:30-169`;
- `shamir-query-types/src/admin/types/index_ops.rs:23-81`.

Рекомендуется:

1. typed enums `IndexKind`, `Tokenizer`, `VectorMetric`, `Quantization`,
   `FunctionalOp`;
2. type-state либо variant-specific builders:
   `BtreeIndexBuilder`, `SortedIndexBuilder`, `VectorIndexBuilder`, etc.;
3. `.try_build() -> Result<BatchOp, BuilderError>` как основной API;
4. compile-time невозможность `sorted + unique + vector`;
5. local validation duplicate/empty paths, include rules, vector dim;
6. typed DDL operation handle/status и typed error codes;
7. typed cursor bookmark с query-shape hash, direction и `RecordId`;
8. parity manifest: каждый public `BatchOp` имеет Rust + TS builder или
   explicit allowlist;
9. golden Rust↔MessagePack↔TS fixtures для каждого DDL family.

У `Query` уже есть `try_build`, но permissive `build` и `From<Query>` остаются
default escape hatch (`query/query.rs:326-410`). Для нового major API лучше
сделать validating terminal нормой, legacy terminal явно назвать
`build_unchecked`.

### После появления engine AST

- join/set-operation builders;
- reusable expression aliases;
- `explain_analyze()`;
- partial-index `.where_(...)`;
- `.concurrently()`/`.offline()`;
- `describe_index`, `show_index_builds`, `cancel/resume_index_build`.

## 9. Производительность — что делать дальше

Порядок важен: сначала correctness gates, затем оптимизация на рабочем
repeatable benchmark host.

### Немедленно измерить

1. F56 writer-drain overhead и cache-line contention.
2. F67 fast-path hit/fallback ratio:
   - no mutation;
   - unrelated index mutation;
   - same index mutation;
   - restart;
   - old AsOf.
3. CREATE INDEX wall time, peak RSS и writer p95/p99.
4. commit-time re-derive с 1/10/100/1000 staged rows.
5. FK fast path: candidates, row re-reads, records scanned, not only ns/op.

### Кандидаты оптимизации

1. packed single-atomic writer gate вместо active counter + пяти flags;
2. streamed legacy index build;
3. index-build delta log/catch-up вместо full-duration writer lock;
4. Top-K: сначала извлекать minimal sort key, а полную SELECT projection
   выполнять только для surviving K. Сейчас scan проектирует каждую
   WHERE-passing row перед `heap.push`
   (`table/read_exec.rs:1033-1069`);
5. filtered sorted-index cursor seek, а не только unfiltered ORDER BY;
6. batch/parallel backend planning при DDL re-derive с bounded concurrency;
7. per-parent/per-child FK cache invalidation вместо whole-repo rebuild, если
   profiles подтвердят значимость;
8. true commit p50/p95/p99 harness: текущий perf gate использует один bulk
   `ns/op` proxy, не latency distribution.

Нельзя принимать optimization по одному числу. Baseline должен хранить runner
fingerprint, несколько independent samples, median и dispersion; rebaseline
должен быть отдельным reviewable diff.

## 10. Release checklist

### Блокирует тег

- [ ] Planner не видит Building/Dropping index.
- [ ] Regular/sorted CREATE concurrent-read tests зелёные.
- [ ] DROP INDEX concurrent-read tests зелёные.
- [ ] Tx sorted epoch публикуется до/атомарно с posting mutation.
- [ ] Epoch корректно инициализируется после CREATE и restart.
- [ ] Old AsOf after CREATE/restart regression tests зелёные.
- [ ] Commit-time re-derive fail closed.
- [ ] Self-hosted perf runner зарегистрирован и hardened.
- [ ] `bench-baseline.json` captured и committed.
- [ ] Perf gate зелёный на frozen release SHA.

### Обязательно перед tag

- [ ] Полный mandatory gate: fmt, clippy, `scripts/test.sh`, integration,
  TS unit/e2e.
- [ ] Отдельный loom job зелёный.
- [ ] Docker boot/health/graceful-stop smoke зелёный.
- [ ] Backup/restore + checksum verification на release binary.
- [ ] Changelog содержит F55–F67 и актуальное описание manual perf trigger.
- [ ] Release limitations честно описывают online DDL и AsOf fallback.
- [ ] Version/tag/changelog section совпадают.
- [ ] SBOM/signatures/checksums созданы и проверены.

### После тега

- [ ] опубликовать reproducible benchmark environment;
- [ ] добавить trusted PR perf approval flow или ephemeral runners;
- [ ] automated dependency/action pin updates;
- [ ] power-loss tests и upgrade/export-import rehearsal;
- [ ] начать JOIN/EXISTS только после стабилизации lifecycle/query semantics.

## 11. Рекомендуемый порядок следующей волны

1. F68: fail-closed commit-time index re-derive.
2. F69: AsOf epoch ordering + restart/create initialization.
3. F70: planner-invisible Building/Dropping для legacy regular/sorted.
4. F71: unified DROP/RENAME lifecycle и cancellation/error semantics.
5. F72: activate perf runner + baseline + loom CI.
6. F73: streamed legacy index build и writer-drain benchmark.
7. F74: typed CreateIndex builders + parity/golden fixtures.
8. Затем OQL JOIN/EXISTS design spike.

## 12. Финальная оценка

По сравнению с `e145b1d3` новая волна заметно повысила качество:

- FK fail-open defects закрыты;
- writer-drain теперь имеет правдоподобное memory-model proof;
- все writer classes подключены к index-create barrier;
- CI supply-chain и self-hosted security стали честнее;
- per-index invalidation — правильное направление.

Главный урок этой волны: **writer serialization не равна index lifecycle**.
Чтобы database могла считаться готовой к публичной alpha, DDL должна защищать
не только snapshot от писателей, но и читателей от partial derived state.
А AsOf optimization должна иметь epoch, отражающий весь возраст current index,
включая CREATE и restart, а не только mutations текущего process.

После закрытия четырёх correctness P0 и ввода perf gate в эксплуатацию первый
публичный alpha выглядит реалистично. До этого релиз будет либо способен
вернуть неполный query result, либо технически не пройдёт собственный release
DAG.
