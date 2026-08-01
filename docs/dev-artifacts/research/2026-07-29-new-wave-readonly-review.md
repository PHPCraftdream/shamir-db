# S.H.A.M.I.R. Database — readonly review новой волны перед первым релизом

Дата: 2026-07-29  
Режим: только чтение Git и файлов; production-код, конфигурация и история не изменялись  
Зафиксированный snapshot: `e145b1d36477391abae30d374c2cea6dfd07ed6d`  
База предыдущего review: `513757463bf6af4ad2313be3875ad4745367e447`

## 1. Краткий вердикт

Новая волна заметно улучшила СУБД, но `e145b1d3` пока не следует помечать
первым публичным alpha-релизом.

Из 42 новых коммитов несколько действительно закрывают прежние выводы:

- F-46 симметрично включает FK-child writers в repo `commit_lock`;
- F-47 правильно объединяет generation и payload reverse-FK cache в одну
  CAS-публикацию через `ArcSwap`;
- F-51 убирает ложный `CreateRepo::path` builder и исправляет описание
  Argon2-профилей;
- F-52 существенно улучшает release archives, permissions и SHA pinning;
- F-53a ограничивает память простого `ORDER BY ... LIMIT`;
- F-53c добавляет полезный индексный путь для FK actions;
- F-54 удаляет действительно недостижимый group-commit код.

Но обнаружены пять database correctness blockers и два release-engineering
blockers:

1. reverse-FK discovery всё ещё молча пропускает таблицу при ошибке resolve;
2. writer-drain не имеет заявленного memory-ordering доказательства;
3. online index build закрыт не для всех путей и даже `index2` не вызывает
   созданный drain;
4. AsOf cursor index seek имеет check-then-use race с concurrent mutation;
5. `MirroredStore::transact` по-прежнему возвращает `Err` после видимых
   ephemeral mutations;
6. perf gate не может пройти: обязательного `bench-baseline.json` нет;
7. `pull_request` исполняет недоверенный код на self-hosted runner, что опасно
   при переводе репозитория в public.

Итог: **NO-GO для тега до закрытия P0 ниже**. Для закрытой alpha после этих
исправлений архитектурное направление выглядит убедительно.

## 2. Границы и объём

Между предыдущей базой и snapshot:

- 42 коммита;
- 117 изменённых файлов;
- 18 212 добавлений, 1 847 удалений;
- основные темы: F-46…F-54, release CI, cursor seek, FK actions,
  online-index lifecycle, top-K.

Во время review ветка двигалась. После исходно исследованного `2fa8bba9`
появился только документационный коммит `e145b1d3`; production diff между
ними отсутствует. Поэтому отчёт зафиксирован на `e145b1d3`.

На момент финальной проверки worktree был чистым. Никакие build/test/bench
команды не запускались: это сознательное ограничение readonly review и
параллельной работы в репозитории. Выводы основаны на статическом чтении
реальных production paths, тестов, workflow и Git diff.

## 3. Матрица новой волны

| Волна | Оценка | Что закрыто | Что осталось |
|---|---|---|---|
| F-46 RI mutual serialization | Хорошо, но не вся FK-система | FK-child Snapshot writer теперь берёт тот же `commit_lock` | discovery может построить неполную карту; lock очень coarse |
| F-47 atomic FK cache | Закрывает заявленную CAS-гонку | generation + cache публикуются одним atomic state | builder может успешно закэшировать неполный scan |
| F-48/F-48b writer drain | Правильная идея, неполное доказательство | RAII counter проведён через non-tx и tx materialize | ordering недостаточен; index2 не вызывает drain |
| F-49 mirror-first | Частичное закрытие | durable subset больше не публикуется в primary до mirror success | mixed batch мутирует ephemeral subset до возможного `Err` |
| F-50 index lifecycle | Сильный частичный результат | stage-plan generation, Building/Ready для index2, recovery | legacy/sorted builds остаются unsafe; index2 drain не подключён |
| F-51 deployment/builder | Закрыто | Argon2 docs и `CreateRepo::path` исправлены | нужен общий builder parity gate |
| F-52 release hardening | В основном хорошо | archives, checksums/SBOM/signatures, permissions, большинство action pins | `dtolnay/rust-toolchain@1.93.0` не immutable SHA |
| F-53a streaming top-K | Хорошо в заявленном scope | heap ограничен `OFFSET + LIMIT` во время scan | всё ещё O(N) scan; DISTINCT/GROUP/aggregate materialize |
| F-53b AsOf seek | Производительный, но unsafe | happy path O(page), tie-breaker и fallback wiring | high-water check не атомарен со scan |
| F-53c FK indexed actions | Хорошо с оговорками | CASCADE/SET NULL избегают scan при чистом tx overlay | discovery fail-open; coarse lock; нужен perf proof |
| F-53d perf CI | Пока не operational | workload list и runbook появились | baseline отсутствует; parser fail-open; public self-hosted risk |
| F-54 dead group commit | Закрыто | удалён дублирующий недостижимый pipeline | отдельный batching roadmap возможен позже |

