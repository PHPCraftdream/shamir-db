# S.H.A.M.I.R. Database — readonly review новой волны перед первым релизом

Дата ревью: 2026-07-28  
Ревьюируемый HEAD: `513757463bf6af4ad2313be3875ad4745367e447`  
База прошлого ревью: `640bc056fc905beafcbf3364755ba4eecf59e72e`  
Размер волны: 27 коммитов, 55 файлов, `+9211/-238`  
Режим: только чтение Git и файлов. Сборка, тесты, clippy, fmt, benchmark и
изменения production-кода не выполнялись.

Рабочее дерево во время снимка уже содержало параллельную работу:

- удаление
  `crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_spike_tests.rs`;
- новый `docs/checkpoints/2026-07-28-0100.md`.

Они не входят в `HEAD`, не изменялись и не учитывались как завершённая часть
волны. Единственное изменение этого ревью — данный отчёт.

## 1. Итоговый вердикт

Волна полезная и в целом движется в правильном направлении: исправлены
несколько реальных ошибок, добавлены хорошие fault-injection/race-тесты,
ужесточены failure paths и конфигурация. Но вывод «все прежние блокеры
закрыты» пока неверен.

Первый публичный alpha-релиз на этом HEAD выпускать не рекомендуется. Остались
как минимум пять correctness-блокеров:

1. RI/FK barrier сериализует parent-side validator, но не сериализует
   concurrent FK-child writer с тем же `commit_lock`;
2. reverse-FK cache всё ещё может опубликовать stale snapshot после
   invalidation — это прямо признано в комментарии реализации;
3. schema activation barrier не дренирует writer, который успел прочитать
   старый `false` до поднятия флага;
4. `MirroredStore::transact` изменяет live primary до durable commit и
   возвращает `Err` с уже изменённым live-состоянием;
5. online index creation всё ещё допускает пропуск postings у транзакций,
   staged до регистрации backend.

То есть общая архитектурная идея исправлений верна, но в трёх местах
реализован только «reader sees completed writer» сценарий, а не полная
двусторонняя сериализация validate/publish window.

### Что можно считать действительно улучшенным

- `ReverseFkEntry` теперь независимо хранит `on_delete` и `on_update`.
- Bootstrap TTL metadata read в handshake стал fail-closed.
- Ошибка persist interner больше не публикует legacy/sorted index до persist.
- `CreateRepo.path` больше не игнорируется сервером молча.
- `set`/`remove` durable keys в `MirroredStore` переведены на mirror-first.
- Базовые connection/result/inflight limits стали разумнее.
- FK discovery errors больше не деградируют молча в permissive path.
- Добавлен typed `RepoEngine`, хотя builder API ещё требует доводки.

## 2. Матрица задач новой волны

| Задача | Результат ревью | Вывод |
|---|---|---|
| F-35, `on_update` role flag | Реализация разделяет `on_delete`/`on_update`, query runner выбирает правильный flag | Закрыто |
| F-36, generation-safe FK cache | Single-flight и generation retry полезны, но compare→publish residual оставлен открытым | Частично |
| F-37, schema write barrier | Блокирует writers, начавшихся после flag-up; уже прошедший fast-path не дренируется | Частично |
| F-38, bootstrap fail-closed | Один fallible metadata read до выдачи session; ошибки отклоняют login | Закрыто в заявленном scope |
| F-39, mirrored transact | Durable subset atomic на mirror, но live primary мутирует до mirror commit | Частично |
| F-40, discovery fail-closed | Направление fallback корректное | Закрыто, но зависит от незакрытого RI-протокола |
| F-41, mirror-first | Правильно для одиночных `set`/`remove`; `transact` остался primary-first | Частично |
| F-42, index/interner ordering | Persist failure теперь происходит до publish | Закрыт узкий failure path; online-build races не закрыты |
| F-43, repo path/engine | Сервер честно отвергает `path`; builder всё ещё предлагает `.path()` | Сервер закрыт, SDK частично |
| F-44, conservative profile | Connection/result limits исправлены; Argon2 ceiling остался 8 GiB | Частично |
| F-45, docs sync | Документация обновлена, но теперь слишком сильно заявляет FK closure | Требует повторной правки |
| F-40b, explicit Snapshot RI barrier | Покрывает writer, успевший полностью commit до parent validation; forward race остаётся | Частично, release blocker |

