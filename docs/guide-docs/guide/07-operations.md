בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 7 — Эксплуатация: рантайм, метрики, логи, конфиг

**Когда подниматься:** выкатка в прод.

До этого этажа сервер запускался вручную: `shamir-server --config …` в
терминале. Для разработки этого хватает. Но продакшен требует: сервисный
режим (systemd / Windows service), observability (`/healthz`, `/metrics`),
структурированные логи, rate-limit, audit trail. Этот этаж — о том, как
ShamirDB живёт в проде.

## 1. Один бинарник — все режимы

ShamirDB — self-contained бинарник (`shamir-server`). Режим запуска
определяется подкомандой:

```bash
# Foreground (разработка / Docker)
shamir-server --config db.ktav

# То же, явно:
shamir-server --config db.ktav run

# Как systemd-сервис (Linux)
shamir-server --config db.ktav run --service
```

Все режимы проходят через одну точку входа — `runtime::serve()`.
Различается только триггер остановки: Ctrl+C/SIGTERM (foreground),
SCM Stop (Windows), `systemctl stop` (systemd). Drain-логика одинакова:
прекратить приём → доработать in-flight (30 s deadline) → долить буферы
→ fsync → отпустить блокировки.

### Foreground

```bash
shamir-server --config /etc/shamir/server.ktav --bootstrap-password "admin-pass"
```

Остановка: `Ctrl+C` (везде) или `SIGTERM` (unix). Процесс доливает все
буферы и выходит.

### systemd (Linux)

```bash
# Установить сервис (генерирует .service + включает):
shamir-server service install --user shamir

# После установки:
systemctl start shamir-db
systemctl status shamir-db
systemctl stop shamir-db
```

`service install` генерирует юнит с абсолютными путями и вызывает
`systemctl enable`. Тип юнита — `Type=notify`: сервер вызывает
`sd_notify(READY=1)` после bind listener'ов — systemd знает, что сервис
готов.

Готовый production-юнит — `deploy/shamir-db.service` (`Type=notify`,
hardening, `RuntimeDirectory=shamir` + `--bootstrap-token-path` для
одноразового bootstrap-токена). Бери файл, а не фрагменты из руководства.

### Windows Service

```powershell
shamir-server service install
```

Регистрирует сервис в Windows SCM. Остановка — через `sc stop` или
`Services MMC`. Recovery policy настраивается через SCM.

### macOS / BSD

`service install` генерирует launchd plist (macOS) или rc.d-скрипт (BSD).

### After a leader crash

После нештатной остановки cold open записывает консервативный GAP
`[J + 1, H]`, если восстановленный horizon H больше persisted clean horizon J.
Это происходит **даже при полном журнале**: точного детектора потерь пока нет.
Старый data directory без J сертифицируется один раз при первом старте;
ошибка записи сертификата запрещает open. Followers с первым pull `<= H`
останавливаются с `JournalGap`, без применения сопровождающих events и без
продвижения bookmark. `ListSubscriptions` и `ReplicationStatus` показывают
`resync_required`; это состояние каталога, не измерение lag.

1. Остановите replication и writes; сохраните **offline** backup лидера и
   followers. Не копируйте живой data_dir: backup — stop-and-copy, не online
   consistent snapshot.
2. Не используйте `Resume` как ремонт и не поднимайте bookmark без данных.
   Новый follower с bookmark 0 снова встретит сохранённый GAP; clean stop
   лидера и пересоздание subscription тоже не очищают интервал.
3. Для полного replay с нуля выгрузите проверенные authoritative logical data,
   создайте **новую независимую source lineage** с тем же schema и загрузите
   данные через обычные writes (они создают новый gap-free journal). Создайте
   пустые followers с тем же schema и без secondary indexes; пересоздайте
   profiles/subscriptions на новый upstream и запустите replay. Старую lineage
   сохраните offline, не удаляйте GAP metadata вручную. Проверяйте данные и
   достижение нового head, прежде чем вернуть writes.
4. Альтернатива — проверенный follower image с данными **и его matching upstream
   bookmark** через H. Произвольный старый backup недостаточен. Встроенного
   snapshot-plus-bookmark seed command и автоматического reseed нет; такой
   image требует отдельного проверенного tooling/procedure.

Источники: `crates/shamir-engine/src/repo/repo_instance.rs:1829-1877`,
`crates/shamir-server/src/replication/follower_loop.rs:311-325`,
`crates/shamir-server/src/replication/supervisor.rs:309-334`,
`crates/shamir-db/src/shamir_db/execute/admin_replication.rs:413-467`,
`crates/shamir-server/src/backup.rs:3-32`.