## 4. Release blockers — P0

### P0-1. Reverse-FK discovery остаётся fail-open

`build_reverse_fk_entries` заявлен как repo-wide authoritative scan, но
`crates/shamir-engine/src/repo/fk_reverse_cache.rs:498` делает:

```rust
let child_table = match resolver.resolve(&child_ref).await {
    Ok(t) => t,
    Err(_) => continue,
};
```

То есть transient/storage/catalogue error на одной child table превращается
не в ошибку cache build, а в «у этой таблицы нет foreign keys». Получившийся
неполный список успешно публикуется и обслуживает последующие RESTRICT,
CASCADE и SET NULL до следующей invalidation.

Это также обесценивает F-40 fail-closed branch в
`query_runner::require_footprint_if_fk_child`: функция расширяет footprint
только когда `get_or_build_by_parent` вернул `Err`, но внутренний resolve
error уже поглощён и наружу приходит `Ok(partial_cache)`.

Отдельный uncached путь `discover_on_update_refs` повторяет тот же шаблон в
`crates/shamir-engine/src/query/batch/fk_on_update.rs:745`.

Контрпример:

1. repo содержит `parent` и FK-child `child`;
2. cache cold;
3. `resolve(child)` временно ошибается;
4. cache успешно публикуется без ссылки `child -> parent`;
5. parent UPDATE/DELETE не видит child rows и может нарушить RI;
6. cache остаётся тёплым и неполным.

Что сделать:

- любое `resolve` failure во время authoritative FK discovery
  пропагировать как `Err`;
- не публиковать partial cache;
- унифицировать ON UPDATE discovery с тем же reverse cache;
- добавить fault-injection test: одна таблица resolve-error, parent mutation
  обязана fail closed;
- разделить `not found because concurrent DROP` и реальную I/O/catalogue
  ошибку только если существует проверяемый catalogue epoch.

### P0-2. WriterDrainBarrier не доказан на weak memory

В `writer_drain_barrier.rs` используются:

- `active.fetch_add(1, Relaxed)` — строка 108;
- barrier flag: отдельный atomic с Release/Acquire;
- `active.load(Acquire)` — строка 124;
- `active.fetch_sub(Release)` — строка 159.

Комментарий утверждает, что coherence chain отдельного barrier flag переносит
happens-before от relaxed increment к чтению `active`. В модели памяти Rust
такой межатомарной синхронизации нет: writer читает старое `false`, поэтому
его Acquire load не synchronizes-with более поздним Release store `true`.
Modification order одного `flag` не заставляет drainer увидеть increment
другого atomic `active`.

Допустим interleaving:

```text
writer                         DDL
active += 1 (Relaxed)
flag.load(Acquire) -> false
                               flag.store(true, Release)
                               active.load(Acquire) -> старое 0
                               DDL продолжает snapshot/proof
writer выполняет write
```

Тесты с `Notify` доказывают конкретный scheduler interleaving на текущем CPU,
но не исключают разрешённое weak-memory наблюдение.

Нужен один из формально корректных протоколов:

- SeqCst ordering на входе/flag/drain с чётким proof;
- seqlock/epoch handshake с обязательным writer re-check после регистрации;
- gate object, где writer либо зарегистрирован до закрытия gate, либо после
  re-check переходит на slow lock;
- loom-модель, перебирающая ordering, до принятия протокола.

Просто заменить `Relaxed` на `Release` недостаточно: Release без
synchronizes-with reader всё ещё не создаёт нужный happens-before.

