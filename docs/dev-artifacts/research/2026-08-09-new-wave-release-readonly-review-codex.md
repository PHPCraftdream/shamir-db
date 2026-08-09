# S.H.A.M.I.R. Database — Codex readonly review новой волны перед первым релизом

Дата среза: 2026-08-09  
Репозиторий: `D:\dev\rust\shamir-db`  
Проверенный `HEAD`: `0936a302c49838652000eb44afae75bfd717bf23` (`master`)  
База сравнения: предыдущий Codex-review `37cc59a3`  
Диапазон новой волны: 70 коммитов, 281 изменённый файл, `+24832/-1878`  
Режим: только чтение исходников, документации и Git. Сборка и тесты не запускались.

## Короткий итог

Вердикт для публикации `v0.1.0-alpha.1` на этом срезе: **NO-GO**.

Новая волна в целом сильная: прежние проблемы с поколениями registry, частичной публикацией backend, межсемейной уникальностью имён, DDL admission, stale transaction plans, DROP-vs-readers и fail-closed recovery в основном исправлены по существу. Это уже не тот код, который был на прошлом review.

Но до тега остаются три release-blocking области:

1. `BumpFtsStats` не принадлежит конкретному FTS backend и при commit рассылается всем index2 backend. При двух FTS-индексах BM25-статистика умножается, а stale bump переживает DROP/recreate. Это тихо искажает ранжирование.
2. DDL operation-status реализован не как надёжная state machine: `InProgress` нигде не пишется, terminal status записывается после удаления recovery tombstone, ошибка записи статуса проглатывается, а выданный сервером `op_id` клиент узнаёт только после успешного ответа. Главный сценарий — потерянный ответ или crash — этим контрактом не разрешается.
3. `GetDdlOpStatus` обходит ACL, а index2 RENAME меняет live registry до durable metadata write и не имеет tombstone/rollback. Ошибка storage или cancellation может оставить runtime и диск с разными именами.

Дополнительно релизу не хватает достоверного performance/quality evidence: Windows CI stall остаётся не локализован, автоматического perf gate нет, baseline отсутствует, а HNSW recall gate был снижен до `0.60` при документации, обещающей `>=0.90` после restart и примерно `95–99%` в обычном HNSW-режиме.

## Что в новой волне сделано правильно

### Состояние выводов предыдущего Codex-review

| Предыдущий вывод | Текущее состояние | Комментарий |
|---|---|---|
| P0-1: неверное поколение `IndexRegistry` | Закрыт | Карты публикуются перед generation watermark; последовательный ABA-сценарий покрыт тестом. API всё ещё полагается на внешний `ddl_admission`, но production callers в просмотренных путях сериализованы. |
| P0-2: sorted RENAME не меняет поколение | Закрыт | Добавлены generation bump и instance provenance. |
| P0-3: reconcile не удаляет stale index ops | Почти закрыт | `SetPosting`/`RemovePosting` получили provenance и stale retraction. Исключение — `BumpFtsStats`, это новый P0 ниже. |
| P0-4: частичная вставка backend в registry | Закрыт | Проверка `by_name` выполняется до публикации `by_id`. Ошибки duplicate persisted name больше не проглатываются на open. |
| P0-5: имена не уникальны между семьями | Закрыт для новых DDL | CREATE preflight выполняется под `ddl_admission`; legacy collisions обнаруживаются и отклоняются в DROP/RENAME. |
| P0-6: DROP не ждёт уже начавшихся readers | Закрыт по correctness | `ReaderDrainGate`/lease введён для regular, sorted и index2; unique использует существующую сериализацию. Performance-цена рассмотрена ниже. |
| P0-7: DDL admission покрывает не весь DDL | Закрыт в просмотренных путях | CREATE/DROP/RENAME четырёх семейств проходят через общую сериализацию и write barriers. |
| P0-8: recovery оставляет неполный backend Ready | Закрыт | Ошибки `drop_all`/`restore_on_open` переводят backend в `Failed`, planner его не видит. |
| P1-1: doctor не является операторской функцией | Закрыт | Добавлен offline `shamir-server doctor`. |
| P1-2: unified DDL result contract отсутствует | Частично | Wire/API появились, но durable semantics и ACL пока неверны. |
| P1-3: обычный Batch builder может panic | В основном закрыт | Fallible builders больше не входят в `IntoBatchOp`; добавлены `TryIntoBatchOp` и `try_*`. Остались отдельные panic/round-trip API, но не основной путь. |
| P1-4: `CreateIndex` stringly typed/fail-open | Существенно улучшен | Есть typed constructors и `IndexSpec`; legacy `.build()` и `IntoBatchOp` всё ещё lenient. |
| P1-5: CREATE INDEX блокирует writes весь backfill | Открыт | Есть подробный draft RFC, реализации нет. |
| P1-6: CI не зелёный/Windows timeout | Не закрыт доказательно | Добавлена диагностика, root cause не найден. |