## 2. Конфиг: `.ktav`

Конфиг — файл в формате Ktav ≥ 0.6: без кавычек и запятых, комментарии —
только целой строкой с префиксом `##`. Минимальный конфиг:

```ktav
data_dir: /tmp/shamir-test

kdf_defaults: {
    memory_kb: 19456
    time: 2
    parallelism: 1
    argon2_version: 19
}

listeners: [
    {
        kind: tcp
        addr: 127.0.0.1:7000
        profile: tls_exporter
    }
]

tls: {
    cert_path: /tmp/shamir-test/cert.pem
    key_path:  /tmp/shamir-test/key.pem
}
```

Production-конфиг — `deploy/server.example.ktav` (все поля с
комментариями); профили `server.{small,medium,large}.example.ktav` — под
объём RAM. Не копируй фрагменты отсюда: разбор и валидация этих файлов
закреплены тестами (`reference_example_parses_and_validates` и соседние).

### Секции конфига

|| Секция | Назначение |
||---|---|
|| `data_dir` | Корень данных (базы, WAL, сертификаты) |
|| `logging` | Уровень логирования, slow-query threshold, файл |
|| `kdf_defaults` | Параметры Argon2id (memory, time, parallelism) |
|| `argon2_concurrent_max` | Лимит одновременных Argon2-верификаций |
|| `listeners[]` | Сетевые endpoints (TCP / WebSocket, TLS-профили) |
|| `tls` | Пути к сертификату и ключу |
|| `security` | Таймауты, лимиты соединений, query limits |
|| `audit` | Audit log с ротацией (audit-line-формат файла) |
|| `observability` | Адрес HTTP-сервера метрик |

### Лимиты выполнения запросов

`security.query_limits` задаёт серверную policy для `Execute` и `TxExecute`.
Эффективный лимит — минимум из лимита клиента, server-owned `ExecPolicy.caps`
оператора и жёсткого `BatchLimits::CEILING` движка; вложенные батчи не могут
поднять его. Потолки движка: 100 MiB / 1000 queries / 3600 s; это caps,
а не defaults запроса (у клиента defaults 10 MiB / 50 / 30 s).
Источники: `crates/shamir-server/src/db_handler/config.rs::policy`,
`crates/shamir-query-types/src/batch/batch_limits.rs`.

|| Поле | Default | Максимум при старте |
||---|---|---|
|| `max_result_size_bytes` | 67108864 (64 MiB) | 104857600 (100 MiB) |
|| `max_execution_time_secs` | 60 | 3600 |
|| `max_queries_per_batch` | 100 | 1000 |
|| `max_nesting_depth` | 64 | 64 |
|| `max_dependency_depth` | 1000 | 1000 |
|| `max_iterations` | 10000 | 10000 |
|| `max_total_queries` | 10000 | 10000 |
|| `max_total_work` | 1000000 | 1000000 |
|| `max_page_size` | 100000 | 100000 |
|| `max_offset` | 1000000 | 1000000 |

Defaults и startup validation: `crates/shamir-server/src/config.rs:456-487,768-777`;
engine caps: `crates/shamir-query-types/src/batch/batch_limits.rs:109-120`.

`max_total_queries` ограничивает число узлов всего дерева батча;
`max_total_work` ограничивает и статическую оценку вложенных батчей/циклов,
и **runtime dispatch**: `ExecScope::charge_work(1)` списывает единицу перед
каждым реально выполняемым alias, включая Batch/ForEach-контейнер и их
вложенные dispatch. Пропущенный по `when` alias не списывает work.
Динамические повторения тоже учитываются; все вложенные исполнения делят
один root counter, предварительный `check_work` не резервирует работу.
Это счётчик dispatch, не строк, CPU-инструкций или времени сканирования
(`crates/shamir-engine/src/query/batch/exec_scope.rs`, `batch_execute.rs`,
`query_runner.rs`). `max_page_size` и
`max_offset` ограничивают пагинацию query, отдельно от `security.cursors`.
Превышение запрошенной страницы/offset возвращает `pagination_too_large`,
работы — `work_budget_exceeded`, вложенности — `nesting_too_deep`,
итераций — `too_many_iterations`.

Все поля выше валидируются при старте. Ноль допустим только для
`max_nesting_depth` (без вложенности), `max_dependency_depth` (только
независимые запросы) и `max_offset` (без пропуска строк). Новые поля по
умолчанию равны CEILING, а не request-default: клиент сохраняет возможность
поднять свои лимиты в пределах движка. `max_result_size_bytes` выше 100 MiB
(потолок движка) отвергается при старте; large-профиль использует ровно
100 MiB.

