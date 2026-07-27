# S.H.A.M.I.R. Database — readonly review новой волны перед первым релизом

Дата ревью: 2026-07-27  
Ревьюируемый HEAD: `640bc056fc905beafcbf3364755ba4eecf59e72e`  
База предыдущего ревью: `5270aa92d4a3b3c663186883cb8acb00283cdd5a`  
Размер волны: 61 коммит, 136 файлов, `+16900/-959`  
Режим: только чтение Git и файлов. Сборка, тесты, форматирование и изменение кода не выполнялись.

## 1. Итоговый вердикт

Новая волна сделана не зря: большинство замечаний прошлого ревью действительно
исправлено, а не просто закрыто документацией. Особенно хорошо выглядят:

- явный отказ для неподдержанного `SelectItem::Expression` вместо тихой потери
  поля;
- атомарная lease-модель cursor reaper;
- конечный default для глобального response-byte budget и новые метрики;
- точная проверка заголовка версии в `CHANGELOG.md`;
- расширение `corrupt_records` на byte-level read paths и SDK;
- rollback validator artifact при неудачном ALTER schema;
- `fsync` каталогов backup;
- полноценное протягивание hybrid engine от storage до DDL и restart tests;
- существенное улучшение FK RYOW и попытка закрыть cross-transaction race.

Но текущий HEAD **не готов к первому публичному alpha-релизу**, если в релизе
объявлять schema-typed keyset, FK actions и hybrid engine рабочими production-like
возможностями. Найдены пять классов release blocker:

1. FK `ON UPDATE` не получает обещанный Serializable upgrade.
2. FK reverse cache может опубликовать устаревший snapshot уже после invalidate.
3. `keyset_safe` вычисляется без барьера против конкурентной записи.
4. bootstrap TTL остаётся fail-open при ошибке чтения server metadata.
5. `MirroredStore` теряет атомарность `Store::transact`, на которую полагаются
   index paths.

Дополнительно есть высокоприоритетные проблемы error atomicity hybrid/DDL,
несогласованность `CreateRepo.path`, чрезмерно широкая базовая конфигурация,
дорогие read pipelines и заметный объём внутренних артефактов в будущем
публичном репозитории.

Рекомендуемое решение: не ставить тег, пока не закрыты P0 ниже. Если релиз нужен
немедленно, временно исключить из публично поддерживаемого surface:

- FK actions (или пометить экспериментальными и выключить по умолчанию);
- hybrid engine;
- schema-enabled keyset cursors.

Bootstrap finding нельзя скрыть флагом: его нужно исправить до любого сетевого
релиза.

## 2. Что исправлено корректно

### 2.1 `SelectItem::Expression`

Исправление F-26 корректно меняет опасное поведение: wire DTO всё ещё принимает
`Expression`, но `SelectProjection::new` и aggregate validation возвращают
`select_expression_not_supported`. Это честнее, чем успешный ответ без
вычисленного поля.

Остаток здесь функциональный, не correctness: тип и parser обещают модель,
которой пока нельзя пользоваться. Для alpha допустимо при ясной документации.

### 2.2 Cursor reaper

`CursorRegistry::get_owned_for_fetch` увеличивает `in_flight` под read guard
карты, а `sweep_and_reap` выполняет expiry predicate и удаление в одном
`DashMap::retain`. RAII `FetchLease` закрывает early-return paths. Прежний
check/remove TOCTOU устранён убедительно.

### 2.3 Schema activation rollback

`compile_table_schema` различает fresh registration и ALTER replacement:

- fresh registration удаляется при последующей ошибке;
- ALTER сохраняет старый artifact и возвращает его через
  `replace_artifact`;
- FK cache invalidируется после окончательного success/rollback состояния.

Это закрывает найденную ранее divergence между catalogue и live registry.

### 2.4 Interner rollback для schema DDL

`SetTableSchema`, `AddSchemaRule` и `RemoveSchemaRule` теперь:

1. precompile-ят schema;
2. сохраняют `rec_prev`;
3. пишут catalogue;
4. вызывают `interner_mgr.persist()`;
5. компенсируют catalogue при ошибке;
6. активируют validator и компенсируют catalogue при ошибке activation.