## 3. Release blockers (P0)

### P0-1. FK RI barrier не образует взаимную commit-сериализацию

`TxContext::record_ri_barrier` записывает child-table token, а parent
transaction с непустым `ri_barrier_tokens` берёт `gate.commit_lock()`:

- `crates/shamir-tx/src/tx_context.rs:539-570`;
- `crates/shamir-engine/src/tx/commit.rs:741-755`.

Однако Snapshot writer в FK-child table получает только
`footprint_tokens` через `require_footprint_for`. Условие взятия
`commit_lock` в `commit_tx_lockfree` проверяет:

- `Serializable`;
- непустой `cas_set`;
- непустой `ri_barrier_tokens`.

`footprint_tokens` в условии отсутствует. Поэтому child writer может
проходить validate→WAL→`record_commit_writes`→publish параллельно с
parent-side RI validator, даже когда parent держит `commit_lock`.

Контрпример:

1. Parent Snapshot tx сканирует child table и записывает RI token.
2. Parent входит в commit, берёт `commit_lock`.
3. Parent выполняет `predicate_conflicts_batch`; новых footprints ещё нет.
4. Child Snapshot writer, не берущий `commit_lock`, получает commit version.
5. Child пишет WAL, публикует footprint и строку.
6. Parent после уже пройденной проверки пишет WAL и удаляет/изменяет parent.
7. Обе операции успешно committed; RESTRICT/CASCADE/SET NULL/ON UPDATE
   решение было принято по устаревшему набору child rows.

Новые тесты не покрывают этот порядок. Их injected writer выполняется **до
полного commit** в after-scan/before-parent-commit seam, и только потом parent
начинает commit validation:

- `fk_ri_barrier_tests.rs:136-150`;
- `fk_ri_barrier_tests.rs:253-286`;
- `fk_race_closure_tests.rs:193-213`.

Это доказывает backward-looking recheck, но не concurrent publish после
recheck.

Что сделать:

- минимум: все transactions с непустым `footprint_tokens` должны участвовать
  в том же validate→publish serialization protocol;
- предпочтительно выделить отдельный per-repo/per-relation RI epoch/barrier,
  чтобы обычный global commit lock не стал bottleneck;
- добавить deterministic pause seam **после parent
  `predicate_conflicts_batch` и до parent publish**, затем запустить child
  writer;
- проверить оба commit order, все четыре FK actions, implicit и explicit
  Snapshot, AsyncIndex visibility и recovery.

До этого нельзя утверждать, что «whichever side commits first wins».

### P0-2. F-36 оставляет stale reverse-FK cache publish

Реализация сама описывает residual:

`crates/shamir-engine/src/repo/fk_reverse_cache.rs:220-237`.

Generation читается после scan, затем отдельно вызывается `populate`, который
делает `ArcSwap::store`. Между compare и store возможен порядок:

1. builder видит generation `G`;
2. DDL вызывает `invalidate`: increment generation, затем `state=None`;
3. builder выполняет `populate(Some(stale_G))`;
4. stale state остаётся активным до следующего DDL.

Фраза «bounded by the next invalidate» не делает это безопасным: следующего
DDL может не быть никогда. Cache определяет:

- является ли table FK child;
- требуется ли footprint;
- надо ли upgrade implicit parent mutation;
- какие reverse actions нужно исполнить.

Следовательно, stale publish может снова разрешить dangling FK или выполнить
неактуальную action.

Что сделать:

- хранить generation и state в одном versioned `ArcSwap` snapshot;
- публиковать build через compare-and-swap относительно exact generation;
- при CAS failure отбрасывать build и повторять scan;
- либо сериализовать invalidate и publish одним async-compatible writer
  protocol;
- тест должен ставить pause именно между final generation load и state
  publish.

Текущее название/документация «generation-safe» сильнее фактической гарантии.

### P0-3. F-37 schema barrier имеет check-before-flag race

DDL делает:

1. берёт `unique_write_lock`;
2. поднимает `schema_activation_barrier`;
3. выполняет count→persist→activate.

Writer делает условный fast-path:

1. читает `needs_write_barrier()`;
2. если `false`, не берёт lock;
3. продолжает validate/write.

Ссылки:

- `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:105-155`;
- `crates/shamir-engine/src/table/table_manager.rs:1138-1172`.