Для батчей `security.query_limits.enforce_result_size` по умолчанию `true`:
движок консервативно оценивает накопленные материализованные байты и
проверяет `max_result_size_bytes` до commit, включая промежуточные результаты
с `return_result: false`, ForEach и вложенные контейнеры, без возврата бюджета
при фильтрации или удалении результатов. Это не cap сериализованного ответа
и не live RSS. Явное `false` отключает runtime-проверку батчей; оценка
upfront-резерва RI-15 сохраняется. Embedded API остаётся OFF по умолчанию.
Страницы курсоров сохраняют жёсткий байтовый cap `max_result_size_bytes`
на сериализованный ответ.
Embedded `ExecPolicy::EMBEDDED` и WASM `FacadeDbGateway` → `execute_as`
оставляют enforcement OFF (флаг серверный:
`crates/shamir-server/src/db_handler/config.rs`,
`crates/shamir-query-types/src/batch/exec_policy.rs`,
`crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs`).

Embedded direct CRUD: `TableManager::{insert, insert_many, delete, set}_with_receipt`
принимает explicit `VisibilityWait` и возвращает must-use `WriteOutcome<T>`:
value + `commit_version: Option<u64>` + visibility. `set` сохраняет `bool`
(created), не unit. Missing delete / empty insert-many / non-MVCC не выдают
позицию 0: version = `None`, без таймера. Bounded Pending — committed, **не retry**;
reconcile на той же incarnation repo, затем открыть новый snapshot. Старые
value-only методы — explicit Strict adapters, даже при bounded repo policy;
reopen-before-visible возвращает `WriteCommittedVisibilityPending`
(`write_committed_visibility_pending`) с committed version, sampled W и reason,
не обычный retryable write failure. Direct CRUD остаётся best-effort/no-WAL,
не transactional atomicity / crash-durability и unsafe to cancel mid-sequence;
отмена только observer освобождает waiter, не отменяет write/recovery. Guards
освобождаются до observation. Аналогичные MVCC receipt APIs используют local
storage/tx visibility types. Implicit batch propagation — VIS-4b; bounded server
default / SDK rollout всё ещё gated. Полный контракт:
[Transaction outcomes](../client-server-protocol-spec/TRANSACTION_OUTCOMES.md).


При включённом enforcement (`ExecPolicy { enforce_result_size: true, … }`) общий root
budget означает **«bytes materialized by this execution»**, а не длину
MessagePack, allocator capacity или live RSS. `ResultSize` детерминированно
оценивает payload и контейнеры: промежуточные результаты, даже не возвращаемые
клиенту, nested Batch и ForEach-оболочки тоже платят. Фильтрация/удаление
результатов не возвращает bytes; внутренние payload контейнера не считаются
повторно при его сборке. Превышение возвращает `LimitExceeded(ResultBytes)`
→ `result_too_large` до решения о commit; уже выполненные non-transactional
операции этим не откатываются. Источники:
`crates/shamir-engine/src/query/batch/exec_scope.rs::charge_bytes`,
`result_size.rs`, `batch_execute.rs`, `query_runner.rs`.

### Время TxExecute и string_agg

Абсолютный срок interactive handle — 300 s. Budget одного `TxExecute` —
минимум effective request/policy time и оставшегося срока handle **с полной
точностью `Duration`**, без округления до секунд и без продления на каждый
вызов. Остаток берётся **после** захвата context lock; authorization тоже
вычитается из него (`crates/shamir-server/src/db_handler/tx_handlers.rs:169-181`,
`crates/shamir-db/src/shamir_db/execute/db_tx.rs:158-165`,
`crates/shamir-engine/src/query/batch/exec_scope.rs`). Deadline cooperative:
застрявшая операция не прерывается таймером; перед commit проверяется deadline
и work budget, а начатый commit не отменяется по timeout.

`string_agg` имеет независимые caps, даже при byte enforcement OFF:
separator ≤ **1024 bytes** (не символов), default separator `,`;
каждый accumulator, в том числе отдельная GROUP BY-группа, ограничен root
`max_result_size` (не более 100 MiB). Без execution context cap — 100 MiB.
Размер строки проверяется **до** добавления значения и разделителя; NULL
пропускается. Nested batch наследует root accumulator cap, даже если его
локальный `max_result_size` меньше. Превышение идёт как **query error** с
диагностикой `result_too_large: ResultBytes …`, не `LimitExceeded`, и само по
себе не удаляет interactive handle. Источники:
`crates/shamir-engine/src/query/read/aggregate.rs::CappedStringAgg`,
`crates/shamir-engine/src/query/filter/eval_context.rs`,
`crates/shamir-engine/src/query/batch/query_runner.rs:1194`.