### Другие хорошие изменения

- Unique index теперь обнаруживает duplicate claims внутри одной транзакции до commit.
- DROP INDEX разрешает реальное семейство по catalog, а не доверяет устаревшему клиентскому `unique` hint.
- `SelectItem::Expression` проходит через единый projection choke point, поэтому работает одинаково для full scan, index paths, temporal reads и cursor paths.
- Rust и TypeScript клиенты получили DDL status API; TS получил удобные `selectExpr` constructors.
- Версии workspace и `CHANGELOG.md` приведены к `0.1.0-alpha.1`; release workflow содержит fmt, clippy, multi-OS tests, TS e2e, version consistency, artifacts, Docker smoke, SBOM и signing.
- README теперь честно говорит, что проект не является drop-in заменой PostgreSQL/MySQL/MongoDB/Redis/Memcached. Для публичного alpha это правильное позиционирование.

## P0-1. BM25-статистика смешивается между FTS-индексами

### Доказательство

`IndexWriteOp::SetPosting` и `RemovePosting` несут:

```text
(family, name_interned, instance_epoch)
```

но `BumpFtsStats { doc_len, sign }` provenance не имеет:

- `crates/shamir-tx/src/index_write_op.rs:90-117` — форма вариантов;
- `crates/shamir-tx/src/index_write_op.rs:130-136` — `set_provenance` для bump является no-op;
- `crates/shamir-engine/src/tx/pre_commit.rs:1296-1314` — stale retraction всегда сохраняет bump;
- `crates/shamir-index/src/fts_ranked_backend.rs:148-235` — каждый ranked FTS backend создаёт свой bump;
- `crates/shamir-index/src/write_ops.rs:106-165` — все bump собираются и broadcast-ятся всем index2 backend;
- `crates/shamir-index/src/fts_ranked_backend.rs:240-270` — каждый FTS backend применяет каждый полученный bump.

### Ошибочное поведение

Если на таблице два FTS-индекса и вставляется один документ:

1. каждый backend планирует `BumpFtsStats(+1)`;
2. commit получает два bump;
3. оба bump отправляются обоим FTS backend;
4. у каждого backend `doc_count` увеличивается на 2 вместо 1.

При `N` FTS-индексах получается `N²` применений вместо `N`. Posting keys остаются корректными, поэтому обычные equality/contains тесты могут быть зелёными, но IDF, average document length и итоговый `$score` неверны.

Есть и ABA-вариант:

1. транзакция stage-ит запись для FTS `search` instance A;
2. параллельно индекс удаляется и создаётся заново как instance B;
3. stale posting ops A отбрасываются provenance-фильтром;
4. stale `BumpFtsStats` сохраняется и применяется instance B.

### Обязательное исправление

- Добавить bump ту же provenance или стабильный backend identity (`index_id` + `instance_epoch`).
- На commit группировать in-memory ops по владельцу, а не broadcast-ить всем backend.
- В `retract_stale_provenance_ops` удалять stale bump так же, как posting ops.
- Не использовать одно только имя: DROP/recreate с тем же именем требует epoch.