Это существенно лучше прежнего порядка. Ниже остаётся отдельный concurrency
gap, но исправленный interner failure path сам по себе выглядит разумно.

### 2.5 Остальные исправления прошлого отчёта

- `Bin` исключён из keyset-eligible scalar types.
- `corrupt_records` проходит через Rust DTO и TS surface.
- `ORDER BY + with_version + count_total` получил regression coverage.
- `max_inflight_response_bytes` при отсутствующем ключе теперь 256 MiB.
- release workflow требует точный `## [${TAG_VERSION}]`.
- backup делает directory fsync на Unix, включая destination и вложенные
  каталоги.

## 3. Release blockers (P0)

### P0-1. `ON UPDATE` race фактически не закрыт

**Где:**

- `crates/shamir-engine/src/repo/fk_reverse_cache.rs:50-59`
- `crates/shamir-engine/src/repo/fk_reverse_cache.rs:247-270`
- `crates/shamir-engine/src/query/batch/query_runner.rs:330-373`
- `crates/shamir-engine/src/query/batch/fk_on_update.rs:65-88`
- `crates/shamir-engine/src/query/batch/fk_on_update.rs:681-721`

`ReverseFkEntry` хранит одно поле `action`. При построении cache в него всегда
кладётся `fk.on_delete`:

```rust
action: fk.on_delete,
```