### Migration source freeze и исход commit

При подготовке миграции source table замораживается для data writes:
raise freeze bit → drain writers → write-lock fence. Freeze guard живёт до
завершения/отмены миграции; отмена подготовки снимает bit. Это не удержание
DDL admission на весь срок: DDL остаётся разрешённым. Direct writes и commit
транзакции, попавшие на freeze, получают `table_migrating`; не делайте busy
retry, дождитесь завершения/отмены и заново разрешите активную таблицу
(`crates/shamir-engine/src/table/table_manager.rs::begin_migration_freeze`,
`table_manager_crud.rs`, `crates/shamir-engine/src/tx/pre_commit.rs`,
`crates/shamir-db/src/shamir_db/execute/admin_migration.rs`).

`wal_requires_reopen` требует reopen/reconciliation, новые commits отказаны.
`status: committed` с `error: wal_durability` — уже опубликованные writes,
**не retry и не rollback**: TS `db.tx()` сохраняет warning в committed receipt.
`materialized: false` не означает abort: проекции отложены до recovery на
следующем open; при durability error нельзя обещать crash-durability.
Полный wire contract и cleanup/retry table:
[Transaction outcomes](../client-server-protocol-spec/TRANSACTION_OUTCOMES.md).

### Client visibility receipts (VIS-5a)

Pending is **committed**, never an instruction to resend. Rust exposes
`CommitResult::Committed { transaction, visibility }` from `tx_commit` and
`execute_transactional`; TS `db.tx()` returns `{ status: 'committed', state,
value, transaction, visibility }`. Published `wal_durability` warnings remain
in `transaction.error`, independently of pending/visible. Aborts and
`wal_requires_reopen` remain errors with their original reason; indeterminate
WAL is not proof that writes are absent. No value-only transaction convenience
hides these outcomes.

```ts
const receipt = await db.tx(async (tx) => {
  await tx.run(write.insert('items', [{ id: 'receipt-example' }]));
  return 'saved';
}, { repo: 'main', visibilityWait: { mode: 'bounded', timeout_ms: 0 } });
if (receipt.state === 'pending') {
  const version = receipt.transaction.commit_version;
  if (version === undefined) throw new Error('missing commit position');
  const observed = await client.waitForVisibility(
    db.name, 'main', BigInt(version), { mode: 'bounded', timeout_ms: 0 },
  );
  if (observed.state === 'visible') {
    // A fresh builder read opens a NEW snapshot; an old transaction never advances.
    const rows = await db.query('items').rows();
  }
}
// Inspect receipt.transaction.error even when state is visible.
```

Rust reconciliation is `client.wait_for_visibility(db, repo, version,
VisibilityWait::Bounded { timeout_ms: 0 }).await?`. Zero is an immediate
predicate check, not infinity. Strict is explicit and may wait indefinitely.
Use the SAME repository incarnation; names cannot detect deletion/recreation.
Observation proves settlement through a position, not whether an unknown write
committed, and upgrades neither durability nor async-index readiness.

Rust positions stay `u64`; TS reconciliation/prefix positions are `bigint`.
Transaction version fields accept a documented lossless wire dual (`number |
bigint`); SDK decoding normalizes present versions to bigint. MessagePack
preserves 2^53+1 and u64::MAX without JSON or Number conversion. Transport
timeouts/disconnects mean UNKNOWN acknowledgement, not pending/abort proof.
Never rollback or replay after a commit attempt; reconcile externally without
assuming settlement proves an unknown transaction committed.

Current wire limitation: only `as_of_visibility_timeout` carries structured
`requested`, `visible_through`, `blocking_version`. Other visibility codes
carry code/message only; typed client mappings do not parse diagnostics or
invent missing positions. Implicit/non-transactional per-alias receipts are
VIS-5b, not this API.

### Валидация при старте

`Config::validate()` проверяет: `kdf_defaults` (и per-listener override) не
ниже floor'ов spec §3.7.2; `argon2_concurrent_max` ∈ 1..=1024;
`auth_init_rate_per_second` ∈ 1..=100000; `listeners` не пуст и каждый
валиден (адрес, `path` для ws, `browser_origin_allowlist` для
browser-профиля, `plain` — только loopback); лимиты курсоров ≥ 1;
`max_inflight_response_bytes` ≥ `max_result_size_bytes`; query limits
не превышают CEILING (нулевые значения — только где имеют смысл). `data_dir`
создаётся, а отсутствующие cert/key генерируются (self-signed) при старте.
Ошибка → процесс не стартует.