### P0-3. Online CREATE INDEX остаётся некорректным

#### Index2 не вызывает существующий drain

`create_index_v2` берёт `unique_write_lock` и поднимает
`index2_create_barrier`, но в
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs:29-360` нет
вызова `drain_writers()`.

Документация primitive и schema DDL прямо говорит, что F-50 подключит этот
вызов. Реально `rg "drain_writers"` показывает production call только в
schema activation (`admin_schema.rs:164`).

Поэтому writer, который зарегистрировался/прочитал barrier до его поднятия,
может закончить store write после того, как index backfill уже прошёл его
позицию. Commit-time re-derive помогает staged tx, чей index generation
изменился, но не заменяет drain для всех write classes и не исправляет
snapshot boundary самого backfill.

#### Regular hash index

`create_index`:

1. собирает `collect_all_current_records`;
2. только затем вызывает `create_index_from_records`;
3. не поднимает intent flag, не берёт lock и не drain-ит writers.

Запись между snapshot и registration отсутствует и в backfill set, и в live
write hook. Комментарий про register-first Option A не соответствует
показанному control flow.

#### Unique index

`create_unique_index` берёт `unique_write_lock`, но до регистрации первого
unique index `needs_write_barrier()` возвращает false. Нет отдельного
`unique_index_create_barrier`, поэтому существующие fast-path writers не
обязаны брать этот lock. В результате возможны как пропущенные postings, так
и duplicate, вставленный после backfill uniqueness proof.

#### Sorted index

`table_manager_sorted_index.rs:10-16` честно документирует:

- register происходит до backfill;
- cancellation оставляет partial live index;
- операция «Do NOT call under timeout».

Она не использует общий DDL barrier/lifecycle. F-50 re-derive sorted ops
закрывает только staged-before-create/commit-after-create miss, но не
register-before-backfill visibility, cancellation и concurrent reader,
который увидит частичный индекс.

Что сделать:

- создать один `OnlineIndexBuildGuard` для regular/unique/sorted/index2;
- под lock поднять gate, корректно drain-ить pre-gate writers;
- planner допускает только `Ready`;
- durable lifecycle минимум `Building -> Ready`, плюс `Failed/Dropping`;
- во время Building live writers должны dual-write либо ждать;
- backfill snapshot и delta catch-up должны иметь одну формальную границу;
- cancellation/error должны либо rollback definition/postings, либо оставить
  явно видимый resumable job, но не «CREATE вернул Err, индекс позже ожил»;
- adversarial tests для каждого index kind: writer до flag, во время
  backfill, staged tx, cancellation, storage error, restart.

До этого безопасная alpha-альтернатива — разрешать `CREATE INDEX` только на
пустой таблице или в явном offline maintenance mode без writers/readers.

### P0-4. AsOf index seek имеет TOCTOU между high-water check и scan

`read_temporal.rs:97` сначала проверяет:

```rust
last_mutation_version() <= pinned_version
```

а затем отдельно запускает current-state sorted-index walk.

Между этими событиями concurrent UPDATE/DELETE может изменить postings.
High-water bump выполняется после apply postings. Нет lock, epoch pin или
повторной проверки, связывающей predicate и весь scan в один consistent
interval.

Per-candidate classifier не закрывает это:

- UPDATE может удалить old posting и переместить row за текущий scan range;
- DELETE полностью удаляет posting;
- отсутствующий candidate никогда не попадёт в `version_of/get_at`, поэтому
  `concurrent_modified` не увеличится;
- page может тихо пропустить row, который существовал в pinned snapshot.

Существующие negative tests изменяют index до high-water gate и подтверждают
fallback. Нужен тест, который паркует read **после gate, до/во время index
walk**, затем делает UPDATE/DELETE.

Варианты исправления:

- seqlock: считать epoch до scan, после scan и принимать page только если
  epoch одинаковый и не odd/in-progress;
- удерживать read epoch/RCU snapshot immutable index root;
- versioned postings;
- как временная мера отключить AsOf index seek и оставить корректный
  full-scan cursor.

### P0-5. `MirroredStore::transact` не соблюдает error atomicity mixed batch

F-49 исправляет durable subset: mirror теперь commit-ится до durable primary
publish. Но полный порядок остаётся:

1. ephemeral ops применяются к primary;
2. durable ops отправляются в mirror;
3. mirror может вернуть `Err`;
4. caller получает ошибку, хотя ephemeral mutations уже видимы.

Это закреплено тестом
`transact_ephemeral_applied_but_durable_aborted_on_mirror_failure`.

Между тем storage README описывает `transact` как atomic mixed-op batch, а
transactional engine использует этот API для commit bundling. Нельзя
одновременно возвращать `Err` и считать, что batch не состоялся, если его
часть уже опубликована.

Нужно выбрать и зафиксировать контракт:

- запретить mixed classified batch и разделять его до transactional boundary;
- сначала durable mirror, затем infallible primary для обоих subsets, если
  semantics допускает;
- ввести MVCC/root-swap primary, поддерживающий atomic mixed publish;
- либо убрать claim об atomic override и не использовать MirroredStore там,
  где engine требует cross-op atomicity.

Для релиза недостаточно только документации residual: это observable
семантика ошибки.

## 5. Release-engineering blockers

### P0-R1. Perf gate сейчас гарантированно красный

`scripts/bench_gate.sh` требует committed root `bench-baseline.json`.
На snapshot такого tracked или untracked файла нет. В gate mode скрипт после
всех дорогих benchmark runs завершится:

```text
no baseline found ... run with --capture-baseline first
```

Кроме того, parser fail-open:

- успешный bench с изменившимся stdout format может дать ноль JSON rows;
- отсутствие ожидаемого cell в свежем output не считается ошибкой;
- новая/переименованная cell без baseline только печатается как `(new)` и не
  gate-ится.

Что сделать:

- снять baseline на том самом dedicated runner;
- commit-нуть baseline вместе с machine fingerprint/toolchain/profile;
- проверить exact set: все 9 ожидаемых keys появились ровно один раз;
- missing/duplicate/new key считать ошибкой, если нет explicit baseline
  update mode;
- хранить raw output как artifact;
- gate должен сравнивать release-profile numbers либо честно называться
  opt-level-1 regression sentinel.

### P0-R2. Public PR нельзя безусловно запускать на persistent self-hosted runner

`.github/workflows/perf-gate.yml` срабатывает на `pull_request` в `master` и
исполняет checkout-код на `[self-hosted, shamir-bench]`.

После открытия repository любой fork PR фактически получает code execution
на benchmark host. Read-only GitHub token не защищает локальную машину,
network credentials, соседние jobs, caches и persistence runner-а.

До public:

- использовать ephemeral одноразовый runner без секретов и доверенной сети;
- либо запускать benchmark только после maintainer-approved label через
  отдельный trusted workflow, который benchmark-ит immutable PR SHA;
- не использовать `pull_request_target` с checkout недоверенного head в
  привилегированном context;
- очистить workspace/caches между jobs;
- документировать threat model и runner rebuild.

Также workflow сам сообщает, что runner пока не зарегистрирован и job будет
вечно Queued. Это operational blocker branch protection.

### P1-R3. Perf gate не входит в tag release DAG

`release.yml` не содержит perf job в `needs`. Прямой push `v*` способен
выпустить artifact без performance gate. Branch protection косвенно помогает
только если tag гарантированно ставится на защищённый merge commit.

Нужен explicit release provenance check: tag SHA должен быть commit,
прошедший required perf run с тем же SHA и baseline version.

### P1-R4. SHA pinning почти полное, но не полное

Большинство Actions pinned на 40-char SHA. Все workflow всё ещё используют
`dtolnay/rust-toolchain@1.93.0`, то есть mutable action ref. Toolchain version
зафиксирован, action implementation — нет.

Нужно pin action commit SHA и оставить комментарий с toolchain/tag.

### P1-R5. Version/changelog/tag state не согласован

Все Rust crates имеют `0.1.0-alpha.1` и `publish = false`. CHANGELOG содержит
датированный раздел `[0.1.0-alpha.1] - 2026-07-26`, а новая волна находится в
`[Unreleased]`. Но Git tag `v0.1.0-alpha.1` отсутствует; единственный видимый
tag — backup ref истории.

До первого релиза выбрать один вариант:

1. если alpha.1 ещё не выпускалась — объединить текущий Unreleased в alpha.1,
   поставить реальную дату и tag после фиксов;
2. если alpha.1 считается выпущенной вне Git — восстановить immutable tag на
   правильном старом SHA, bump workspace/README/CHANGELOG до alpha.2.

Нельзя тегировать текущий SHA как alpha.1, оставив его изменения в
`Unreleased`: опубликованный artifact и changelog будут расходиться.

## 6. Высокий приоритет — P1

### P1-1. FK commit serialization слишком coarse

F-46 correctness-oriented и разумен как срочное закрытие, но теперь любой
Snapshot writer в любую FK-child table берёт общий repo `commit_lock`.
Независимые child tables и разные keys сериализуются.

После correctness fix стоит заменить table-wide footprint на versioned
per-child-table epoch/range dependency либо sharded barrier. Измерять:

- 1/2/4/8 concurrent writers;
- одна hot child table против разных child tables;
- FK и non-FK table;
- p50/p95/p99 commit latency, abort rate.

### P1-2. `ri_barrier_tokens` нарушает заявленный lock-free стиль

`TxContext` хранит `std::sync::Mutex<TFxSet<u64>>`, а commit path несколько
раз делает `.lock().unwrap()`. Guard не живёт через await и contention обычно
низкий, но это прямо расходится с AGENTS invariant для engine runtime и имеет
poisoning semantics.

Можно изменить API FK scan так, чтобы он получал `&mut TxContext`, либо
использовать `scc`/small lock-free set, либо собирать deps в operation-local
state и merge-ить при staging.

### P1-3. Index2 lifecycle имеет неоднозначную DDL error semantics

До backfill durable metadata уже содержит `Building`. При backfill error или
cancellation CREATE возвращает ошибку, но restart recovery способен
самостоятельно достроить индекс. После live registration `set_state(Ready)`
предшествует финальному metadata save; save failure возвращает Err при уже
живом Ready index.

Это полезно для self-healing, но не атомарная DDL semantics. Нужны:

- operation/build id;
- `SHOW INDEX BUILD`/`DESCRIBE INDEX`;
- terminal `Failed` и explicit resume/drop;
- точное правило: accepted async job или synchronous CREATE;
- idempotency key для retry.

### P1-4. Mutation high-water слишком глобален

`SortedIndexManager::last_mutation_version()` выглядит manager-wide, хотя
seek планирует конкретный index. Изменение другого sorted index отключит
seek для всех cursors этой table. После P0-4 нужен per-index epoch, иначе
fast path будет деградировать сильнее необходимого.

### P1-5. Top-K решает память, но не CPU/I/O

F-53a — хорошее исправление: простой finite `ORDER BY` больше не держит все
matched rows. Но full scan и projection каждого match остаются O(N), даже
когда sorted index способен вернуть K строк.

Следующий шаг — planner path `ORDER BY indexed_field LIMIT/OFFSET`:

- ordered index walk;
- residual filter;
- early stop после `offset + limit`;
- covering projection из included fields;
- fallback для unsupported shape.

### P1-6. FK indexed action не должен тихо скрывать read errors

В fast paths candidate re-read использует конструкции вида
`Ok(Some(bytes)) => bytes, _ => continue`. Storage/decode error и row absent
объединяются. После authoritative index lookup реальная read error должна
прерывать RI operation, а не уменьшать action set.

### P1-7. Код перегружен change-history комментариями

В hot production files комментарии F-xx/A9/#534 местами длиннее
реализации и уже устарели: например, `backfill_index2_backend` всё ещё
описывает Step 2/3 как будущие, хотя коммиты присутствуют; документация
`drain_writers` утверждает, что F-50 вызывает его, но call отсутствует.

После correctness wave сделать отдельный docs/refactor commit:

- оставить invariants, preconditions, failure semantics;
- историю решений перенести в ADR/research docs;
- добавить ссылки на stable ADR, а не narrative всей кампании;
- не смешивать cleanup с исправлениями.

## 7. DDL — что развивать

### Обязательно до release

1. Единый корректный lifecycle online index build для всех index families.
2. Явная DDL atomicity/error/cancellation semantics.
3. `DESCRIBE INDEX` со state, progress, last error, build generation.
4. Structured error codes вместо `Internal(String)` для invalid index type,
   duplicate, unsupported combination и build failure.
5. Запрет или offline-only fallback для shapes, которые пока нельзя строить
   безопасно.

### Сразу после alpha

- `ALTER TABLE` как versioned schema evolution:
  add/drop/rename field rule, defaults, nullability, validation mode;
- `VALIDATE CONSTRAINT` отдельно от объявления constraint;
- partial indexes (`WHERE predicate`);
- composite sorted indexes и составной keyset bookmark;
- generated columns с immutable expression contract;
- transactional catalogue mutation или explicit non-transactional DDL;
- idempotency key и operation status для долгих DDL;
- dependency-aware `DROP ... RESTRICT/CASCADE`;
- schema/index version в EXPLAIN и diagnostics;
- export/import как обязательный upgrade bridge между alpha formats.

Не стоит добавлять больше DDL surface, пока CREATE INDEX не имеет единой
семантики: breadth поверх ненадёжного lifecycle увеличит число failure modes.

## 8. OQL — что развивать

### Наибольшая продуктовая ценность

1. **JOIN/EXISTS/semi-join.** Это главный функциональный разрыв относительно
   relational DB. Начать с equi inner/left join с hash/index nested-loop и
   строгими budgets.
2. **Set operations:** UNION ALL, затем UNION/INTERSECT/EXCEPT.
3. **Scalar и EXISTS subqueries** с decorrelation только для безопасных
   shapes.
4. **Window functions:** `row_number`, `rank`, `lag/lead` после устойчивой
   sort/spill infrastructure.
5. **MERGE/upsert с predicate** и ясной concurrency semantics.

### Исправить раньше широкой функциональности

- ORDER BY должен уметь сортировать по полю, не включённому в SELECT;
- формализовать NULL/absent/NaN ordering и three-valued filter logic;
- stable deterministic tie-breaker во всех paginated orderings;
- `EXPLAIN ANALYZE` с estimated/actual rows, chosen index, fallback reason,
  memory and spill;
- query cancellation/deadline, включая WASM/scalar/index operations;
- server-side result streaming для shapes без global materialization.

`ForEach` уже даёт контролируемую data-dependent composition. Добавлять
универсальный `while` перед budgets, cancellation и static cost model не
следует: это превращает query API в потенциально неограниченный interpreter.

## 9. Query builders — что расширить

### API fixes до публичной фиксации

- заменить строковые `index_type`, tokenizer, vector metric,
  functional operation и FK action на typed enums;
- не допускать противоречивые `sorted + unique + index_type=vector`
  комбинации конструкцией типов;
- унифицировать `.build() -> Result<BatchOp, BuilderError>` для builders с
  обязательными/взаимоисключающими полями;
- локально проверять empty fields, duplicate include, invalid vector dim,
  unsupported composite sorted index;
- ввести typed cursor bookmark `(order values, RecordId, direction,
  query-shape hash)`, а не заставлять пользователя вручную собирать After;
- typed decoding response/error codes для DDL и cursor fallback;
- parity test: каждый public `BatchOp` либо имеет Rust builder и TS builder,
  либо внесён в explicit allowlist;
- golden MessagePack round-trip Rust builder -> wire DTO -> TS fixture.

### Полезные расширения

- composable JOIN/set-operation builders после появления engine AST;
- reusable expression aliases;
- `explain_analyze()`;
- schema builder для named constraints и generated fields;
- `create_index(...).partial(filter)` и `.concurrently()/offline()`;
- `describe_index`, `show_index_builds`, `resume_index_build`.

Макросы не должны скрывать network/DDL side effects. Лучше сохранить
явный builder для lifecycle операций и использовать macros только для
expressions/filters.

## 10. Производительность — следующий рациональный план

### Сначала correctness-preserving infrastructure

1. Loom-модель writer/drain gate.
2. Per-index mutation epoch/seqlock для cursor seek.
3. Один index-build delta protocol для всех backends.
4. Fail-closed FK cache и per-table epochs.

### Затем измеряемые оптимизации

1. Индексный early-stop для ORDER BY LIMIT.
2. FK action benchmark должен показывать не только ns/op, но и
   `records_scanned/index candidates/read count`.
3. Уменьшить общий FK commit-lock scope.
4. Убрать повторное projection/serialization в top-K: heap должен хранить
   sort key + minimal row handle, projection — только для финальных K, если
   semantics позволяет.
5. Covering sorted indexes: реально читать included values без table fetch.
6. Cursor pages: benchmark глубины 1/10/100/1000 и fallback after mutation.
7. DDL benchmarks с concurrent writers и tail latency, а не только quiescent
   create time.
8. Recovery benchmarks должны проверять correctness digest, не только время.

Perf gate должен иметь:

- release-like profile;
- runner fingerprint;
- warmup и несколько independent samples;
- median + dispersion;
- p95/p99 для commit path отдельным harness;
- correctness counters;
- обязательную полноту всех cells;
- baseline update review diff.

## 11. Release/public checklist

### Блокирующее

- [ ] P0-1: FK discovery fail closed.
- [ ] P0-2: формально корректный writer-drain + loom.
- [ ] P0-3: безопасный CREATE INDEX либо release-time restriction.
- [ ] P0-4: закрыть/выключить AsOf index seek race.
- [ ] P0-5: определить и обеспечить mixed transact error atomicity.
- [ ] Добавить и проверить perf baseline.
- [ ] Не запускать public fork code на persistent self-hosted runner.

### Обязательное перед tag

- [ ] Freeze release candidate SHA.
- [ ] `cargo fmt --all -- --check`.
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- [ ] `./scripts/test.sh --locked`.
- [ ] integration, TS unit/e2e, restart/durability suites.
- [ ] adversarial race tests из P0.
- [ ] benchmark gate на том же frozen SHA.
- [ ] version/changelog/tag consistency.
- [ ] pin `dtolnay/rust-toolchain` action implementation.
- [ ] проверить branch protection и required checks.
- [ ] secret/history scan перед public switch.
- [ ] build archives локально/в CI и smoke-test распакованного содержимого.
- [ ] проверить checksums, SBOM и cosign verification по инструкции.
- [ ] disaster-recovery drill: backup, corrupt/kill, restore, digest compare.

### Документация release notes

README правильно смягчил позиционирование: S.H.A.M.I.R. не объявляется
drop-in заменой PostgreSQL/MySQL/MongoDB/Redis/Memcached. Это следует
сохранить.

Для alpha отдельно и заметно указать:

- один process/node, без mature clustering/sharding;
- нет SQL и JOIN parity;
- alpha storage/wire compatibility без in-place upgrade guarantee;
- experimental migration API disabled by default;
- ограничения online DDL до их закрытия;
- independent backups обязательны;
- performance numbers привязаны к hardware/profile/dataset.

## 12. Рекомендуемый порядок следующей волны

1. F-55: fail-closed FK discovery + read-error propagation.
2. F-56: writer-drain protocol proof + loom + подключение ко всем DDL.
3. F-57: единый online index lifecycle; временно restrict unsafe CREATE.
4. F-58: AsOf seek seqlock/epoch или feature-disable.
5. F-59: MirroredStore mixed transact contract.
6. F-60: harden perf gate, baseline, public self-hosted isolation.
7. Release candidate freeze и полный gate.
8. Только затем OQL/builder breadth.

## 13. Финальная оценка

Новая волна сделана не зря: она закрыла несколько реальных дыр и заметно
подняла уровень кода. Особенно хороши atomic cache publish, stage-plan
re-derive, bounded top-K, FK indexed candidate path и release packaging.

Главная проблема — несколько исправлений доказали happy path, но не замкнули
весь concurrency protocol:

- drain существует, но его ordering и index wiring неполны;
- high-water существует, но check не связан атомарно со scan;
- index lifecycle существует только для части index families;
- mirror-first существует только для durable subset;
- fail-closed wrapper существует, но inner discovery поглощает error.

После закрытия этих границ проект можно честно выпускать как раннюю alpha
для экспериментального embedded/self-hosted применения. До этого публичный
релиз создаст слишком сильное впечатление надёжности для кода, в котором
ещё есть воспроизводимые пути к нарушению RI, пропущенным index rows и
неконсистентной snapshot pagination.

## 14. Ограничения review

- Код не собирался и тесты/benchmarks не запускались.
- GitHub branch protection, registered runners, secrets и фактические CI
  результаты локально не видны.
- Power-fail/fsync guarantees нельзя доказать одним static review.
- Внешние зависимости и CVE state не перепроверялись через сеть.
- Выводы относятся строго к snapshot `e145b1d3`.