### Минимальные regression tests

1. Два FTS-индекса на разных полях, один insert: `doc_count == 1` в каждом.
2. Два FTS-индекса, update одного поля: статистика меняется только у владельца соответствующих ops.
3. Stage tx -> DROP FTS -> CREATE FTS с тем же именем -> commit: новый backend не получает stale bump.
4. Те же сценарии для delete и abort.
5. Проверять не только счётчик, но и BM25 ranking против независимого reference calculation.

До этого исправления FTS ranking нельзя объявлять корректным.

## P0-2. DDL status contract не разрешает crash ambiguity

Название «unified DDL result contract» сейчас шире реальной реализации.

### `InProgress` существует только в типах

`DdlOpState::InProgress` документирован как состояние, записываемое до первой мутации (`crates/shamir-query-types/src/read/ddl.rs:79-92`), но в production-коде нет ни одного `DdlOpState::InProgress`.

`admin_result_with_op_id` вызывается только двумя handler:

- DROP INDEX;
- RENAME INDEX.

CREATE variants существуют в `DdlOpKind`, но production-кода, который их пишет, нет. CREATE/DROP TABLE, DB, repo и остальные DDL также не входят в контракт.

### Terminal status записывается слишком поздно

На normal path DROP/RENAME сначала завершают DDL, включая очистку tombstone, и только затем handler пишет `Succeeded`:

- `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:663-744`;
- `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:823-870`.

Если `write_op_status` падает, ошибка только печатается, а клиент получает inline success. Poll затем возвращает `Unknown`.

На recovery path порядок тоже небезопасен:

- hash DROP tombstones загружаются до `IndexManager::new`, но status пишется после recovery, которая tombstones уже очистила (`crates/shamir-engine/src/table/table_manager.rs:397-441`);
- index2 DROP сначала выполняет `recover_index2_drops`, очищающий tombstone, затем пишет status (`table_manager.rs:638-666`);
- hash RENAME сначала вызывает `clear_all_renaming`, затем пишет `SucceededViaCrashRecovery` (`table_manager_index_mgmt.rs:1483-1526` и `1577-1615`).

Crash или storage error между этими шагами оставляет завершённую операцию без tombstone и без status. На следующем restart восстановить связь с `op_id` уже невозможно.

Это прямо расходится с документацией `DdlOpState`, где terminal state должен быть записан до/в момент очистки tombstone.

### Серверный `op_id` недоступен в главном аварийном сценарии

`op_id` создаётся сервером внутри handler и возвращается только после успешного завершения синхронного DDL. Если:

- сервер упал до ответа;
- TCP connection оборвалась после мутации, но до получения ответа;
- DDL вернул ошибку после частичной мутации;

клиент не знает `op_id` и не может вызвать poll. Даже идеальный status log не решает ambiguity, если correlation id неизвестен запрашивающей стороне.

### Требуемая state machine

Рекомендуемый минимальный контракт:

1. Клиент передаёт `request_id`/`op_id` в DDL request, либо сначала делает `BeginDdl` и получает его до мутации.
2. Под ACL и до первой мутации сервер durable-записывает `InProgress`.
3. Recovery tombstone содержит тот же id.
4. Завершение DDL и переход в terminal state имеют crash-safe порядок:
   - лучший вариант — одна atomic `Store::transact` для terminal status + удаления tombstone;
   - допустимый вариант — сначала durable terminal status, затем idempotent tombstone clear.
5. Retry с тем же id идемпотентно возвращает текущий status, а не запускает вторую DDL.
6. Ошибка status persistence не может превращаться в «успех с будущим Unknown».
7. Terminal status имеет versioned envelope, а не голый bincode enum без format version.

Если это слишком большой объём для alpha, безопасная альтернатива — убрать обещание crash-resolvable contract из релизного API, оставить endpoint experimental и документировать, что он best-effort. Нынешнее промежуточное состояние опаснее обоих вариантов: интерфейс обещает гарантию, которой нет.