Но `implicit_tx_isolation_for_fk_parent` используется и DELETE, и UPDATE и
документирован как проверка non-`NoAction` `on_delete`/**`on_update`**.
Следовательно, FK вида:

```text
on_delete = NoAction
on_update = Restrict | Cascade | SetNull
```

не помечает parent как action-bearing. Неявный UPDATE открывается в `Snapshot`,
хотя `plan_fk_on_update` независимо обнаруживает `on_update` action и выполняет
child-table scan. Конкурентный child insert может попасть между scan и commit.

Иными словами, комментарии заявляют closure, а данные cache отражают только
DELETE-семантику.

Regression tests `fk_race_closure_tests.rs` создают FK через
`ForeignKeyRef::with_on_delete` и не содержат отдельного deterministic
`on_update` race test. Поэтому дефект не ловится текущим proof suite.

**Что сделать:**

- хранить в cache `on_delete` и `on_update` отдельно либо вычислять два role
  flag;
- UPDATE выбирает Serializable по `on_update`, DELETE — по `on_delete`;
- добавить deterministic tests для Restrict/Cascade/SetNull, где
  `on_delete = NoAction`, а `on_update != NoAction`;
- проверить обе очередности commit: child-first и parent-first.

### P0-2. Invalidate FK cache может быть потерян

**Где:** `crates/shamir-engine/src/repo/fk_reverse_cache.rs:120-229`.

`get_or_build_by_parent` реализует обычный cache-aside:

1. читает `None`;
2. асинхронно строит snapshot;
3. без проверки поколения вызывает `populate`.

`invalidate` только делает `state.store(None)`. Возможна последовательность:

1. query A видит cache miss и начинает O(tables) build старой schema;
2. DDL B меняет FK schema и вызывает `invalidate()`;
3. A заканчивает старый build и публикует его через `populate()`;
4. cache снова считается тёплым и может оставаться устаревшим до следующего
   DDL.

Это не только performance stampede. Устаревший cache участвует в выборе
Serializable isolation и child footprint. Он может пропустить новый FK и
снова открыть dangling-reference race.

Также утверждение «build exactly once» неверно: два параллельных miss запускают
два полных scan.

**Что сделать:**

- добавить generation/epoch: `invalidate` увеличивает epoch, builder публикует
  результат только если epoch не изменился;
- либо использовать single-flight rebuild с async mutex/OnceCell на поколение;
- не держать lock во время table scan; после scan выполнить compare-and-publish;
- добавить deterministic test `build paused -> invalidate -> resume old build`,
  доказывающий, что старый snapshot не публикуется;
- лучше возвращать `Arc<[ReverseFkEntry]>`, чтобы cache hit не клонировал все
  строки.

### P0-3. `keyset_safe` имеет writer race

**Где:**

- `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:297-345`
- `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:348-532`
- `crates/shamir-server/src/db_handler/cursor_handlers.rs:418-493`

Исправление F-17 проверяет `table.count() == 0`, но schema RMW mutex
сериализует только schema DDL. Он не блокирует INSERT/UPDATE.

Возможная последовательность:

1. таблица пуста;
2. DDL читает `count == 0` и ставит `keyset_safe = true`;
3. конкурентная запись проходит по старой/no schema и вставляет значение
   неправильного типа;
4. DDL сохраняет и активирует новую schema;
5. cursor gate доверяет `keyset_safe = true`.

Historical homogeneity в этой последовательности не доказана.

**Что сделать:**

- schema activation должна использовать тот же per-table write barrier, что
  unique-index DDL, на всём отрезке `prove current rows -> persist -> activate`;
- ещё лучше не ограничиваться `count == 0`: для post-hoc binding выполнить
  snapshot validation всех существующих rows и активировать schema только при
  успешном scan;
- зафиксировать commit/version watermark proof, чтобы запись не могла попасть
  между validation и activation;
- добавить deterministic paused-DDL/concurrent-insert test.

До исправления проще и безопаснее всегда ставить `keyset_safe = false` для
DDL поверх уже созданной таблицы и разрешать `true` только при CREATE TABLE,
когда таблица ещё недоступна writers.

### P0-4. Bootstrap TTL всё ещё fail-open при metadata read error

**Где:**

- `crates/shamir-server/src/server_meta.rs:456-499`
- `crates/shamir-server/src/connection/handshake.rs:398-439`

Новая проверка правильно отвергает читаемый и истёкший token. Но
`bootstrap_token_active`, `bootstrap_username` и `bootstrap_token_expired`
подавляют `read_blob` errors через `.ok()` и превращают их в `false`/`None`.

После успешной проверки password handshake делает:

```text
metadata read error -> active=false -> expiry check skipped -> session granted
```

То есть F-25 закрывает «expired and readable», но не fail-closed security
contract. Комментарий в handshake прямо признаёт fail-open как pre-existing
behavior. Для bootstrap credential это release blocker, а не допустимый
residual.

**Что сделать:**

- ввести один `read_bootstrap_state() -> Result<BootstrapState, MetaError>`;
- на auth path читать state один раз, а не тремя независимыми getter calls;
- при ошибке metadata после валидного proof возвращать generic auth failure
  или internal-unavailable без выдачи session;
- сохранить constant-time/user-enumeration свойства ответа;
- fault-injection test: валидный, но истёкший bootstrap password + metadata
  read failure никогда не создаёт session.

### P0-5. Hybrid ломает атомарность `Store::transact`

**Где:**

- `crates/shamir-storage/src/storage_mirrored.rs:327-338`
- `crates/shamir-storage/src/types.rs:179-205`
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:862-889`
- `crates/shamir-index/src/legacy/index_manager.rs:686`

`MirroredStore` не переопределяет `transact`; trait default выполняет операции
последовательно через `set/remove` и прямо документирован как **NOT atomic**.

Но index code вызывает `info_store.transact(ops)` именно ради all-or-nothing
семантики. Например, sorted-index rename пишет новый posting и удаляет старый
в одном batch и комментирует это как crash/failure atomicity. На hybrid
`info_store` является `MirroredStore`, поэтому гарантия исчезает.

Даже если postings не классифицируются как durable, live in-memory readers
могут увидеть частично применённый batch, а ошибка посередине оставит
частичный index state.

**Что сделать:**

- реализовать native `MirroredStore::transact`;
- разделить ops на ephemeral и durable subset;
- primary batch должен применяться через атомарный `InMemoryStore::transact`;
- durable subset — через `mirror.transact`;
- заранее определить и документировать failure ordering/compensation между
  двумя независимыми stores;
- fault-injection tests для ошибки после N-й операции и конкурентного reader;
- проверить все вызовы `transact` для `info_store`: legacy, sorted, vector.

## 4. Высокий приоритет (P1)

### P1-1. FK closure остаётся fail-open и неполной для explicit Snapshot

`require_footprint_if_fk_child` при ошибке `resolve_repo`/cache build только
логирует warning и разрешает write без footprint. Parent-side helper при
ошибке возвращает `Snapshot`. Это defense-in-depth по комментарию, но механизм
уже является correctness precondition для обещанной FK atomicity.

Кроме того, `KNOWN_LIMITATIONS.md` оставляет race открытым для explicit
`Snapshot` transaction. Referential integrity не должна зависеть от того,
попросил ли клиент Serializable isolation.

До stable:

- сделать role discovery fail-closed для FK-relevant writes;
- автоматически усиливать explicit parent mutation либо добавлять специальный
  RI barrier, не меняющий весь isolation mode;
- не рекламировать FK actions как полностью закрытые, пока Snapshot path
  сохраняет дыру.

### P1-2. `MirroredStore` расходится с disk state при ошибке mirror

`set` и `remove` сначала мутируют RAM primary, затем mirror. Если mirror
возвращает ошибку, caller видит failure, но процесс продолжает читать уже
изменённое primary state. После restart вернётся старое disk state.

Это нарушает ожидаемую error atomicity DDL:

- API сообщает «операция не выполнена»;
- live process ведёт себя так, будто часть операции выполнена;
- restart снова меняет наблюдаемое состояние.

Нужен чёткий protocol: mirror-first + publish-primary, rollback primary, либо
staged two-phase update. Для config writes предпочтительно durable mirror
commit до публикации в primary.

Отдельно hydration копирует **все** записи mirror, не применяя classifier.
Это доверяет вечному инварианту «на диск никогда не попадал лишний key» и плохо
переживает upgrade classifier/corruption. Для `__info__` стоит фильтровать
hydration текущим allowlist и диагностировать rejected keys.

### P1-3. Index DDL возвращает error после уже выполненного create

**Где:** `table_manager_index_mgmt.rs:437-513`.

`create_index`/`create_unique_index_locked` сначала создают и регистрируют
index, затем вызывают `interner.persist()`. Если persist/flush падает, DDL
возвращает ошибку, но созданный live index не откатывается.

Для hybrid это особенно опасно: index definition ссылается на interner ids,
ради сохранения которых и добавлен persist.

Нужно либо:

- interning + persist делать до публикации index definition;
- либо иметь RAII/compensating rollback index registration и postings;
- fault-injection tests должны проверять не только `Err`, но и отсутствие
  index после ошибки и корректный restart.

### P1-4. `CreateRepoOp.path` и builder `.path()` не имеют server semantics

Wire DTO содержит `path`, Rust builder публично предлагает `.path()`, а
`handle_create_repo` полностью игнорирует `op.path` и всегда строит путь как
`data_root/<db>/<repo>`.

Это тихая contract divergence. Клиент думает, что выбрал storage location, но
сервер отвечает success и использует другой путь.

Перед релизом выбрать одно:

- удалить/запретить `path` на network DDL как небезопасный operator-only knob;
- либо валидировать и реально применять его в разрешённом root;
- при переданном, но неподдержанном `path` возвращать typed error, никогда не
  игнорировать.

### P1-5. Базовый deployment profile слишком широк для заявленной аудитории

`deploy/server.example.ktav` задаёт:

- `max_active_connections = 10000`;
- `max_result_size_bytes = 1 GiB`;
- `max_inflight_response_bytes = 4 GiB`.

Кодовый default безопаснее: 1000 connections, 64 MiB result, 256 MiB
in-flight. Для small/indie продукта base example фактически отменяет safe
defaults и разрешает гигабайтные ответы.

Рекомендуется сделать `server.example.ktav` консервативным (medium/small
profile), а high-capacity значения вынести в явно названный large profile.

### P1-6. Read cursor не является engine cursor

Каждая страница заново выполняет pinned-version query. В keyset path это
повторные scans с растущими tie retries; в offset path — повторная работа от
начала coordinate space. `KNOWN_LIMITATIONS.md` это честно признаёт.

Для небольших наборов допустимо, но на средних проектах latency суммируется
примерно как `pages × scan_cost`. Следующий performance milestone — настоящий
engine continuation:

- pinned iterator/continuation state;
- стабильный `(order_key, RecordId)` tie-breaker;
- bounded page materialization;
- no rescan of already consumed rows.

### P1-7. Top-K всё ещё держит все строки

`read_collecting` сначала формирует полный `qv_result`, и только потом
`apply_order_by_topk(records: Vec<QueryValue>, ...)` строит heap размера K.
Комментарий `O(skip + take) memory` верен только для heap, но не для pipeline:
входной `Vec` уже O(N).

Для реального O(K) projection/order pipeline heap нужно обновлять во время
scan, не после materialization. Аналогично:

- DISTINCT выделяет `IndexSet` на N, keep-mask и новый output;
- GROUP BY/aggregates сохраняют `raw_acc` всех matched rows;
- обычный ORDER BY хранит rows, pre-resolved keys, permutation indices и
  временный `Vec<Option<T>>`.

Приоритет оптимизации:

1. streaming aggregate states;
2. scan-time Top-K;
3. index-covered ORDER/LIMIT;
4. memory budget/spill для unavoidable full sort/group.

### P1-8. Release ещё не воспроизводим из публичного remote state

На момент снимка:

- `master` чистый, но на 89 коммитов впереди `origin/master`;
- `v*` tags отсутствуют;
- `HEAD` не помечен tag;
- `CHANGELOG.md` уже содержит `0.1.0-alpha.1`;
- все 24 Cargo manifests имеют `0.1.0-alpha.1`;
- TS package имеет `0.1.0-alpha.1`, но `shamir-client-node/package.json`
  остаётся `0.1.0`.

Это не code bug, но релиз пока нельзя проверить как immutable remote commit.
Перед tag нужен push, зелёный CI на точном SHA и dry-run release workflow.

## 5. Средний/низкий приоритет (P2)

### P2-1. Финальный docs sweep исправил два stale-пункта, но FK claim требует коррекции

Коммит `640bc056` исправил устаревшие замечания про migration API и
`max_inflight_response_bytes` в base-конфигурации. Это полезная финальная
сверка документации с кодом.

Однако объявленное в `KNOWN_LIMITATIONS.md` полное закрытие FK parent-side
`ON UPDATE` не подтверждается реализацией: reverse cache хранит только
`on_delete`, а explicit Snapshot и cache invalidate/build races остаются.
После исправления P0-1/P0-2 документ нужно снова сверить с фактическими
гарантиями. Для следующих волн стоит добавить automated docs consistency
check или хотя бы обязательный release review всех `STILL OPEN`/`CLOSED`
markers.

### P2-2. Public-repo hygiene

В tracked tree:

- 752 файла в `docs/dev-artifacts`;
- 423 prompt-файла;
- 23 checkpoint-файла;
- 50 research-файлов;
- около 8.4 MiB внутренних docs/checkpoints;
- 2492 tracked files всего;
- в корне tracked `bench-iters.txt` и `cooldown.toml`.

То есть примерно треть файлов — внутренний процесс разработки. Это не мусор в
смысле correctness, но для публичного проекта создаёт шум поиска, увеличивает
clone/review surface и раскрывает внутренние prompts/task IDs.

До public switch определить policy:

- оставить только архитектурные ADR/research с долгосрочной ценностью;
- checkpoints/prompts перенести в отдельный history/archive branch или repo;
- benchmark raw output убрать из root либо положить в versioned benchmark
  dataset с README;
- не удалять историю без отдельного filter-repo плана и backup refs.

### P2-3. Release archives слишком минимальны

Binary archive содержит только `shamir-server`/`.exe`. Для первого публичного
релиза стоит добавить:

- `README`;
- `LICENSE-APACHE`, `LICENSE-MIT`;
- безопасный sample config;
- checksum verification и cosign verification instructions;
- краткий upgrade/backup warning для alpha.

Docker job строит и smoke-test-ит image, но не публикует его в registry. Это
нормально только если официальный Docker image не обещается в alpha.

### P2-4. Supply-chain pinning actions

Workflow использует version tags (`actions/*@vN`,
`Swatinem/rust-cache@v2`, `sigstore/cosign-installer@v3`), а не immutable
commit SHA. Для публичного release pipeline желательно pin-to-SHA с
Dependabot/Renovate updates.

### P2-5. Hybrid cache API и небольшие performance долги

- `stores_list_routed` делает `Vec::contains` для каждого disk store — O(n²),
  пусть число stores обычно мало.
- каждый reverse-FK cache hit клонирует `Vec<ReverseFkEntry>` и все `String`.
- `fk_on_update::discover_on_update_refs` по-прежнему делает O(tables) scan
  на каждый update, хотя cache документация обещает общий двухсторонний map.
- `MirroredStore` оставляет batch methods trait defaults, поэтому кроме
  atomicity теряет native batching и создаёт несколько async/backend
  round-trips.

## 6. DDL: что развивать

### До первого alpha

- Закрыть schema writer barrier и DDL error atomicity.
- Убрать молчаливый `CreateRepo.path`.
- Ввести typed/structured errors для hybrid durability failures.
- Явно перечислить supported engine values в query types, а не передавать
  произвольный `String`.
- Проверить crash semantics CREATE/DROP/RENAME index и schema на hybrid.

### После alpha, до stable

- Настоящие schema migrations: rename/drop field, backfill/default,
  validation phase, resumable progress, rollback.
- Composite unique и composite FK.
- Deferred constraints/constraint validation at commit.
- Transactional DDL или ясно определённая DDL serialization model.
- Rename table с declarative schema вместо текущего запрета.
- Поддерживаемая storage migration: durable coordinator, write interception,
  restart recovery, destination engines кроме `in_memory`.
- Introspection: machine-readable schema/index/constraint status, last
  validation error, rebuild progress.

## 7. OQL/query model: что развивать

### Самый логичный следующий шаг

Реализовать уже существующий `SelectExpr`:

- arithmetic `Add/Sub/Mul/Div`;
- field/literal;
- null propagation;
- numeric promotion и division-by-zero contract;
- alias required/derived-name rules;
- reuse expression evaluator в SELECT, ORDER BY, GROUP BY/HAVING и functional
  indexes там, где это безопасно.

Сейчас wire/parser/type уже существуют, поэтому feature выглядит
«почти поддерживаемой», но всегда падает на execution.

### Более крупные пробелы

- JOIN/semi-join/lookup surface отсутствует; без него позиционирование рядом с
  SQLite/PostgreSQL/MySQL ограничено.
- Нет оконных функций.
- Нет subquery/CTE model.
- Нет server-side prepared query/plan cache contract.
- EXPLAIN является preview; нужны `EXPLAIN ANALYZE`, estimated/actual rows,
  index selectivity, bytes read, spill/top-K indication.
- Нужна явная query capability/version negotiation, чтобы клиенты не строили
  syntactically valid, но runtime-unsupported expressions.

Для alpha не нужно имитировать весь SQL. Практичнее сделать небольшой,
последовательный object-query core: lookup join, computed projections,
streaming aggregates, stable keyset pagination.

## 8. Query builders: что расширить

### Конкретные несоответствия

1. `CreateRepo.engine` — `String`; заменить/добавить `RepoEngine` enum
   (`InMemory`, `Fjall`, `Hybrid`) с explicit escape hatch только при
   необходимости.
2. `.path()` сейчас строит поле, которое server игнорирует. Удалить или
   сделать operator-safe semantics.
3. `OrderByItem.field` — `FieldPath`, но `OrderByItem::asc/desc` и
   `Query::order_by_asc/desc` принимают один `String`, поэтому ergonomic
   builder не умеет nested ORDER BY без ручной сборки DTO. Использовать
   `IntoFieldPath`.
4. Добавить constructors для `SelectExpr` и `select::expr_as`, когда evaluator
   будет готов.
5. `Query::build()` остаётся permissive, а `try_build()` opt-in. Для нового
   major API безопаснее сделать validated build default, legacy path назвать
   `build_unchecked`.
6. Builder validation стоит расширить:
   - empty SELECT;
   - keyset tuple arity vs ORDER BY;
   - `after` без deterministic ORDER BY;
   - aggregate/non-aggregate projection compatibility;
   - alias collisions;
   - unsupported `with_version` combinations;
   - invalid index option combinations;
   - literal-only aggregate function args.
7. Добавить typed return/bookmark helper, который извлекает seek tuple и
   `RecordId` из последней строки, чтобы пользователь не собирал keyset
   bookmark вручную.

## 9. Performance roadmap

### P0/P1 correctness-performance

- Сделать FK cache generation-safe и single-flight.
- Не клонировать cache rows на hit: immutable `Arc` snapshots.
- Объединить on-delete/on-update discovery.
- Вернуть atomic/native batching hybrid store.

### Read path

- scan-time Top-K вместо post-collect heap;
- streaming COUNT/SUM/AVG/MIN/MAX и GROUP BY accumulators;
- true engine cursors;
- RecordId tie-breaker во всех ordered pagination paths;
- index-only projection для covered fields/include paths;
- bounded sort/group memory и spill-to-disk;
- separate corruption side-channel для stream API, если stream станет
  публичным production surface.

### Измерения перед релизом

Нужен не только microbenchmark, но release baseline:

- cold/warm point read;
- full scan 10K/100K/1M;
- ORDER BY LIMIT 10/100;
- GROUP BY cardinality 10/1K/100K;
- cursor traversal до конца;
- concurrent read/write p50/p95/p99;
- FK insert/delete/update under contention;
- hybrid DDL + restart + injected disk failures;
- RSS peak и allocated bytes на каждый сценарий.

Сохранить baseline в компактном machine-readable формате и установить
regression thresholds в CI для нескольких стабильных сценариев, не для
шумных wall-clock абсолютов.

## 10. Release checklist

### Обязательно до tag

- [ ] Исправить `on_update` role flag и добавить race tests.
- [ ] Закрыть invalidate-vs-build race FK cache.
- [ ] Сделать FK role/footprint errors fail-closed либо временно отключить FK
      actions.
- [ ] Закрыть schema `count==0` vs concurrent writer race.
- [ ] Сделать bootstrap metadata read fail-closed.
- [ ] Восстановить atomic `transact` для hybrid.
- [ ] Исправить hybrid primary/mirror error divergence.
- [ ] Сделать index-create + interner-persist error atomic.
- [ ] Удалить/запретить молчаливо игнорируемый `CreateRepo.path`.
- [ ] Сверить FK claims в `KNOWN_LIMITATIONS` после исправления P0-1/P0-2.
- [ ] Снизить base deployment limits.
- [ ] Решить, является ли hybrid публично supported в alpha.

### Release engineering

- [ ] Push точного SHA и зелёный CI на нём.
- [ ] Dry-run tag workflow без публикации либо на throwaway prerelease tag.
- [ ] Проверить Linux/macOS/Windows archives на чистой машине.
- [ ] Проверить backup/restore между двумя независимыми process runs.
- [ ] Проверить cosign bundle verification по опубликованной инструкции.
- [ ] Синхронизировать Node package version или исключить binding из release
      scope явно.
- [ ] Добавить licenses/config/README в archives.
- [ ] Принять решение о Docker registry publication.
- [ ] После всех проверок создать immutable `v0.1.0-alpha.1`.

### Public repository

- [ ] Утвердить, какие dev-artifacts/prompts/checkpoints остаются публичными.
- [ ] Проверить root tracked files и убрать raw benchmark artifacts отдельным
      clean-history решением.
- [ ] Проверить README claims против реального supported surface.
- [ ] Добавить minimal quickstart, compatibility warning, security contact,
      backup-before-upgrade warning.

## 11. Рекомендуемый порядок следующих задач

1. **FK correctness patch:** on-update role + generation-safe cache +
   deterministic races.
2. **Bootstrap fail-closed patch.**
3. **Schema activation write barrier.**
4. **Hybrid atomicity patch:** `transact`, error ordering, hydration filter,
   failure injection.
5. **Index DDL compensation.**
6. **API honesty patch:** `CreateRepo.path`, typed engines, docs consistency.
7. **Release profile hardening.**
8. **Performance wave:** true cursor, streaming aggregates, scan-time Top-K.
9. **Public-repo cleanup and release dry run.**

## 12. Ограничения этого ревью

Это статический readonly аудит. Я не запускал:

- `cargo fmt --all -- --check`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `./scripts/test.sh`;
- TS tests;
- benchmarks;
- fault injection;
- release workflow.

Поэтому вывод «код выглядит исправленным» означает соответствие прочитанным
инвариантам и tests, а не подтверждённый зелёный runtime gate. Перед релизом
обязателен полный gate из `AGENTS.md` на точном tagged SHA.