`security.query_limits.max_inflight_response_bytes` (RI-15) — если задан,
должен быть `>= max_result_size_bytes`: иначе ни один максимально большой
батч-ответ не смог бы пройти глобальный бюджет памяти. Если бюджет не задан,
он не ограничивает суммарный размер одновременно висящих в памяти ответов;
`max_result_size_bytes` остаётся оценкой резерва для батчей, при
`enforce_result_size: true` также ограничивает накопленные материализованные
байты до commit, и сохраняет жёсткий байтовый cap для страниц курсоров.

Важная деталь для планирования ёмкости: с CR-B2 резервирование из
`max_inflight_response_bytes` происходит ЗАРАНЕЕ (upfront) и ПЕССИМИСТИЧНО —
на основе `max_result_size` батча (обрезанного до серверного cap), а не
реального итогового размера ответа. Поэтому `max_inflight_response_bytes` —
это не только предел памяти, но и ЖЁСТКИЙ лимит числа одновременно
ВЫПОЛНЯЮЩИХСЯ батчей: `effective_concurrency ≈ max_inflight_response_bytes /
max_result_size_bytes` (батч с меньшим `limits.max_result_size` резервирует
меньше и меньше влияет на этот предел — граница считается по ХУДШЕМУ
случаю, а не по типичному). На числах из примера конфига выше: `4 GiB /
100 MiB ≈ 40` — не более 40 одновременных батчей, даже если каждый реальный ответ
занимает всего несколько KiB. Оператору стоит подбирать
`max_inflight_response_bytes` с учётом этого деления, а не только как
бюджет памяти: если нужная конкурентность превышает то, что подсказывает
чистый memory-бюджет, либо увеличивайте `max_inflight_response_bytes`
соответственно, либо просите клиентов задавать меньший per-batch
`limits.max_result_size` (это уменьшает каждое резервирование и повышает
число батчей, которые тот же cap допускает одновременно).

`security.cursors.*` (FG-5b — результирующие курсоры `CreateCursor`/
`FetchNext`/`CancelCursor`):
- `idle_timeout_secs` (default **60**) — сколько секунд курсор может
  простоять без `FetchNext`, прежде чем фоновый reaper его закроет
  (освобождая закреплённый MVCC snapshot). `0` отклоняется при старте —
  нулевой idle-timeout закрыл бы почти любой курсор мгновенно.
- `max_cursors_per_session` (default **16**) — сколько курсоров может быть
  одновременно открыто в рамках одной сессии; `CreateCursor` сверх лимита
  возвращает `cursor_limit_exceeded`. `0` отклоняется при старте — это
  сделало бы `CreateCursor` невозможным вообще.
- `max_cursor_page_size` (default **10000**) — верхняя граница `page_size`
  для `CreateCursor`/`FetchNext`; запрос выше границы отклоняется целиком
  (не обрезается молча). `0` отклоняется при старте по той же причине.

## 3. Observability: `/healthz`, `/readyz`, `/metrics`, `/info`

Отдельный HTTP-сервер (по умолчанию `127.0.0.1:9090`) — **не** на
основном listener'е. Предназначен для K8s probes, Prometheus, мониторинга.

```bash
curl http://127.0.0.1:9090/healthz   # → 200 OK "ok\n"
curl http://127.0.0.1:9090/readyz     # → 200 (ready) или 503 (not yet)
curl http://127.0.0.1:9090/metrics    # → Prometheus text format
curl http://127.0.0.1:9090/info       # → msgpack: uptime, bound_addrs, ready
```

### `/healthz`

Всегда `200 OK "ok\n"`. Тривиальный liveness probe: процесс жив и
реагирует на HTTP.

### `/readyz`

`200 OK` — после того, как все listener'ы привязаны (accept loop
запущен). `503 Service Unavailable` — до готовности. K8s readinessProbe.

### `/metrics`

Prometheus text format. Метрики:

|| Метрика | Тип | Описание |
||---|---|---|
|| `process_cpu_seconds_total` | counter | CPU time |
|| `process_resident_memory_bytes` | gauge | RSS |
|| `process_open_fds` | gauge | Открытые файловые дескрипторы |
|| `auth_attempts_total{result=…}` | counter | Аутентификации (success, bad_proof, locked_out, …) |
|| `shamir_tx_started_total` | counter | Транзакции начатые |
|| `shamir_tx_committed_total` | counter | Транзакции закоммиченные |
|| `shamir_tx_aborted_ssi_total` | counter | SSI-конфликты |
|| `shamir_tx_aborted_expired_total` | counter | Истёкшие tx |
|| `shamir_gc_runs_total` | counter | GC-циклы |
|| `shamir_gc_entries_deleted_total` | counter | GC-удаления |