## P0-3. `GetDdlOpStatus` не проверяет ACL

Server handler принимает только `db/repo/table/op_id` и напрямую вызывает core:

- `crates/shamir-server/src/db_handler/handler.rs:758-772`;
- `crates/shamir-db/src/shamir_db/shamir_db/core.rs:745-780`.

Ни handler, ни core не вызывают `authorize_access`. В отличие от DROP/RENAME, poll endpoint не получает actor и не требует хотя бы `Read` на таблицу.

В результате любой аутентифицированный клиент, знающий `op_id`, может:

- проверить существование db/repo/table по различимому ответу;
- получить имя индекса и тип DDL;
- прочитать `Failed.detail`, который может содержать внутренние детали storage/recovery.

Случайность `RecordId` уменьшает вероятность угадывания, но не заменяет authorization. IDs могут попасть в логи, метрики, bug reports или другой клиентский процесс.

Исправление:

- прокинуть actor/session в handler;
- до table resolution выполнить тот же table-level authorization, что DDL operation;
- не создавать existence oracle через различимые `not found`/`access denied` ответы;
- добавить тест: пользователь без доступа знает корректный `op_id`, но получает `access_denied` без status/detail.

## P0-4. Index2 RENAME не является crash-safe

`TableManager::rename_index` для index2:

1. меняет `IndexRegistry::by_name` и descriptor name через `rename_entry`;
2. после этого вызывает `save_index2_metadata`.

См. `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:2163-2197`.

У этого пути нет rename tombstone, pending state или rollback. Если metadata save возвращает ошибку или future отменяется между шагами:

- текущий процесс видит новое имя;
- на диске остаётся старое имя;
- handler возвращает ошибку без usable `op_id` response;
- после restart имя откатывается к старому.

Для sorted RENAME durable tombstone есть, для regular/unique hash тоже есть. Index2 — заметный пробел именно в унифицированном lifecycle.

Нужно ввести persisted pending rename record с `(index_id, old_name, new_name, op_id, instance_epoch)` и recovery matrix, либо сделать publication через отдельный immutable metadata snapshot с crash-safe commit marker. Тесты должны инжектировать ошибку каждого storage write и cancellation после каждого await.

## P1. Неполнота и ресурсные проблемы DDL API

### Неверная классификация семейств

DROP sorted записывается как `DropHashIndex`, а RENAME sorted/index2 — как `RenameHashIndex`:

- `admin_table_index.rs:698-719`;
- `admin_table_index.rs:831-845`.

Sorted tombstone вообще не несёт `op_id`, хотя публичная документация утверждает, что `recover_in_progress_drops` пишет status с тем же id. Index2 RENAME status/recovery отсутствует.

Добавить отдельные `DdlOpKind::{DropSortedIndex,RenameSortedIndex,RenameIndex2}` и использовать фактически разрешённое семейство, зафиксированное до мутации.

### Status log растёт без ограничения

`DDL_OP_LOG_CAP = 10000` существует только как dead-code constant, а `maybe_evict_terminal_records` — no-op без call sites (`crates/shamir-engine/src/table/ddl_op_log.rs:22-88`). Каждая успешная tracked DDL навсегда добавляет запись в `info_store`.

Нужны реальная retention policy, индекс по времени/sequence и crash-safe GC. Для alpha достаточно фиксированного count + age cap, но он должен реально выполняться и быть наблюдаемым через metrics/doctor.

### Op status должен стать общим framework

Не расширять текущий handler-by-handler copy/paste. Нужен один DDL executor:

```text
authorize -> reserve id/idempotency -> InProgress -> mutate/recover -> terminal -> GC
```

И один typed result для всех длительных/восстанавливаемых операций: CREATE/DROP/RENAME INDEX, schema activation, migrations, table/repo operations. Не обязательно включать всё в alpha, но включённые операции должны иметь одинаковые semantics.

## HNSW quality gate не соответствует документации