Открытый порядок:

1. Writer читает flag=`false`, но ещё не записывает строку.
2. DDL берёт lock и поднимает flag.
3. DDL получает `count()==0`.
4. Writer, уже выбравший lock-free branch, вставляет строку.
5. DDL persist/activate выставляет `keyset_safe=true`.

Новые тесты запускают writer только после поднятия flag и поэтому не
воспроизводят этот случай (`schema_activation_barrier_tests.rs:64-146`).

Это тот же check-then-act residual, который уже честно описан для
`index2_create_barrier` в
`table_manager_index_mgmt.rs:407-416`.

Что сделать:

- reader/writer epoch protocol с active-writer counter;
- либо seqlock: writer читает epoch до работы, публикует mutation и проверяет
  epoch после; при смене — сериализуется/retries;
- либо единый async RW barrier, где DDL берёт write guard, а каждый writer —
  дешёвый read participation guard;
- тест: writer paused сразу после false-check, DDL проходит count, writer
  продолжает.

Просто менять ordering «сначала flag, потом lock» недостаточно: это создаст
другие окна и не дренирует уже вошедших writers.

### P0-4. `MirroredStore::transact` всё ещё error-non-atomic для live state

Одиночные classified `set`/`remove` теперь правильно делают mirror-first:

- `storage_mirrored.rs:351-364`;
- `storage_mirrored.rs:381-390`.

Но `transact` применяет durable operations к primary **до**
`mirror.transact`:

- primary apply: `storage_mirrored.rs:561-573`;
- durable commit: `storage_mirrored.rs:575-579`.

Если mirror возвращает `Err`, caller видит failure, но live process уже читает
новые config/index metadata из primary. Restart откатывает их к старому
durable state. Комментарий называет это приемлемым, но для DDL это
противоречит обычному контракту `Result`: операция не может одновременно
вернуть ошибку и остаться live-visible.

Это особенно опасно для index rename/metadata transactions, ради которых
override и добавлялся.

Что сделать:

- durable subset: сначала `mirror.transact`, затем infallible apply to primary;
- для согласованного read publish — собрать новый primary snapshot и
  RCU-swapнуть его после durable success;
- если mixed ephemeral/durable transact нужен, явно определить общий
  atomicity contract; иначе запретить mixed batches;
- fault tests должны после mirror failure проверять и immediate live reads, и
  reopen.

### P0-5. Online index build остаётся некорректным

Production-комментарии прямо фиксируют два открытых окна:

- tx staged до backend registration несёт старый `index_write_set` и после
  регистрации коммитит row без posting;
- writer, прочитавший barrier=false до flag-up, не дренируется.