`process_*` опрашиваются каждые 5 секунд (crate `metrics-process`).

### `/info`

```msgpack
{
  "uptime_seconds": 3600,
  "bound_addrs": ["0.0.0.0:7331", "0.0.0.0:7332"],
  "ready": true
}
```

### Безопасность observability

Loopback-only по умолчанию. Для non-loopback — нужно явно разрешить
`allow_public_metrics: true` в блоке `observability` (не рекомендуется).
Экспозиция метрик на public network — M-tier audit event (M5).

Пустой `addr: ""` отключает observability-сервер полностью.

## 4. Логирование

Два режима:

|| Режим | Когда | Как работает |
||---|---|---|
|| Stdout | `logging.file` не указан | Non-blocking tracing_appender, lossy on overflow |
|| Batched file | `logging.file: "/var/log/shamir-db/server.log"` | MPSC → single worker → BufWriter, flush каждые `flush_interval_ms` (default 2000) или при burst ≥ 256 KB |

### Slow query logging

```ktav
logging: {
    level: info
    slow_query_threshold_ms: 1000
}
```

Батч, выполняющийся дольше `slow_query_threshold_ms`, логируется на
уровне WARN с деталями (batch id, execution_time_us, num_queries).

### Namespace masks (lock-free)

13 неймспейсов с индивидуальными уровнями: `shomer`, `wal`, `tx`,
`storage`, `engine`, `query`, `vector`, `fts`, `fn`, `auth`, `wire`,
`server`, `migration`. Hot-path `enabled()` — одно atomic load +
longest-prefix match. Нет mutex/RwLock.

### Live reload (SIGHUP)

На unix: `kill -HUP <pid>` → сервер перечитывает конфиг → lock-free mask
swap. Без рестарта. Windows: restart required.

## 5. Rate limiting

Token-bucket per `/24` IPv4 или `/64` IPv6 subnet:

```ktav
security: {
    auth_init_rate_per_second: 10
}
```

* Default: 10 tokens/sec per subnet.
* Warmup: ÷4 в первые 60 секунд после boot (spec §8.6).
* GC: idle buckets удаляются через 5 минут.
* Persistence: `RateLimitSnapshotSink` — durable rehydration при restart.

## 6. Connection limiter

Глобальный лимит одновременных соединений:

```ktav
security: {
    connection: {
        max_active_connections: 10000
    }
}
```

Atomic counter + RAII guard. Новое соединение сверх лимита —
отклоняется. 0 = без лимита.

### Slow-loris защита

```ktav
security: {
    connection: {
        auth_init_timeout_ms: 5000
    }
}
```

Если клиент не отправил `auth_init` за 5000 ms — соединение закрывается.

## 7. Audit log

```ktav
audit: {
    max_file_size_mb: 100
    retention_days: 30
}
```

Audit-лог в audit-line-формате с HMAC-chain (каждая запись включает HMAC от предыдущей).
Ротация по размеру; устаревшие файлы удаляются автоматически.
Покрытие (состояние реализации): в durable HMAC-chained audit-лог сегодня попадают **только события аутентификации** (успех/сбой/прерывание рукопожатия — это единственный append call site, `crates/shamir-server/src/connection/handshake.rs`). DDL, ACL-изменения (chmod/chown) и admin-операции (`CreateScramUser`, `SetSuperuser`, retention/purge, drop и т.п.) durable-следа **не оставляют** — для них существует только эфемерный вывод `log`/`tracing`, который не персистируется и не является enforcement-gate (см. `shamir-types::access::trace_access`). Расширение покрытия (мост `AuditSink` → `AuditChainWriter` + append call sites на DDL/ACL/admin-операциях) — запланированное улучшение, пока не реализовано (P1).

## 8. Backup

Одноразовый snapshot всего `data_dir` — модель **stop-and-copy** (v1):
**сервер должен быть остановлен перед запуском `backup`**, онлайн/живого
режима резервного копирования сейчас нет.

```bash
# 1) Остановить сервер (systemctl stop shamir-server / SIGTERM — дождаться graceful shutdown)
# 2) Запустить backup — data_dir берётся из --config, снимок пишется в <to>/<UTC-timestamp>/
shamir-server --config db.ktav backup --to /backup/shamir
# 3) Перезапустить сервер
```