Commit `3cd0d7ad` снизил restart recall@10 floor с `0.75` до `0.60` после реального результата Ubuntu CI `0.638`. Локально в комментарии указано `0.986–0.990`, а точная причина огромного межплатформенного разрыва не найдена (`crates/shamir-index/src/vector/tests/crash_recovery_tests.rs:492-535`).

При этом guide всё ещё утверждает:

- restart e2e на 10K vectors имеет recall@10 `>=0.90` (`docs/guide-docs/guide/06-search.md:454-468`);
- HNSW на 1K vectors показывает примерно `95–99%` (`06-search.md:580-589`).

Тест фактически использует `N_E2E=3000`, а не заявленные 10K. Снижение assertion до значения чуть ниже наблюдаемого падения делает CI зелёнее, но не объясняет пользовательское качество.

До релиза нужно:

1. Разделить две проверки:
   - fidelity: один и тот же graph до dump и после load возвращает идентичные neighbours/scores;
   - quality: recall построенного graph против brute force.
2. Прогнать статистическую матрицу Linux/macOS/Windows, несколько graph builds, фиксированные datasets, разные thread counts.
3. Публиковать percentile/min distribution, а не один случайный graph.
4. Либо вернуть доказуемый quality floor, либо исправить guide и честно указать наблюдавшийся минимум/experimental status.
5. Рассмотреть dependency/adapter, позволяющий deterministic seed и контролируемый insertion order; текущий `hnsw_rs` с OS RNG затрудняет regression testing.

До объяснения `0.638` нельзя одновременно оставлять маркетинговые `95–99%` и считать тест `>=0.60` достаточным доказательством.

## OQL / projection review

### Что сделано хорошо

- `SelectExpr::{Add,Sub,Mul,Div,Field,Literal}` сериализуется явно и транслируется в уже проверенный `FilterValue::Expr`.
- Projection строит caches один раз, а не интернирует paths заново для каждой строки.
- TS SDK имеет компактный `selectExpr` DSL.

### Что исправить до публичного обещания expressions

#### `SELECT *` молча удаляет остальные projection items

`SelectProjection::new` ставит `is_all`, если хотя бы один item равен `All`, после чего полностью очищает `fields` и `funcs` (`crates/shamir-engine/src/query/read/select_projection.rs:89-138`).

Запрос вида:

```text
[All, Expression(price * qty AS total)]
```

возвращает только исходную запись; `total` исчезает без ошибки. Нужно либо поддержать merge `* + extras`, либо валидировать и отклонять смешанную форму.

#### Alias collisions молча перезаписывают данные

Все expressions без alias получают ключ `"expr"`; functions по умолчанию используют имя function; fields — последний segment path. Результат вставляется в map через `obj.insert` (`select_projection.rs:185-205`). Два одинаковых output key приводят к silent last-write-wins.

Перед execution нужна проверка уникальности output columns. Для expression разумнее требовать alias или генерировать детерминированное имя, но не использовать один общий `"expr"`.

#### Ошибки вычисления превращаются в `Null`

`resolve_filter_query(...).unwrap_or(QueryValue::Null)` скрывает любые evaluator/scalar errors. Division by zero и type mismatch тоже сходятся в `Null`. Для аналитических expressions это может быть допустимым SQL-like режимом, но semantics должны быть явными. Рекомендуется:

- strict mode по умолчанию для user/WASM scalar errors;
- отдельно определить null-propagation, divide-by-zero и numeric overflow;
- не превращать infrastructure/storage/function trap в обычный null.

#### Numeric model слишком узок

`SelectExprValue` поддерживает только null/bool/i64/f64/string. Нет Decimal/Big, binary, date/time, list/map. Overflow i64 в общей expression machinery промотируется в f64, что способно тихо потерять точность.

### Развитие OQL после correctness fixes

Приоритетный компактный набор:

1. `alias` validation и `* + expression` semantics.
2. `mod`, unary minus, comparisons, boolean `and/or/not`.
3. `coalesce`, `if/case`, `cast`, `is_null`.
4. String/date helpers через typed function catalog.
5. Decimal-preserving arithmetic.
6. Expression support в `ORDER BY`, `GROUP BY`, aggregate arguments и functional indexes через один общий AST, а не параллельные несовместимые типы.