См.
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs:357-424`.

F-42 исправляет только interner persist failure ordering. Он не закрывает:

- backfill/register race;
- stale staged index-op plan;
- cancellation/partial backfill;
- atomic publication index metadata + complete postings.

Sorted index дополнительно документирован как `cancel-safe: NO` и регистрирует
definition до завершения backfill:
`table_manager_sorted_index.rs:8-17`, `:75-98`.

Что сделать перед релизом:

- ввести lifecycle `Building -> CatchingUp -> Ready` в persisted metadata;
- во время build либо dual-write в building backend, либо replay delta по
  version range;
- commit-time re-derive index ops against current backend generation;
- planner использует только `Ready`;
- crash/restart продолжает или откатывает build;
- DDL cancellation не оставляет partially-queryable index;
- добавить race tests для stage-before-build/commit-after-ready и
  writer-passed-fast-path-before-flag.

Если это не успевает в alpha, публичный online `CREATE INDEX` надо временно
ограничить пустой/offline table либо явно требовать maintenance write stop.

## 4. Высокий приоритет (P1)

### P1-1. FK barrier слишком coarse и использует запрещённый sync mutex

`ri_barrier_tokens` реализован как `std::sync::Mutex<TFxSet<u64>>` в
`TxContext` и несколько раз lock/unlock-ится на commit path.

Это противоречит repo invariant из `AGENTS.md`: engine runtime hot paths
должны избегать `std::sync::Mutex`/`RwLock`. Обоснование «low frequency» не
снимает риск blocking/poisoning в transaction core.

Кроме того, dependency имеет форму полного `TableScan`. Любая запись в child
table конфликтует с parent action, даже если меняет другой FK, другое значение
или вообще нерелевантное поле. После симметричного исправления P0-1 это может
сильно снизить concurrency на популярных child tables.

Следующая итерация:

- lock-free/scc set либо owned immutable token vec после planning;
- dependency `(child_table, child_field, referenced_value/range)`;
- index-backed FK probe должен записывать key/range predicate, а не полный
  table scan;
- метрики RI retries/conflicts отдельно от generic phantom conflicts.

### P1-2. Base config всё ещё способен зарезервировать 8 GiB под Argon2

F-44 снизил connections и response budgets, но оставил:

- `memory_kb=131072`;
- `argon2_concurrent_max=64`.

Потенциальный auth-RAM ceiling — `64 * 128 MiB = 8 GiB`. Это прямо написано в
`deploy/server.example.ktav:14-29`. Для файла, который предлагается
копировать первым, профиль нельзя называть conservative.

Рекомендация:

- base: 4–8 concurrent Argon2 jobs в зависимости от заявленного minimum host;
- large profile также не должен переносить число 64 без привязки к RAM;
- config validation/warning: вычислять ceiling и сравнивать с operator
  memory budget;
- добавить отдельные `small`, `medium`, `large` sizing examples и таблицу
  ожидаемой RAM.

### P1-3. `CreateRepo.path` убран не до конца

Сервер теперь правильно возвращает `unsupported_field`, но Rust builder всё
ещё публично предлагает:

`crates/shamir-query-builder/src/ddl/create_repo.rs:80-83`.

Док-комментарий «Set the data path» обещает рабочую возможность, которая
гарантированно отвергается сервером.

Перед release:

- удалить метод до первого опубликованного API или пометить compile-time
  deprecated с точным объяснением;
- лучше убрать `path` из нового wire version, оставив legacy decode;
- если feature планируется, нужен allowlist под `data_root`, canonicalization,
  symlink/traversal policy и ACL.

### P1-4. `KNOWN_LIMITATIONS.md` переутверждает FK guarantee

Документ честно упоминает narrow cache residual, но одновременно заявляет:

- race «CLOSED»;
- «whichever side commits first wins»;
- explicit Snapshot gap «CLOSED too».

См. `KNOWN_LIMITATIONS.md:91-204`.

После P0-1/P0-2 эти claims нужно переписать. До исправления использовать
формулировку «mitigated; two forward/invalidation races remain».

### P1-5. Top-K экономит heap, но не O(N) materialization

Read pipeline сначала складывает все matched projected rows в `rec_acc`,
затем превращает их в полный `qv_result`, и только после этого передаёт Vec в
`apply_order_by_topk`:

- accumulation: `crates/shamir-engine/src/table/read_exec.rs:970-1059`;
- heap call: `read_exec.rs:1067-1108`.

Поэтому комментарий «O(K) memory instead of O(N)» относится лишь к sort
workspace, но не к query memory. Реальная память всё ещё O(N), плюс проекция
всех строк.

Нужно сливать WHERE→projection→sort-key прямо в bounded heap во время stream.
Для `count_total` можно вести счётчик отдельно; для `with_version` heap item
может нести `RecordId`.

### P1-6. Cursor — повторный полный pinned scan на каждую страницу

`KNOWN_LIMITATIONS.md:358-369` это правильно признаёт. Cursor снижает wire и
client memory, но не server CPU/IO. Offset fallback особенно дорог:
каждая следующая page пересканирует и пересортировывает всё до offset.

Нужно:

- настоящий engine cursor/continuation;
- index seek по composite `(order_key, RecordId)`;
- pinned MVCC iterator или resumable range handle;
- direct streaming response chunks с bounded serialization buffer.

### P1-7. Release state пока не immutable

На снимке:

- `master` ahead of `origin/master` на 117 коммитов;
- release tags отсутствуют;
- Cargo crates и TS package имеют `0.1.0-alpha.1`;
- `crates/shamir-client-node/package.json` остаётся `0.1.0`;
- workflow version-consistency проверяет workspace Cargo/CHANGELOG, но Node
  package не виден в текущем consistency contract.

Перед tag нужен push exact reviewed SHA, зелёный remote CI и release dry-run
на throwaway prerelease tag.

### P1-8. Release archives содержат только binary

`.github/workflows/release.yml:428-448` намеренно кладёт в archive только
`shamir-server`. Пользователь не получает рядом:

- `LICENSE-MIT`, `LICENSE-APACHE`;
- README/quickstart;
- sample safe config;
- checksum verification/signature instructions;
- third-party notices.

SBOM/signatures в workflow — сильная сторона, но binary archive должен быть
самодостаточным юридически и операционно.

### P1-9. GitHub Actions используют mutable major/version tags

Workflow использует `actions/checkout@v6.0.3`,
`Swatinem/rust-cache@v2`, `actions/upload-artifact@v4`,
`softprops/action-gh-release@v2` и другие tags, а не immutable commit SHAs.

Для supply-chain-sensitive database release:

- pin third-party actions на full SHA;
- оставить version comment рядом;
- Dependabot обновляет SHA отдельными PR;
- проверить permissions каждого job.

## 5. Средний приоритет и code debt (P2)

### P2-1. Production group-commit implementation выглядит отключённым

`commit_tx_inner` после AsyncIndex branch всегда вызывает
`commit_tx_lockfree`; production references к `run_leader`/pending queue не
обнаружены, кроме тестов и самого модуля. При этом F-40b расширяет
`group_commit.rs` как будто это активный commit route.

Нужно решить:

- вернуть group commit в production и benchmark-нуть;
- либо удалить/feature-gate dead implementation;
- не поддерживать correctness logic в недостижимом path параллельно с двумя
  реальными pre-commit paths.

### P2-2. Public-repo hygiene всё ещё тяжёлая

Tracked tree содержит:

- 768 файлов в `docs/dev-artifacts`;
- 436 prompt-файлов;
- 23 checkpoints;
- 53 research-файла;
- около 8.6 MB внутренних artifacts/checkpoints;
- 2516 tracked files всего;
- root `bench-iters.txt` и `cooldown.toml`.

Перед public release стоит вынести prompts/checkpoints/research в отдельный
engineering-history archive или оставить curated subset. Это уменьшит шум,
случайную публикацию внутренних данных и размер source distribution.

### P2-3. `MirroredStore` docs расходятся с реализацией

Комментарий у default bulk methods всё ещё говорит
«primary-then-conditional-mirror», хотя F-41 перевёл single writes на
mirror-first (`storage_mirrored.rs:441-450`).

Также термин «atomic-ish» для `transact` слишком мягок для storage API.
Контракт должен отдельно описывать:

- durable atomicity;
- live visibility atomicity;
- error atomicity;
- mixed classified/unclassified batch semantics.

### P2-4. FK discovery и actions остаются scan-heavy

Cold reverse-cache miss делает O(number of tables) schema scan. Затем
RESTRICT/CASCADE/SET NULL могут делать full child-table scans; cascade
повторяет это по уровням.

Нужно:

- persisted reverse-FK catalog вместо runtime full discovery;
- обязательный supporting index уже проверяется DDL — planner должен
  использовать его для всех scalar-compatible FK actions;
- batch lookup old parent values;
- collect affected IDs через postings, не через повторный stream;
- cache action plans by schema generation.

### P2-5. Документация содержит очень длинные change-history comments в hot code

Некоторые production functions имеют сотни строк исторической аргументации,
task IDs и расследований. Это помогло аудиту, но усложняет чтение инварианта.

Перед public release:

- оставить рядом короткий invariant + failure contract;
- подробные доказательства перенести в ADR/research;
- ссылки делать на stable ADR title, а не internal F-number.

## 6. DDL: что развить

### Обязательно до release

1. Исправить online index lifecycle из P0-5.
2. Исправить schema activation protocol из P0-3.
3. Сделать DDL operation status/recovery для долгих create/rebuild.
4. `DESCRIBE` должен показывать index state: building/ready/failed.
5. Добавить `VALIDATE INDEX`/`REINDEX` и doctor check postings vs rows.

### Сильный post-alpha roadmap

1. Schema evolution:
   - rename field;
   - add/drop field с default/backfill;
   - change type через explicit transform;
   - dry-run validation и migration progress.
2. Composite constraints:
   - multi-field UNIQUE;
   - composite FK;
   - named constraints;
   - deferred validation.
3. Index DDL:
   - partial indexes (`WHERE`);
   - TTL/expiration indexes;
   - unique+sorted combination;
   - multiple vector indexes per table;
   - index options as typed enums.
4. Transactional DDL либо чёткий DDL transaction journal с idempotent recovery.
5. Persisted system catalog с schema/index generation и dependency graph.
6. Расширить hybrid/storage migration только после write interception,
   durable coordinator и restart recovery.

Не следует пытаться сразу копировать весь SQL DDL. Для alpha важнее небольшой,
но crash/race-safe набор операций.

## 7. OQL/query language: что развить

### Высокая ценность

1. Реализовать `SelectItem::Expression`, уже присутствующий в wire DTO и
   parser, используя существующий `$expr` evaluator.
2. Добавить `EXPLAIN ANALYZE`:
   - selected plan/index;
   - rows scanned/matched/returned;
   - bytes decoded;
   - sort/top-K memory;
   - FK/index probes;
   - elapsed time per stage.
3. Настоящий streaming/resumable execution plan для cursors.
4. Явный semi-join/lookup primitive для common relational access. Полный SQL
   JOIN engine можно отложить, но batch references не заменяют удобный
   indexed one-to-many lookup.
5. Поддержать expressions в projection/order/group/having единообразно.
6. Capability introspection: клиент может узнать, какие filter/expression/
   cursor/index features поддерживает server protocol version.

### Позже

- UNION/UNION ALL;
- window functions;
- recursive graph traversal;
- durable materialized views;
- query plan hints только после стабильного cost model.

## 8. Query builders: что расширить

### Нужные исправления API до публикации

1. Удалить/депрекейтнуть `CreateRepo::path`.
2. Сделать один общий fallible contract:
   - сейчас `Query::build` lenient, `try_build` validating;
   - `Update/Delete/AddSchemaRule` fallible;
   - `Insert/CreateIndex/CreateRepo` infallible.
3. Добавить `try_build`/`BuildError` всем DDL и writes и рекомендовать его в
   docs; infallible `build` можно оставить только как явно unchecked legacy.
4. Проверять client-side:
   - пустой insert;
   - index без fields;
   - `unique && sorted`;
   - vector options без vector type;
   - zero vector dimension;
   - FTS options на non-FTS index;
   - пустые field paths/names;
   - unsupported CreateRepo path.
5. `order_by_asc/desc` должны принимать `IntoFieldPath`, как `select` и
   `group_by`, а не flat `Into<String>`.
6. Typed enums:
   - index kind;
   - tokenizer/language;
   - vector metric/quantization;
   - schema type tag/compare op/format;
   - storage engine без обязательного raw-string path.
7. Builder-generated operations, которые server гарантированно отвергает,
   должны отклоняться client-side с тем же stable error code.
8. Добавить ergonomic helpers для:
   - computed select expression + alias;
   - multi-column ORDER BY с null ordering;
   - cursor creation/continuation;
   - explain/analyze;
   - conflict retry policy.

### Не раздувать API

`RepoEngine::Other(String)` полезен для forward compatibility, но docs должны
различать:

- checked known variants;
- unchecked raw escape hatch.

Сейчас `.engine(impl Into<String>)` всё ещё принимает любой string, поэтому
наличие enum само по себе не делает выбор compile-checked.

## 9. Производительность: приоритетный план

### P0/P1 correctness-first performance

1. Исправить RI serialization без превращения всех FK-child writes в один
   global repo mutex.
2. Versioned CAS snapshot для reverse-FK cache.
3. Stream rows непосредственно в top-K heap.
4. Commit-time regenerate index ops только при смене backend generation.
5. Mirror durable commit до live publish.

### Query execution

1. Push LIMIT и ORDER BY в sorted/index2 scan.
2. Covering index должен обходить record fetch, когда projection полностью
   покрыта.
3. Aggregates:
   - streaming accumulators;
   - spill или bounded group cardinality;
   - index min/max/count fast paths.
4. Избежать repeated RecordView/QueryValue materialization между filter,
   projection, sort и serialization.
5. Cursors — direct seek, не re-run whole query per page.

### FK

1. Reverse catalog + direct index lookup.
2. Group probes по `(child_table, child_field)`.
3. Deduplicate values до lookup.
4. Predicate dependency уровня key/range вместо table token.
5. Отдельные benchmark suites:
   - no-FK write;
   - one FK indexed;
   - high fan-out cascade;
   - concurrent parent delete/child insert;
   - DDL invalidation storm.

### Release performance gates

Зафиксировать reproducible baselines для:

- point get/set;
- 1k/100k scan;
- indexed equality/range;
- ORDER BY LIMIT K;
- transaction commit p50/p95/p99;
- concurrent FK workload;
- cursor pages 1/10/100;
- startup/recovery;
- backup/restore;
- hybrid config writes.

Регрессия должна оцениваться относительно pinned machine/profile, а не
случайного локального `bench-iters.txt`.

## 10. Release engineering и публичная готовность

### Что уже хорошо

- multi-OS fmt/clippy/tests;
- integration и TS gates;
- version consistency;
- Docker smoke;
- SHA-256 archives;
- CycloneDX SBOM;
- keyless cosign artifacts;
- prerelease GitHub Release.

### Что сделать перед tag

- [ ] Закрыть P0-1 — mutual RI serialization.
- [ ] Закрыть P0-2 — atomic cache generation publish.
- [ ] Закрыть P0-3 — drain pre-flag writers.
- [ ] Закрыть P0-4 — hybrid transact error atomicity.
- [ ] Закрыть/ограничить P0-5 — online index build.
- [ ] Исправить FK claims в `KNOWN_LIMITATIONS`.
- [ ] Снизить Argon2 concurrency в base profile.
- [ ] Удалить unsupported `.path()` из builder.
- [ ] Синхронизировать Node package version или явно исключить его из release.
- [ ] Добавить licenses/README/config в binary archives.
- [ ] Pin Actions на immutable SHAs.
- [ ] Curate public internal artifacts/root benchmark files.
- [ ] Завершить текущую параллельную работу и получить clean tree.
- [ ] Выполнить обязательные repo gates:
  `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh`.
- [ ] Выполнить race/stress/loom-equivalent suites на exact candidate SHA.
- [ ] Push exact SHA; дождаться зелёного remote CI.
- [ ] Dry-run release workflow на disposable prerelease tag.
- [ ] Проверить скачанный archive на чистой Linux/macOS/Windows машине.
- [ ] Проверить checksum, cosign bundle, SBOM и license presence.
- [ ] Только после этого создать `v0.1.0-alpha.1`.

## 11. Рекомендуемый порядок следующей волны

1. F-46: RI mutual serialization + adversarial post-recheck race tests.
2. F-47: atomic versioned FK cache publication.
3. F-48: schema/index DDL writer-drain protocol.
4. F-49: hybrid transact mirror-first + live RCU publish.
5. F-50: index build lifecycle or temporary offline-only restriction.
6. F-51: docs/config/builder truthfulness sweep.
7. F-52: release packaging/version/action pinning.
8. F-53: streaming top-K and cursor/index seek performance wave.

Каждая correctness-задача должна начинаться с adversarial red test на
конкретный interleaving, а не только с happy path и writer-completed-before-
validator сценария.

## 12. Позиционирование

Текущий README правильно отказался от заявления о drop-in replacement для
SQLite/MySQL/PostgreSQL/MongoDB/Redis/Memcached. Это нужно сохранить.

Реалистичное alpha-позиционирование:

> self-contained Rust database/runtime для небольших и независимых проектов,
> которым важны единый binary, document records, async API, WASM logic,
> authenticated networking и экспериментальные P2P/replication возможности.

До широкой формулировки «замена SQLite» особенно нужны:

- безусловно корректный index DDL;
- компактный embedded API story;
- crash/recovery matrix;
- predictable migrations/upgrades;
- true streaming;
- долгие compatibility guarantees.

До формулировок о замене server RDBMS/Redis/Memcached нужны существенно более
широкие workload-specific guarantees и benchmarks. Сравнение должно
оставаться use-case based, а не feature-count based.

## 13. Ограничения этого ревью

Из-за readonly-режима выводы основаны на committed Git diff и статическом
чтении файлов. Не выполнялись:

- Rust/TS compile;
- fmt/clippy;
- unit/integration/e2e tests;
- Miri/Loom/sanitizers/fuzz;
- benchmarks/profiling;
- release workflow;
- runtime crash/power-loss injection.

Поэтому отчёт способен найти ошибки протокола и API-контракта, но не заменяет
зелёный release gate. Положительная оценка отдельного изменения означает
«статически выглядит корректно в заявленном scope», а не подтверждение
исполнением.