Команда рекурсивно копирует **весь** `data_dir` (хранилище, WAL, реестры таблиц,
audit-лог и его ротации, TLS PEM) в каталог `<to>/YYYYMMDD_HHMMSS/`; существующий
целевой каталог перезаписываться отказывается. Источник (`data_dir`) задаётся
файлом `--config`, отдельного флага источника нет; реальная точка входа —
`crates/shamir-server/src/main.rs` (`Subcmd::Backup { to }` → `backup::backup`).

> ⚠ **Нельзя запускать `backup` против живого (запущенного) сервера.**
> Никакой блокировки или quiesce-механизма нет: фразы вроде «блокирует данные
> на время snapshot» в старых версиях этого руководства были **неверны**. Защита
> от torn-write здесь — **только** отсутствие конкурирующего writer'а благодаря
> остановленному серверу. Копия, снятая на фоне работающего сервера (mid-WAL-write
> / mid-SST-flush), может быть повреждена и **молчаливо непригодна к
> восстановлению** — дефект всплывёт только при restore.

In-process online-snapshot без downtime — запланированное улучшение **P2**,
пока не реализовано
(см. комментарий-модуля `crates/shamir-server/src/backup.rs`, «Future enhancement
(P2)»).

**Scheduled backup** — cron / systemd timer, но задание **обязано** сначала
остановить сервер, затем снять копию, затем поднять его обратно. Пример обёртки
для systemd: один `.service` с `ExecStartPre=systemctl stop shamir-server`,
`ExecStart=shamir-server ... backup --to ...`, `ExecStopPost=systemctl start
shamir-server`; либо wrapper-скрипт `stop → backup → restart`. Наивный cron,
запускающий `backup` против всё ещё живого сервера, — в точности тот риск,
которого избегает это правило.

**Restore.** `shamir-server --config db.ktav restore --from <to>/<YYYYMMDD_HHMMSS>`
— только при остановленном сервере (проверяется блокировка `data_dir`;
`--force` её пропускает). Команда сверяет `manifest.json` (sha256 каждого
файла), атомарно подменяет `data_dir` (прежний сохраняется как
`.pre_restore_backup_<timestamp>`) и инвалидирует все resumption-тикеты.
Счётчики lockout не сбрасываются. Не восстанавливай простым `cp -r`: так
пропускаются проверка манифеста и инвалидация тикетов.

**Point-in-time recovery нет:** восстановление возможно только к моменту снимка.

**Проверка целостности.** `shamir-server --config db.ktav doctor [--apply]`
(offline) — диагностика индексов и счётчиков, с `--apply` — перестройка.

### Durability (fsync) гарантии backup/restore (F-12, #802)

Backup и restore теперь явно fsync'ируют каждый скопированный файл, manifest и
каталог после каждого atomic rename, чтобы данные реально достигали stable
storage до возвращения успеха (а не оставались в OS page cache):