Не стоит пытаться до alpha имитировать полный SQL. Лучше один небольшой, строго специфицированный expression language с одинаковой сериализацией и semantics во всех SDK.

## Query Builders review

### Исправление panic-path выполнено удачно

Fallible `Update`, `Upsert`, `Delete`, schema/subscription builders больше не маскируют `Result` внутри `IntoBatchOp`; для них введён `TryIntoBatchOp` и `Batch::try_*`. Это правильная API-граница.

### `CreateIndex` всё ещё имеет два класса API

Typed constructors (`hash`, `unique_index`, `sorted_index`, `fts`, `functional`, `vector`) формируют валидный `IndexSpec`. Но legacy chain:

```text
.fields(...).index_type(...).build()
```

остаётся lenient, а `IntoBatchOp for CreateIndex` вызывает именно `build()`, не `try_build()` (`create_index.rs:468-488`, `619-628`). Поэтому invalid combinations всё ещё можно передать как обычный builder.

Alpha — удобное время сделать breaking cleanup:

- `build()` сделать fallible/strict;
- старый путь назвать `build_unchecked()` или `raw_create_index()`;
- `IntoBatchOp` реализовывать для уже валидированного `IndexSpec`/`BatchOp`, а не mutable stringly builder;
- добавить server capability-aware validation для tokenizer/function/vector restrictions, не копируя весь server catalog вручную.

### Rust expression ergonomics отстаёт от TypeScript

TS уже имеет `selectExpr.add/sub/mul/div/field/literal`; Rust требует вручную писать nested enum + `Box`. Добавить `select::expr::{field,lit,add,sub,mul,div}` или operator-composable `Expr` type.

### `Batch::try_build` делает дорогой codec round-trip

Для поиска `$query` refs каждый `BatchOp` сериализуется в MessagePack, затем декодируется в `QueryValue`; `when` сериализуется отдельно (`batch.rs:902-993`). На больших insert/update payload это лишние O(payload) allocations и копирование только ради validation.

Нужен typed visitor по `BatchOp`/`FilterValue`, возвращающий refs без codec round-trip. Он одновременно уберёт `expect("... serialization is infallible")` из public non-test path.

### Другие расширения builder, которые окупятся до/сразу после alpha

- typed `DdlOperation` handle с `poll()/wait()` и idempotency key;
- единый `FieldPath` newtype вместо `Vec<String>`/dot-string различий;
- builder-side duplicate alias validation;
- compile/validate method, возвращающий warnings вместе с errors;
- parity matrix Rust/TS: каждый wire operation, enum value, default и validation error должны иметь общий fixture.

## Performance review

### P1. Online CREATE INDEX не реализован

Текущий regular/unique/sorted CREATE удерживает write barrier на всём backfill. Draft RFC `docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md` правильно предлагает:

```text
snapshot scan -> durable delta capture -> catch-up -> short publish barrier
```

и разумно начинает с regular hash. Но RFC всё ещё `DRAFT — pending review`, task `#1050` не реализован.

Для первого alpha возможны два честных решения:

- реализовать slice 1 и доказать bounded writer pause;
- либо оставить offline/blocking CREATE, но явно документировать lock duration, recommended maintenance window и table-size envelope.

Без одного из них продукт нельзя обещать как бесшовную замену серверной СУБД для medium workload.

### ReaderDrainGate корректен, но дорог и имеет unbounded spin-wait

Hot read платит один `SeqCst fetch_add`, `SeqCst load` и `SeqCst fetch_sub`. Gate общий на всё семейство manager, поэтому DROP одного индекса затрагивает admission всех его siblings. DROP ждёт readers через unbounded `yield_now` loop, удерживая DDL/write locks (`crates/shamir-index/src/reader_drain_gate.rs:109-145`, `193-230`).

Это приемлемый correctness-first patch, но до заявлений high-performance нужно:

- benchmark indexed QPS/latency до и после gate на 1/8/32/64 threads;
- сделать gate per index instance, где это возможно;
- проверить, можно ли доказать memory ordering на Acquire/Release вместо трёх SeqCst операций;
- заменить yield loop на `Notify`/event-count/futex-like wakeup;
- добавить cancellation/timeout policy и диагностику конкретных reader owners.

Не ослаблять ordering без loom/shuttle-style model test.

### Другие подтверждённые hotspots

- `CachedStore::flush` дважды реализован как unbounded polling `pending_writes + yield_now` (`crates/shamir-storage/src/storage_cached.rs:296-308`, `611-624`). Нужен `Notify`/JoinSet и корректная ошибка фонового write; один счётчик не сообщает, что write упал.
- `BruteForceAdapter::upsert/delete` делает `yield_now` после каждого channel send (`crates/shamir-index/src/vector/brute_force.rs:233-259`), мешая actor batching. Bounded channel уже обеспечивает backpressure; visibility должна иметь explicit ack/flush, а не scheduler hint.
- Все index2 backend, кроме vector snapshot path, при open обычно делают full data-store rebuild (`table_manager.rs:682-716`, default `IndexBackend::restore_on_open`). На больших FTS/functional indexes restart остаётся O(rows × indexes).
- Cursor уменьшает клиентскую память, но server повторно запускает pinned-snapshot scan на каждой странице; offset fallback особенно дорог. Это честно описано в CHANGELOG/KNOWN_LIMITATIONS, но должно войти в sizing guide.

### Performance gate отсутствует как release evidence

`release.yml` намеренно не запускает benchmarks; `perf-gate.yml` только manual/self-hosted, а `bench-baseline.json` в корне отсутствует. Это соответствует зафиксированному operator decision `#1020`, но значит, что tag pipeline сам по себе не доказывает claim **High-performance**.

Минимум перед tag без изменения workflow:

1. Вручную выполнить perf gate на доверенном стабильном runner.
2. Зафиксировать machine spec, commit SHA, dataset, p50/p95/p99, throughput, RSS, binary size, startup/recovery time.
3. Сохранить подписанный/immutable artifact рядом с release notes.
4. Определить красные пороги хотя бы для CRUD, tx commit, regular/sorted lookup, FTS, vector, 100K backfill и restart.

## CI и release engineering

### Windows stall остаётся открытым

Investigation `2026-08-07-p1-6-windows-ci-batch-timeout-investigation.md` фиксирует четыре крошечных e2e batch test, завершившихся примерно через 139–140 секунд с сообщением о 30-секундном budget. Root cause не найден; добавлена диагностика.

`ExecutionDeadline` проверяется только между operations и не может прервать один зависший operation. Поэтому 30-second budget не является настоящим wall-clock timeout/cancellation boundary.

До tag нужно как минимум:

- дождаться/спровоцировать повтор с новой telemetry;
- сделать watchdog на request/batch boundary, который логирует stuck alias/op/phase;
- решить cancellation safety каждого DDL/write operation до оборачивания его в `tokio::time::timeout`;
- получить зелёный multi-OS release workflow на том же SHA, который будет tagged.

Простое увеличение timeout проблему не исправляет.

### Документы и packaging

Положительное:

- README имеет корректный alpha disclaimer и честное comparison positioning.
- CHANGELOG содержит отдельный heading `0.1.0-alpha.1` и предупреждение об отсутствии alpha-to-alpha upgrade guarantee.
- Release workflow проверяет tag/version/CHANGELOG consistency и собирает подписанные multi-platform artifacts.

Что обновить непосредственно перед tag:

- `SECURITY.md` сейчас корректно говорит «versioned releases нет»; после публикации tag эту секцию и supported versions table нужно изменить в том же release PR/следующем немедленном commit.
- Удалить stale comments в `release.yml`, где всё ещё говорится, что CHANGELOG имеет только `[Unreleased]`.
- Решить и документировать Node package cadence: сейчас mismatch `0.1.0` против workspace `0.1.0-alpha.1` сознательно исключён из consistency gate и npm publish workflow отсутствует.
- Проверить итоговый `shamir-server` binary реально меньше заявленных 50 MB на всех target, а не только заявлять это архитектурно.