- **`copy_dir_recursive`** (общий для `backup()` и `restore()`'s step 3):
  после `fs::copy` файл переоткрывается на запись и вызывается `sync_all()`.
  Ошибка файлового sync'а **пропагируется** как `BackupError::Io` /
  `RestoreError::Io` — корректность содержимого файла IS the point (по
  прецеденту `segment_meta.rs`, где файловый `sync_all` тоже жёстко
  протагируется).
- **`write_manifest`**: `fs::write` заменён на `File::create` + `write_all` +
  `sync_all` — manifest durable до того, как `backup()` вернёт успех.
- **`restore()` step 5**: после КАЖДОГО успешного `fs::rename` (оба
  swap-rename, rollback-rename и single fresh-target rename) вызывается
  `fsync_dir(parent)` — fsync содержащего каталога, чтобы directory-entry
  update самого rename'а был durable. Это закрывает классический gap «you
  fsync'd the file but not the directory» на ext4/xfs. Ошибка каталогового
  fsync **только логируется**, не пропагируется (по прецеденту
  `wal_segment.rs::fsync_parent_dir` — деградация power-loss window, но не
  corruption); `RestoreError` не получил нового варианта.

**Windows / non-unix caveat:** `fsync_dir` — documented no-op (этот workspace
уже решил в `wal_segment.rs`'s `#[cfg(not(unix))]` stub, что Windows не требует
этой конкретной directory-entry durability guarantee). Истинное power-loss
crash-тестирование непортативно и не покрывается unit-тестом; регрессионный
guard — interrupted-copy тест в `tests/restore_tests.rs`, проверяющий ближайший
портативный прокси. См. `KNOWN_LIMITATIONS.md` §9.

## 9. Capacity planning

Оценки ниже выведены из структур в коде, а не измерены — сверяй с `/metrics` своей инсталляции (источник: `docs/dev-artifacts/ops/CAPACITY_PLANNING.md`, частично устарел). Кратко:

|| Подсистема | Память |
||---|---|
|| Idle процесс | ~50 MB |
|| Активная сессия | ~2 KB |
|| Активное соединение | ~8 KB |
|| Argon2 peak | `argon2_concurrent_max × memory_kb × 1.05` |
|| Steady-state | `50 MB + sessions × 2 KB + connections × 8 KB` |

10 000 сессий → ~134 MB (без Argon2 spike).

|| Операция | CPU |
||---|---|
|| Argon2id verify | ~50 ms (defaults) |
|| TLS handshake | ~5 ms |
|| Post-auth request | ~20 µs |
|| Batch query (cold) | ~1 ms / query |

## 10. Prometheus + Grafana

`deploy/README.md` содержит готовую конфигурацию Prometheus:

```yaml
scrape_configs:
  - job_name: shamir-db
    static_configs:
      - targets: ['127.0.0.1:9090']
```

И примеры alert-правил:
* `auth_attempts_total{result="bad_proof"}` > threshold → brute-force alert.
* `shamir_tx_aborted_ssi_total` growing → contention hot-spot.

## Что важно знать уже сейчас (дозированно)

* **Один бинарник — все платформы.** `service install` генерирует
  платформенный юнит автоматически. Не нужен отдельный пакет.
* **Observability включена по умолчанию** на `127.0.0.1:9090` (loopback).
  Отключается только явным `addr: ""`; публичный адрес требует
  `allow_public_metrics: true` и внешней аутентификации.
* **Graceful shutdown — bounded.** 30 s deadline и общий бюджет репозиториев
  остаются неизменными. Ошибка flush/clean horizon или истечение deadline —
  unclean shutdown: exit code **70**, один ERROR с именами неуспешных
  репозиториев и счётчик `shutdown_unclean_total`. Clean требует quiesce,
  drain, W = allocation high-water и сохранённого J. При следующем старте
  uncertified интервал (J,H] сертифицируется как conservative replication
  gap (пустой интервал не создаёт gap); OS reclaim'ает сокеты.
* **Single-instance guard.** Файловая блокировка (`fs4`) на `data_dir`.
  Два процесса на одних данных → второй не стартует.
* **Logging — non-blocking.** Никогда не блокирует горячий путь. Overflow
  — lossy (лучше потерять лог, чем запрос).

## Куда дальше

||| Упёрся в… | Поднимайся на |
||---|---|---|
|| «нужна децентрализация / P2P» | [Этаж 8 — Interconnect](./08-interconnect.md) |

### `[transactions]` — visibility policy

| Setting | Default | Meaning |
|---|---:|---|
| `visibility_ack_mode` | `"strict"` | Strict by default; `"bounded"` is accepted at startup and returns committed receipts |
| `visibility_ack_timeout_ms` | `5000` | Default Bounded commit budget; no effect on Strict. At most `max_visibility_wait_ms` |
| `max_visibility_wait_ms` | `30000` | Maximum explicit Bounded commit/reconciliation wait, including nested overrides; excessive requests fail before writes, never clamp |
| `allow_strict_visibility_wait` | `true` | Permit explicit Strict commit/reconciliation waits; does not forbid the configured Strict default when the commit override is absent |
| `read_visibility_timeout_ms` | `5000` | Allocated-version AsOf wait, capped by remaining execution budget |

All millisecond values accept zero (immediate predicate check, never infinity)
and must fit `0..=4294967295`. Strict waits intentionally retain indefinite-wait
risk. Bounded applies to explicit and implicit DML batches, including nested
Batch/ForEach and multi-repository batches. Inspect every write alias's `receipt`,
even when row results are suppressed. Receipt-less Call/admin writes fail closed.
For connection-resource liveness, prefer Bounded and handle Pending as committed,
not a retry signal. Dependent read aliases report `visibility_dependency_pending`
in their `value`, with the awaited position and `skipped: true`, without executing
the read. Data-flow consumers also stop; unrelated operations inside nested
bodies and across ForEach iterations continue. Same-connection order is not a fence.
The budget excludes inner commit (including async-index durability-error tail
awaits), admission and network backpressure; post-ack soft pressure is not a
hard memory admission guarantee. Reconcile Pending with WaitForVisibility and
open a new snapshot for reads. Materialization is independent of visibility.
Protocol v3 gating is unchanged; supported SDK adoption remains VIS-5 work.
Ktav uses `transactions: { visibility_ack_mode: strict ... }` with the same keys.