## Обязательный план до релиза

### Этап A — correctness/security freeze

1. Исправить owner/provenance routing `BumpFtsStats` и добавить multi-FTS + ABA tests.
2. Добавить ACL в `GetDdlOpStatus`.
3. Переделать DDL status ordering и correlation/idempotency так, чтобы lost response реально разрешался poll-ом.
4. Сделать index2 RENAME crash-safe.
5. Исправить DDL family kinds, sorted/index2 op-id plumbing и реальный status-log retention.
6. Исправить или запретить silent projection loss (`* + expr`, duplicate aliases).

После каждого самостоятельного fix — обязательный gate из `AGENTS.md`:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh
```

### Этап B — release evidence

1. Запустить полный gate на чистом release candidate SHA.
2. Получить зелёный Windows/Linux/macOS release workflow на этом SHA.
3. Провести HNSW cross-OS quality experiment и синхронизировать docs с доказательствами.
4. Провести manual perf gate и сохранить immutable result artifact.
5. Выполнить crash/fault-injection matrix для DDL status, index rename/drop, FTS commit и restart.
6. Собрать release binaries, проверить размер, запуск без внешнего runtime, Docker smoke, SBOM/signatures.

### Этап C — alpha scope decision

До тега явно принять одно из решений по каждому пункту:

- blocking CREATE INDEX: исправляется сейчас или документируется как maintenance-only;
- server-side cursor rescans: accepted alpha limitation с sizing envelope;
- FTS/functional startup rebuild: accepted limitation с restart estimates;
- vector quality: supported guarantee или experimental;
- DDL status: production contract или experimental best-effort.

Accepted limitation должна быть видна в README/guide/release notes, иметь конкретную границу и tracking issue. Она не должна выглядеть как уже реализованная гарантия.

## Release bar

`GO` можно дать, когда одновременно выполнено:

- нет известных silent correctness bugs в FTS/index lifecycle;
- каждый публично поддерживаемый DDL status переход crash-safe и ACL-protected;
- error/cancellation injection не создаёт runtime/disk catalog divergence;
- OQL projection не теряет requested columns молча;
- release candidate зелёный на всех target;
- vector docs совпадают с наблюдаемым quality floor;
- есть воспроизводимый performance report для exact release SHA;
- ограничения blocking DDL, cursor scan и startup rebuild явно приняты и опубликованы;
- tag ставится на проверенный commit, а не на иной последующий HEAD.

До выполнения этих условий текущий код выглядит как сильный, быстро взрослеющий engineering alpha, но ещё не как безопасный первый публичный release candidate.

## Методика и ограничения этого review

Проверены Git history новой волны и ключевые production paths:

- index registry/generation/provenance;
- transaction pre-commit reconciliation;
- regular/sorted/index2 reader-drain lifecycle;
- DROP/RENAME recovery и DDL op log;
- server/core DDL poll routing и authorization boundary;
- FTS ranked write planning/commit;
- OQL select expressions/projection;
- Rust/TS builders;
- vector recall tests и guide claims;
- release/perf workflows и release-facing docs.

Review намеренно не запускал сборку, tests, benchmarks или server. Утверждение checkpoint о `4104/4104` зелёных тестах не считалось независимым доказательством. На момент среза worktree уже содержал чужой untracked `docs/checkpoints/2026-08-09-0945.md`; он не изменялся.

Это широкий, но не полный security audit. Не были исчерпывающе проверены unsafe/FFI и Node binding, cryptography/TLS, WASM sandbox/fuel/capabilities, replication protocol, каждая storage implementation, migration API, procedural macros и supply-chain dependencies. Для публичного релиза эти области требуют отдельных специализированных reviews; данный отчёт не подтверждает отсутствие дефектов вне просмотренных путей.
