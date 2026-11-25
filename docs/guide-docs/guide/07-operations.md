בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 7 — Эксплуатация: рантайм, метрики, логи, конфиг

**Когда подниматься:** выкатка в прод.

## Repository durability changes

Use `ALTER REPO` to patch a repository's durability override. Each successfully applied ALTER produces its own audit event, including operations inside sub-batches and loop bodies; row-result suppression does not suppress the event. Earlier successful operations remain audited if a later operation fails. Failed or suppressed operations do not generate success events.

До этого этажа сервер запускался вручную: `shamir-server --config …` в
терминале. Для разработки этого хватает. Но продакшен требует: сервисный
режим (systemd / Windows service), observability (`/healthz`, `/metrics`),
структурированные логи, rate-limit, audit trail. Этот этаж — о том, как
ShamirDB живёт в проде.

Server configuration is strict: unknown top-level sections are rejected, and `logging.level` is validated against supported levels. Correct section/key names and log-level values before startup; these mistakes are not silently ignored. See [PR1c migration](./PR1C-MIGRATION.md).

## 1. Один бинарник — все режимы
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
PW='admin-pass'
printf '%s\n' "$PW" | shamir-server --config /etc/shamir/server.ktav --bootstrap-password-stdin
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

### Per-repository durability

Repository durability can override the server default at creation or be patched while running with `ALTER REPO`. Overrides are sparse: unspecified fields inherit the server policy, and ALTER preserves previously stored fields not present in the patch. The policy is swapped live for the open repository and persisted in its `system.repositories` record; legacy records without a `durability` field mean no override. At boot, a stored override takes precedence over the server default.

Use the query builder for these operations (builder examples are not protocol wire specifications):

```ts
const create = ddl.createRepo('archive', { durability: { wal_commit: 'synced' } });
const alter = ddl.alterRepo('archive', { wal_retention_max_bytes: 1073741824 });
const described = ddl.describeRepo('archive');
```

`DESCRIBE REPO` строится через `ddl.describeRepo('archive')` и сообщает эффективную `durability` и sparse `durability_override`. Rust DTO запроса всегда передаёт retention десятичной строкой. TypeScript принимает положительные безопасные числа, `bigint` или канонические десятичные строки и без потери точности нормализует значения выше `i64::MAX` в текст. В эффективном ответе значения до `i64::MAX` включительно — native MessagePack integer, декодируемый в TypeScript как `number` или `bigint` согласно ширине wire integer; значения выше `i64::MAX` — десятичные строки. Неизвестные поля и недопустимые значения enum отклоняются. Retention должен быть не меньше удвоенного effective WAL segment size. `replication_serve` принимает `durable` (default) и `completed_reseed` (см. ниже). In-memory и hybrid WAL игнорируют `wal_commit`; выбор `synced` не делает их persistent.

### After a leader crash

После нештатной остановки cold open записывает консервативный GAP
`[J + 1, H]`, если восстановленный horizon H больше persisted clean horizon J.
Во время работы перед каждой попыткой удаления запечатанных WAL-сегментов
drainer фиксирует settled-префикс: сначала сохраняет и сбрасывает recovery floor
не ниже текущего J, затем сбрасывает события и GAP-журнал и продвигает J.
Если checkpoint завершается ошибкой, WAL-сегменты не удаляются. При превышении
лимита retained WAL режим `refuse_writes` отклоняет новые транзакции до успешного
безопасного удаления; режим `declare_gap` сначала надёжно записывает GAP и J с
соответствующим recovery floor, затем удаляет только сегменты ниже безопасной
границы history/interner. Если журнал недоступен, запись отклоняется. Для
in-memory/hybrid репозиториев без постоянного WAL checkpoint и ограничение
retention не применяются. Холодная сверка сохранившихся WAL-событий относится
к PR4, поэтому runtime-checkpoint не устраняет все возможные GAP после сбоя.
Follower при холодном старте (PR6c) сертифицирует журнал через J из своей
bookmark-записи: неопределённым остаётся только in-flight батч; без известного
J действует прежний консервативный gap.
Follower при чтении через известный GAP останавливается с `JournalGap`, не
применяя сопутствующие события и не продвигая bookmark.

Дополнительно действует универсальный identity-fence (S5c-X PR7) на КАЖДОЙ
подписке — с сидом и без: follower записывает `(lineage, history epoch)`
лидера при первом успешном Hello/Pull в один durable record со своим
bookmark; каждое продвижение bookmark сохраняет identity. Отсутствующая
identity запрещает даже первый контакт; отключения fence в production нет.
Старый bookmark без identity принимает её один раз с WARN. Follower терминально останавливается с `seed_wrong_identity` /
`seed_wrong_history_epoch` (структурированные `recorded_identity` и `observed_identity`
в `ReplicationStatus`),
если позже лидер ответит другой lineage или epoch. Перезапущенный
`in_memory`/`hybrid` лидер получает новый lineage (его `__tx__` живёт
только в RAM) и потому останавливает подписки; durable (`fjall`) лидер
сохраняет идентичность. Ремонт — операторский: проверить историю лидера и
пересидировать (см. `guide/09-seed-recovery.md`).

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

Источники: `crates/engine/shamir-engine/src/repo/repo_instance.rs:1829-1877`,
`crates/server/shamir-server/src/replication/follower_loop.rs:311-325`,
`crates/server/shamir-server/src/replication/supervisor.rs:309-334`,
`crates/engine/shamir-db/src/shamir_db/execute/admin_replication.rs:413-467`,
`crates/server/shamir-server/src/backup.rs:3-32`.

## API token listener operations

Bearer APIs and HTTP password login are served by the configured HTTP listener. See [the TypeScript token guide](../../../crates/client/shamir-client-ts/docs/TOKENS.md) for client usage. The examples there assume an HTTPS endpoint at `https://db.example.test`, port 443, with the UI served same-origin (or through a same-origin reverse proxy); token requests do not provide CORS headers.

A minimal illustrative example of **one `listeners[]` entry** for that deployment is:

```ktav
{
    kind: http
    addr: 0.0.0.0:443
    tls: true
    host_authority_allowlist: [db.example.test]
    browser_origin_allowlist: [https://db.example.test]
    trusted_proxies: []
    external_tls: false
    max_request_body_bytes: 16777216
    body_timeout_ms: 30000
    request_timeout_ms: 120000
    login: {
        enabled: true
        authority_allowlist: [db.example.test]
        ## Optional explicit stable identity; if omitted, generated identity is persisted in server metadata.
        listener_id: 0123456789abcdef0123456789abcdef
    }
}
```

A `kind: http` listener requires `tls: true` or `tls: false`; this example uses direct TLS termination and assumes a valid TLS 1.3 certificate. Login configuration contains only `enabled`, `authority_allowlist`, and optional `listener_id`; `external_tls` is a flattened listener-level field. `login.authority_allowlist` defaults to loopback aliases; the example sets the exact HTTPS authority. A supplied listener ID must be stable and exactly 32 lowercase hex characters. `browser_origin_allowlist` is listener-level, separate from `login`. Login BEGIN input is capped at 4096 bytes and FINISH at 256 bytes (not a 4 KiB cap on both messages).

| Key | Type | Default | Scope | Validation / semantics |
|---|---|---|---|---|
| `login.enabled` | bool | `false` | per listener | Disabled routes return 404; enabled wrong method returns 405. |
| `login.authority_allowlist` | array | loopback aliases | per listener | Exact canonical Host authority allowlist. |
| `login.listener_id` | string | generated once | per logical listener | Optional stable 16-byte identity, configured as exactly 32 lowercase hex characters; a generated ID persists in server metadata keyed by the configured listener `addr` (two listeners with the same `addr` share it; changing `addr` generates a new one). |
| `tokens.login_ttl` | u32 seconds | 86400 | global | Range 1–86400, additionally capped by `tokens.max_ttl_days`; invalid configuration fails startup. |
| `tokens.login_allow_read_write` | bool | `false` | global | Opt-in required for read-write login issuance; ordinary rights still apply. |
| `security.login_enumeration_hardening` | bool | `true` | global | HTTP login only; mismatching stored KDF tuples make accounts individually ineligible. Raising defaults may affect existing users; preflight and disclose impact. |

`login.authority_allowlist` is optional in configuration; when omitted on loopback, the launcher derives bound-port loopback aliases. Direct non-loopback TLS and proxy TLS require an explicit nonempty authority list. `login.listener_id` can be generated and persisted if omitted. Listener defaults: `max_request_body_bytes` 16 MiB, body/request timeouts 30 s/120 s, head timeout inherited from `security.connection.auth_init_timeout_ms` (typically 5 s); remote plaintext login must stay disabled.

Shared token policy: `tokens.max_per_user` is 20, `tokens.max_sockets_per_token` is 8, `tokens.max_sockets_per_owner` is 32, `tokens.session_max_age_hours` is 24, `tokens.default_ttl_days` is 30, and `tokens.max_ttl_days` is 365. These are global shared token settings, not login-only defaults.

During shutdown, login workers receive `min(request_timeout_ms, 7s)` and the audit consumer gets up to 2s within the outer 10s shutdown deadline. Detached workers may outlive the listener; runtime stop may interrupt issuance or compensation, leaving a token live until its TTL. Audit is best-effort; events abandoned on abort are not counted as dropped.

`login_enumeration_hardening` is under the actual `security` namespace, not a top-level key. `kdf_override` is forbidden on login-enabled listeners. Login's BEGIN/FINISH body caps (4096/256 bytes) and admission/store capacities are fixed, not tunable; server request body and timeout settings are separate. Actual proxy TLS requires `tls: false`, loopback bind, nonempty `trusted_proxies`, nonempty authority, and nonempty canonical HTTPS origins; combining actual TLS with `external_tls` is invalid. Login head timeout must be <=5000 ms. HSTS is enabled for login over direct TLS; with proxy TLS the external HTTPS edge must provide HSTS. The internal HTTP backend does not itself establish browser HTTPS security.

These names/defaults and profile constraints are defined by `crates/server/shamir-server/src/boot/config/login_listener.rs`, `http_listener.rs`, `boot/config.rs`, `boot/config/tokens.rs`, and enforced by `engine/server/server_launcher/http/launcher.rs`.

For full listener setup, token administration, browser constraints, limits and troubleshooting, see the [token API operator guide](./token-api.md).

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
|| `argon2_concurrent_max` | Лимит одновременно работающих серверных Argon2id KDF при создании пользователей. Обычный вход (проверка proof) не берёт permit вообще: серверный password KDF при нём не запускается; обычная смена пароля тоже. Исключение — первый вход по bootstrap-токену: он запускает ротацию credential, и она идёт вне гейта. Допускается не более `2 ×` этого значения задач (работающих и ожидающих; при максимальной capacity `1024` это `2 048`), остальные отклоняются сразу. Пул блокирующих потоков общий и по умолчанию равен 512, поэтому при значении выше 256 один backlog гейта больше пула и может оттеснить несвязанную блокирующую работу (WAL sync, HTTP-lookup verifier/snapshot). Отдельного пула нет. |
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
Источники: `crates/server/shamir-server/src/db_handler/config.rs::policy`,
`crates/query/shamir-query-types/src/batch/batch_limits.rs`.

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

Defaults и startup validation: `crates/server/shamir-server/src/config.rs:456-487,768-777`;
engine caps: `crates/query/shamir-query-types/src/batch/batch_limits.rs:109-120`.

`max_total_queries` ограничивает число узлов всего дерева батча;
`max_total_work` ограничивает и статическую оценку вложенных батчей/циклов,
и **runtime dispatch**: `ExecScope::charge_work(1)` списывает единицу перед
каждым реально выполняемым alias, включая Batch/ForEach-контейнер и их
вложенные dispatch. Пропущенный по `when` alias не списывает work.
Динамические повторения тоже учитываются; все вложенные исполнения делят
один root counter, предварительный `check_work` не резервирует работу.
Это счётчик dispatch, не строк, CPU-инструкций или времени сканирования
(`crates/engine/shamir-engine/src/query/batch/exec_scope.rs`, `batch_execute.rs`,
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
`crates/server/shamir-server/src/db_handler/config.rs`,
`crates/query/shamir-query-types/src/batch/exec_policy.rs`,
`crates/engine/shamir-db/src/shamir_db/shamir_db/db_gateway.rs`).

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
`crates/engine/shamir-engine/src/query/batch/exec_scope.rs::charge_bytes`,
`result_size.rs`, `batch_execute.rs`, `query_runner.rs`.

### Время TxExecute и string_agg

Абсолютный срок interactive handle — 300 s. Budget одного `TxExecute` —
минимум effective request/policy time и оставшегося срока handle **с полной
точностью `Duration`**, без округления до секунд и без продления на каждый
вызов. Остаток берётся **после** захвата context lock; authorization тоже
вычитается из него (`crates/server/shamir-server/src/db_handler/tx_handlers.rs:169-181`,
`crates/engine/shamir-db/src/shamir_db/execute/db_tx.rs:158-165`,
`crates/engine/shamir-engine/src/query/batch/exec_scope.rs`). Deadline cooperative:
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
`crates/engine/shamir-engine/src/query/read/aggregate.rs::CappedStringAgg`,
`crates/engine/shamir-engine/src/query/filter/eval_context.rs`,
`crates/engine/shamir-engine/src/query/batch/query_runner.rs:1194`.

### Migration source freeze и исход commit

При подготовке миграции source table замораживается для data writes:
raise freeze bit → drain writers → write-lock fence. Freeze guard живёт до
завершения/отмены миграции; отмена подготовки снимает bit. Это не удержание
DDL admission на весь срок: DDL остаётся разрешённым. Direct writes и commit
транзакции, попавшие на freeze, получают `table_migrating`; не делайте busy
retry, дождитесь завершения/отмены и заново разрешите активную таблицу
(`crates/engine/shamir-engine/src/table/table_manager.rs::begin_migration_freeze`,
`table_manager_crud.rs`, `crates/engine/shamir-engine/src/tx/pre_commit.rs`,
`crates/engine/shamir-db/src/shamir_db/execute/admin_migration.rs`).

`wal_requires_reopen` требует reopen/reconciliation, новые commits отказаны.
Попытка commit с неизвестным исходом WAL (frame уже поставлен в очередь)
возвращается как `status: "indeterminate"`, `error: "wal_outcome_unknown"`, с ATTEMPTED
`commit_version` (без claim успеха) и `snapshot_version`; reopen и сверка чтением решают, что сохранилось.
Отказ ДО admission (repository уже остановлен) остаётся `status: "aborted"`.
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
`wal_requires_reopen` remain errors with their original reason, and an abort
exposes the failing op's stable code in `transaction.abort_code` when one
exists (`crates/query/shamir-query-types/src/ops/batch/req/transaction_info.rs:25-31`); indeterminate
WAL is not proof that writes are absent and is reported as
`status: "indeterminate"`. `transaction.reason` is not uniformly debug text:
`skip_aborted` is the one plan-abort special case that is written into
`reason` as its own code
(`crates/engine/shamir-engine/src/query/batch/runner/query_runner/helpers.rs:155-161`),
and commit-time classifications also populate `abort_code` now — `tx_conflict`,
`unique_violation`, `version_conflict`, `table_migrating`,
`wal_retention_exceeded`, `commit_admission_refused`, `wal_requires_reopen`,
`cancelled`
(`crates/engine/shamir-engine/src/tx/commit/pipeline/phase/commit/entry.rs:398-463`).
Inside `for_each`, a transactional body's `indeterminate` outcome stops the
loop with `wal_requires_reopen` (the iteration may have committed, so the
prefix is unknown and must be reconciled, not retried), while a committed
prefix is NOT rolled back when a later iteration fails
(`crates/engine/shamir-engine/src/query/batch/runner/query_runner/helpers.rs:35-94`,
ForEach call site `crates/engine/shamir-engine/src/query/batch/runner/query_runner/arms_read.rs:488-499`).
A transactional `Batch` sub-batch surfaces its aborted or indeterminate commit
the same way instead of an empty child value; the sub-batch caller has no
committed-prefix knowledge, so an abort is always reported as
`transaction_aborted` with its cause in the text
(`crates/engine/shamir-engine/src/query/batch/runner/query_runner/arms_read.rs:170-178`).
No value-only transaction convenience
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
committed, and upgrades neither durability nor `async` index visibility.

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
invent missing positions.

### Клиентские квитанции неявных батчей и changes_since (VIS-5b)

Не-транзакционный батч с записями возвращает квитанцию на КАЖДЫЙ write-алиас
в `QueryResult.receipt`. Квитанция — это ИЗВЕСТНОЕ committed-решение:
`pending` — не ошибка и не команда на повторную отправку. Rust:
`client.execute_with_receipts(db, batch, visibility_wait)` возвращает ответ и
`Vec<AliasReceipt>` (alias, transaction, visibility, repo); репозиторий
выводится из table-ref запроса. Если репозиторий вывести однозначно нельзя
(алиас внутри вложенного sub-batch/for-each) — `repo: None`, и
`reconcile_receipts` ОТКАЗЫВАЕТСЯ угадывать типизированной ошибкой.
TS: `client.executeWithReceipts(db, batch, visibilityWait?)` — то же самое;
подавленный алиас (`return_result: false`) приходит «оболочкой» без строк, но
с квитанцией — читайте её из оболочки, а не из строк.

Reconciliation группирует pending-квитанции по репозиторию и делает ОДНО
наблюдение на репозиторий на максимальном pending-позиции; ни одной записи
не отправляется повторно, циклов нет. Позиции привязаны к ТЕКУЩЕЙ инкарнации
репозитория: удаление и повторное создание под тем же именем — это другой
репозиторий, wire этого не различает — не мирите квитанцию через
delete/recreate. После наблюдения открывайте НОВЫЙ снапшот.

```ts
const { receipts } = await client.executeWithReceipts('db', batch, { mode: 'bounded', timeout_ms: 0 });
const pending = receipts.filter((r) => r.visibility.state === 'pending');
// один вызов waitForVisibility на репозиторий, на максимальном позиции:
for (const r of pending) {
  if (r.repo === null) throw new Error('cannot derive repository; refusing to guess');
}
```

`changes_since` теперь типизирован: Rust `client.changes_since(db, repo,
cursor, limit)` (запрос строит query-builder) и TS
`client.changesSince(db, repo, cursor, limit?)` возвращают
`{ changes_since, events, gap_at, visible_through }`. Клиент декодирует u64
точно (`u64` в Rust DTO — включая десятично-строковую форму
`QueryValue::Big`; `bigint` в TS; курсор принимает `number | bigint`,
значения выше 2^53 — только `bigint`). ОГОВОРКА: сервер сегодня семплирует
и кастирует эти позиции через `i64`, поэтому значения выше `i64::MAX` он
сейчас не производит — lossless-декодирование это клиентская гарантия, а не
свойство текущего wire. `visible_through` — выборка settled-префикса W ДО чтения
журнала: это образец settlement, а НЕ сертификат покрытия. Пустой `events` —
не доказательство abort/vanish pending-записи; подтверждение журнала может
отставать от W; события по-прежнему ограничены cursor/limit/gaps/retention.
НЕ двигайте курсор событий до `visible_through` только на основании этого
поля; `WaitForVisibility` остаётся основным reconciliation-API, `gap_at` и
retention требуют ресинхронизации.

### Серверная durability и диагностика replication lag (PR1)

Блок `durability` задаёт серверный default (см. [Этаж 2](./02-durability.md)):
`wal_commit: buffered`, `wal_retention_max_bytes: 1073741824`,
`on_wal_retention_exceeded: refuse_writes`, `replication_serve: durable`.
`wal_commit: synced` ждёт coalesced WAL fsync; клиентский `buffered` его
не ослабляет. Для файлового WAL `buffered` ack — локальный WAL write:
падение процесса не равно потере питания. Фоновый таймер 250 мс номинальный,
**не hard maximum**; проверяйте storage errors и задержки scheduler/I/O.
In-memory/hybrid WAL не persistent; tier игнорируется с одним warn при open.

Boot проверяет enum/неизвестные вложенные ключи и retention floor
`>= 2 × effective segment bytes` через общий resolver, включая env override
`SHAMIR_WAL_SEGMENT_MAX_BYTES`, до init базы. Верхнеуровневый детектор
похожих на durability опечаток не добавлен. `resolve` принимает и
`completed_reseed` (см. ниже).
Retention budget и overflow action **только parsed/validated до PR2**:
не считайте 1 GiB enforced лимитом диска, `declare_gap` пока только warns.

Durable Pull ограничен `min(confirmed, W, R)`, где R — непрерывный префикс
recoverability (не max fsynced version). Pull запрашивает coalesced sync
по требованию и ограниченно ждёт продвижения R; ошибки/таймауты сохраняют
старый safe bound. Lag может включать journal confirmation, uncovered WAL
prefix и собственно скорость apply follower; это разные причины.

### replication_serve: durable и completed_reseed (S5c-X PR8)

`durability.replication_serve` задаётся как серверный default (блок
`durability`, `deny_unknown_fields`) и как per-repository override
(`CREATE`/`ALTER REPO` `durability`). Режимы:

- `durable` (DEFAULT): отдача ограничена `min(confirmed, W, R)`, где
  R — непрерывный префикс recoverability (WAL fsync coverage).
- `completed_reseed`: отдача ограничена `min(confirmed, W)` — completed
  prefix БЕЗ R-clamp. Выбирайте его, когда минимальная replication latency
  важнее, и вы принимаете reseed-after-crash. Это
  "safe convergence, not lossless replication: speculative observations
  may be retracted, and an unclean restart conservatively costs a reseed
  even if the bytes survived."

Модель lifecycle/epoch под `completed_reseed`:

1. До первой speculative страницы пишется и сбрасывается durable lifecycle
   record `Armed` (per-repo, в `__tx__` info store; атомарный fast path
   избавляет от его повторного чтения в каждом pull).
2. Чистая остановка (admission stopped, writes settled, durable data/WAL,
   journal horizon через `record_clean_horizon`, allocation evidence)
   пишет сертификат `Clean` ПОСЛЕ того, как всё это зафиксировано. Под
   `durable` shutdown ничего не пишет.
3. При открытии под `completed_reseed` запись `Armed` без более позднего
   `Clean` = unclean restart: recovery завершается приватно, затем
   `history_epoch` инкрементируется durably (checked add; неудачный
   bump/flush БЛОКИРУЕТ admission, overflow тоже), старая replication
   continuity инвалидируется (lineage остаётся STABLE — двигается только
   epoch), после чего service допускается. Это стоит reseed, даже если
   WAL-байты пережили сбой — консервативно by design.
4. Fencing (PR7): каждая subscription/bookmark привязана к lineage+epoch
   (fence выполняется на Hello и перед каждым Pull, до любого
   decode/apply). Follower со старым epoch в bookmark получает отказ;
   supervisor помечает подписку `resync_required` (counter
   `shamir_repl_resync_required_total`) и она остаётся в карантине до
   установки replacement seed. Legacy peers без lineage/epoch identity
   отвергаются (`MissingIdentity`), fail-closed.
5. Переключение `durable` → `completed_reseed` при существующей записи
   `Armed` безопасно: следующий fast-mode open видит Armed-without-Clean
   и делает bump один раз. Обратное переключение не стирает `Armed`; под
   `durable` он инертен.
6. In-memory и hybrid: ничего не переживает рестарт (identity store
   RAM-backed — каждый reopen, чистый или нет, выпускает новую
   incarnation/new lineage); для репликации такие репозитории
   обслуживаются только с явным `completed_reseed`.
7. Карантин исключает обычные чтения (ordinary reads, PR8b): пока подписка
   в `resync_required`, чтения (get/query/scan, включая temporal
   AsOf/History) и локальные changefeed/journal pull покрытых репозиториев
   возвращают ошибку `resync_required` (non-retryable). Карантин хранится
   как durable-маркер `quarantined` на строке подписки (плюс baseline
   `resync_from_receipt`), поэтому он ПЕРЕЖИВАЕТ Pause/SetProfile: Pause,
   SetProfile и обычный `Resume` карантинной подписки отклоняются той же
   типизированной ошибкой `resync_required` — сменой состояния карантин не
   снять. Оператор видит причину через `ReplicationStatus`/
   `ListSubscriptions` (state + status_message), DESCRIBE и статус остаются
   читаемыми. Порядок действий оператора: 1) увидеть карантин через
   status/list; 2) `DROP` карантинной подписки; 3) установить replacement
   seed настоящим установщиком по runbook (`09-seed-recovery.md`) — в НОВЫЙ
   каталог данных follower'а (установщик отклоняет существующую обычную
   репозиторий/БД назначения — `check_seed_target` в `seed/catalogue.rs`) —
   он создаёт СВЕЖУЮ paused-подписку с новым `seed_receipt` (без маркера);
   4) `Resume` новой подписки; 5) первое чтение после этого успешно
   (автоматического reseed нет). На healthy path стоимость — один
   атомарный load на batch.

`wal_commit` (`buffered|synced`) независим от `replication_serve`:
запросная `durability` может УСИЛИТЬ подтверждение, но никогда не может
ослабить serve-политику репозитория. WAL-less follower apply path
сохраняет durable-floor-before-history ordering независимо от режима.

Явная админ-установка `set_durability` с `wal_commit: synced` на
in-memory/hybrid репозитории (нет persistent WAL) НЕ отвергается:
`synced` игнорируется — тихое понижение до `buffered` через
`for_wal_backend` с одним warn в лог на каждый open; `completed_reseed`
на них разрешён (и обязателен явно при publication), `durable`
разрешён на engine-уровне и проверяется при publication time.

Стоимость (hot path): commit path не изменён; serve path добавляет один
атомарный load policy/mode на каждую replication page read и, при первом
speculative pull после открытия, одну запись Armed в store с flush;
unclean-open epoch bump — это одна запись metadata-ключа + flush на
recovery path, никогда на commit path.

Наблюдаемость: `DESCRIBE REPO` теперь несёт `durability` (effective,
включая `replication_serve`), `durability_override`, `lineage_id` (hex) и
`history_epoch`; gauges `shamir_replication_serve` (effective mode),
`shamir_replication_served_bound`, `shamir_wal_watermark_w` и новый
counter `shamir_repo_history_epoch_bumps_total` (плюс существующие
counters identity-mismatch и новый resync-required).

В текущем exporter все durability series имеют **только labels `db` и `repo`**.
Источники принадлежат конкретному repo и его WAL; clones разделяют одно
состояние. `shamir_wal_effective_commit` сохраняет configured tier
(0 buffered, 1 synced), несмотря на историческое имя: Mem WAL игнорирует
configured Synced и использует effective Buffered без sync demands.
`shamir_replication_serve` (0 durable, 1 completed_reseed — effective mode
репозитория) и
`shamir_wal_backend_mem` (1 для initialized Mem WAL, 0 для
file/uninitialized) семплируются при scrape, без открытия WAL или stores.
`shamir_wal_synced_lag_versions` и `shamir_wal_pending_versions` —
**span W − R**, не количество sparse coverage entries.
`shamir_wal_first_pending_version` — R + 1 при ненулевом span, иначе 0;
`shamir_wal_first_pending_age_ns` — нижняя оценка возраста с первого
наблюдения этой позиции scrape, **не** время allocation/commit. Смена
первой uncovered позиции сбрасывает её observed age.

`shamir_wal_sync_now_calls` считает demands сразу при регистрации, включая
already-covered и cancelled calls. `shamir_wal_sync_rounds` считает
завершённые rounds; `shamir_wal_sync_coalesced` — demands, наблюдавшие
активного worker до своего start (не calls − rounds и не размер очереди).
`shamir_wal_sync_errors` включает failed rounds и unwind/cancellation worker;
`shamir_wal_sync_duration_ns` — cumulative время snapshot replay и sink sync,
не только syscall fsync. Pending demands уже включены в calls.
`shamir_replication_pull_held_back_ns` и
`shamir_replication_pull_held_back_count` — total-ns/count ожиданий R,
включая cancelled waits, **не latency histogram**.

`shamir_wal_watermark_w`, `shamir_wal_recoverable_through` — текущие
наблюдения initialized WAL; `shamir_replication_confirmed_bound` и
`shamir_replication_served_bound` — последний pre-clamp/served sample pull
этого repo. Разные gauges/counters не являются единым атомарным снимком
и не заменяют fencing/coverage certificate. Counters экспортируются через
`absolute`, поэтому повторный scrape не удваивает totals. При reopen или
замене repo с теми же labels source totals сбрасываются, но recorder хранит
старый максимум: меньшие totals скрыты, пока не превысят его (или до restart
recorder). Старые labelled series могут оставаться после удаления repo.

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

Prometheus text format. `/metrics` требует рабочего Prometheus recorder; процессный recorder общий для одного процесса и устанавливается один раз. Если второй observability-enabled сервер в том же процессе не смог установить собственный recorder, он всё равно запускает listener с fallback без handle: его `/metrics` отвечает `503 Service Unavailable`, а не выдаёт метрики первого сервера. Метрики:

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
|| `shamir_visibility_ack_total{mode,result}` | counter | Acknowledgement outcomes: `mode=strict\|bounded`, `result=visible\|pending_timeout\|pending_reopen\|error` (VIS-4c) |
|| `shamir_visibility_ack_wait_seconds_bucket{mode,result,le}` | gauge (cumulative histogram) | Duration of acknowledgements that ACTUALLY waited; the non-blocking fast path is counted only in `shamir_visibility_ack_total` |
|| `shamir_visibility_ack_wait_seconds_count{mode,result}` | gauge | Observed waits per label set |
|| `shamir_visibility_pending_total{reason,kind}` | counter | Pending outcomes: `reason=timeout\|reopen_required`, `kind=explicit_tx\|implicit_batch\|direct_write` |
|| `shamir_visibility_read_wait_timeouts_total` | counter | AsOf reads that timed out waiting for visibility |
|| `shamir_visibility_reconciliation_total{result}` | counter | `WaitForVisibility` outcomes |
|| `shamir_visibility_reconciliation_wait_seconds_bucket{result,le}` | gauge | Reconciliation wait durations |
|| `shamir_visibility_ack_waiters{db,repo}` | gauge | Acknowledgements currently parked (reuses the completion tracker's waiter registration) |
|| `shamir_gate_*` | gauge | REPO-DIAGNOSTICS per-repo gate snapshot (allocation high water, visible floor, oldest blocker, …) |
|| `shamir_wal_size_bytes{db,repo}` | gauge | Приблизительный размер WAL на диске: активный сегмент + запечатанные удержанные (Mem-бэкенд пишет 0) |
|| `shamir_wal_segments{db,repo}` | gauge | Удержанные файлы сегментов WAL: запечатанные + один активный (Mem-бэкенд пишет 0) |
|| `shamir_wal_truncation_lag_versions{db,repo}` | gauge | Отставание truncate: max committed version минус старейшая усекаемая версия |
|| `shamir_group_commit_waiters{db,repo}` | gauge | Участники группового коммита, ожидающие покрытия WAL fsync |
|| `shamir_wal_bytes_total{db,repo}` | counter | Байты полезной нагрузки WAL с момента создания каталога WAL |
|| `shamir_wal_fsync_seconds{db,repo}` | histogram (`_bucket`/`_count`/`_sum`) | Длительность раунда sync-worker: replay снапшота + fsync приёмника (тот же интервал, что `shamir_wal_sync_duration_ns`); `le` и `_sum` — в секундах |
|| `shamir_index_entries{db,repo,table,index}` | gauge | Записи в индексах, снимок за scrape (усечение до 64 таблиц / 32 индексов на репозиторий) |
|| `shamir_index_build_seconds{db,repo,kind}` | histogram (`_bucket`/`_count`/`_sum`) | Длительность завершённой сборки индекса, в секундах |
|| `shamir_index_rebuild_seconds{db,repo,kind}` | histogram (`_bucket`/`_count`/`_sum`) | Длительность завершённого rebuild (doctor/repair, миграция identity), в секундах |
|| `shamir_interner_names{db,repo}` | gauge | Число интернированных имён полей |
|| `shamir_interner_high_water{db,repo}` | gauge | Персистентный high-water mark интернера |
|| `shamir_export_tables_truncated{db,repo}` | gauge (1/0) | Усечение по лимитам кардинальности активно |
|| `shamir_export_tables_known{db,repo}` | gauge | Всего открытых таблиц, известных репозиторию |
|| `shamir_backup_started_total{kind}` | counter | Запущенные online-backup прогоны (`kind=online`) |
|| `shamir_backup_completed_total{kind}` | counter | Успешно завершённые online-backup прогоны (`kind=online`) |
|| `shamir_backup_failed_total{kind,stage}` | counter | Неудачные online-backup прогоны по стадии сбоя (`stage=preparing\|captured\|copying\|sealing\|verifying`) |
|| `shamir_backup_active` | gauge (0/1) | 1, пока online-backup выполняется |
|| `shamir_backup_bytes_copied` | gauge (bytes) | Кумулятивные байты, скопированные завершёнными online-backup прогонами |
|| `shamir_backup_duration_seconds` | gauge (seconds) | Длительность последней **успешной** online-backup копии (не выставляется до первого успеха) |
|| `shamir_backup_writer_block_seconds{phase}` | gauge (seconds) | Последнее закрытое окно блокировки writer'ов (`phase=capture\|seal`), включая неудачные удержания; не выставляется до первого закрытого окна |
|| `shamir_backup_pin_seconds` | gauge (seconds) | Длительность удержания truncation-pin; не экспортируется, пока не измеряется |
|| `shamir_backup_wal_segments_copied_total` | counter | Скопированные сегменты WAL; не экспортируется, пока протокол не отдаёт счётчик сегментов |

Серии WAL/storage (R4a-2) — снапшоты за scrape, метки только `db`/`repo`
(у bucket-серий дополнительно `le`):

`shamir_wal_size_bytes` — приблизительный размер WAL на диске в байтах
(активный сегмент + запечатанные удержанные; Mem-бэкенд пишет 0). Норма —
ограничена конфигурацией retention; рост сверх неё — проверить удержание
сегментов.

`shamir_wal_segments` — число удержанных файлов сегментов (запечатанные +
один активный), безразмерное. Норма — от единиц до десятков; непрерывный рост
означает, что truncate не успевает за seal.

`shamir_wal_truncation_lag_versions` — отставание truncate в версиях (0, когда
усекать нечего), безразмерное. Норма — небольшая; устойчивый рост — накопился
backlog на truncate, проверить фоновую очистку сегментов.

`shamir_group_commit_waiters` — число участников группового коммита,
приостановленных в ожидании покрытия WAL fsync, безразмерное. Норма — меньше
параллелизма коммитов; устойчивый рост указывает на узкое место fsync.

`shamir_wal_bytes_total` — кумулятивный счётчик байтов полезной нагрузки,
добавленных в WAL с момента создания каталога WAL. Растёт пропорционально
скорости записи; по нему оценивают интенсивность записи (rate()).

`shamir_wal_fsync_seconds` — гистограмма длительности раунда sync-worker
(replay снапшота + fsync приёмника — тот же интервал, что
`shamir_wal_sync_duration_ns`) в секундах: кумулятивные `_bucket` с меткой
`le` (`+Inf` последней), `_count`, `_sum` — сумма в секундах (внутренние
ячейки — наносекунды). Норма на локальном диске — суб-миллисекундная. Серии
не экспортируются, пока WAL не инициализирован (`fsync == None`); «рваный»
снапшот (`torn`) отбрасывается вместо блокировки scrape.

Серии индексов и интернера (R4a-2) — снапшоты за scrape, берутся из уже
открытых таблиц/индексов; закрытые репозитории, таблицы и индексы невидимы —
то же ограничение open-registry, что у `shamir_gate_*` и degraded-index.

`shamir_index_entries{db,repo,table,index}` — gauge числа записей в индексе,
снимается при scrape по живым счётчикам записей. Норма — порядок числа строк
таблицы. Кардинальность ограничена: не более 64 таблиц и 32 индексов на
репозиторий с полными метками; выбор — по текущему числу записей (tie-break —
имя таблицы, детерминированно), остальные агрегируются в одну строку
`table="(other)"` на репозиторий. Лимиты 64/32 — константы, а не настройки.

`shamir_index_build_seconds{db,repo,kind}` — гистограмма длительности
завершённой сборки индекса в секундах, с меткой `kind`; наблюдение один раз на
завершённую сборку (на горячем пути — только чтение часов и observe).

`shamir_index_rebuild_seconds{db,repo,kind}` — та же гистограмма для
завершённого rebuild (doctor/repair и миграция identity), те же метки.

`shamir_interner_names{db,repo}` — gauge числа различных интернированных имён
полей (чтение длины map без итерации). Норма — рост с числом различных имён;
intent для алерта — неограниченный рост без новых полей.

`shamir_interner_high_water{db,repo}` — gauge персистентного high-water mark
интернера; значение монотонно почти не убывает.

`shamir_export_tables_truncated{db,repo}` — gauge 1/0: усечение по лимитам
кардинальности активно (есть таблицы сверх 64/32, попавшие в `(other)`).

`shamir_export_tables_known{db,repo}` — gauge общего числа открытых таблиц,
известных репозиторию; вместе с `_truncated` показывает, сколько осталось за
лимитом (`_known`, не `_total` — суффикс `_total` зарезервирован за counter'ами).

Версии, ожидающие GC, уже экспортируются как `shamir_gate_pending_versions` —
отдельная серия `shamir_gc_pending_versions` не создаётся.

`shamir_query_duration_seconds` — гистограмма длительности операций чтения (`read`), записи (`write`), пакетного запроса (`batch`), commit транзакции (`tx`) и dispatch вызываемой функции (`function`), в секундах, с метками `db`,`repo`,`op`; bucket-серии дополнительно имеют `le`, `_count` — число наблюдений, `_sum` — сумма секунд. Read/write измеряют span leaf-dispatch, batch — весь запрос, tx — span commit, а function — dispatch вызова callable, отдельно от внешнего вызова WASM, учитываемого только на глубине 0. Контейнерные и дочерние spans отражаются разными `op` и не складываются между метками. Типичный диапазон — 1 мс–1 с; метки ограничены открытыми `db`/`repo`, без таблиц, функций, пользователей и кодов. Состояние хранится в движке и опрашивается сервером; рваный снимок гистограммы пропускается, учитываются только открытые репозитории.

`shamir_query_errors_total{db,repo,code}` — counter ошибок запросов, увеличиваемый один раз при отображении ошибки leaf-операции; `code` ограничен статическим allowlist, включая `other`, поэтому пользовательские коды не создают меток. Обычно близок к нулю; всплески указывают на инцидент.

`shamir_query_rows_viewed_total{db,repo}` — кумулятивный counter строк, реально просмотренных движком при выполнении запросов, а не число строк в финальном результате; строки, отброшенные фильтром, вытесненным в поток хранилища, движок не видит и не учитывает. Обновляется без горячих аллокаций; ожидаемо пропорционален объёму сканирования.

`shamir_query_rows_returned_total{db,repo}` — кумулятивный counter строк, фактически возвращённых запросами после обработки; это отдельное значение от просмотренных строк и оно обычно близко к размеру запрошенных страниц.

`shamir_shapes_tracked{db,repo}` — гейдж числа строк статистики форм, отслеживаемых для репозитория на момент scrape: без аккумулятора `(other)` и не больше 256. Отдельных серий у форм нет, меток пользователя или сеанса тоже нет; значение относится к конкретному узлу и обнуляется вместе с его реестром форм.

`shamir_tx_aborted_unique_total` — counter транзакций, прерванных конфликтами уникальности, из существующего `TxMetrics`; обычно редок.

`shamir_tx_aborted_phantom_total` — counter транзакций, прерванных phantom-конфликтами, из существующего `TxMetrics`; обычно редок.

`shamir_tx_materialization_deferred_total` — counter commit-транзакций с отложенной материализацией, из существующего `TxMetrics`; ожидаемо не выше темпа транзакций.

`shamir_tx_post_wal_orphans_total` — counter операций, оставшихся orphan после WAL, из существующего `TxMetrics`; в штатной работе равен нулю.

Серии нагрузки снимаются при каждом запросе `/metrics` с источников текущего запуска сервера. Имена `shamir_inflight_response_bytes_used` и `shamir_inflight_response_bytes_cap` сохранены для совместимости, без алиасов; глобальный опрос бюджета раз в 5 секунд больше не используется.

| Метрика | Тип | Метки | Единицы и смысл | Норма | Что означает отклонение |
|---|---|---|---|---|---|
| `shamir_connections_active` | gauge | — | Число соединений, удерживающих guard `ConnLimiter`, даже при лимите `0`. Включает начальный handshake и HTTP-сокеты с guard, но не listener observability. | `0..limit` при конечном лимите; иначе от `0`, по нагрузке. | Устойчивое приближение к конечному лимиту означает риск отказов при новых подключениях. |
| `shamir_connections_limit` | gauge | — | Соединения: единый глобальный лимит для всех listeners, обслуживающих данные; `0` — без ограничения. | Настроенное значение, постоянное в рамках запуска. | Неожиданное значение, в том числе `0`, — повод проверить конфигурацию. |
| `shamir_connections_rejected_total` | counter | — | Отказы: по одному на неудачную попытку получить guard из-за глобального лимита; отказы по лимиту на IP не входят. Сбрасывается при перезапуске. | Прирост и скорость прироста обычно равны `0`. | Положительный прирост означает, что глобальный лимит уже отсекает подключения. |
| `shamir_inflight_response_bytes_used` | gauge | — | Зарезервированные байты бюджета ответов, не фактический RSS. `0`, если бюджет не ограничен или не отслеживается. | `0..cap` при конечном бюджете. | Близость к `cap` вместе с устойчивым ожиданием указывает на нехватку бюджета. Возможно временное превышение: небольшое при уточнении резерва либо при резервировании сверх `cap` на пустом бюджете. |
| `shamir_inflight_response_bytes_cap` | gauge | — | Байты: настроенный предел бюджета ответов; `-1` — бюджет не ограничен или отсутствует. | Настроенное значение, постоянное в рамках запуска. | Неожиданное значение, в том числе `-1`, — повод проверить конфигурацию. |
| `shamir_response_budget_waiters` | gauge | — | Число одновременных попыток резервирования, перешедших на путь ожидания. Доступность бюджета проверяется повторно; порядок не FIFO. | `0`. | Устойчивое ненулевое значение означает, что запросы регулярно ждут освобождения бюджета. |
| `shamir_post_auth_rate_limit_rejected_total` | counter | — | Запросы нативного протокола и токенных REST/WS, отклонённые посессионным лимитом частоты после аутентификации. Не включает отказы на входном gate, при аутентификации или по scope/политике. | Прирост и скорость прироста обычно равны `0`. | Положительный прирост означает, что запросы упираются в посессионный лимит. |
| `shamir_argon2_permits_capacity` | gauge | — | Ёмкость boot-семафора серверного Argon2id-KDF; равна настроенному `argon2_concurrent_max`, без меток, постоянна в рамках запуска. | Настроенное `argon2_concurrent_max`. | Значение ниже ожидаемого — проверить конфигурацию. |
| `shamir_argon2_permits_available` | gauge | — | Число свободных разрешений boot-семафора на момент scrape; значение может быть неточным на коротком промежутке из-за optimistic acquire и клампинга. | `0..argon2_concurrent_max`. | Устойчивый `0` указывает на давление на capacity. Новые запросы ожидают разрешение только после успешного admission; остальные отклоняются сразу. |
| `shamir_argon2_busy_refusals_total` | counter | — | Отказы при admission сверх `2 × capacity`, истечение трёхсекундного ожидания разрешения с момента admission, сбои чтения часов и расчёта deadline (включая сбой при получении permit). Один инкремент на отказ. Post-auth отказ получить per-session in-flight slot в Argon2 counter не входит. | Прирост и скорость прироста обычно равны `0`. | Устойчивый рост не обязательно означает давление capacity: тот же счётчик растёт и от сбоя чтения часов либо невозможности вычислить deadline. Сначала проверьте, читаются ли часы на хосте (NTP/виртуализация), и только потом сверяйте admission/permit с конфигурацией и нагрузкой. |

`shamir_sessions_active{db}` не экспортируется: на доступном пути наблюдения сессия не привязана к БД. Глобальная `shamir_active_sessions` не изменилась. `shamir_admission_queue_depth` нет, поскольку общей очереди допуска нет.



`argon2_concurrent_max` задаёт предел работающих серверных Argon2id derivations при создании пользователей; обычный login (проверка proof) и self-service смена пароля на сервере KDF не запускают. Исключение — первый вход по bootstrap-токену: успешный login под bootstrap-пользователем запускает ротацию credential (`handshake.rs:580-589`; `boot/bootstrap.rs:229-249`), и этот KDF идёт вне гейта. Сначала действует ограниченный admission: одновременно допускаются не более `2 × capacity` задач, включая работающие и ожидающие permit. Превысившие этот предел запросы сразу получают отказ `server_busy` и не отправляются в blocking pool. Билет admission закреплён за blocking-задачей и освобождается после её завершения, даже если запрос отменён. Блокирующий пул при этом общий и не изолирован: при `argon2_concurrent_max` выше 256 собственный backlog гейта (`2 × capacity`) больше дефолтных 512 потоков пула и может занять его целиком, поставив несвязанную блокирующую работу (WAL sync, HTTP-lookup verifier/snapshot — это не вычисление proof) в очередь за задачами деривации. Максимум допущенных задач — `2 048` (capacity `1 024` × 2); сверх него отказ приходит сразу, до blocking pool. Отдельного пула для дериваций нет, и этот предел не настраивается.

Для допущенных задач трёхсекундный бюджет получения permit отсчитывается от admission и включает ожидание в очереди blocking pool. При admission фиксируются оба отсчёта — wall-clock deadline и монотонный `Instant`, — и остаток ожидания равен минимуму из двух остатков: wall-clock `deadline − сейчас` и монотонного остатка `3 с − elapsed`, где `elapsed` — время с момента admission, а `3 с` — полный бюджет `deadline − admitted`, зафиксированный тогда же (`remaining_budget` в `crates/net/shamir-connect/src/server/limits/argon2_semaphore.rs`). Если истёк любой из двух остатков — wall-clock достиг `deadline` или монотонный `elapsed` достиг полного бюджета, — задача отказывается, не получив permit. Поэтому шаг системных часов назад, пока задача стоит в очереди, не возвращает истёкший бюджет (он может увеличить wall-остаток относительно предыдущей пробы, но не исходный бюджет), а шаг вперёд может только сократить ожидание. Это бюджет ожидания permit, а не дедлайн запроса и не срок выполнения или отправки ответа. Сбой чтения часов или расчёта deadline при admission также приводит к отказу. Каждый отказ увеличивает `shamir_argon2_busy_refusals_total` один раз. Bootstrap superuser (с явным паролем или случайным токеном) и ротация bootstrap-токена при первом token login проходят вне гейта. Обычный вход (проверка proof) сервер выполняет без password KDF.

Разность `capacity` − `available` — оценка числа работающих дериваций, а не точное число держателей разрешений. Счётчик учитывает отказы, а не доставленные ответы. Тот же код `server_busy` возвращается при исчерпании посессионных слотов запросов после аутентификации, но такой отказ не увеличивает счётчик Argon2.

Флаг `retryable` в SDK TypeScript и Node.js лишь классифицирует ошибку. SDK не повторяют запросы автоматически, и этот флаг не делает безопасным повтор смешанного батча.

Известные ограничения (ни одно не обходит гейт):

1. `server_busy` — фиксированный ответ admission gate, не универсальный
   исход операции. Если request-slot admission стоит до проверки availability,
   здоровая и quarantined таблицы получают одинаковый busy-ответ; порядок
   `GetRecord` обратный для absent/closed таблиц (сначала `unknown_table`,
   затем slot), quarantine проверяется позже. Argon2 admission для создания
   пользователей допускает не более `2 × capacity` задач; переполнение,
   истечение permit-бюджета или сбой часов дают `server_busy` до KDF. Это
   описывает отказ самого gate и НЕ гарантирует, что он сохранится через
   внешний `Call` stored function. Не повторяйте смешанный батч по этому коду.
2. Стабильный код отказавшей операции в строгом транзакционном батче читается из `transaction.abort_code`, а не из `transaction.reason`: у отклонённого `CreateUser` там будет `server_busy` (`crates/engine/shamir-engine/src/query/batch/exec/batch_execute/transactional.rs:300-307`). `reason` парсингу не подлежит, но и не всегда является debug-текстом: `skip_aborted` — единственный план-аборт, который записывается в `reason` собственным кодом (`crates/engine/shamir-engine/src/query/batch/runner/query_runner/helpers.rs:155-161`), а commit-time классификации теперь тоже заполняют `abort_code` — `tx_conflict`, `unique_violation`, `version_conflict`, `table_migrating`, `wal_retention_exceeded`, `commit_admission_refused`, `wal_requires_reopen`, `cancelled` (`crates/engine/shamir-engine/src/tx/commit/pipeline/phase/commit/entry.rs:398-463`). Поля `abort_code` может не быть вовсе — когда у аборта нет стабильного слова (`Storage` без собственного `code`, `Expired`; `crates/query/shamir-query-types/src/ops/batch/req/transaction_info.rs:25-31`), — поэтому полагаться на код в каждом аборте нельзя. Внутри `for_each` первая неуспешная итерация останавливает цикл. Уже независимо закоммиченный префикс сохраняется; повтор всего внешнего `ForEach` не эквивалентен повтору только упавшей итерации и может дублировать префикс. `indeterminate` у тела останавливает цикл с `wal_requires_reopen` (итерация могла закоммититься — сначала reconcile, не слепой повтор; `crates/engine/shamir-engine/src/query/batch/runner/query_runner/helpers.rs:35-94`, вызов в `crates/engine/shamir-engine/src/query/batch/runner/query_runner/arms_read.rs:488-499`). TS SDK помечает `tx_conflict` как retryable для новой транзакции, но автоматически запрос не повторяет.
3. Внешний `Call` хранимой функции несёт стабильный код отказа, который классифицировал шлюз БД на вызове `db_execute` или другого `db_*` (`crates/engine/shamir-db/src/shamir_db/execute/core/function_invoker.rs:47-65`). Код пишет только сервер (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:14-19`), поэтому гость не может его подделать: текст guest-trap, лишь упоминающий кодовое слово, кодом не становится. Если шлюз отказ не классифицировал, стабильного кода у `Call` нет, и судить о причине по тексту ошибки нельзя. Вложенный хранимый вызов (функция вызывает функцию), внутри которого транзакционный `db_execute` абортится, кода наружу не передаёт: абортированный `BatchResponse` — это успех на шлюзе (данные, которые разбирает гость), а транзакционная проверка гостя поднимает общий abort без кода (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:23-32`; `crates/wasm/shamir-lang/src/back/emit/support/db/tx.rs:254-300`), тогда как код наружного `Call` берётся только из ускользнувшего отказа шлюза (`crates/engine/shamir-db/src/shamir_db/execute/core/function_invoker.rs:45-62`). `DbGatewayError::code()` классифицирует только `not_found` и `access_denied`, поэтому отказ на ЧТЕНИЕ таблицы по ACL даёт наружному `Call` вообще без стабильного кода (не `unknown_table`): `unknown_table` намеренно не имеет ветки и попадает в общий `ExecutionSource` (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:208-238`), а сам `code()` возвращает код лишь для этих двух классов (`crates/wasm/shamir-wasm-host/src/gateway/db_gateway.rs:67-82`).
4. Фиксированный серверный конверт отказа доходит до клиента со своим кодом при любом клиентском `limits.max_result_size`, но таких случаев четыре: `cancelled`, типизированный `unknown_table` (текст `table is unavailable`), типизированный `server_busy` с пустым алиасом (отказ Argon2-gate прямого `CreateUser`) и типизированный `WalOutcomeUnknown` (текст `WAL outcome indeterminate` с указанием `commit_version`). Их байты резервируются заранее (`FIXED_ERROR_ALLOWANCE`), предел для них — серверный `max_result_size_bytes`. Любой ДРУГОЙ ответ, включая ошибки с контекстными полями (`resync_required`, `read_only_replica`, `permission_denied`, `AsOfVisibilityTimeout`), выше клиентского лимита заменяется на `result_too_large`. Типизированный отказ `server_busy` с пустым алиасом теперь проходит через вложенные оболочки `Batch`/`ForEach` неизменным (`crates/engine/shamir-engine/src/query/batch/runner/query_runner/helpers.rs:10-33`) и доходит до клиента своим фиксированным конвертом `server_busy` при любом клиентском лимите; в множество фиксированных отказов входит и типизированный `WalOutcomeUnknown` — bounded server-owned текст с указанием попытанного `commit_version`, код `wal_requires_reopen` сохраняется при любом клиентском лимите (`crates/server/shamir-server/src/engine/db_handler/repo_admin/execute.rs:430-443,539-541`).

Если создание было отклонено до записи, повторите после паузы только обычный нетранзакционный `CreateScramUser`, не весь батч. Стабильный код теперь и есть ответ гейта, но он не обещает, что повтор пройдёт: `server_busy` утверждает занятость, а не успех, и клиент не может вывести из кода ничего сверх того, что код сам утверждает.

Отказы native auth-init в синхронном handshake уже учитывает `auth_attempts_total{result="rate_limited"}`, а отказы входного токенного gate по частоте — `shamir_token_auth_total{channel,result="rate_limited"}`; дублирующих счётчиков нет. `shamir_token_request_denied_total{endpoint}` учитывает отказы по scope/политике, а не посессионный лимит частоты после аутентификации.

`process_*` опрашиваются каждые 5 секунд (crate `metrics-process`).

Серии online backup `shamir_backup_*` (R2a, дизайн `online-backup.md` §4.8) —
счётчики несут метку `kind=online` (offline-прогоны, если появятся, получат
`kind=offline`); `shamir_backup_failed_total` дополнительно — `stage`, стадию,
на которой протокол зафиксировал сбой (`preparing|captured|copying|sealing|verifying`).

`shamir_backup_started_total` / `shamir_backup_completed_total` /
`shamir_backup_failed_total{kind,stage}` — кумулятивные счётчики прогонов за
время жизни процесса. В норме completed растёт вместе с started, а failed
остаётся нулевым; рост failed по конкретной стадии указывает, где деградирует
backup (например, `stage=sealing` — не успевает seal-фаза).

`shamir_backup_active` — gauge 0/1: выполняется ли online-backup прямо сейчас.
Одновременный допускается только один (одна блокировка на сервер); `1` дольше
ожидаемого — признак зависшего прогона. Gauge сбрасывается на завершении
протокола копирования, до прохода очистки, поэтому `0` ещё не означает, что
очистка закончилась: приём в реестре (`running`) держится дольше.

`shamir_backup_bytes_copied` — кумулятивные байты, скопированные завершёнными
прогонами (счётчики хешированных байтов протокола); по rate() оценивают
скорость копирования.

`shamir_backup_duration_seconds` — длительность последней **успешной** копии
в секундах; не выставляется до первого успеха: серия рендерится только из
последнего успешного прогона, отказ её не меняет. Сравнивают между
собой последовательные значения, а не с абсолютной нормой.

`shamir_backup_writer_block_seconds{phase=capture|seal}` — gauge в секундах,
содержит последнее закрытое окно блокировки writer'ов по фазе, включая
неудачные удержания; не выставляется до первого закрытого окна.
`shamir_backup_pin_seconds` и `shamir_backup_wal_segments_copied_total`
по-прежнему **не экспортируются**: протокол не отдаёт удержание pin и счётчик
сегментов WAL (серии появятся без изменения имён, как только появятся данные).

`shamir_function_duration_seconds` — гистограмма длительности вызовов WASM в секундах, с метками `db`,`repo` (без метки функции); серии `*_bucket` имеют дополнительную метку `le`, это кумулятивные gauge, `*_count` — counter, `*_sum` — gauge в секундах. Учитывается только внешний вызов глубины 0, включая вложенное время, ошибки и отмену; обычно — микросекунды–миллисекунды. Снимки берутся при scrape без блокировок и I/O экспортера; реестр ограничен 64 именованными парами с агрегатом `(other)`.

`shamir_function_fuel_consumed_total` — counter фактически израсходованных единиц fuel с метками `db`,`repo`; суммируется расход при приостановке и окончательном расчёте, без повторного учёта вложенных вызовов и без зарезервированного fuel. Норма зависит от числа и сложности вызовов.

`shamir_compile_cache_hits_total` — counter успешных обращений к инкрементальному compile-cache, без меток; считает единицы компиляции, а не вызовы функций. После прогрева обычно преобладают попадания.

`shamir_compile_cache_misses_total` — counter промахов кэша и компиляций без кэша, без меток; считается по единицам компиляции, не по вызовам функций. После прогрева обычно близок к нулю.

`shamir_compile_cache_bytes` — gauge размера индексированных живых кэшей компиляции в байтах, без меток; обычное значение укладывается в бюджет кэша.

`shamir_compile_cache_entries` — gauge индексированных записей кэша, без меток; запись инкрементальной компиляции не обязательно соответствует целому модулю. Обычно значение укладывается в бюджет кэша.

`shamir_module_admission_rejected_total{reason}` — counter отклонённых модулей при admission, с одной из причин `parse`,`validation`,`import`,`operator`,`abi`,`interface`,`limits`; обычное значение — 0.

`shamir_function_call_depth_high_water` — gauge максимальной глубины стека вызовов в кадрах, с метками `db`,`repo`; глубина контекста начинается с 0, значение монотонно растёт. Норма — ниже настроенного предела; действует реестр на 64 именованные пары и агрегат `(other)`.

`shamir_replication_lag_versions` — gauge текущего отставания в версиях, с метками `db`,`repo`. Оно равно текущей версии лидера минус durable bookmark, сообщённый follower'ом; для `from_version` bookmark — это `from_version - 1`. Отрицательное значение ограничивается нулём.

Лидер учитывает bookmark из каждого Pull-запроса в пределах времени жизни этого запроса, включая запросы, завершившиеся успешно. Отставание считается от подтверждённого durable bookmark, а не от последнего отданного события: ещё не подтверждённое событие не уменьшает lag.

После завершения Pull-запроса его лидерский поток помечается `stopped`. Перекрывающиеся запросы остаются отдельными активными потоками; завершение одного прекращает только его собственную запись, и агрегированное состояние остаётся активным, пока существует другой запрос.

`shamir_replication_lag_seconds` — gauge возраста последнего наблюдённого durable receipt, в секундах, с метками `db`,`repo`; значение равно 0 до первого receipt. У follower время фиксируется при фактическом применении receipt.

У лидера время фиксируется при наблюдаемом продвижении сообщённого durable bookmark. Это время наблюдения лидером, а не точное время удалённого commit; оно может быть позже фактического commit.

Возраст receipt продолжает расти при здоровом простое без новых bookmark/receipt. Поэтому сам по себе большой `lag_seconds` во время отсутствия новых записей не доказывает задержку репликации.

`shamir_replication_stream_state{state}` — one-hot gauge состояния с метками `db`,`repo`,`state`; `state` — одно из `starting`,`streaming`,`backing_off`,`stopped`. Состояние учитывает как follower subscriptions, так и активные лидерские Pull-запросы.

Роли не имеют отдельной метки: для каждой пары `db`,`repo` `lag_versions` и состояние агрегируются только по активным ролям; `lag_versions` выбирает максимальный текущий lag. Время receipt — самый старый ненулевой timestamp среди активных ролей и сохранённой истории подтверждений именованного leader, даже если сейчас активен только follower. Если активных ролей нет, lag и состояние берутся из последней наблюдавшейся роли; завершённый leader lag не сохраняется, но история подтверждений именованного leader сохраняет timestamp receipt.

Общее состояние пары также детерминированно агрегируется: приоритет от худшего к лучшему — `backing_off` > `starting` > `streaming` > `stopped`; экспортируется одно соответствующее one-hot состояние. Поэтому активный проблемный поток не скрывается здоровым потоком той же пары.

Restart follower'а отслеживается маркером конкретной subscription. Reconnect counter увеличивается только при фактическом пересоздании источника на границе supervisor factory; первоначальное создание даёт 0, а повторы запросов без пересоздания источника не считаются.

`shamir_replication_reconnects_total` — lifetime counter пересозданий источника, с метками `db`,`repo`; это не счётчик сетевых retry. Метрики не добавляют метку роли: обе роли используют те же пары `db`,`repo` и существующие метки состояния.

Нормальный диапазон `shamir_replication_reconnects_total` — 0 при стабильной работе; при наличии записей lag обычно мал. После потери состояния lag восстанавливается при следующем наблюдении durable progress.

До 256 пар `db`,`repo` хранятся поимённо. Значения для превышающих лимит потоков агрегируются через слабо удерживаемые handles: пока есть активные handles, lag равен максимальному текущему lag среди них, не lifetime high-water; когда lag падает с 10 до 0, агрегат тоже падает до 0. Overflow-leader Pull учитывается только в `lag_versions` и не сохраняет receipt timestamp; поэтому без receipt от overflow follower `lag_seconds` равен 0. Если активных handles нет, overflow-агрегат выдаёт нулевые значения.

Остановленные overflow-потоки исключаются из lag-агрегации. После завершения всех overflow-потоков строка `(other)` остаётся с нулевыми значениями gauges; lifetime reconnect counter при этом сохраняется и продолжает экспортироваться. Новых серий или меток для этой агрегации нет.

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
Автоматическое удаление старых архивов по возрасту выполняется только при ротации по
размеру и ненулевом `retention_days`; `max_file_size_mb: 0` отключает ротацию по размеру
и вместе с ней — автоматические удаления. Возраст архива определяется по времени модификации
файла в файловой системе (`retention_days * 86400` секунд), а не по числовому суффиксу имени;
копирование или восстановление архивов может сбросить время модификации (возраст тогда
считается от получившегося времени), `touch` тоже меняет время модификации и тем самым
возраст, а если время модификации недоступно, прореживание останавливается. Удаляется только
старейший непрерывный префикс архивов, достигший возраста `retention_days`, и всегда перед
новосозданным при этой ротации новейшим архивом (он сохраняется). Суффиксы новых ротаций
строго возрастают, но уже существовавшие имена этому быть не обязаны; суффиксы служат
только для упорядочения. Если следующий суффикс исчерпал бы пространство (достиг бы
`u64::MAX`), ротация пропускается с предупреждением, а активный файл продолжает расти
сверх настроенного размера; имя с суффиксом `u64::MAX` при запуске отвергается. Архив с временем в будущем
или не достигший возраста считается негодным: он сохраняется, и из-за него нельзя удалить
ни один более поздний архив — префикс строго непрерывный. (Будущее **имя** не означает
будущее время файла.) Если время модификации архива оказалось в будущем (архив записан,
когда часы шли вперёд), прореживание сдвигает время каждого такого архива, который в этот
момент видит и у которого время модификации читается, на текущий момент (только метаданные
файла: HMAC-цепочка и оба якоря от него не зависят; сдвиг записывается в лог
предупреждением), поэтому такой архив становится пригодным к удалению через
`retention_days` после этого прореживания, а не через величину ошибки часов. Сдвиг
возможен только в прореживании, а прореживание выполняется только при `retention_days > 0`
и только в ротации, дошедшей до успешного завершения; архивы с нечитаемым временем
модификации не сдвигаются. Если сдвиг не удался (нет права записи), предупреждение
повторяется не «при каждой ротации», а при следующем таком прореживании, которое снова
увидит это будущее время, — то есть при следующей успешной ротации с включённым retention;
если ротаций больше не будет, оно не повторится. Сдвиг проверяет, что кандидат — обычный
файл (`symlink_metadata`), и затем отдельно открывает его на запись, поэтому эти два шага
не атомарны: путь, подменённый между ними, был бы открыт через подмену. Гонку не
закрывают, её ограничивает offline maintenance lock — эксклюзивная блокировка `data_dir`
(`<data_dir>/.shamir.lock`), которую работающий сервер удерживает всё время жизни
процесса, а offline-команды берут на время команды; процесс, игнорирующий эту блокировку,
ею не ограничен. Пригодность наступает по времени, а удаление происходит только при
успешной ротации с ненулевым `retention_days`. Из остановленного `data_dir`
оператор может вынести (и удалить) только старейшие непрерывные полные архивы в порядке следования записей; отдельного CLI
для этого нет. При ручном прореживании отсутствие активного `audit.log` обрабатывается так
же, как пустой: валидный непустой суффикс без genesis-записей (начинающийся с номера
больше 1) должен содержать хотя бы один проверенный якорь,
и при пустом (или отсутствующем) активном файле оставьте новейший архив. Ключ и оба якорных
хранилища (`audit_checkpoint` и `server_meta`) нельзя редактировать или удалять.

Правила offline CLI (`token list`, `token create` и другие offline token-команды): каждая
сначала захватывает блокировку `data_dir`; при занятой блокировке (живой сервер
или другой CLI) команда отказывает немедленно, а ошибка
доступа к каталогу (permission) сообщается отдельно, не как «требуется остановленный
сервер». Секретный файл вывода создаёт только `token create`, и он отвергает `--out-file`,
который разрешается в `audit.log`, в любой из его архивов или в путь, первый компонент
которого внутри `data_dir` начинается с `audit.log`; префикс `audit.log` сопоставляется
без учёта регистра ASCII (зарезервированы и варианты вроде `Audit.log`). Отказ происходит
**до** создания файла: в файл вывода и в аудит ничего не записывается (при захвате
блокировки может обновиться PID в файле блокировки). Проверка опирается прежде всего на
идентичность каталога: каталог назначения сравнивается с `data_dir` по паре «устройство и
inode» (на Windows — по серийному номеру тома и идентификатору файла), поэтому псевдоним
каталога через bind-mount распознаётся и отвергается; сравнение компонентов пути остаётся
только резервным вариантом, когда идентичность недоступна. Защиты от одновременных
изменений каталога (состояние гонки с другим процессом) она не обещает. Захват блокировки при необходимости создаёт сам `data_dir` и файл
блокировки. Файл диагностического лога проверяется отдельно: если он лежит непосредственно
в `data_dir` и его имя зарезервировано (`audit.log` или `audit.log.<цифры>`, без учёта
регистра ASCII), сервер **отказывается запускаться** с типизированной ошибкой — и в обычном
режиме, и при запуске как служба Windows; offline-команды файловый sink вообще не открывают,
поэтому такой файл не создают и не проверяют.

Если `token create` отказывает со ссылкой на аудит — событие аудита не записано либо
контрольная точка не может заверить журнал, — запись токена уже существует и действует, но
секрет так и не был выдан: команда не доставляет секрет ни в stdout, ни в файл. В сообщении
указан id токена: оператор должен выполнить `token revoke <id>`, и эта команда работает
offline под той же блокировкой обслуживания. Созданный файл вывода команда удаляет, если
может подтвердить его идентичность; на файловой системе без идентичности файла (например,
FAT/exFAT) пустой файл вывода может остаться, но секрета в нём нет. Использовать токен не
может никто: секрет не был записан никуда, а в хранилище остаётся только
`mac = HMAC(key, id, secret)`, из которого секрет не восстановить.

Покрытие (состояние реализации): durable-записи пишут события аутентификации рукопожатия и возобновления `shamir-sync`, создание и отзыв токенов сервисом токенов (включая user-wide) и агрегированные ошибки HTTP-аутентификации; отказы операций отмены `cancel_request`, `cancel_tx` и `cancel_cursor_admin` (исходы `AlreadyCommitting` и `NotCancellable`) и каждая успешная отмена чужой работы (`cancel_tx` и `cancel_cursor_admin`) аудируются по принципу
«лучшее из возможного»: при заполненной очереди запись может быть пропущена, а в деталях только целевой дескриптор или id и исход. `alter_repo` аудируется как успешное событие (`result="ok"`, `repo_audit.rs`), а не как отказ. Полное покрытие DDL и ACL-изменений **не** заявляется — для части таких операций durable-следа пока нет; приведённый список не является исчерпывающим обещанием.

**Счётчик отброшенных отказов `cancel_request` — это отказы и потери в этом процессе, а не «записи, пропавшие с диска».** Он увеличивается на каждый отказ в приёме задачи аудита (очередь заполнена, приём закрыт, ожидающая задача сброшена при остановке) и на каждый отказ самой записи, а не на расхождение между ответом клиенту и содержимым `audit.log`. Ответ на `cancel_request` корректен независимо от того, попал ли отказ в аудит, и повторная запись не выполняется. Число отброшенных — диагностика давления на очередь и на диск, а не обещание полноты аудита. Кроме общего предела очереди в 256 задач действует доля одного принципала — 32 задачи (восьмая часть общего предела). Это потолок, а не резерв: он не даёт одному имени занять все 256 слотов в одиночку, но не гарантирует конкретным восьми принципалам по полной доле, если очередь уже занята другими именами. Доля считается точно по имени пользователя: у каждого имени свой счётчик, имена никогда не делят один счётчик. Если обе квоты свободны, но другой submitter удерживает spawn-gate реестра, событие аудита немедленно отбрасывается — caller не ждёт завершения этого отказа. Конкуренция за map-bucket обрабатывается ограниченными неблокирующими повторами `try_entry`, а не синхронным ожиданием писателя bucket. Отказанный admission не публикует счётчик принципала: он принадлежит первой принятой задаче аудита и удаляется, когда её число возвращается к нулю. Освобождение при уничтожении задачи — блокирующий доступ к bucket на audit worker, а при остановке runtime может выполняться на submitter. Запись в реестре создаётся только после успешного резервирования общего слота, поэтому каждая опубликованная запись удерживает живой слот, а превышения предела 256 нет. На пути ответа нет ожидания записи аудита, гейта персистентности или сброса; резервирование выполняется под коротким мьютексом spawn-gate. Обновление счётчика (операции записи `scc`) и короткий замок реестра задач при постановке могут кратко ждать другой отказ, который их удерживает. Отказ, не нашедший свободного места по доле, учитывается как отброшенный ровно так же, как отказ при заполненной общей очереди.

**Две разные границы ожидания при остановке.** Трёхсекундная константа (`DRAIN_BUDGET`) ограничивает только ожидание уже принятых задач аудита отказов; она ничего не говорит о том, сколько ждёт остановка в целом. Асинхронные ожидания стадии аудита (присоединение планировщика и фонового сброса, финальный сброс) ограничивает общий `AUDIT_TEARDOWN_DEADLINE` (10 секунд, `server_launcher/support/audit_teardown.rs`) — один и тот же для штатной остановки и для обеих ветвей отката запуска. Это бюджет ожиданий, а не жёсткий срок по часам: ожидание задач аудита отказов имеет свой отдельный таймер, а синхронная работа, уже занявшая единственный поток однопоточного runtime (например, запись аудита рукопожатия под тем же гейтом цепочки), дедлайном не прерывается. Порядок внутри этой стадии и есть гарантия: приём новых задач закрывается, планировщик периодических задач получает сигнал остановки, дожидаются уже принятые задачи аудита отказов (не дольше трёх секунд: по истечении ещё не начавшиеся задачи отбрасываются, а остановка помечается нештатной), и планировщик присоединяется ДО того, как начнётся финальный сброс (поэтому запланированная контрольная точка аудита не может перемешаться с финальной синхронизацией журнала и записью якоря), затем останавливается и присоединяется фоновый сброс, и только после этого выполняется финальный сброс буфера, запись якоря цепочки и метаданных в хранилище `server_meta` (каталог `data_dir/server_meta`, его открывает загрузчик). Финальный сброс сам захватывает тот же синхронный гейт персистентности, что и контрольная точка, и делает это на blocking-потоке: ни захват гейта, ни `fsync`, ни `SyncAll` в fjall не ждут в опросе future остановки. Планировщик получает сигнал до присоединения, поэтому ожидание не может растянуться до следующего тика (для финализации ключей идентичности — час). Контрольная точка планировщика тоже выполняется на blocking-потоке, а не на рабочем потоке runtime, поэтому удержанный ею гейт не мешает сработать дедлайну даже на однопоточном runtime.

**Последний шаг стадии аудита выполняется только после всех трёх условий.** Финальный сброс буфера, запись якоря цепочки и метаданных — это и есть «финальное закрытие», и оно запускается ТОЛЬКО когда внутри `AUDIT_TEARDOWN_DEADLINE` выполнены все три условия: (а) планировщик периодических задач остановился и присоединился, (б) фоновый сброс остановился и присоединился, (в) ожидание принятых задач аудита отказов установило, что незавершённой принятой работы не осталось. Если хотя бы одно условие не выполнено, задача всё ещё может находиться внутри гейта персистентности, а принятая задача отказа — ещё дойти до собственной встроенной контрольной точки, а значит финальное закрытие, запущенное сейчас, могло бы выполниться РАНЬШЕ неё — тот самый порядок, ради которого стадия существует, был бы инвертирован именно там, где это важнее всего. Поэтому в этом случае финальное закрытие не запускается вовсе, в отчёт попадает `audit final close not attempted …`, и это означает: финальное закрытие не сохранило последние буферизованные события аудита и якорь цепочки (задача, не уложившаяся в дедлайн, ещё может записать часть или всё), а следующий запуск проверяет цепочку так же, как после аварийного завершения, и может отказать при недопустимом хвосте. Отказ фонового сброса различается по слою. Строку с префиксом `audit flusher task did not join cleanly: …` формирует сама стадия остановки (общая для штатной остановки и обеих ветвей отката запуска), а хвост сообщения приходит из одного из двух мест: от обёртки задачи, которая вернула защёлкнутый отказ вложенной блокирующей задачи сброса (обёртка к этому моменту завершилась и вернула этот текст, значит вложенная работа остановлена — совпадение с защёлкнутым сбоем это доказывает, и финальное закрытие всё равно выполняется, сбрасывая оставшийся буфер), либо от `JoinError` самой обёртки (паника или отмена задачи — ничто не доказывает остановку вложенной блокирующей работы, поэтому финальное закрытие НЕ запускается, как и при нарушении любого другого из трёх условий). Финальное закрытие выполняется на ОТДЕЛЬНОМ потоке, а не в общем blocking-пуле: отменённые задачи аудита отказов ещё держат ссылки на writer, пока их не заберёт рабочий, поэтому занятый пул мог бы исчерпать дедлайн до начала закрытия. Ожидание по-прежнему ограничено общим дедлайном, а истёкшее ожидание не отменяет уже начатый `fsync`.

**Что показывает отчёт при истечении дедлайна или ошибке сброса.** У каждого отдельного отказа свой фиксированный префикс, и каждый попадает в отчёт в момент наблюдения; более поздний тайм-аут не теряет уже записанную ошибку. Одно и то же сообщение защёлкнутого сбоя фонового сброса попадает в отчёт ОДИН раз: наблюдение при присоединении регистрируется под своим префиксом, а повторное наблюдение того же текста при финальном закрытии подавляется. Префиксы: `stores still held after the release timeout …` (последняя завершённая проба показала удерживаемые блокировки каталогов; это снимок наблюдения, а не утверждение о состоянии ровно в момент дедлайна; немедленный перезапуск тех же `data_dir` может завершиться `Locked`), `store release could not be verified …` (занятый blocking-пул оставляет состояние НЕИЗВЕСТНЫМ только если ещё нет первого пригодного наблюдения; после завершённой пробы с удерживаемыми блокировками последующий тайм-аут пробы возвращает `StillLocked` по предыдущему завершённому наблюдению; неудачная проба, неполное обнаружение хранилищ или исчерпанный бюджет до запуска пробы также оставляют состояние НЕИЗВЕСТНЫМ), `audit teardown exceeded …` (стадия не уложилась в дедлайн, с указанием шага), `scheduler did not stop within …` (планировщик не остановился), `audit flusher task did not join cleanly: …` (фоновый сброс не присоединился), `audit flush failed at shutdown: …` (финальный сброс буфера или журнала), `audit anchor or metadata write failed at shutdown: …` (запись якоря цепочки или метаданных), `audit final close not attempted: …` (финальное закрытие не запускалось, потому что планировщик, фоновый сброс или ожидание дренажа отказов не уложились в дедлайн) и `audit flush task did not complete: …` (задача сброса не завершилась: dedicated final-close thread не удалось запустить или его результат не был получен) и `refusal audit drain expired: …` (ожидание принятых задач аудита отказов истекло: ещё не начавшиеся задачи отброшены, уже работающие больше не ожидаются). Сам `ShutdownReport` хранит строки с этими фиксированными префиксами, а не типизированные варианты: супервизор и оператор различают причины по префиксу, поэтому префикс — часть контракта. Любая из них делает отчёт нештатным: код возврата, который выводит супервизор или встраивающий процесс, — 70 (`exit_code()`), даже если все репозитории сохранены чисто. Сам отчёт не завершает процесс принудительно: ни `main.rs`, ни служба Windows не вызывают `std::process::exit`, они возвращают этот код после возврата `serve`, а процесс завершается позже, когда закрывается runtime, поэтому дедлайн ограничивает стадию аудита, а не срок жизни процесса. Откат неудавшегося запуска возвращает исходную ошибку запуска, а не отчёт с кодом 70. Нечитаемый репозиторий по-прежнему отмечается отдельно.

**Тайм-аут останавливает ожидание, а не `fsync`.** Истёкший дедлайн аудита прекращает ждать: остановка переходит к сбросу репозиториев и к освобождению каталога, а задача сброса, уже находящаяся внутри `fsync`/`SyncAll`, продолжает выполняться — её нельзя прервать, и прерывать её небезопасно. Поэтому отчёт может быть нештатным, а запись — всё равно дописаться позже (или остаться незавершённой, если процесс завершится). Незапущенные задачи аудита отказов (принятые, но ещё не вошедшие в writer) сбрасываются не по дедлайну аудита, а по истечении трёхсекундного ожидания очереди отказа (`DRAIN_BUDGET`), и учитываются как отброшенные; задачи, уже вошедшие в writer, дорабатывают — дедлайн аудита лишь прекращает их ждать. Задача, отправленная уже после закрытия приёма (`close()`), не запускается вовсе: `close()` берёт ТОТ ЖЕ мьютекс, что и решение «зарегистрировать или не запускать» перед созданием задачи, поэтому закрытие не может вернуться, пока такое решение выполняется, а дескриптор уже запущенной задачи лежит в списке, который сбрасывает очередь.

**Планировщик резервного копирования и блокировка каталога данных живут дольше дедлайна остановки.** Порядок при штатной остановке прежний: задача планировщика резервного копирования и её текущий прогон дожидаются ДО сброса репозиториев. Дедлайн `serve` (30 секунд) ограничивает всю остановку целиком (дренаж соединений и фоновых задач, ожидание планировщика резервного копирования, стадии аудита и репозиториев, освобождение хранилищ): при истечении сбрасывается та стадия, что ещё выполняется, а отчёт помечается нештатным; но дедлайн не обрывает запланированную задачу и не освобождает блокировку каталога данных. Загружающий путь забирает и задачу, и блокировку у дескриптора сервера ДО запуска дедлайна, а future остановки берёт этого владельца по `&mut`, не забирая его себе: поэтому при истечении дедлайна future отбрасывается, а работник дожидается уже после дедлайна. Блокировка освобождается при возврате внутренней `drain`, то есть ДО того, как `serve` вернёт отчёт, и только после того, как работник закончил. Практическое следствие: остановка может длиться дольше дедлайна — ровно на время работы нативного работника резервного копирования, который уже прошёл свою последнюю точку проверки отмены. Ожидание работника — это сигнал о завершении прогона (канал планировщика), а не join нативного потока.

**Известные ограничения: чего дедлайн не покрывает.** Отмена внутри нативного прогона резервного копирования проверяется между этапами (preflight, копирование, retention); внутри хеширования файлов она проверяется ТОЛЬКО в retention-проходе — его проверка отмены опрашивается между порциями по 1 MiB, перед каждым чтением (`offline.rs`, `offline/v2_verify.rs`), поэтому хеширование кандидата на удаление прерывается запросом остановки. Обычная (не retention) проверка, проверка при восстановлении и самопроверка писателем этого хука не получают и хешируют весь объём до конца: прогон, вошедший в длинное хеширование вне retention, доработает его до конца. Синхронная работа вне пути аудита и вне запланированного бэкапа дедлайном не прерывается, но больше не занимает рабочий поток runtime там, где это важно: на blocking-потоке идут сборка мусора счётчиков и тик аудита-контрольной точки (`boot/scheduler.rs`), а также периодические и финальные снимки метаданных сервера (`engine/server/meta_sinks.rs`), и остановка их дожидается. Тики lockout/rate-limit/session-GC и финализации идентичности по-прежнему идут на рабочем потоке планировщика: они короткие и дедлайном не прерываются. Дедлайн не может прервать `fsync` и не может прервать блокирующее чтение на единственном рабочем потоке runtime — он только прекращает ожидание и помечает отчёт нештатным.

**Восстановление при запуске.** Удерживая серверную блокировку `data_dir`,
сервер до начала записи проверяет HMAC всех сохранённых записей в `audit.log`
и файлах ротации сохранённым ключом, а также непрерывность номеров и HMAC-связей.
Запись продолжается с проверенного хвоста (следующий номер — номер последней
записи плюс один, предыдущий HMAC — её HMAC), а не с контрольной точки.
Проверки при запуске и в offline CLI (`token list`/`token create`) — одни и те
же, общая процедура восстановления; при любой ошибке CLI отказывается
продолжать. Файлы ротации — это имена ровно вида `audit.log.` плюс 20
ASCII-десятичных цифр; любой другой файл с этим префиксом игнорируется
с предупреждением в логе, а имя правильной формы с повреждёнными записями или
нарушенным порядком по-прежнему приводит к отказу. Восстановление синхронно и
линейно по объёму сохранённых байт плюс сортировка архивов: архивы
упорядочиваются по номеру первой записи внутри файла, а не по имени, номера и
HMAC-связи остаются единственной инстанцией порядка. Якоря — `audit_checkpoint`
и `server_meta`; учитываются только присутствующие якоря, каждый проверяется в
достижимой позиции: якорь за проверенным хвостом (past-tail) всегда приводит к
отказу, а якорь старее сохранённого суффикса с ним не сравнивается. Для сохранённого суффикса, начинающегося
с номера больше 1 (genesis-записей нет), обязателен совпадающий якорь — на позиции
непосредственно перед первой записью суффикса, на любой более поздней позиции внутри
суффикса или на проверенном хвосте. Конфликт двух якорей на одной и той же позиции
отвергается ещё до сканирования журнала. Пустая история при наличии не начального якоря
недопустима.

После сбоя можно отбросить только неразбираемый фрагмент в конце активного
`audit.log` без завершающего LF и в пределах допустимого размера записи,
и только после успешной проверки всех полных записей и доступных якорей
(их может не быть).
Разбираемая запись без LF (её отклоняют и CLI, и загрузка, не изменяя её),
фрагмент без LF в файле ротации, превышение
лимита размера и неразбираемая запись с LF приводят к отказу.

Оба якоря продвигаются durably на каждой границе ротации (до того, как
старейшие архивы могут быть удалены по retention) и при каждом **успешном**
закрытии аппендера аудита (финальный сброс фиксирует финальную позицию);
порядок записи выбран так, чтобы сбой между двумя записями всё равно
восстанавливался. Отличайте сбой процесса от потери питания: сбой процесса
не равнозначен потере питания и сам по себе файловую последовательность не нарушает,
тогда как потеря питания может её нарушить. На Unix сброс (fsync) записи
родительского каталога выполняется после переоткрытия активного файла при
восстановлении на запуске, после ротации и после каждого успешного удаления
архива при прореживании; на Windows эквивалентной гарантии для записи каталога
(барьера упорядочивания) нет: потеря питания может привести к потере или
нарушению порядка новейшего расклада файлов (в том числе к тому, что позднее
удаление сохранится, а более раннее — нет, оставив дыру). Если из-за этого в цепочке
возник разрыв или потерялись записи, покрытые якорем, восстановление отказывает, а не
принимает историю; это не заявление, что перехватываются все
возможные потери содержимого каталога. Файловые данные активного файла уже
покрыты сбросом дескриптора.

Если история не проходит проверку, сервер отказывается запускаться и не начинает
новую цепочку. Launch-слой возвращает типизированные ошибки, но исполняемый файл
печатает только их `Display`-сообщение, например
`server boot failed: audit recovery: ...`; имена вариантов (`BootError::AuditRecovery`,
`AuditKeyRead`, `AuditCheckpointRead`, `AuditAppenderCheckpoint`,
`AuditMetaCheckpoint`) в вывод не попадают — они перечислены здесь только для
навигации по коду.
Online-копия, снятая сервером **этой** версии, по построению audit-согласована:
проход data-каталога сначала копирует хранилища якорей (`audit_checkpoint`,
`server_meta`) и только затем перечисляет и копирует файлы аудита — архивы и
последним активный `audit.log`, — поэтому якорь в копии не указывает за хвост
журнала, в том числе когда аппендер ротирует журнал во время копирования (см.
[раздел «Audit-файлы в online-копии»](#audit-файлы-в-online-копии)).
Копия, снятая более старой версией сервера (или собранная вручную), может
пройти проверку манифеста (он проверяет байты файлов, а не согласованность
аудита), но audit-согласованной быть не обязана: `audit.log` и якоря
могли быть сняты в разные моменты. Такой каталог может отказать не на restore, а на
**первом запуске**: `AnchorPastTail`, если якорь оказался за хвостом журнала; для
восстановления берите
audit-согласованный комплект из доверенной резервной копии: журнал с файлами
ротации, ключ и обе контрольные точки. После восстановления проверьте
комплект, запустив сервер на отдельной остановленной копии восстановленного
каталога.
Журнал с повторяющимися genesis-записями
(до-апгрейдный) при проверке **отказывается** — сохранить его как рабочую
историю нельзя, и другая копия той же истории не поможет. Оригинал такого
каталога сохраните целиком и без изменений для диагностики (безусловно, вне
рабочего каталога сервера). Если доверенные якоря — genesis или отсутствуют
и осознанный запуск новой audit-цепочки согласован: остановите сервер и любой
offline CLI и вынесите активный `audit.log` и все архивы точного вида
`audit.log.` + 20 цифр (только их, вместе) за пределы `data_dir`, сохранив
оригиналы, ключ и якорные хранилища нетронутыми; после перезапуска сервер
начнёт новую цепочку. Если существует хоть один не-genesis якорь —
используйте соответствующую валидную резервную копию или обратитесь в
поддержку: не очищайте строки журнала и не переносите `server_meta`.
Никогда не сбрасывайте и не правьте якоря вручную и не пытайтесь обойти
проверку правкой файлов или отключением ротации.

При аварии события, ещё не сохранённые на диске, могут быть потеряны.
Удаление записей после последнего надёжно сохранённого якоря само по себе
может остаться незамеченным.

## 8. Backup

Есть два режима резервного копирования:

- **Online-копия** — на **работающем** сервере: вручную через `StartBackup` /
  `BackupStatus` / `CancelBackup` или встроенным планировщиком. Сервер
  продолжает принимать записи, снимок пишется в заданный каталог.
- **Offline `backup --to`** — stop-and-copy: сервер должен быть **остановлен**
  перед запуском. Против живого источника команда теперь **отказывает** с
  адресной ошибкой, указывающей на online-операцию; force-флага нет.

Offline-режим:

```bash
# 1) Остановить сервер (systemctl stop shamir-server / SIGTERM — дождаться graceful shutdown)
# 2) Запустить backup — data_dir берётся из --config, снимок пишется в <to>/<UTC-timestamp>/
shamir-server --config db.ktav backup --to /backup/shamir
# 3) Перезапустить сервер
```

Offline-команда рекурсивно копирует `data_dir` (хранилище, WAL, реестры
таблиц, audit-лог и его ротации, TLS PEM) в каталог
`<to>/YYYYMMDD_HHMMSS/`, исключая производные данные (`cache/`); существующий целевой каталог перезаписываться
отказывается. Источник (`data_dir`) задаётся файлом `--config`, отдельного
флага источника нет; реальная точка входа —
`crates/server/shamir-server/src/main.rs` (`Subcmd::Backup { to }`).

### Online-копия: когда и как

Используйте online-копию, когда сервер нельзя останавливать; offline
`backup --to` — когда остановка допустима и нужна максимально простая
гарантия (полная остановка writer'ов).

Условия и ограничения:

- Операция требует прав **superuser**; follower (read-only replica) отказывает
  в `StartBackup` и `CancelBackup` со стабильным кодом
  `online_backup_follower_read_only`; `BackupStatus` на follower разрешён.
- **Одна копия за раз** на сервер (in-process permit; второй `StartBackup`
  получает отказ).
- Операция node-local: копируется локальный `data_dir` этого узла.

Процедура через типизированный клиент и builder (только builder, без сырого
JSON; на каждую операцию — свежий `Batch` и `batch.build()`):

```rust
use shamir_client::builder::batch::Batch;
use shamir_client::builder::ddl::{backup_status, cancel_backup, start_backup};

// 1) Запуск копии в каталог назначения:
let mut batch = Batch::new();
batch.op("start-bk", start_backup("/backups/snap-1"));
let resp = client.execute("default", batch.build()).await?;

// 2) Статус (в т.ч. на follower) — опрашивать до стадии Done,
//    прежде чем использовать снимок:
let mut batch = Batch::new();
batch.op("bk-status", backup_status());
let resp = client.execute("default", batch.build()).await?;

// 3) Отмена выполняющейся копии (кооперативная: проверяется в точках
//    протокола и между файлами копирующего прохода — между файлами
//    копирование останавливается сразу, стадия не «донашивается»):
let mut batch = Batch::new();
batch.op("bk-cancel", cancel_backup());
let resp = client.execute("default", batch.build()).await?;
```

Статус — приблизительный: стадия меняется по мере продвижения, `dest_dir`
известен только после завершения прогона и только при успехе. Прогон
обычно завершается терминальной стадией: `Done` — при успешной
заключительной проверке, иначе `Failed`; исключение — отказ создания
timestamped-каталога (см. ниже). Ориентируйтесь на `running` и
`last_error`, а не только на стадию. При отказе в статусе доступен
отдельный необязательный `progress.failed_stage` (не во всех случаях —
см. ниже) и `last_error`.

### Кооперативная отмена, живой статус и границы снимка

**Точки проверки отмены.** `CancelBackup` кооперативный: флаг отмены
проверяется в фиксированных точках протокола — в начале подготовки (шаг A),
перед барьером захвата (шаг B), перед началом копирования (шаг C), перед
началом уплотнения (шаг E, seal) и перед началом верификации и записи
манифеста (шаг F), а также **между файлами** внутри каждого копирующего
прохода (отмена, замеченная между файлами, — это отмена, а не сбой).
Стадия при этом **не донашивается**: отмена между файлами выходит из
копирования сразу, ни текущий проход, ни копия дальше не продвигаются;
на пути отмены каталог
назначения удаляется, pins/seals снимаются. После начала заключительной
верификации новых проверок нет: отмена, запрошенная в этот момент, прогон
не остановит — он завершится со стадией `Done`, но только при успешной
проверке (при её сбое — `Failed(Verifying)`), а `cancel_requested` в
статусе просто останется выставленным.

**Ранняя ошибка — не один случай.** Отказы до тела протокола различаются
по тому, что при них публикует реестр:

- `StartBackup` валидирует каталог назначения **до** регистрации в реестре
  (`registry.rs::validate_destination`): невалидный каталог — просто ошибка
  вызова, запись реестра не создаётся и не меняется.
- Протокол атомарно создаёт каталог `<to>/<timestamp>/`. При отказе
  создания реестр публикует `running = false`, стадию `Failed`,
  `progress.failed_stage = Preparing` и `last_error`.
- Если поток прогона не удалось запустить, вызов возвращает ошибку, а реестр
  публикует `running = false`, стадию `Failed`, `progress.failed_stage = Preparing`
  и `last_error`. Следующий прогон может получить допуск.

**Живой статус.** Пока прогон выполняется, стадия — это подсказки адаптера
на границах обращений к движку: до начала копирования видна `Preparing`, с
начала шага C (`begin_copy`) — `Copying`, затем `Sealing`, но только если
протокол вошёл в seal-барьер хотя бы одного репозитория (при пустом захваченном
наборе подсказка остаётся `Copying` до `Verifying`); раз выставленная `Sealing`
держится и через остаточный проход. С реального начала шага F (`begin_verify`)
— `Verifying`. Живого `Captured` не бывает: эта стадия встречается только как
`failed_stage` и как метка метрики. Нормальные терминальные стадии — `Done` и `Failed`. Per-repo
счётчики хеширования: во время `running` публикуемый `per_repo` пуст
(локальные нулевые учётные записи шага B в статус не попадают —
`registry.rs:173-177`, `212-227`, `305-322`); реальные байты/файлы по
репозиториям появляются только после завершения прогона — при успехе
владелец заполняет их из манифеста снимка. Запись реестра и её прогресс
публикуются под одним lock, поэтому `running = true` финальных итогов по
репозиториям не несёт.

**Поздние репозитории.** Различайте два случая. Репозиторий, созданный
после захвата каталога (шаг A), не попадает в captured-набор: его файлы и
строка каталога ещё могут быть скопированы в снимок, но это не делает его
частью набора; не зарегистрированный после restore — другой случай. При
restore он именуется в отчёте только если его строка каталога оказалась
в staged-каталоге snapshot-копии: тогда строка
durable-удаляется из staged-каталога и репозиторий попадает в список
исключённых. Если строки в копии нет, репозиторий в снимке и в отчёте
просто отсутствует.

**Lock-файлы в снимке.** Online-снимок не содержит `.shamir.lock`
(maintenance-lock сервера) и вложенных Fjall-артефактов `lock` — это
runtime-артефакты, копирующий движок пропускает их по имени на всех ОС,
чтобы манифест не зависел от платформы. Обычный старт сервера
пересоздаёт `.shamir.lock` (`create(true)` + эксклюзивный fs4-lock).
Fjall свои `lock` **сам** не пересоздаёт: артефакт восстанавливает
staged-restore (`restore/online.rs` через `restore/fjall_locks.rs`) в
каталогах, опознанных как Fjall-хранилища. Offline `backup --to` своего
поведения не менял и артефакты lock по-прежнему копирует; остальные
исключения (`cache/`) без изменений.

### Точка консистентности манифеста

`manifest.json` online-копии содержит `kind = "online"`, `format_epoch`,
`binary_version` и на **каждый репозиторий** — `wal_position` (точка
консистентности, `V*`). Restore гарантированно содержит все записи,
**подтверждённые до** захвата точки; записи, подтверждённые **после** захвата,
в копию могут не попасть — это НЕ обещается.

**Поведение writer'ов при копировании.** Записи продолжаются во время
первого прохода копирования. При seal приём новых коммитов приостанавливается
по репозиторию до конца повторного копирования: внутри этой паузы снимается
seal-образец и повторно копируются WAL и хранилище; gate снова открывается
только после того, как безопасный шаг завершился и guard повторного
копирования снят.
Бюджет по умолчанию один: 8 с монотонно на репозиторий (включая барьер).
Байтового лимита по умолчанию в сервере НЕТ: повторное копирование
переписывает изменившиеся файлы целиком, а не только дельту, поэтому размер
изменяемого активного WAL/journal/LSM-хранилища — не граница задержки
записи. (Протокол синхронизации сохраняет опциональный явный байтовый лимит,
кумулятивный и включая повторы, но это не конфигурационный параметр для
оператора, пока код его не выставляет.)
Часы кооперативные — проверяются на границах шагов барьера и при повторном
копировании между файлами, порциями ввода-вывода/чтением/хешированием и в
паузах повторов, поэтому незавершённая запись/системный вызов прервана быть
не может и может превысить границу. Превышение — типизированный
`Failed(Sealing)` с очисткой. Writer ждёт, а после существующих 15 с может
получить типизированный повторяемый отказ; запись можно повторить.

### Audit-файлы в online-копии

Активный `audit.log` и его архивы копируются вместе с `data_dir`, но отдельным
этапом того же прохода: захват идёт **двумя фазами** (см.
[раздел «Audit log»](#7-audit-log)).

**Фаза 1** копирует остальные файлы `data_dir`, включая **оба** хранилища
якорей — `audit_checkpoint/` и `server_meta/`. Файлы аудита в ней
пропускаются, как и всё, что копия не переносит вообще: рабочие lock-файлы
(`lock`, `.shamir.lock`), подкаталог `cache/`, служебные файлы манифеста в
корне и не регулярные записи (например, символические ссылки). Исключения
проходов различаются: проход **D** исключает только каталоги WAL, а
остаточный проход **E** — каталоги WAL **и** хранилищ репозиториев (их
перекопировал seal-повтор каждого репозитория).

**Фаза 2** начинается только после её завершения: каталог аудита
перечисляется заново, копируются архивы (`audit.log.` + 20 цифр) в порядке
возрастания имени, затем — активный `audit.log`, если он есть в перечислении
этого раунда. Архивы имеют приоритет перед активным файлом: любой не захваченный
архив вызывает немедленный отказ, даже если остался и активный `audit.log`.

Почему это даёт согласованность: аппендер аудита сбрасывает (flush + fsync)
байты журнала **до** записи якоря — при контрольной точке и на границе ротации
(там же fsync, переименование, переоткрытие, сброс активного дескриптора,
fsync каталога — на платформах не-Unix он ничего не делает — и только затем оба якоря). Любой якорь, попавший в копию,
покрывает байты, которые к моменту перечисления фазы 2 уже лежали в файлах
аудита; журнал только дописывается, а ротация лишь **добавляет** архив и
никогда не переписывает существующие. Поэтому инвариант копии: `next_seq`
захваченного якоря не больше проверенного хвоста захваченного журнала.

Ротация, случившаяся между перечислением фазы 1 и захватом якорей, — то самое
окно, которое одиночное перечисление закрыть не может: новый архив появился бы
после списка, а активный журнал был бы уже новым и пустым. Порядок фаз его
закрывает: якоря захватываются раньше, чем журнал вообще перечислен. Если
ротация попадает внутрь фазы 2 (между её перечислением и копированием), свежее
перечисление после раунда находит архив, которого раунд не копировал, и проход
делает ещё один раунд (каждый раунд копирует все перечисленные файлы заново).
Неустойчивых раундов захвата не больше четырёх. Дополнительно допускаются до
трёх попыток перечислить и захватить заново активный журнал, пока остаток
назначения — один лишь `audit.log`; итого — не более семи раундов захвата на
один проход data-dir. Это предел
числа выполненных работ, а не временной интервал или дедлайн. Устойчивый раунд
является кандидатом на завершение: он завершает проход, если проверка назначения
не находит не захваченных файлов; допустимый остаток одного лишь активного
журнала может потребовать повторной попытки. Отсутствие устойчивого раунда —
типизированный отказ прохода (`AuditFilesUnstable`), а не молча несогласованный
снимок. Нечитаемая запись каталога аудита — ошибка самого итератора или чтение
метаданных — типизированный отказ прохода (`AuditListing`; пути файла в тексте
отказа нет), а не молча пропущенная запись: пропуск мог бы скрыть новый архив.
`NotFound` итератора также является `AuditListing`; пропускается только
`NotFound` при чтении метаданных уже перечисленной записи.

Реконсилиация **не удаляет** из каталога назначения ни файлы аудита, ни файлы
`server_meta` (публикация удаляет только собственный
`backup.meta.json.tmp`, а очистка неудачного прогона пытается удалить весь
каталог прогона целиком — см. ниже). Если в каталоге назначения остался
служебный файл цепочки аудита (`audit.log` или архив `audit.log.` + 20 цифр),
которого принятый (определяющий манифест) раунд не захватил — его скопировал
более ранний раунд этого прохода, более ранний проход (D) или ротация,
переименовавшая активный журнал и ещё не переоткрывшая его, — проход отказывает
типизированно и без пути в тексте (`AuditArchiveLost`) **до** публикации манифеста.
Любой не захваченный архив вызывает немедленный отказ, даже если оставшийся
активный `audit.log` тоже не захвачен. Проверка имён архивов сама по себе
подтверждает только появление нового имени, но не непрерывность цепочки и не
содержимое архива. Единственное исключение
— окно переоткрытия активного журнала: только если не захвачен один лишь
`audit.log`, проход делает до трёх дополнительных попыток захвата (в пределах
общего лимита семь), чтобы увидеть переоткрытый файл и скопировать его. Сама
повторная попытка не требует свидетельства; свидетельство требуется для ПРИЁМКИ
вновь появившегося активного журнала: принятый раунд должен показать архив,
которого не было в базе назначения, замеченной в раунде без активного журнала
(свидетельство переносится на момент повторного появления). Замена содержимого
под тем же именем без нового архива не доказывает ротацию и остаётся отказом;
проверка имён не доказывает непрерывность цепочки, доказательство — новый архив.
Замена перезаписывает устаревшую копию активного журнала в каталоге
незавершённого прогона до отказа: манифест не публикуется и завершённые копии не
затрагиваются. Отказ — безопасный исход: очистка
пытается удалить весь каталог прогона, но удаление может завершиться ошибкой
и оставить каталог. Последующий подходящий тик может начать новый прогон с
нуля; повторная попытка планировщика для того же прогона не гарантируется.
Публикация в таком состоянии либо описала бы набор файлов, которому каталог назначения
противоречит (ту же копию отвергла бы собственная инвентаризация при
self-verification), либо потеряла бы байты, которые может называть захваченный
якорь.
Ротация, которая переименует активный журнал ровно в момент повторного
копирования уже изменившегося файла (или его повторного хеширования), НЕ срывает
этот раунд: в фазе аудита исчезнувший **источник** в этом раунде считается
пропавшим (`missing`), свежее перечисление находит новый архив, и следующий
раунд его копирует — в снимке остаются все байты, которые перенесла ротация.
Терпится именно исчезновение источника в раунде, а не прогон целиком: если
активный файл не вернётся до конца ограниченного окна переоткрытия, проход
отказывает (см. выше), а не публикует снимок со старой копией. В любом другом
проходе (WAL, хранилища, фаза 1 остаточного прохода, seal-повтор) исчезнувший
источник во время повторного копирования по-прежнему завершает проход
типизированной ошибкой ввода-вывода (`NotFound`), и такую копию надо повторить.

Следствие для оператора: в копии журнал может быть **длиннее** якоря — это
нормально и рассогласованием не является: записи после последней контрольной
точки якорь не покрывает, а при запуске их проверяет восстановление по правилам
раздела «Audit log»: оборванная последняя запись активного журнала при этом
усекается, но разборчивая запись, у которой отсутствует только терминатор,
восстановлением отвергается. Рассогласование — обратное: якорь за хвостом
журнала.

Ретенция аудита. Если ретенция удалила архив между перечислением фазы 2 и его
копированием и копия этого файла в каталоге назначения не осталась, архив просто
исключается — как любой удалённый исходный файл. Если копия осталась (её сделал
более ранний раунд или более ранний проход), проход отказывает
(`AuditArchiveLost`, см. выше). Потеря архива не обязательно означает
`UnanchoredSuffix`: восстановление принимает любой якорь, попадающий в
сохранённый диапазон, и продолжает с проверенного HMAC-хвоста, поэтому более
старый, но совпадающий якорь может быть корректен. Непокрытым остаётся узкий
случай: ретенция удалила сегмент, на который указывают захваченные якоря,
ПРЕЖДЕ чем хоть один проход оставил копию этого архива в каталоге назначения
(например, до перечисления прохода D) — тогда копия не содержит этих байтов,
запуск откажет, но принятый раунд NAME-ONLY проверки `AuditArchiveLost` не
имеет остаточной копии для обнаружения: это не валидатор audit chain. Архив,
захваченный принятым раундом, сам по себе не вызывает `AuditArchiveLost`.
Небезопасное повторное появление активного `audit.log` может вызвать эту ошибку
и без остаточной копии: guard повторного появления срабатывает раньше проверки
остатка. Проверка `AuditArchiveLost` сравнивает только точные зарезервированные имена файлов, без приведения к нижнему регистру; case-folding применяется только инвентарём v2. Валидатор фазы F перечисляет зарезервированные audit-файлы из завершённого каталога назначения: архив, присутствующий только в копии, проверяется, а оставшийся только в живом источнике не требуется; источник решает лишь, должна ли цепочка существовать вообще. Инвентарь v2 отклоняет payload-путь, совпадающий после приведения к нижнему регистру с именем корневого control-файла (`manifest.json`, `backup.meta.json`), если написание не точное. Проверка идентичности staged receiver на стороне restore (`restore/staged_listing.rs`) этими правилами не охватывается.

`server_meta` копируется файл за файлом, пока сервер работает, вне любого seal,
поэтому файлы могут быть сняты не в одной согласованной точке: живое хранилище
Fjall дописывается хуком аудита на каждой границе ротации. Проверка фазы F
выполняется через портовый hook, по умолчанию no-op; её реализует только серверный
адаптер, поэтому копия от старого сервера такой проверки не проходила. На фазе F,
до публикации манифеста, disposable clone копий `server_meta` и appender anchor
`audit_checkpoint` открывается для проверки Fjall и строгой проверки обоих якорей
(`server_meta` anchor и appender `audit_checkpoint` anchor) против скопированной
audit chain. Scratch root живёт под каталогом временных файлов ОС и создаётся
эксклюзивно, но удаляется ПО ПУТИ; identity-bound deletion при допускающем замену
родителе находится вне области этой гарантии. Recovery не изменяет итоговое дерево
снимка; этот этап неотменяем внутри выполнения, поскольку scratch clone
пропорционален `server_meta`, хранилищу `audit_checkpoint` и скопированным
audit-файлам. Ошибка валидации фазы F выигрывает через `?`; отмена проверяется после
успешной валидации, сразу перед публикацией манифеста, и этот последний check
охватывает подготовку и кодирование манифеста. При ошибке порта результат фазы —
`verifying: port-failed`. Проверка может обнаружить, например, смешанную копию, где
новый `current` указывает на отсутствующую версию, но не доказывает полноту
исходного архива и не восстанавливает байты, потерянные до того, как проход оставил
их копию в назначении. `verify_manifest` проверяет файлы, но никогда не проверяет
audit chain.

Офлайн/v2 verifier отклоняет case-folded псевдонимы и требует точного
совпадения написания каждого пути манифеста с перечисленным путём на диске.

Копия от **более старой** версии сервера (или собранная вручную) не проходила
эту проверку фазы F: `audit.log` мог быть снят раньше якорей. Такой каталог может
отказать не на restore, а на **первом запуске**: сервер не стартует и печатает
`server boot failed: audit recovery: ...` (имя варианта ошибки в вывод не
попадает; если якорь оказался за хвостом журнала, в коде это `AnchorPastTail`).
Ручного ремонта якорей не предусмотрено — см. [раздел «Audit log»](#7-audit-log):
берите audit-согласованный комплект из доверенной резервной копии (журнал с
файлами ротации, ключ и обе контрольные точки) и не правьте строки журнала,
`audit_checkpoint` и `server_meta` вручную.

`on-disk` перечисление запрещает имена файлов, не представимые в UTF-8: они
отклоняются как `UnmanifestedFile`, поскольку JSON-манифест не может их назвать.
Дубликаты дисковых идентичностей после `to_lowercase()` отклоняются как
`DuplicateManifestEntry`; lossy identity не используется.

Команда `shamir-server backup-verify SNAPSHOT` — read-only офлайн-проверка с кодом
0 при успехе, 1 при ошибке использования и 5 при ошибке проверки; тот же
верификатор доступен через API `shamir_server::backup::verify_manifest(<каталог
снимка>)`. Оба проверяют **файлы**: контрольные суммы по манифесту, наличие всех
перечисленных файлов и отказ от незаявленных файлов; сверяют эпоху манифеста со
штампом и проверяют listing WAL. Тело WAL не валидируется — это
делает restore (replay). `binary_version` из метаданных — информационное
поле, не условие отказа.

### Restore online-копии

Restore — offline-действие на **остановленном** целевом сервере. Для
`kind = "online"` перед подменой `data_dir` выполняется replay WAL каждой
репозитории из `consistency.per_repo` до её `wal_position`. Fail-closed
валидации до swap не подменяют и не усекают целевой `data_dir` при отказе,
но это не означает отсутствия любых побочных изменений: настроенное файловое
логирование CLI может создать файл лога внутри цели ещё до проверок restore.
Для цели, являющейся корнем файловой системы, staging размещается внутри неё;
его очистка при отказе best-effort. Репозиторий, созданный **после** захвата,
не входит в захваченный набор: его каталог может физически оказаться в снимке,
но в restore он не регистрируется и не восстанавливается — когда и с какой
меткой он появляется в отчёте, см. выше («Поздние репозитории»).

**Содержимое хранилища и WAL выше точки `V*` — fail-closed отказ.** Strict
replay сверяет версии с точкой `V*` **до** применения какой-либо записи:
WAL-версия выше точки даёт типизированный отказ `EntryAboveVStar`, версия
открытого хранилища выше точки — отказ `Recovery` поверх `Validation`; swap и
усечение цели не выполняются. Это не обещает полностью неизменный `data_dir`:
файловый лог CLI может быть создан в нём до проверки, а при цели-корне staging
создаётся внутри цели и очищается best-effort. Хранилище на уровне точки
или ниже — не повреждение: это допустимо, только если сохранённый WAL
покрывает его до точки, и replay доводит состояние до `V*`. Ручная подмена
файлов, дающая версии выше точки, всё же приводит к отказу, а не к приёму
сомнительных данных.

**Windows:** копирующий движок выполняет ограниченное число повторов при
sharing violation (os error 32/33); при исчерпании повторов копия завершается
отказом, а не портит данные.

Метрики online backup — экспортируемые сегодня серии `shamir_backup_*`
(`kind=online`): счётчики
`shamir_backup_started_total`, `shamir_backup_completed_total`,
`shamir_backup_failed_total{kind,stage}` (стадии сбоя
`preparing|captured|copying|sealing|verifying`), gauges
`shamir_backup_active` (0/1), `shamir_backup_bytes_copied`
(кумулятивные байты завершённых прогонов), `shamir_backup_duration_seconds`
(длительность последнего **успешного** прогона) и
`shamir_backup_writer_block_seconds{phase=capture|seal}` — последнее закрытое
окно блокировки writer'ов по фазе, включая неудачные удержания; не выставляется
до первого закрытого окна. Зарезервированные серии `shamir_backup_pin_seconds`
и `shamir_backup_wal_segments_copied_total` **пока не экспортируются**.

Хранилище записывает в отчёт метки исключённых репозиториев; агрегированный
online-статус — только по новым репозиториям.

### Расписание и хранение копий

Без блока `backup` встроенное расписание выключено. Пример блока верхнего уровня в конфиге сервера:

```ktav
backup: {
    destination: /backup/shamir-scheduled
    interval: 24h
    run_on_start: true
    retention: {
        keep_last: 7
    }
}
```

- `destination`: заранее создайте отдельный приватный абсолютный каталог, не ссылку и не reparse point.
  Канонический путь не должен совпадать с `data_dir`, лежать внутри него или быть его предком. Сервер не
  создаёт этот корень и не исправляет права; перед использованием проверяет путь и приватность заново.
- `interval`: целое положительное число ASCII-цифрами с суффиксом `s`, `m`, `h` или `d` (`d` = 24 часа),
  без пробелов, дробей и составных значений. Минимум 60 секунд; переполнение и некорректные значения
  отклоняются при старте.
- `window` (например, `window: 23:00-04:00`) по умолчанию отсутствует. Это локальное время сервера,
  начало включено, конец исключён; одинаковые границы запрещены. Окно ограничивает **запуск**, не
  завершение. При переходе часов повторённый час может принять два разных тика, пропущенный — ни одного.
- `run_on_start` по умолчанию `false`: первый тик через интервал после готовности сервера. При `true`
  первый тик сразу после готовности, но окно и занятость по-прежнему проверяются.
- `retention.keep_last` по умолчанию 7, допустимо от 1 до 1000: обе границы проверяются при разборе
  конфига отдельной константой политики (`MAX_KEEP_LAST`), а не вычисляются из лимита обхода корня.
  Лимит прохода — отдельная проверка: не более 4096 элементов корня, включая свежую копию и каждый
  посторонний элемент. Ключ `retention.max_age_days` удалён: политики по возрасту нет, удаления
  определяют `keep_last` вместе с отдельной защитой свежей копии; выбираются только пригодные
  кандидаты — помеченные этим владельцем либо принятые по записи о происхождении, — а непроверяемые
  элементы не выбираются и попадают в `shamir_backup_retention_untouched`.

**Изменение.** Ключ `retention.max_age_days` удалён: конфиг, где он ещё указан, не запускается и
завершается ошибкой неизвестного поля; удалите строку `max_age_days` из блока `retention`.

Офлайн-подкоманды (`restore --from`, `backup-verify`, `export/import logical`) проверяют только СИНТАКСИС блока расписания: существование каталога назначения, его приватность и непересечение с `data_dir` проверяются при запуске сервера (`Config::validate_for_launch`), поэтому офлайн-команда не зависит от смонтированного каталога резервных копий.

Тики идут по монотонным часам; расчёт прошедшего времени ограничен снизу нулём. При позднем пробуждении
рассматривается не более одного тика; пропущенные тики не догоняются. После запроса остановки новый
запуск не начинается; затем проверяется окно (`skipped_window`), затем занятость (`skipped_busy`).
Очереди и немедленных повторов нет. Планировщик работает локально от имени системы, без сессии;
ограничения superuser/follower для ручных операций не меняются.

Ручные и плановые копии используют один реестр и один допуск: `running` в статусе удерживается до полного
завершения обработки хранения, поэтому ручной запуск и следующий тик отклоняются и во время очистки; сам
`BackupPermit` (допуск на копирование) освобождается до её начала. Исключение — путь unwind: `RunGuard`
сам публикует запись об отказе и снимает `running`. Паника ПОСЛЕ публикации исхода копии учитывается как
ОДНА ошибка хранения (`shamir_backup_retention_failed_total`) и только у планового прогона; второго сбоя
копии она не даёт.

#### Исход копии, ошибки и серия сбоев

Исход копии публикуется на завершении протокола, ДО прохода очистки: там же меняются
`shamir_backup_completed_total`, `shamir_backup_failed_total`, `shamir_backup_active` и сигнал
планировщику о завершении прогона. Паника в очистке считается ОДНОЙ ошибкой хранения
(`shamir_backup_retention_failed_total`) и не является сбоем копии: исход копии к этому моменту уже
опубликован. Два исключения из этого порядка. Паника работника ДО публикации закрывает канал исхода без
значения, и запись публикует `RunGuard` на unwind — как отказ. Если запись уже принадлежит другому
прогону (`started_at_unix_ns` не совпадает: замена или сброс после неудачного spawn), `RunGuard` не
публикует ничего и только доставляет поздний сигнал. Отмена, замеченная в проходе очистки, ошибкой
хранения не считается.

`shamir_backup_scheduled_failure_streak` растёт на каждом плановом `failed` (включая отказ допуска
запуска) и сбрасывается на `completed` — то есть на завершении копии, а не очистки; пропуски
(`skipped_busy`, `skipped_window`) её не меняют.

`last_error` планового прогона не содержит ни путей, ни текста ОС: это `<стадия>: <причина>`
(`OnlineBackupError::stage_and_reason`). Стадии — `preparing`, `captured`, `copying`, `sealing`,
`verifying`, `done` и `failed`; причины — `busy`, `cancelled`, `port-failed`, `manifest-failed`,
`verify-failed`, `recopy-cap-exceeded`, `io-<kind>`, `destination-io-<kind>` (ошибка подготовки или
публикации каталога назначения), `copy-cancelled`, `copy-io-<kind>` (ошибка
чтения источника, в том числе отказ перечисления каталога аудита),
`copy-destination-io-<kind>` (ошибка каталога назначения, в том числе отказ его
перечисления перед публикацией), `copy-recopy-cap`, `copy-audit-files-unstable` и
`copy-audit-archive-lost` (в каталоге назначения остался служебный файл цепочки
аудита, которого принятый раунд не захватил; копия ничего не удаляет). Если архив исчез до любого прохода, не оставив копии в назначении, `AuditArchiveLost` нечего обнаружить; копия, оставленная в назначении, — именно то, что этот guard обнаруживает. На фазе F до публикации манифеста скопированный `server_meta` проверяется на disposable clone; это не доказывает полноту исходного архива. Неудачный прогон не повторяется на месте, а подходящий позднее тик пропускает его или начинает новый прогон; `<kind>` — имя `io::ErrorKind` в kebab-case. Примеры: `copying: copy-destination-io-permission-denied`, `sealing: recopy-cap-exceeded`, `sealing: copy-audit-archive-lost`, `verifying: port-failed` (ошибка проверки disposable clone на фазе F), `verifying: verify-failed`.
Исключение — паника потока планового прогона: тогда в `last_error` лежит проза, а не `<стадия>:
<причина>`, и стадия указана в ней только как подсказка (`stage hint`), а не как точное значение. При
этом ни пути файловой системы, ни текста ошибки ОС в таком сообщении нет.
Ручной прогон по-прежнему хранит в `last_error` полную цепочку `Display` — с текстом ОС и путём; поля
`destination` и `dest_dir` ответа `BackupStatus` остаются путями намеренно. Плановые журнальные записи о
подготовке маркера (`reason = "marker-unavailable"` и `kind`) и об отказе capture манифеста (`reason =
"manifest-capture-refused"`) тоже без пути.

Если неудачный прогон не смог удалить собственный частичный каталог-снимок, это сообщается отдельно: к
коду отказа добавляется суффикс `; cleanup-failed-<kind>`, первичная причина остаётся главной, а `kind` —
только `io::ErrorKind` неудавшегося удаления.

#### Последний успех, перезапуск и восстановление

`shamir_backup_last_success_timestamp_seconds` — самое свежее свидетельство работоспособности копирования:
время последней успешной копии (плановой или ручной) либо, до первого успеха, время ПЕРВОГО взведения
расписания. Обе отметки хранятся в `server_meta` (keyspace fjall `server_meta_v1`):
`backup_last_success_unix_secs_v1` пишется на успехе протокола, `backup_armed_at_unix_secs_v1` — один раз
при первом включении расписания, и удаляется, когда расписание не настроено. Если расписание удалить и
позже включить снова, запись к этому моменту уже удалена, поэтому первая же загрузка с расписанием
запишет НОВОЕ время взведения: отсчёт устаревания начинается с неё, а промежуток, когда расписания не было,
устареванием не считается. Удаление записи — best-effort и НЕ атомарно: строка сначала удаляется, затем сбрасывается на диск, поэтому при сбое на диске остаётся либо старая запись, либо ничего — исход НЕОПРЕДЕЛЁН; в журнал попадает один ограниченный токен. Если старая запись сохранилась, следующая загрузка с расписанием её оставляет, и отсчёт продолжается от СТАРОГО времени взведения; если её нет, взведение записывается заново. Нечитаемая запись — отдельный случай: загрузка берёт текущее время ТОЛЬКО для этого запуска (`read-armed-at-failed`) и запись не перезаписывает.
При загрузке значение засевается максимумом этих двух записей, успех его поднимает, а значение из будущего
ограничивается текущим временем ОДИН раз — последующие scrape возвращают то же значение, поэтому возраст
растёт; перезапуск и восстановленный `data_dir` несут обе сохранённые записи того снимка. На диске хранятся
только отметки времени, поэтому `shamir_backup_last_duration_seconds`, `shamir_backup_last_bytes` и
`shamir_backup_duration_seconds` относятся к успеху текущего процесса: после перезапуска они 0 (последний
не экспортируется) до первого успеха.

#### Остановка сервера

При штатной остановке сервер запрашивает кооперативную отмену (корневой токен → дочерний флаг каждого
прогона), и новый тик не начинается. У прогона ДВА сигнала: исход копии (серия, время успеха, счётчики)
публикуется на завершении протокола, до очистки; ПОЗДНИЙ сигнал «работник закончил» — только после очистки
(включая пойманную панику в ней) и финальной публикации записи. Планировщик ждёт поздний сигнал, поэтому
`ShutdownOwner::join_schedule` ждёт весь прогон с очисткой, и блокировка каталога данных освобождается
только после этого. Выполняющийся системный вызов не прерывается. Общий 30-секундный тайм-аут `serve` может
отметить остановку как нештатную, но работник и блокировка каталога данных дожидаются уже после дедлайна,
и сервер принудительно не завершается; принудительное убийство процесса оператором может оставить
недописанную или частично удалённую копию. При внешнем дедлайне ранние этапы разделяют фиксированную границу `outer deadline − LATE_RESERVE` (1 с): тайм-аут раннего этапа записывается в отчёт и дренаж переходит дальше, но сам по себе не запускает очистку, которая ещё не начиналась. Каждый поздний WAIT заканчивается на более раннем из `stage start + cap` и `outer deadline − reserve`: для присоединения задачи метаданных cap равен 5 с, reserve — 500 ms; для освобождения хранилищ cap равен 10 с, reserve — 250 ms. Это ограничивает только ожидания (bounded waits), синхронная работа не прерывается cutoff. Истечение ожидания прекращает только ожидание — задача стадии может продолжить работу, она не останавливается ровно у резерва.

Удаление снимков запускается только после успешной плановой копии. При этом начальный проход, уже после
проверки и пометки новой копии, может убрать подходящий аварийный staging-файл из корня; отказ позже не
отменяет эту уборку. Для retention-кандидатов рассматриваются только непосредственные каталоги с именем
`YYYYMMDD_HHMMSS` (UTC) и проверенным манифестом v2 online: копия пригодна, если несёт метку этого
владельца, либо — оставшись без метки — точно совпала с записью происхождения (то же имя, время создания
манифеста и SHA-256 его байтов), и тогда проход сначала ставит метку в цикле принятия; совпадение с копией,
уже несущей метку этого владельца, сообщается как принятое без повторной записи. Порядок
удаления — от старых к новым по времени из манифеста, при равенстве — по имени каталога. Новейшие
`keep_last` и только что созданная копия защищены даже при откате часов. Если свежая копия не проходит
проверку или упёрлась в лимит до пометки, метка не ставится: отказ структурной проверки каталога или
проверки манифеста возвращается как `Refusal`, проход не падает, а вызывающий считает это одной ошибкой
хранения.

Посторонние файлы в общем случае остаются на месте; исключение — зарезервированное имя
`.shamir-retention-staged-` плюс ровно 16 ASCII-букв или цифр: старый (не моложе 15 минут) plain regular
файл такой формы может быть удалён при очистке после аварийного завершения staging. Возраст защищает
живой файл staging лишь пока его время изменения моложе порога (остановившийся писатель или скачок часов
могут пережить порог; время изменения не позже эпохи Unix, в том числе нулевое, мусором не считается), не
обеспечивает единственного владельца корня и не вводит блокировку между планировщиками. Более новые
файлы, имена только с префиксом, суффиксы другой длины/формы, каталоги, ссылки и reparse points
сохраняются и считаются обычными элементами корня. Чужие, ручные, немаркированные (без записи о
происхождении), частичные и непроверяемые снимки, а также небезопасные деревья не выбираются на удаление.
Если проход отказал до завершения сканирования или проверки перед меткой, он не удаляет снимки; уже
выполненная уборка старого staging-файла остаётся. Различайте три случая подготовки маркера блокировки:
- **при старте сервера** — best-effort: сбой даёт только WARN-запись в журнале и в политику хранения
  (метрики, исходы очистки) не включается;
- **перед публикацией плановой копии** — отдельная попытка, включаемая в политику хранения: если она была
  выполнена и сорвалась, очистка этого запуска считается ошибкой хранения — независимо от результата
  самой копии (успех, отказ или отмена); удаления в этом прогоне нет;
- **в начале прохода удаления** — проход запускается только после успешной плановой копии; сбой
  подготовки маркера здесь — тоже ошибка хранения: удаления снимков нет. После устранения препятствия
  более поздний проход может повторить работу, но это не обещано. Исход копии учитывается отдельно; сбой
  маркера меняет только исход хранения.

Только копия, протокол которой завершился успешно после неудачной попытки подготовить маркер **до
копирования**, может быть записана для условного принятия. После возврата протокола сервер читает
манифест ограниченным чтением и сохраняет его время создания и SHA-256 байтов; этот capture происходит
после протокола, не «при публикации». Затем проход сканирования и поздняя проверка перед меткой
сравнивают SHA-256 байтов манифеста; время создания входит в эти байты. Кандидат без метки получает
метку; запись, совпавшая с копией, которая уже несёт метку этого владельца, считается принятой без
повторной записи метки. Замена каталога или payload-файлов при неизменном манифесте сравнением не
обнаруживается. Если capture и декодирование успешны, замена до capture будет зафиксирована в памяти как
состояние, наблюдённое после возврата протокола. Изменения payload, не отражённые в манифесте, сравнением
не выявляются; другие изменения корня тоже могут остаться незамеченными. Принятие опирается на
single-writer контракт, а не на доказательство идентичности каталога или ownership его содержимого.

Не всякая немаркированная копия принимается: например, сбой capture не создаёт запись, а отмена,
замеченная после успешного протокола и до начала пометки, при исправном маркере оставляет завершённую
копию без метки и без записи для adoption (отмена, замеченная после подготовки метки и до её публикации,
тоже оставляет копию без метки; после публикации метки свежая копия остаётся помеченной и без записи).
Копия, у которой позже завершился проход с ошибкой, также не получает запись для adoption, если маркер
был исправен до копирования: такие проходы не регистрируют происхождение для последующего принятия.
Оператору следует проверить такую копию (`verify_manifest`), затем вручную переместить её за пределы
корня или вывести из эксплуатации/удалить — только при остановленном или не пишущем в корень сервере и без
параллельного копирования и очистки: корень однописательский по контракту, и метку вручную не ставят.
Также поступайте с копиями, запись о которых утрачена при перезапуске сервера. Копирование копии в другое
место **не** уменьшает корень и не освобождает занятое им
место. Перемещение за пределы корня на той же файловой системе уменьшает число элементов корня, но не
освобождает блоки диска; место на этой файловой системе освобождают удаление или перенос на другую
файловую систему. Реестр происхождения хранит не более **64 записей**; при переполнении вытесняется самая
старая, и её копия не попадёт под политику хранения. Он существует только в памяти работающего сервера:
после перезапуска записи теряются. Пропуск снимка, занятого читателем, ошибкой очистки не считается — это
только debug-запись в журнале сервера. Ошибки очистки записываются в журнал и метрики отдельно от исхода
копирования. Метка владельца зависит от постоянного случайного ID в `server_meta` и канонического пути
`data_dir`, а не от публичной идентичности или ключей сервера. Ротация ключей не меняет метку; перенос
или клонирование на другой канонический путь меняет её, и старые копии не удаляются по новой метке. Для
клона и ручных копий используйте отдельные каталоги. Не редактируйте содержимое управляемого корня, не
делите его с другим писателем и ничего не монтируйте внутри: Unix-проверка устройства не обнаруживает
bind mount на том же устройстве. Не настраивайте два сервера на один и тот же корень резервных копий: код
этого не запрещает.

Лимиты одного прохода очистки: не более 1 ТиБ суммарного размера прочитанных файлов, не более 4096
элементов корня (свежая копия занимает слот наравне с каждым посторонним элементом; зарезервированный
маркер `.shamir-snapshot.lock` и удалённые staging-файлы слот не занимают), не более 100 000 элементов
дерева одной копии и не более 1 000 000 элементов дерева за проход — суммарно по всем проверяемым копиям,
а не на корень. Байты считаются по чтениям проверяющего с учётом момента предварительного начисления: файл из манифеста — один раз, сегмент WAL — дважды (общий цикл файлов и цикл WAL хешируют его дважды). Для присутствующего штампа первая инспекция оплачивается до её вердикта; если вердикт допустим, вторая инспекция предварительно оплачивается сразу после него — до последующих структурных проверок, чтений содержимого и запуска проверяющего. Если штамп перечислен в манифесте, его хеш файла в общем цикле файлов также предварительно оплачивается до хеширования. Поэтому последующий структурный отказ или ошибка проверяющего могут оставить в бюджете начисление за чтение, которое не состоялось. Ограниченные чтения служебных файлов тоже оплачиваются: `manifest.json` при классификации и ещё раз при принятии, `backup.meta.json` один раз на кандидата (свежая копия помечается уже прочитанным сайдкаром), а при принятии кандидата поздние повторные чтения сайдкара и манифеста для усыновления начисляются до сравнения дайджеста. Отклонённый сравнением кандидат оплачивает эти чтения, если они состоялись; если бюджет исчерпан во время поздних чтений, дальнейшие чтения не выполняются. Каждое ограниченное чтение через `budget.read` начисляется как `meta.len() + 1` байт: один запасной байт ловит смену размера. Обход каталога содержимое не читает, а лишний файл, который проверяющий не читает, стоит один элемент каталога. Отказавший кандидат не оплачивает чтения, до которых выполнение не дошло; начисление за чтение, предварительно выставленное до проверки, не гарантирует, что байты действительно были прочитаны.
Превышение любого лимита останавливает очистку: проход завершается
предупреждением в журнале, счётчик `shamir_backup_retention_failed_total` растёт, срабатывает алерт
`BackupRetentionFailing`, а копии продолжают создаваться. Освободите место
вручную — переместите или удалите старые копии за пределами корня — и очистка возобновится на следующем
успешном проходе; автоматического продолжения с места остановки нет.

Проверенная копия этого сервера, оставшаяся без метки владельца из-за аварийного завершения,
автоматически не принимается и не удаляется: её нельзя отличить от чужой. Проверьте её
(`verify_manifest`) и вручную переместите за пределы корня либо удалите — когда сервер не пишет в корень и
не идёт ни копирование, ни очистка.

Если сервер запущен как сервис systemd и использует `ProtectSystem=strict` (см.
`deploy/shamir-db.service`), каталог назначения должен быть явно разрешён на запись, иначе каждая
плановая копия отказывает уже на создании каталога. Если каталог назначения уже лежит в поддереве,
которое unit разрешает на запись (поставляемый unit разрешает `/var/lib/shamir-db` и
`/var/log/shamir-db`), отдельный drop-in не нужен. Иначе добавьте drop-in (подставьте свой путь):

```
# /etc/systemd/system/shamir-db.service.d/backup-destination.conf
[Service]
ReadWritePaths=/path/to/backup/destination
```

После добавления выполните `systemctl daemon-reload` и перезапустите сервис.

Дополнительные серии `/metrics` (кроме `result` у счётчика запусков, собственных меток у них нет):

| Метрика | Что показывает |
|---|---|
| `shamir_backup_last_success_timestamp_seconds` | Самое свежее свидетельство работоспособности копирования, Unix-секунды: время последней успешной online-копии (ручной или плановой) либо, до первого успеха, время ПЕРВОГО взведения расписания (долговечная запись `backup_armed_at_unix_secs_v1`, один раз при первом включении, удаляется без расписания). Обе отметки засеваются при загрузке максимумом, значение из будущего при scrape ограничивается текущим временем ОДИН раз (дальше возраст растёт); восстановленный `data_dir` несёт обе записи того снимка. |
| `shamir_backup_last_duration_seconds`, `shamir_backup_last_bytes` | Секунды и байты данных по манифесту последней успешной копии; относятся только к текущему процессу (на диске хранится лишь отметка времени): после перезапуска 0 до первого успеха. |
| `shamir_backup_scheduled_runs_total{result=…}` | `started`, `skipped_busy`, `skipped_window`, `failed`, `completed`; исход копии публикуется на завершении протокола и очистку не включает. |
| `shamir_backup_scheduled_failure_streak`, `shamir_backup_schedule_enabled` | Серия сбоев подряд и признак включения (0/1). Серия растёт на плановом `failed` (включая отказ допуска) и сбрасывается на `completed`, то есть на завершении копии, до очистки; пропуски и ручные копии её не меняют, паника в очистке её не трогает. Обе серии живут в памяти процесса и начинаются с 0 после перезапуска. |
| `shamir_backup_schedule_started_timestamp_seconds` | Время запуска расписания в секундах Unix; 0 до включения; после перезапуска — новое значение. |
| `shamir_backup_retention_deleted_total`, `shamir_backup_retention_failed_total`, `shamir_backup_retention_untouched` | Счётчики удалений и ошибок, число оставленных чужих или непроверяемых элементов по последнему полному обходу. Ошибки включают срывы подготовки маркера перед плановой копией и в начале прохода удаления, ошибки метки, аренды удаления и удаления, а также панику в проходе очистки; сбой при старте сервера (best-effort), отмена прохода и пропуск занятого читателем снимка сюда не входят. |

**Prometheus.** Настройте сбор `/metrics` каждого узла, установите
`deploy/alerts/shamir-backup.rules.yml` на сервер Prometheus и добавьте его путь в `rule_files`.
Проверьте итоговый файл через `promtool check rules`, перезагрузите конфигурацию Prometheus и проверьте
доставку в Alertmanager.

- **BackupStale:** при `schedule_enabled == 1` возраст считается от
  `shamir_backup_last_success_timestamp_seconds` — самого свежего из последнего успеха и первого взведения
  расписания (поэтому до первого успеха — от взведения). Сигнал срабатывает, когда возраст превышает порог
  непрерывно в течение `for: 5m`.
  `BACKUP_STALE_THRESHOLD_SECONDS` — параметр подстановки при развёртывании, не переменная Prometheus:
  замените литерал `129600` секунд (1,5 × 24 ч) на 1,5 × интервал в секундах, увеличив порог при
  необходимости с учётом времени копирования, допустимого окна и плановых простоев; для разных расписаний
  ограничьте варианты правила соответствующими целями сбора. Ручная успешная копия сбрасывает
  устаревание.
- **BackupFailing:** расписание включено, серия плановых сбоев и счётчик плановых исходов `failed` оба не
  меньше 3; именно серия доказывает, что сбои шли подряд. Серия растёт на каждом плановом `failed`
  (включая отказ допуска запуска) и сбрасывается на `completed` в момент завершения копии; очистка и
  паника в ней на серию не влияют. Причина видна в `last_error` статуса: у планового прогона это `стадия:
  причина` из словаря выше — `copy-destination-io-*` указывает на права, место или приватность корня,
  `io-*` на доступность каталога данных, `recopy-cap-exceeded` и `copy-recopy-cap` на длительность
  seal-фазы.
- **BackupRetentionFailing:** `increase(shamir_backup_retention_failed_total[1h]) > 0`. Счётчик включают
  срыв подготовки маркера перед публикацией копии и в начале прохода, ошибки метки, аренды удаления и
  удаления, превышение лимитов обхода и паника в проходе очистки. Не включают пропуск снимка, занятого
  читателем (`skipped_in_use`, только debug-запись), отмену прохода и best-effort попытку маркера при
  старте сервера. Сама копия при этом могла быть успешной.

**Если копии перестали появляться**, сначала проверьте доступность цели сбора и `schedule_enabled`, затем
окно, часовой пояс, счётчики пропусков, занятость ручной копией и журнал сервера. При сбоях проверьте
свободное место, приватность и доступность корня; при ошибках очистки дополнительно проверьте привязку
владельца, целостность и лимиты обхода. Пока очистка корня откладывается, копии продолжают накапливаться:
следите за свободным местом и числом снимков — при превышении лимитов обхода последующие проходы могут
отказывать даже после ухода читателей; такой отказ — отдельная ошибка очистки (WARN и рост метрики
ошибок), а не пропуск занятого снимка. Если очередь читателей или упор в лимиты обхода стали регулярными,
отключите новые запуски (приостановите расписание) и дождитесь полного завершения уже допущенного
планового прогона и ухода всех читателей, и только потом перемещайте проверенные снимки наружу по
правилам ниже; тотального удаления каталогов без обследования нет и быть не должно.

Проверка манифеста **управляемого снимка** держит общую блокировку на всём протяжении чтения. Restore
держит её только при проверке и копировании источника; дальше lease освобождается до проверки staging,
replay и подмены каталога. Пока читатель **управляемого снимка** фактически держит lease (общую
блокировку файла-маркера `.shamir-snapshot.lock` в каноническом родителе), retention не удаляет ни одного
кандидата во всём корне. Читатель без lease защиты не имеет: см. ниже про окно безмаркерного перехода.
Занятый снимок — только возможность повторной попытки в более позднем проходе, не обещание, что он
обязательно выполнит удаление. Обычный пропуск не растит метрику ошибок. Зависший или долгий читатель
откладывает ВСЕ удаления снимков в корне; его видно по журналу (debug, поле `skipped`) и по растущему
числу снимков; действие — завершить или снять его. Выносить управляемый снимок наружу ради безопасности
не нужно, пока его читатель фактически держит lease. Резервный файл блокировки (маркер) в корне не
удаляйте и не редактируйте, в том числе пока им пользуется читатель: на Unix удаление удерживаемого
маркера тем же аккаунтом может привести к его пересозданию и блокировке другого inode — это режим вне
документируемого контракта стабильного маркера. Управляемым считается обычный каталог, у которого имя —
метка времени UTC и маркер уже существует в каноническом родителе; читатели (`verify_manifest`, `restore
--from`) маркер никогда не создают. Маркер создаёт только планировщик, и все три момента — best-effort
попытки, а не неизбежность в любом корне: при старте сервера с расписанием на этом корне, перед
публикацией плановой копии (ручные копии маркер не создают) и в начале прохода удаления; корни,
оставшиеся от прежних версий, обслуживает старт самого планировщика. Новый маркер создаётся с правами
Unix `0600`, а на проверенном Windows-корне наследует owner-only ACL корня. Уже существующий **обычный**
маркер только открывается: он никогда не чинится и не переприватизируется; некорректный существующий
маркер — отказ без чтения (fail-closed). Отдельно вынесенный экспорт в немаркированном родителе вне
планового корня читается без блокировок, даже если в нём осталась метка владельца, и retention его не
затрагивает; поэтому при выносе планового снимка не переносите маркер в новый родитель. И наоборот: не
копируйте каталог с меткой времени UTC как простой каталог в родителя, где маркер уже лежит, — такой
каталог будет признан управляемым. Произвольное копирование (`cp`) с очисткой не согласовано: если всё же
выносите снимок вручную, предотвратите новые запуски и дождитесь полного завершения уже допущенного
планового прогона — копирования и его очистки. Для доставки в объектное хранилище используйте свои
инструменты с учётом этого правила; обратно в управляемый корень ничего не записывайте и не трогайте
маркер, пока идут чтение или очистка.

**Окно безмаркерного перехода (задокументированное ограничение by design).** Если маркер в корне
отсутствует и вы включаете расписание на существующем корне, мигрируете его или восстанавливаете маркер
(в том числе перед автоматическим созданием маркера при старте сервера): завершите или отложите начатые
безмаркерные чтения этого корня до первой загрузки после обновления или восстановления маркера и не
начинайте новые ручные `verify_manifest` и `restore --from`, пока маркер снова не лежит в каноническом
родителе и не прошёл первый успешный плановый проход. Чтение, начатое на корне без маркера,
классифицируется как чтение экспорта и идёт без блокировок на всём своём протяжении; само по себе
отсутствие маркера защиты не даёт. Если маркер появится во время такого чтения, читаемая копия может
стать кандидатом на удаление; читатель маркер не создаёт и не чинит. После первого успешного планового
прохода гарантия действует для читателей, начавших позже и фактически держащих lease, пока маркер
остаётся в корне.

### Шифрование копий своими средствами

Снимок — непрозрачный каталог с `manifest.json`. Шифруйте/расшифровывайте
**весь каталог** средствами вне сервера (например, `age`/`gpg` по
tar-архиву каталога); никогда не редактируйте файлы внутри снимка и
расшифровывайте каталог до `verify`/`restore`. Встроенное шифрование копий
(R2c) отменено; за шифрование отвечает оператор.

### Границы

- **Point-in-time recovery нет:** восстановление возможно только к точке
  консистентности снимка.
- Внешние cron / systemd timer остаются вариантом. Online-копию можно
  запускать внешним планировщиком без остановки сервера; остановка
  нужна только offline-CLI-команде `backup --to`. Пример обёртки для systemd: один `.service` с
  `ExecStartPre=systemctl stop shamir-server`,
  `ExecStart=shamir-server ... backup --to ...`,
  `ExecStopPost=systemctl start shamir-server`; либо wrapper-скрипт
  `stop → backup → restart`. Наивный cron, запускающий `backup --to` против
  ещё живого сервера, получит отказ — так и задумано.

**Restore.** `shamir-server --config db.ktav restore --from <to>/<YYYYMMDD_HHMMSS>`
— только при остановленном сервере (проверяется блокировка `data_dir`;
`--force` её пропускает). Команда сверяет `manifest.json` (sha256 каждого
файла), атомарно подменяет `data_dir` (прежний сохраняется как
`.pre_restore_backup_<timestamp>`) и инвалидирует все resumption-тикеты.
Счётчики lockout не сбрасываются. Не восстанавливай простым `cp -r`: так
пропускаются проверка манифеста и инвалидация тикетов. Известное ограничение
копирования: на регистронезависимой файловой системе Windows два имени
источника, различающиеся только регистром, при восстановлении сводятся к
одному staged-файлу.

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

### Штамп формата и обновление версии (format epoch)

В корне `data_dir` сервер хранит `format.stamp`: эпоху формата данных, сведения о создателе штампа и версию последнего открывавшего каталог бинарника. Штамп создаётся автоматически при первом старте; пустой каталог или каталог без штампа может быть «усыновлён» базовой эпохой с WARN в логе. До открытия хранилищ сервер сверяет эпоху: несовместимая версия не откроет данные молча.

**Процедура обновления версии.** Сервер должен оставаться остановленным до завершения format migration:

1. Останови сервер и дождись завершения процесса.
2. До любого вызова `format-migrate` сними backup:
   `shamir-server --config db.ktav backup --to /backups`.
   Команда создаст каталог снимка с фактической UTC-меткой, например
   `/backups/20261116_143000/`. Используй именно выведенный путь-корень
   снимка (каталог, содержащий `manifest.json`), а не `/backups` и не
   предполагаемое время. Backup содержит штамп; `cache/` в него не входит.
3. Установи новый бинарник, но пока не запускай сервер.
4. Необязательный просмотр плана:
   `shamir-server --config db.ktav format-migrate --from-backup /backups/YYYYMMDD_HHMMSS --plan`.
   Без `--yes` команда по умолчанию также только печатает план. `--plan`
   остаётся планом и при наличии `--yes`: он не берёт блокировку, не создаёт
   и не меняет файлы и их mtime, в том числе из-за логирования. Он работает
   с отдельной offline-копией в родителе без маркера; блокировок и записей
   план не выполняет. Обычный каталог с меткой времени UTC под родителем
   с маркером отвергается независимо от метки владельца; помеченный экспорт
   в родителе без маркера принимается — но «принимается» здесь означает
   только прохождение классификации lease (читается без блокировок), а НЕ
   прохождение backup gate: свежесть, копия штампа и покрытие имён могут
   отказать такой копии отдельно. План показывает
   цепочку, состояние журнала, результат проверки backup и оставшиеся отказы.
   «Результат проверки backup» при отказе — это напечатанный вердикт отказа.
   Коды выхода плана различаются: некоторые отказы плана возвращают
   ненулевой код, а напечатанный `backup gate: FAIL` может сопровождаться
   нулевым кодом выхода — читайте вердикт в тексте плана, а не только код
   возврата.
5. Выполни format migration с тем же фактическим путём снимка:
   `shamir-server --config db.ktav format-migrate --from-backup /backups/YYYYMMDD_HHMMSS --yes`.
   По умолчанию backup должен быть не старше 24 часов; окно можно изменить
   через `--max-backup-age-hours N`. `--yes` подтверждает выполнение и нужен
   также для старого adopted-штампа или данных без штампа.
6. Только после успешного завершения, пока сервер всё ещё остановлен, выполни
   `shamir-server --config db.ktav doctor` для отдельной диагностики; затем
   запусти сервер. Не запускай сервер, если format migration завершилась
   отказом или прервалась.

**Ручное восстановление повреждённого штампа.** Удаляй `format.stamp` и
`format.stamp.tmp` ТОЛЬКО после сверки данных с backup: каталог после этого
считается данным базовой эпохи и будет «усыновлён» при следующем старте.

**Пре-release данные** (до первого релиза с штампами) перемещаются только
логическим экспортом/импортом — обновление таких каталогов на месте не
поддерживается.

**Что проверяет backup gate.** По умолчанию `--from-backup` должен указывать на
снимок с корректным `manifest.json` и проверяемыми контрольными суммами. Его
копия `format.stamp` должна совпадать с исходным каталогом по эпохе и
`created_by`, а имена верхнего уровня снимка должны покрывать имена живого
каталога. Время `created_at` не должно быть в будущем и должно попадать в
окно `--max-backup-age-hours` (по умолчанию 24 часа). Пустой снимок и снимок
без manifest-backed пользовательских данных отвергаются; строгая проверка
покрытия может также отвергнуть пустой каталог, отсутствующий в manifest.
Для покрытия не учитываются только `cache/`, `.shamir.lock` и
`format_migration.journal`.

`--i-have-a-backup` явно обходит проверку манифеста, идентичности и свежести
и сопровождается предупреждением; используй его только если сам независимо
проверил пригодный снимок до format migration. Для данных без штампа проверка
по копии штампа невозможна, поэтому stampless base требует такого явного
override (и `--yes` для выполнения). Старый `adopted`-штамп тоже требует
`--yes` и сопровождается громким предупреждением. Проверка идентичности —
не математическое доказательство: другой каталог той же эпохи с тем же
`created_by` и теми же именами верхнего уровня может пройти gate. Это
ограничение Q9; привязка по UUID каталога отложена.

**Отказы и их смысл.** Старшая эпоха, новый layout, повреждённый штамп или
оборванный временный штамп остаются отказами: `format-migrate` их не исправляет;
восстанови проверенный backup либо используй соответствующую версию для
ручного восстановления. Более старая эпоха, для которой есть полная
поддерживаемая цепочка, — штатный случай для этой команды. Данные без штампа трактуются как базовая эпоха и
требуют явного подтверждения. Если эпоха уже текущая, команда сообщает
«nothing to do» и завершается с кодом 0. Если команда не может взять
`.shamir.lock`, сервер или другая обслуживающая команда ещё работает:
останови её и повтори запуск. Не обходи эту проверку.

**Сбой и продолжение.** Пока не завершены все шаги и проверка целостности,
штамп остаётся старым, поэтому обычный запуск сервера продолжит отказывать.
После сбоя оставь `format_migration.journal` на месте и повтори команду тем же
бинарником и той же цепочкой модулей; исходный backup до миграции можно снова
передать через `--from-backup`, только пока он укладывается в окно свежести.
Журнал позволяет продолжить с нужного шага, включая повтор незавершённого
шага. Не снимай backup частично преобразованного каталога, не удаляй и не
редактируй журнал. Повреждённая запись в середине журнала, чужая цепочка или
другая версия бинарника не возобновляются: восстанови исходный backup до
миграции. Штамп продвигается только после последнего шага и заключительной
проверки целостности.

Проверка целостности перед записью нового штампа ограничена существующим
read-only `TablesRegistry::open`: она декодирует `wire_tables.mpack`; отсутствие
или пустое содержимое реестра допускается. Это не проверка здоровья таблиц,
индексов или WAL и не полный integrity scan. Переиспользовать `doctor` до
обновления штампа нельзя: старую эпоху блокирует format gate, а сам `doctor`
открывает engine. Поэтому после успешной миграции отдельно запускается
остановленный `doctor`; не считай проверку перед штампом эквивалентом `doctor`.

**Откат — только восстановление.** In-place отката формата нет. Останови
сервер, восстанови снимок, снятый до начала миграции, например
`shamir-server --config db.ktav restore --from /backups/YYYYMMDD_HHMMSS`,
затем используй старый бинарник, соответствующий восстановленной эпохе.
Restore переносит штамп вместе со снимком; восстановленный старый формат при
необходимости снова потребует явной format migration.

Если обновление на месте невозможно, запасной путь R3 — логический export
старым бинарником и import новым; см. раздел «Logical dump: offline export
and import» ниже.

**Windows:** гарантии durability штампа опираются на fsync; на Windows
directory-flush отсутствует — при потере питания возможна потеря самой
последней записи штампа (каталог тогда трактуется как без штампа, что
безопасно).

**Backup/restore и штамп.** `backup` копирует `format.stamp` байт-в-байт
(он входит в манифест с SHA-256), `restore` переносит его вместе со снимком
и сам штамп не пишет. `cache/` — производные данные: он исключён из штампа
и snapshot.

**Референсный тест совместимости.** В репозитории лежит коммит-фикстура
`reference_epoch_1` — каталог данных, записанный реальным релизным бинарником
эпохи 1 (генератор `scripts/runners/make_reference_datadir.py`, CI-workflow
`reference-datadir.yml`). Интеграционный тест `shamir-server` при каждом прогоне
CI копирует эту фикстуру, проверяет штамп, запускает `format-migrate`
(сегодня — «nothing to do» с байт-в-байт неизменным содержимым) и открывает
копию свежим сервером, сравнивая содержимое семантически: строки и схемы
таблиц, метаданные индексов каждого вида, функции (включая `call_signature`),
принципалы/права, восстановление WAL. Когда генератор меняется, CI дополнительно
генерирует каталог заново и сверяет свежий прогон с закоммиченной фикстурой.
Тем самым путь «старая эпоха → текущая» всегда остаётся исполнимым и
проверенным — ещё до первого реального изменения формата.

## Logical dump: offline export and import

Логический дамп — это переносимый interchange-артефакт, а не физический backup:
он хранит логическую схему и выбранное состояние одной базы в каноническом
named-MessagePack стриме с контрольными суммами. Это плоскость для переноса
между машинами и для fallback «старый бинарник export → новый бинарник import»,
когда in-place format migration невозможна. История, удалённые записи,
replication-конфигурация (`profiles`/`publications`/`subscriptions`), токены и
секреты в дамп не попадают никогда; импортированная база получает новую,
независимую lineage.

**Stored shapes, не исходные типы.** Storage codec сохраняет `Dec`/`Big` как
строки и `Set` как список, поэтому дамп фиксирует то, что реально хранится:
сохранённая строка возвращается как `str`, сохранённый список — как `list`
(рекурсивно внутри вложенных map/list). Round trip байт-в-байт воспроизводит
сохранённые байты; `Dec`/`Big` приходят обратно как `str`, `Set` — как `list`.
Импорт принимает все теги value envelope (`dec`, `big`, `set` включительно),
поэтому будущий экспортёр с явной типизацией не потребует изменений импорта.

**TTL.** Per-table `buffer.ttl_ms` — это настройка memory buffer'а, а не
срок жизни записи: длительность переносится verbatim в конфигурации таблицы,
содержимое buffer'а пересоздаётся при импорте. Per-record expiry в системе нет,
сроки не переводятся и записи как «истёкшие» не отбрасываются.

### logical-export

Offline-команда: сервер не запускается, `data_dir` берётся из `--config` и
открывается с эксклюзивной блокировкой (recovery завершается до freeze).

```bash
shamir-server --config db.ktav logical-export --db app --output /backup/app.dump
# сужение области видимости:
shamir-server --config db.ktav logical-export --db app --repo archive --output /backup/archive.dump
shamir-server --config db.ktav logical-export --db app --repo archive --table events --output /backup/events.dump
# только определения (без записей):
shamir-server --config db.ktav logical-export --db app --schema-only --output /backup/app-schema.dump
```

| Flag | Meaning |
|---|---|
| `--db <name>` | Обязательный. Имя базы; внутренняя `__system__` запрещена. |
| `--output <path>` | Обязательный. Путь артефакта; этапные файлы пишутся рядом и публикуются атомарно после проверки. |
| `--repo <name>` | Только указанный repository (по умолчанию — все). |
| `--table <name>` | Требует `--repo`. Только указанная таблица. |
| `--schema-only` | Опустить секцию записей; все выбранные определения и все консервативные ограничения всё равно проверяются. |
| `--include-artifacts` | Зарезервировано; всегда явный отказ («guest functions/modules are not implemented»), никогда не принимается молча. |

Экспорт выполняется под maintenance-freeze: после холодной инициализации и
проверки каталога/движка захватывается snapshot-читатель, страницы пишутся через
ограниченный внешний merge sort (raw key bytes упорядочиваются на диске, не в
памяти), затем файл sync'ается, закрывается, полностью верифицируется и только
потом атомарно публикуется (same-volume no-replace commit). Незавершённые
этапные файлы при ошибке/отмене удаляются. Если после успешной публикации
Unix directory fsync упал с ошибкой, команда сообщает «published a verified
artifact … final file retained» — это durability-ошибка с готовым файлом, не
неизданная попытка: не удаляй и не blindly повторяй.

### logical-import

Импортирует проверенный дамп в **новый** серверный data directory: цель должна
отсутствовать или быть пустой; слияние и перезапись невозможны.

```bash
shamir-server --config new.ktav logical-import --input /backup/app.dump --target /data/restored
# переименовать базу во всех ссылках:
shamir-server --config new.ktav logical-import --input /backup/app.dump --target /data/restored --as-db app2
# требовать schema-only артефакт (зеркалит экспортный --schema-only):
shamir-server --config new.ktav logical-import --input /backup/app-schema.dump --target /data/restored --schema-only
```

| Flag | Meaning |
|---|---|
| `--input <path>` | Обязательный. Путь артефакта. |
| `--target <dir>` | Обязательный. Новый data directory: отсутствующий или пустой. |
| `--schema-only` | Принимать (и только) schema-only артефакт; несовпадение scope — отказ. |
| `--as-db <name>` | Переименовать импортируемую базу во всех ссылках: имя `DatabaseItem`, scope записей, ACL resource refs и строки владения в каталоге. |

Отмена — через процессный сигнал (Ctrl-C), отдельного флага нет: операция
join'ится, а не отсоединяется; до публикации этап отбрасывается, после
успешной публикации сообщается успех. Прогресс — на stderr, итог — на stdout.

### Security: идентичности и ACL

В дамп попадают только логические имена владельцев и их kind, никогда —
учётные данные: salt/stored/server key/KDF-параметры, токены, ticket-счётчики,
roles, superuser/replicator-состояние и user ID не экспортируются. Импортированные
identity создаются **disabled и passwordless** (`insert_disabled_import`):
авторизация для них закрыта, как для неизвестного пользователя, но имя
резолвится для маппинга владения. Права никому не выдаются неявно: ни grants,
ни administrator-статус. Системное владение остаётся null — требует явного
решения оператора, а не молчаливой привилегии. ACL-владелец, которого нет среди
экспортированных identity, — типизированный отказ, никогда молчаливый `System`
grant. Уже существующие chown/chgrp работают с импортированными владельцами как
обычно.

### Что отказывается сегодня и почему

Fail-closed: неподдерживаемое содержимое вызывает отказ экспорта/импорта,
никогда не пропускается молча и не превращается в «пустой успех».

* **Индексы** всех семейств (regular/unique, sorted, full-text, functional,
  vector) — дескрипторы не реализованы; данные/определения не теряются молча.
* **Схемы таблиц** (stored schemas, schema validators, transforms, FK-биндинги) —
  некорневые `schema_version`/`schema` не переносятся.
* **Функции, validators, function folders** — любая строка этих семейств в
  глобальном каталоге вызывает отказ даже если выбранная база на них не
  ссылается: анализ замыкания ссылок — будущая работа.
* **Группы** — group identities, membership, group ACL и расширенные grants —
  будущая работа; group-метаданные в repository/table/ACL-элементах — отказ.
* **Guest modules** — `--include-artifacts` всегда отказ; артефакт с
  guest-artifact scope на импорте отклоняется.
* Также отказываются: in-memory/hybrid repositories, non-default durability
  overrides, non-default buffer sizing/flusher (кроме `ttl_ms`), quarantined
  (`resync_required`) и rename-in-doubt repositories, application-schema
  version в заголовке артефакта, required features, unknown поля каталога.

### Ограничения и бюджеты

Это бюджеты (format limits), а не измеренные результаты — больших датасетов
acceptance не прогонялся. Фрейм ≤ 16 MiB, агрегированные метаданные ≤ 64 MiB,
артефакт ≤ 1 TiB (файловый preflight отвергает превышение до чтения), глубина
вложенности 64, до 65 536 элементов на контейнер (один `List`/`Set`/`Map` или
инвентарь секции). Дополнительные scope-лимиты: 8 MiB на кодирование выбранного
определения, до 4096 ACL и identity, до 1024 байт на логическое имя,
2048 узлов / 512 KiB на запись (ключ/бинарные байты), 1 MiB на сортировочную
запись. Целевые бюджеты CI (не measurements): 64 MiB датасет — export/import/verify
≤ 60 s, index build/validation ≤ 180 s; 1 GiB — ≤ 600 s; incremental RSS
≤ 256 MiB над baseline открытой базы.

### Lock, stage и восстановление после сбоя (import)

Всё держится **рядом с целью**, не внутри неё — публикация это один same-volume
rename на отсутствующее имя:

* `<target>.import.lock` — эксклюзивный create-new lock, берётся **до** осмотра
  цели и держится весь запуск. Существующий lock (конкурентный или от
  сбойнувшей попытки) — типизированный отказ. Снимай его только убедившись,
  что процесс-владелец мёртв; не обходи проверку.
* `<parent>/.logical-import-<unique>/` — этапный каталог. Устаревшие этапы
  сбойнувших попыток удаляются только под удерживаемым lock, перед созданием
  нового этапа.

После сбоя/отмены до публикации этап удаляется целиком; записи не
резюмируются — повтор всегда начинает свежий этап. Загрузка, flush всех
buffer'ов (включая TTL), закрытие движка, повторное открытие и валидация из
свежего open'а, запись receipt'а и только затем — rename. Переименование на
Windows ограниченно повторяется для transient sharing-ошибок (straggler
handles), при этом появившаяся между попытками цель немедленно отменяет retry.

### Idempotent retry и receipts

Успешный импорт кладёт в опубликованный каталог
`.logical-import-receipt.json`: SHA-256 всего артефакта, имя исходной базы,
эффективное имя (после `--as-db`), `schema_only`, format epoch, generator.
Повторный запуск против уже импортированной цели принимается идемпотентно
**только** при совпадении digest и всех effective options **и** при
прохождении integrity check (читаемый format stamp, движок переоткрывается,
содержимое по-прежнему равно артефакту). Любое несовпадение — типизированный
отказ; перезаписи нет. Цель без receipt'а или с повреждённым receipt'ом —
отказ, а не «переимпорт». Обычная цель для повторной попытки — crash/power-loss
после публикации, но до того, как успех был сообщён оператору.

### Verify-only workflow

Отдельной CLI-подкоманды верификации логического дампа сегодня нет — верификатор
живёт в библиотеке: `shamir_sync::replication_seed::verify_dump_file(path)`
проверяет magic, версию, required features, каноничность кодирования, лимиты,
контрольные суммы фреймов/секций, счётчики, trailer prefix digest и EOF **без
открытия базы**. Экспорт всегда верифицирует артефакт до публикации; импорт
прогоняет полный verify pass в preflight до любого изменения диска. Для
операторской проверки артефакта используй это API (или дождись CLI-обёртки):
коррупция, усечение, trailing bytes, несовпадение инвентаря — отказ до
какой-либо публикации.

### Пример запроса (builder, не wire-JSON)

Проверка результата после импорта — обычным клиентом и query builder'ом
(builder-примеры не являются спецификацией wire-протокола; никакого ручного
serde_json):

```ts
const repos = ddl.listRepos('app2');           // после --as-db
const tables = ddl.listTables('app2', 'archive');
const described = ddl.describeRepo('app2.archive');
```

## 9. Capacity planning

Оценки ниже выведены из структур в коде, а не измерены — сверяй с `/metrics` своей инсталляции (источник: `docs/dev-artifacts/ops/CAPACITY_PLANNING.md`, частично устарел). Кратко:

|| Подсистема | Память |
||---|---|
|| Idle процесс | ~50 MB |
|| Активная сессия | ~2 KB |
|| Активное соединение | ~8 KB |
|| Argon2id KDF working set (гейт создания пользователей, сервер) | `argon2_concurrent_max × kdf_defaults.memory_kb × 1.05` — оценка Argon2 matrix working memory в KiB. Пароли и task state ожидающих допущенных задач, стеки потоков-ожидателей, память процесса, bootstrap и bootstrap-token rotation, outbound SCRAM фолловера и SQL `argon2id` учитываются отдельно. |
|| Steady-state | `50 MB + sessions × 2 KB + connections × 8 KB` |

10 000 сессий → ~134 MB.

|| Операция | CPU |
||---|---|
|| Argon2id KDF (сервер, только создание пользователей) | Зависит от `kdf_defaults` и оборудования |
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
* `increase(shamir_visibility_ack_total{result="pending_timeout"}[5m])` > 0 →
  commits are answering Pending (bounded mode): a writer version is unsettled —
  correlate with `shamir_gate_oldest_blocker_*` before restarting anything.
* `increase(shamir_visibility_pending_total{reason="reopen_required"}[5m])` > 0 →
  the repository needs reopen/recovery attention, not client retries.
* `increase(shamir_visibility_read_wait_timeouts_total[5m])` > 0 → AsOf reads
  are waiting past `read_visibility_timeout_ms`: check the blocking writer.
* `shamir_visibility_ack_waiters` near the connection limit → acknowledgement
  parking is consuming connection slots; switch the service to
  `visibility_ack_mode = "bounded"` (connection-resource liveness).
* `increase(shamir_visibility_reconciliation_total{result="pending_timeout"}[15m])`
  high → callers reconcile repeatedly against a stalled version; investigate
  `shamir_gate_unsettled_versions` / orphan-replay counters.
* Expiry of any of these waits also emits a rate-limited WARN log (at most one
  per repository per 5 s) with `target`, `visible_through`, `blocking_version`
  and `elapsed_ms` — alert on the metric, read the log for the position.

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
  cold reconciliation восстанавливает выводимые v3 journal events из
  сохранившегося WAL; совпадающие события остаются без изменений,
  несовпадающие bytes и любые I/O/decode ошибки запрещают open. v3 entries
  с event header, из которых не выводится событие, gap не создают. Для
  неизвестных legacy entries или версий с отсутствующими evidence может
  остаться минимальный консервативный gap; уже существующие gap не
  очищаются. Journal и recovery floor flush'ятся до продвижения J к H, а J
  сохраняется до serving. Поэтому сам unclean shutdown не означает
  replication gap или reseed; OS reclaim'ает сокеты.
  «Graceful shutdown — bounded» не следует понимать как исчерпывающее доказательство гонок остановки: конкретное поведенческое тестовое покрытие не доказывает все варианты гонок с уже выполняющимися задачами или все случаи сбоя обнаружения освобождения ресурсов.
* **Single-instance guard.** Файловая блокировка (`fs4`) на `data_dir`.
  Два процесса на одних данных → второй не стартует.
* **Logging — non-blocking.** Никогда не блокирует горячий путь. Overflow
  — lossy (лучше потерять лог, чем запрос).

## Application schema migrations (Rust client)

Schema changes ship as an **ordered, checksummed migration set compiled into
your application**, applied through `shamir_client::schema_migration`. There
is **no down or rollback API** and **no automatic retry**: after any uncertain
apply you ask the server what happened instead of resending. Bodies are built
**only** from the typed DDL builders (`shamir-query-builder`) — hand-written
wire JSON has no canonical form and no checksum the server can verify.
Version 1 admits **DDL only — no DML**.

```rust
use shamir_client::builder::{ddl, SchemaMigration};
use shamir_client::schema_migration::{
    ApplicationMigrationSet, MigrationError, PrefixCoverage, SchemaMigrationTarget,
    UnknownApplyResolution,
};

let m1 = SchemaMigration::new("add-audit-table", 1, "add audit table")
    .op(ddl::create_table("audit").repo("app").build())? // build() yields a typed BatchOp
    .build()?; // validates against the admitted DDL subset
let m2 = SchemaMigration::new("add-audit-archive", 2, "add audit archive")
    .op(ddl::create_table("audit_archive").repo("app").if_not_exists().build())?
    .build()?;
let set = ApplicationMigrationSet::new(vec![m1, m2])?;
```

`ApplicationMigrationSet::new` validates the set **before anything can be
sent**: ordinals contiguous from exactly 1, unique ids, typed errors. A gap or
malformed body is a **local** rejection — zero requests. Each body has
versioned canonical bytes and a SHA-256 checksum (cached by the set); the
server **recomputes** the checksum from the submitted typed body, so an
authored raw-JSON body or a client-reported checksum is never proof. A body
is immutable once applied: changing it changes its checksum — drift, refused
before any apply. Correct a released migration with a new forward migration,
never by editing the old body.

#### Functional indexes in migrations

`create_index` migrations admit functional indexes only with
`index_type = "functional"` and a `functional_op` of exactly
`lower | upper | trim | length` — the built-ins the server's index backend
constructs directly (`write_domain_index_admitted.rs`). Functional options on
any other (or omitted) index type, a functional index without an explicit op,
a name outside the allowlist, and ANY supplied `functional_args` — including
`[]` — are refused locally, before the journal intent is written. The typed
refusal is `SchemaMigrationError::UnsafeOperation` from the operation builder;
when the op goes through a migration-set builder, that error surfaces wrapped
as `MigrationError::InvalidSet(SchemaMigrationError::UnsafeOperation(…))`. `functional_args`
are refused because the server builds the index expression from the indexed
field and the op alone and never reads them. Note the index semantics:
`length` counts UTF-8 bytes of a string (element count of a list), not
Unicode characters.

### Target identity and permissions

`SchemaMigrationTarget { database_identity }` carries the **known stable
database identity**, supplied separately from the connection name;
`MigrationStatus::reported_target` echoes the serving identity on
`schema_migration_identity_mismatch`, which is the supported discovery path
for a wrong identity. Permissions (design §5.2): all three operations need
`Read` on the database — the preflight read before a write — while `plan` and
`apply` additionally need `Manage`, and apply still passes every underlying
operation's normal DDL authorization (that server-side `Manage` separation is
the server's, not the client's). The journal, fence generation and stored
bodies live in the server's durable system catalogue and survive restart;
`status` and `plan` perform **no migration journal or DDL changes** — they
are read-only with respect to migrations (the server's own legacy identity
backfill/persistence tail may run, but it never migrates anything on their
behalf).

### `status`, `plan`, `apply_next`

```rust
let target = SchemaMigrationTarget { database_identity };
client.status(db, target, &set).await?;      // by-value target, borrowed set
client.plan(db, target, &set).await?;        // read-only with respect to migrations
client.apply_next(db, target, &set).await?;  // applies exactly one next ordinal
```

`status` compares the **entire ordered applied prefix** against your local
set: the server's status wire reports every applied migration in ordinal
order (`applied`: `id` / `ordinal` / `checksum` per entry) plus the durable
`fence_generation`, so a client can prove its full local prefix, not just
the head. `plan` verifies the submitted set on the server against the same
journal.

`apply_next` itself reads `status` first and refuses **before sending the
APPLY** when the journal drifts from your set, when your set is shorter
than the applied journal, when the applied ordinals have a gap, when the
prefix is unverified, or when nothing is left to apply. The only zero
network round trip is a locally invalid set construction — every journal
refusal costs one read-only `status`. Otherwise it submits exactly **one**
next ordinal with the expected predecessor id and the fence generation taken
**directly from `status`'s `fence_generation` field** — never derived or
guessed. Concretely: on an empty journal the status reports
`fence_generation: 0`, the apply acquires generation **1**, and after the
fence is released the next status reports `fence_generation: 2` — the
expected generation of the next fresh apply. The generation an apply
acquired is not reused as the next expectation — the next expectation is
always the fresh `fence_generation` of the following status, a distinct
number read from the wire, not inferred from ordinals. When the set is
already fully applied, a repeat call applies nothing and returns
`NothingToApply`.

### Lost response: resolve, never resend

A transport error or timeout **after** an apply was sent returns
`MigrationError::UnknownOutcome { attempt, .. }` — the client never resends
on its own. The opaque `UnknownApply` token binds the database, the target
identity and the exact id / ordinal / canonical checksum of the in-flight
body; the resolver correlates the returned status against your set locally —
it does not ask the server "by id". Reconnect, then resolve through the
targeted status helper:

```rust
// `reconnected_client: &Client` — an already connected client to the SAME
// server and database as the original apply.
match client.apply_next(db, target, &set).await {
    Err(MigrationError::UnknownOutcome { attempt, .. }) => {
        // one read-only status query; the apply is never resent
        let resolution = reconnected_client
            .resolve_unknown_apply(&attempt, &set)
            .await?;
        match resolution {
            // journal proves the attempt applied with the same
            // id/ordinal/checksum — anywhere in the verified reported
            // `applied` list, even when a LATER migration already applied:
            // success, never resend
            UnknownApplyResolution::Applied { .. } => {}
            // attempt IS the unresolved head: operator reconciliation or
            // the identical-body resume below; the server's reason is in
            // `reason`
            UnknownApplyResolution::InDoubt { .. } => {}
            // absence AND the predecessor are both established: safe to
            // call `apply_next` again
            UnknownApplyResolution::ReadyToApply => {}
            // head overtook the attempt without proving it, or an
            // unresolved event for a DIFFERENT migration: no false
            // success; reconcile out of band
            UnknownApplyResolution::EvidenceUnavailable { .. }
            | UnknownApplyResolution::UnmatchedUnresolved { .. } => {}
        }
    }
    Err(MigrationError::UnsupportedServer { .. }) => { /* upgrade the server */ }
    other => { /* §5.3 stable codes mapped onto MigrationError, message kept */ }
}
```

The server's stable codes (`schema_migration_in_progress`, `_in_doubt`,
`_checksum_mismatch`, `_gap`, `_predecessor_mismatch`, `_follower_read_only`,
`_durability_unavailable`, `_postcondition_failed`, `_identity_mismatch`,
`schema_migration_operation_deferred:`) map onto typed variants without
losing the server message.

`resolve_unknown_apply` checks the attempted id / ordinal / canonical
checksum against the **whole verified reported `applied` list**, not only
the head: the proof survives even when a later migration has already
applied past the attempt. The safe-to-reapply verdict (`ReadyToApply`) is
issued **only** when both the absence of the attempt and its predecessor
are established — a proven, drift-free prefix at the attempt's predecessor,
no unresolved event, and the attempt's ordinal being the set's next local
ordinal. All correlation happens locally against your compiled set; the
apply is never replayed on the strength of a status answer alone.

**Resume after in-doubt** uses the *same* `apply_next`: the identical stored
body, expected predecessor, and the fence generation reported for the
unresolved migration in `status` (the unresolved entry's own generation) —
the server refuses to take over a still-running owner, then acquires a new
generation when it fences the old owner and reconciles. The server
re-checks the checksum of the submitted body and concurrent contention.
An in-doubt diagnostic alone does not prove the outcome, and the client
cannot confirm an in-flight local attempt from its own side — only the
server's answer can. Do **not** assume a refused apply has succeeded. Recovery is an operator decision after reading the diagnostics,
or a restore from backup — never a body edit or hand-run DDL, which would
break checksum identity and make every later apply refuse.

### Status wire and old peers

The public status result carries two additive fields (shared contract
`crates/query/shamir-query-types/src/admin/types/schema_migration/protocol/status.rs`):

* `fence_generation: u64` — the highest durable fence generation of the
  journal (`0` when the journal holds no event): the
  `expected_fence_generation` of the next fresh apply.
* `applied: Vec<SchemaMigrationAppliedEntry>` — every applied migration in
  ordinal order, each with `id`, `ordinal` and the 32-byte canonical
  `checksum`.

The existing fields (`last_applied_id`/`_ordinal`/`_checksum`,
`checksum_validation`, the unresolved `in_flight` entry with its
`in_doubt_reason` evidence) are unchanged; a typed wire error surface stays
separate from the public `MigrationStatus::server_error`.
On the client, `PrefixCoverage` now reports full-prefix coverage:
`PrefixCoverage::Verified` when every reported entry matches your set by
ordered id / ordinal / checksum, and `PrefixCoverage::Drift` when any entry
differs; the existing variants for an empty journal and for a set shorter
than the applied journal remain, with new coverage for an unverifiable
ordinal (`Unverified { ordinal }`) and a reported gap
(`Gap { expected, actual }`).

A peer old enough to answer `status` but omit `applied` and
`fence_generation` while the journal head is non-empty is not silently
treated as an empty journal: the client returns
`MigrationError::UnsupportedServer` — upgrade the server. There is no
guessing of fence generations, no per-id server lookup, and **no fallback
to unjournaled DDL on any path**.

```rust
// Sequential applies: each call reads status again before applying.
let s0 = client.status(db, target, &set).await?;
assert_eq!(s0.coverage, PrefixCoverage::Empty);        // empty journal, fence 0
client.apply_next(db, target, &set).await?;            // acquires generation 1, releases it
let s1 = client.status(db, target, &set).await?;
assert_eq!(s1.coverage, PrefixCoverage::Verified { ordinal: 1 });
assert_eq!(s1.fence_generation, 2);                    // next expected
client.apply_next(db, target, &set).await?;            // acquires generation 3
// Fully applied: a repeat call applies nothing and returns NothingToApply.
```

### Порядок эксплуатации: backup → status → plan → apply

Перед применением любой миграции снимите резервную копию (см. раздел
«Restore до/вместо обновления схемы» ниже) — это единственный путь назад: у миграций **нет**
down/rollback. Restore выполняйте только из **верифицированного полного**
snapshot (манифест проверен, копирование и seal завершены); усечённый или
непроверенный снимок для отката миграции непригоден. Дальше строгий порядок:

```rust
let s = client.status(db, target, &set).await?;    // только чтение: сверка префикса и fence_generation
let plan = client.plan(db, target, &set).await?;   // только чтение: серверная сверка plan до apply
// Осмотрите s (и план) ПЕРЕД apply: проверенный префикс, отсутствие in-flight,
// следующий ординал. Ничего не применяйте вслепую.
if s.journal_checksum_verified() && s.in_flight.is_none() && s.next_local_ordinal.is_some() {
    client.apply_next(db, target, &set).await?;    // ровно один следующий ординал
}
```

`status` и `plan` не пишут ни журнал миграций, ни DDL; `apply_next` сам
читает `status` и берёт `fence_generation` прямо из его ответа — не
выводите поколение сами и не кэшируйте его между запусками.

### Maintenance fence: что блокируется и что видит писатель

Для обычных записей, DDL и вызовов хранимых процедур (`CallOp` —
гостевой код может писать через прямые интринсики) стабильный код зависит
от состояния **целевой базы**: `reserved` (резерв до долговременного
захвата) и `live` (раннер держит захваченное поколение) дают
`schema_migration_in_progress`, маркер `in_doubt` —
`schema_migration_in_doubt`. Для операций над обслуживаемой базой проверка
на входе обходит всё дерево батча, включая вложенные `Batch`/`ForEach`;
если она находит запрещённую операцию, батч отклоняется целиком.
Лист с явно указанной целью, совпадающей с обслуживающей базой (в том
числе `RENAME`/`DROP` её самой или `CHMOD`/`CHOWN`/`CHGRP` её ресурсов),
классифицируется как операция над обслуживающей базой: батч отклоняется
на входе, до первого эффекта, с кодом текущего состояния обслуживающей
базы (`schema_migration_in_progress` или `schema_migration_in_doubt`) и
псевдонимом верхнеуровневой операции: самой операции, если она
верхнеуровневая, а для операции внутри вложенных `Batch`/`ForEach` —
псевдоним той верхнеуровневой операции, в которую она вложена; эффекты
более ранних операций не выполняются, и
ничего не остаётся наполовину применённым. Для `RENAME`/`DROP` это
исключение из первого правила: ограждённая цель в `reserved`/`live`,
обслуживаемая другой (свободной) базой, тоже даёт
`schema_migration_in_doubt` — граница кодов жизненного цикла описана
ниже.
Операции только для чтения проходят проверку fence, но не освобождаются
от проверок доступа. `ApplicationSchema` (status/plan/apply) не блокируется
этой проверкой: apply допускается отдельно по durable-журналу, а DDL
самой миграции выполняется под её собственным fence.

Ранний отказ **до резервирования** не создаёт ни нового fence, ни нового
маркера и не меняет существующее состояние. Если apply прерван в
`reserved` до первого опроса (`poll`) операции коммита захвата fence,
новый резерв снимается, а при resume восстанавливается прежний маркер
`in_doubt`. Если изменения журнала уже подготовлены и начался опрос
коммита, тот мог опубликовать изменения даже после отмены запроса.
В таком случае выход из `reserved` оставляет маркер `in_doubt` для
поколения попытки. После перехода в `live` неуспешный или
прерванный раннер оставляет маркер `in_doubt`. При успешном завершении
база освобождается после долговременной записи `FenceReleased`, а не
просто после записи `Applied`.

**При перезапуске сервера.** Маркер хранится только в памяти. При открытии
движок полностью проверяет историю журнала и её управляющую запись;
для загруженной базы с всё ещё занятым durable fence он восстанавливает
маркер `in_doubt`. Сюда входит окно после `Applied`, но до
`FenceReleased`: результат миграции уже записан, но база ещё ограждена.
Перезапуск сам по себе fence не снимает.

**Повреждённый журнал при открытии (старт прерывается целиком).** Если
обычная валидация истории при открытии обнаруживает повреждение
durable-журнала миграций — как отнесённое к конкретной базе (у записи
пригодный record key с identity базы, но содержимое повреждено:
несовпадающие или невалидные метаданные, тип или payload записи,
недостающие события, расхождение последовательности или поколения,
отсутствующая control-запись при непустой истории этой базы), так и
неотносимое ни к одной базе (запись без record key / identity либо с
противоречащими друг другу ключом и identity, или повреждение, из-за
которого нельзя прочитать сам конверт записи), — **открытие всего
ShamirDb/сервера прерывается** с отказом того же типизированного кода
`schema_migration_durability_unavailable`; стабильный код виден
оператору в **диагностике старта**, а не как wire-ответ на запрос.
Повреждённая запись не отбрасывается и не «чинится» на лету.
Восстановление: полный restore каталога данных (`data_dir`: журнал
живёт в системном хранилище, и схема, данные и журнал должны быть
восстановлены согласованно из одной верифицированной копии; системное
хранилище отдельно от репозиториев не восстанавливают) и повторный
запуск; никогда не угадывайте, какая запись «лишняя», и не правьте
каталог вручную — офлайн-хирургии не предусмотрено.

**Изоляция при запросе (рантайм), не при старте.** Если повреждённое
событие/тело или несогласованный раздел журнала маршрутизируется на
одну базу (у конверта записи пригодный ключ с identity), отказ
получают `status`/`plan`/`apply` **только этой базы** — с тем же кодом
`schema_migration_durability_unavailable`; базы, чьи разделы журнала
прочитались без ошибок, продолжают обслуживаться. Конверт без
пригодного ключа маршрутизации не относится ни к одной базе, поэтому
отказать могут `status`/`plan`/`apply` **всех** баз. В рантайме тот же код возвращают
`status`/`plan`/`apply` и при обычных отказах системного хранилища в
момент запроса — нечитаемость, отказ записи или синхронизации,
недоступность раздела, — поэтому сам по себе этот код в рантайме **не**
означает повреждение и **не** является основанием немедленно
восстанавливать backup: сначала диагностируйте и восстановите
доступность хранилища, затем заново прочитайте `status` и только по его
результату решайте следующий шаг. Слепые повторы apply недопустимы;
если durable-намерения (события журнала) уже были записаны до отказа,
незавершённый run может вместо этого отчитаться
`schema_migration_in_doubt` — тогда действуйте по процедуре in-doubt
ниже, а не как при повреждении журнала.

**Жизненный цикл базы.** `RENAME` и `DROP` (включая `CASCADE`)
проверяют базу, которую переименовывают или удаляют. При `reserved`,
`live` или `in_doubt` диспетчеризация жизненного цикла через чужую
(обслуживающую) базу возвращает единый код жизненного цикла
`schema_migration_in_doubt` для всех трёх состояний; если цель совпадает
с обслуживающей базой, отказ приходит раньше — на входе в батч, — с кодом
текущего состояния обслуживающей базы (`schema_migration_in_progress` при
`reserved`/`live`; при маркере это тот же самый
`schema_migration_in_doubt`), как описано выше.
При активном раннере дождитесь его завершения; иначе проверьте `status`
и следуйте процедуре восстановления ниже. Состояние привязано к
стабильному `DatabaseIdentity`, а имя служит для поиска. Переименовать
ограждённую базу нельзя; разрешённое переименование свободной базы
сохраняет её ID.

Резервирование цели для изменения жизненного цикла и резервирование
миграции (в том числе resume) взаимно исключаются. Операция жизненного
цикла может зарезервировать только свободную цель и освобождает свой
резерв при выходе. При `DROP … CASCADE` защита действует до первой
мутации дочерних объектов и на протяжении удаления.

`__system__` — точное зарезервированное имя внутреннего хранилища.
Типизированные `CREATE DATABASE` и `RENAME DATABASE TO __system__`
отклоняются валидацией на общем для всех путей уровне. В старых версиях
пользовательская база с таким именем могла быть создана, но при
перезапуске она не загружалась; если у неё были сохранённые репозитории,
открытие всего ShamirDb могло завершиться ошибкой. Миграции таких баз
нет.

`CREATE DATABASE` резервирует имя, не изменяя существующую базу. Если
имя отличается от имени обслуживающей базы, этот лист батча не
проверяется по её fence и может пройти даже при выставленном fence.
После проверки права `Create` на корне занятое имя даёт `exists` для
обычного `CREATE DATABASE` и успешный no-op (`created: false`,
`existed: true`) для `IF NOT EXISTS`, даже если база с этим именем
ограждена. Если имя совпадает с именем обслуживающей базы, её fence
проверяется на входе в батч: при выставленном fence оба варианта
отклоняются до проверки существования имени.

**Явные цели и права.** Лист с явно указанной целевой базой пропускает проверку fence обслуживающей базы, только если цель отлична от неё; если цель совпадает с обслуживающей базой, лист классифицируется как операция над обслуживающей базой (см. раздел «Maintenance fence» выше). Операции, изменяющие существующую целевую базу, отличную от обслуживающей, проверяют её fence отдельно в момент диспетчеризации. Например, привязка или отвязка валидатора через другую базу не обходит fence целевой базы; fence только на обслуживающей базе сам по себе такую операцию не блокирует. `CHMOD`/`CHOWN`/`CHGRP` также проходят явную проверку целевой базы для
любого пользовательского имени; специальная обработка системного имени
в ACL корректна, поскольку пользовательскую базу `__system__` создать,
переименовать под это имя или опубликовать под ним seed-образ нельзя (отказ — ошибка валидации без отдельного стабильного кода). Это проверка facade, общая для `CREATE DATABASE`, `RENAME DATABASE` и записи seed-образа в каталог; CLI-установщик seed отклоняет такой образ ещё раньше — как неподдерживаемое содержимое (`unsupported seed format or contents`), не доходя до этой проверки. Граница кодов жизненного цикла: `RENAME`/`DROP` ограждённой цели, обслуживаемой другой (свободной) базой, отклоняются проверкой жизненного цикла цели с кодом `schema_migration_in_doubt` даже при `reserved`/`live` цели; если же цель совпадает с обслуживающей базой, отказ приходит раньше — на входе в батч, с кодом состояния обслуживающей базы. Обычная авторизация сохраняется: серверный шлюз требует администратора для bind/unbind, пакет авторизуется в обслуживаемой базе (`Read` на базу и права на пары «действие, ресурс» из дерева запросов), а bind/unbind дополнительно требует `Write` на целевой таблице (`db.repo.table`). Отдельной проверки `Manage`/`Root` на целевой базе для bind/unbind нет. Для `RENAME` требуется `Write` на переименовываемой базе, для `DROP` — `Delete` на удаляемой. Множество баз-держателей привязок валидатора выводится **авторитетно — из сохранённых конвертов привязок таблиц**, а не из обратного индекса каталога. Перед заменой (`replace`), переименованием или удалением сервер опрашивает каждую живую пользовательскую базу и читает для каждой таблицы только конверт привязок валидаторов из `__info__`, непосредственно из существующего backing-хранилища: проверка существования и открытие store выполняются в facade под DDL-lock, отсутствующий store не создаётся, wrapper не гидратируется. Это редкий DDL-путь; он не инициализирует менеджеры таблиц, не читает строки данных или историю и не запускает восстановление индексов. Bind/unbind синхронно сбрасывают изменения привязок в backing-хранилище. На Fjall проверка существования и открытие store защищены facade DDL-lock, но удаление store извне facade этим lock не координируется. Ошибки чтения метаданных приводят к отказу, а не к пропуску держателя. Обратный индекс `bound_in` не участвует в выводе держателей: он ни допускает, ни блокирует операцию, а ссылка на прежнее воплощение того же имени не влияет на новую таблицу. Вне вывода держателей `bound_in` читается: при замене регистрации (`replace`) — чтобы сохранить состояние каталога и кэша, и при reconcile миграции — как свидетельство о привязке. Настроенная таблица без конверта привязок или без info-хранилища считается непривязанной: таблица, которая никогда не была привязана, законно не имеет конверта, поэтому конверт, потерянный из-за повреждения хранилища, этим опросом не обнаруживается (известное ограничение). Исключение — таблица, чей менеджер уже поднят и чей флаг in doubt поднят: она считается держателем **любой** идентичности валидатора даже при отсутствующем конверте, потому что конверт — ровно то, чему здесь доверять нельзя. Если запись привязки или отвязки не удалось сбросить на диск, операция восстанавливает прежний логический конверт — **полный** список привязок, включая ops и priority, с новым временным штампом, а не прежние байты; отсутствовавшая запись восстанавливается как **присутствующая пустая** запись, а не как отсутствующая; это возможно только для bind — обычный unbind отсутствующей привязки возвращает `false` ещё до записи, поэтому у неудавшейся отвязки прежняя запись всегда есть; операция сама записывает и сбрасывает конверт и проверяет результат: запись и сброс повторяются не более трёх раз подряд, без задержки между попытками, и результат компенсации не игнорируется. Успешная компенсация возвращает прежнее состояние привязок, и повтор той же операции начинает с него; до успешного сброса таблица остаётся для записи привязанной, так что неудачная отвязка не даёт параллельной обычной записи пройти без валидатора. Если после последней из попыток компенсацию не удалось записать **или** сбросить (достаточно одного из двух; три попытки — принятая политика, а не свидетельство того, что сбой сброса постоянен или уже исчез), операция возвращает типизированную ошибку «состояние привязок таблицы под сомнением» (без имени таблицы в тексте), и таблица до перезапуска процесса остаётся в состоянии in doubt. Флаг выражает **неопределённость**, а не гарантированное исключение: он поднимается и тогда, когда не удалась прямая запись, и тогда, когда future брошен (отмена) на прямой записи, на прямом сбросе или на шаге компенсации — отмена сама является источником сомнения; точки приостановки между успешным результатом сброса и публикацией нет, поэтому отменяемым остаётся сброс, который уже подействовал в backing-хранилище, пока future не завершён. Пока флаг поднят: bind и unbind валидаторов на этой таблице, активация схемы на этой таблице и переименование самой таблицы отказывают типизированной ошибкой «состояние привязок таблицы под сомнением» без пути к таблице; замена, переименование и удаление валидатора (если эта таблица входит в опрошенные базы) возвращают ту же ошибку, но порядок различается: обычные проводные пути (замена валидатора, проводное переименование, удаление) сначала резервируют базы-держатели и только затем спрашивают флаг, поэтому при уже занятом слоте миграции у базы-держателя раньше срабатывает прежний отказ забора — он сохраняет свой стабильный код и может называть базу в тексте; preflight миграционного переименования, наоборот, проверяет флаг **до** любого резервирования чужих целей, а прямое (`ExclusionOnly`) удаление валидатора резервирования не выполняет вовсе; свободен от пути только сам конфликт in doubt; DROP самой таблицы (`DROP TABLE`) флаг не проверяет: он отклоняется только обычными проверками (внешние ключи, незавершённое переименование), поэтому на свободной базе таблицу in doubt удалить можно. Обычные чтения и записи при этом не блокируются: живой вектор остаётся последним согласованным состоянием, проверки флага на пути записи нет (клонирование менеджера добавляет лишь клонирование `Arc` флага). Прямой API-встраивания (`create_validator_from_wasm`, `rename_validator`) выполняет replace/rename без опроса держателей: этот забор защищает проводные и миграционные пути. Переименование самой таблицы, наоборот, отклоняется facade-проверкой флага и при прямом вызове, а изменение «сырого» `TableManager` доверенным вызывающим в обход facade этим забором не покрывается. Флаг in doubt есть только у поднятого менеджера таблицы: у никогда не открытой (cold) таблицы его нет, и опрос её флагом не помечает, но сохранённый конверт такой таблицы всё равно читается, и если он называет валидатора, таблица попадает в именованное свидетельство отказа `DROP` (с путём `db/repo/table` в тексте). Перезапуск **сбрасывает флаг, но не чинит записи**: он создаёт новый менеджер с чистым флагом и перечитывает уцелевший конверт, снова считая его авторитетным, поэтому перезапуск не является восстановлением. После двойного сбоя (сброс не удался **и** компенсацию не удалось сделать durable) перезапуск может оставить два остаточных состояния. (а) Объявленный `SET TABLE SCHEMA`, чей прямой конверт уцелел: при открытии загружается привязка-сирота к артефакту, который был снят; валидируемые записи таблицы отказывают на разрешении этой привязки, а повтор той же схемной DDL выпускает новый id и **дописывает** его, не удаляя первый. Штатного средства снять привязку-сироту нет: unbind адресуется именем валидатора, а имя схемного валидатора (`__schema__/<db>/<repo>/<table>`) после перезапуска в реестре не материализуется (каталог таблицы схемы уже не содержит), поэтому unbind по имени отказывает «validator not found», а правки конверта привязок код не поддерживает. Ручной шаг оператора — пересоздать таблицу (`DROP TABLE` снимает её привязки, затем `CREATE TABLE`): это УДАЛЯЕТ данные таблицы, новая таблица пуста (получает новый идентификатор хранилища и не открывает конверт-сироту заново). Альтернатива — восстановить из резервной копии, снятой до этой DDL: это операция над всем `data_dir`, а не восстановление одной таблицы на месте. (б) Миграционный UNBIND, чей пустой конверт уцелел: живые и сохранённые привязки согласны в отсутствии, но реестр и каталог всё ещё несут старую обратную ссылку `bound_in`, и возобновление миграции отказывает на этом расхождении ещё до доказательства завершения или повтора; повторные перезапуски это свидетельство не меняют. Через обычный проводной путь это состояние не чинится: долговременный забор миграции перезапуском не снимается (пакет, обслуживаемый целевой базой, отклоняется на входе, обслуживаемый другой базой — на резервировании цели), а BIND в миграции отложен. Согласование выполняет доверенный офлайн-встраиватель прямыми API привязок `ShamirDb`, которые этот забор обходят: повторный bind той же привязки на этой таблице и последующий завершённый unbind снимают ссылку из реестра и каталога и сбрасывают пустой конверт (флаг после перезапуска чист, поэтому bind/unbind не отклоняются). Запись `bound_in` в каталог выполняется по возможности (best-effort), поэтому до возобновления миграции оператор обязан проверить, что ссылка действительно снята, и только затем возобновлять.
Внутреннее системное хранилище определяется по собственному handle, а не по migration identity или имени; оно опрашивается так же — только метаданные, — никогда не входит в множество резервирования, а его привязки приводятся лишь как свидетельство при отказе `DROP`. Флаг in doubt внутренних таблиц не читается вовсе (внутреннее хранилище не резервирует базу, поэтому поднимать его здесь нечем): свидетельством отказа `DROP` служит только сохранённый конверт, и привязка, снятая в backing-хранилище до согласования неудавшейся отвязки, для `DROP` не видна — известное ограничение. Обычные (wire) замена, переименование и удаление привязанного валидатора резервируют каждую базу-держатель до завершения операции; `DROP` привязанного валидатора по-прежнему отказывает «still bound», даже если все держатели свободны. Preflight миграции резервирует держателей только для переименования (`RenameValidator`): замена и удаление требуют проверяемого HMAC, в тело миграции не допускаются и отклоняются ещё на входе с `hmac_required`, до первой записи в журнал. Собственная база раннера от резервирования освобождена по идентификатору (не по имени) — её fence раннер уже держит. Если preflight обнаружил ограждённого чужого держателя, отказ с его
целевым стабильным кодом происходит после резервирования слота базы-
раннера, но до долговечного приобретения fence и записи `Started`.
Свежий резерв базы-раннера при этом снимается, а при resume
восстанавливается прежний маркер `in_doubt`; эффектов нет. Это относится
только к обнаруженному preflight-конфликту: повторная проверка во время
эффекта и другие поздние ошибки метаданных всё ещё возможны после
`Started` и могут оставить базу-раннер в `in_doubt`. Возобновление (resume) закрепляется за уже существующим fence прерванной попытки на её текущем поколении; preflight новых долговременных fence не создаёт.

**Границы проверки.** Fence ограничивает допуск новых запросов: он не
дожидается завершения уже допущенных обычных операций и не отменяет их.
В интерактивной транзакции проверяются новые запросы (`tx_execute`), но
отдельный `COMMIT` уже подготовленных записей не повторяет эту проверку.
Прямые низкоуровневые вызовы через хэндлы репозиториев и таблиц обходят
проверку на входе в батч `ShamirDb`; fence не останавливает эти вызовы и
не ждёт их завершения. Глобальные реестры функций и управление
репликацией не привязаны к одной базе: fence одной базы не является
запретом на глобальные изменения или остановкой репликации.
Непривязанные регистрации валидаторов не резервируют базы-цели, однако
глобальный флаг rename-механизма может отклонить несвязанную с миграцией
явную свежую регистрацию валидатора или bind, даже если соответствующая
пользовательская база свободна. Декларативная активация схемы этот флаг
не проверяет — это остаётся пробелом защиты; простая проверка флага в
точке регистрации также блокировала бы разрешённые побочные эффекты той
же миграции, если не передавать туда доказательство владения миграцией.
Никаких утверждений об остановке репликации через fence нет. Перед apply
остановите конкурирующие записи и дождитесь завершения уже допущенной
работы, включая открытые пишущие транзакции; не используйте fence как
замену этому шагу.

### Известные ограничения забора: что он не закрывает

Три окна остаются известными ограничениями, а не обещаниями забора. Их устранение (повторная проверка забора в точке эффекта либо резервирование базы на всё время выполнения DDL, атомарная публикация строки каталога, забор пути pull-применения) вынесено за пределы этого релиза; поведение продукта здесь не меняется.

1. **Обычный DDL, допущенный до захвата забора, может создать объект с другим определением.** Проверка забора выполняется один раз на входе батча (`crates/engine/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:136-150`); дальше обработчик индексов только разрешает таблицу и проверяет существование семейства индексов, не резервируя базу и не перепроверяя забор (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:349-378`). Поэтому обычный `CREATE INDEX i`, прошедший вход до захвата, может выполниться уже после того, как операция `Apply` записала свой `pre_state=absent`, и создать **другой** индекс с тем же именем. Возобновление считает существование объекта эффектом операции и проверяет только наличие, а не определение (`crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/apply/apply_operations.rs:628-640,707-735`; `crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/apply/ddl_postconditions.rs:120-148`). Расхождение не обнаруживается автоматически: перед возобновлением сверьте объекты из `status`/`plan` с фактическим каталогом и при расхождении действуйте по процедуре из раздела [In-doubt: что читать из status и кто reconcile](#in-doubt-что-читать-из-status-и-кто-reconcile).
2. **`RENAME DATABASE` публикует новую строку каталога и удаляет старую отдельным шагом.** Между записью новой строки (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:361`) и удалением старой (`:363`) падение оставляет два имени с одной идентичностью; при открытии `ensure_database_identity` отвергает повторное назначение идентичности (`crates/engine/shamir-db/src/shamir_db/system_store/catalogue/schema_migrations/identity.rs:83-89,149-181`), и открытие `ShamirDb` завершается ошибкой. Отдельной процедуры ремонта такой пары имён в руководстве нет: восстанавливайте `data_dir` из резервной копии, снятой до переименования (см. [Restore до/вместо обновления схемы](#restore-до-вместо-обновления-схемы)).
3. **Pull-применение репликации пишет назначение вне забора.** Цикл follower применяет пачку прямо к репозиторию (`crates/server/shamir-sync/src/sync/replication/follower_loop/loop_body.rs:676-682`), не проходя ни входную проверку обслуживаемой базы, ни резервирование цели; активация подписки публикует строку подписки, сбрасывает интернер и каталог и обновляет кэш карантина (сам путь применения к назначению она не заборит) (`crates/engine/shamir-db/src/shamir_db/execute/admin_db/admin_replication.rs:468-742`). Забор не останавливает репликацию сам: перед `apply` остановите её на целевых базах (см. [Maintenance fence: что блокируется и что видит писатель](#maintenance-fence-что-блокируется-и-что-видит-писатель)).

Двойные сбои, описанные выше в том же разделе о заборе, остаются ограничениями и здесь: привязка-сирота после перезапуска устраняется только пересозданием таблицы, а расхождение `bound_in` после миграционного UNBIND — только согласованием через доверенный офлайн-встраиватель; штатной процедуры ни для одного из них нет; восстановление привязки-сироты из резервной копии, снятой до этой DDL, — операция над всем `data_dir`, а не восстановление одной таблицы на месте.

### Follower / read-replica

Apply **и любые другие application-schema операции — `status` и `plan` тоже** —
разрешены только на primary: у follower нет консистентного вида
журнал+схема, поэтому серверный шлюз отказывает **все три** операции со
стабильным кодом `schema_migration_follower_read_only` ещё до входа в
движок. Режим узла опускается в движок один раз при запуске сервера;
отдельная проверка на стороне движка страхует только **apply**, если
серверный шлюз когда-нибудь обойдён, — `status` и `plan` такой второй
проверки не имеют.

### Checksum drift: только вперёд

Тело применённой миграции **неизменяемо**: сервер пересчитывает
канонический checksum из присланных типизированных тел, и любое изменение
уже применённого тела меняет его checksum — `status` завершится успешно
и вернёт диагностическую `PrefixCoverage::Drift` (с ординалом расхождения),
а локальный `MigrationError::Drift` возвращает **только `apply_next`** —
он отказывает ещё до отправки APPLY; серверные `plan` и apply тоже
отказывают с кодом `schema_migration_checksum_mismatch` — `plan` отвергает
изменённое тело уже применённого ординала без всякой записи. Это локальный
типизированный вариант клиента, отдельный от wire-кода. Исправление — всегда **новая
миграция вперёд** с новым ординалом; правка старого тела запрещена.
Обратите внимание: ручной DDL **меняет фактическую схему, но не меняет
checksum тела в журнале**, поэтому такой дрейф `status` сам не
обнаруживает — сверяется journal body, а не физическая схема. Восстановление согласия между данными и журналом при
дрейфе — повторный restore из консистентного backup (см. ниже).

### In-doubt: что читать из status и кто reconcile

Если в журнале есть незавершённая попытка apply, `status` возвращает
запись `in_flight` с полями `migration_id`, `ordinal`,
`operation_index` (индекс неуверенной операции внутри тела, когда
известен), `fence_generation`; причина in-doubt — в поле
`in_flight.in_doubt_reason`
(запись `in_flight`), а **не** в статусном коде верхнего уровня — читайте
причину и индекс операции именно оттуда, не гадайте. Поле
`in_doubt_reason` необязательное: оно появляется только для записей
`InDoubt`/`Failed`, у которых есть зафиксированное свидетельство; при
его отсутствии ориентируйтесь на статусный код верхнего уровня и поле
`in_flight.status`.

Если история не позволяет установить исход коммита (например, маркер
остался после отмены, а соответствующей записи `in_flight` нет), либо
подтверждение освобождения fence не получено и маркер сохраняется,
несмотря на видимую запись `FenceReleased`, выполните контролируемое
переоткрытие хранилища. Сравните `status` до и после переоткрытия и
выбирайте дальнейшие действия по восстановленной истории журнала.
Отсутствие записи при чтении до переоткрытия не доказывает откат, а
видимая запись `FenceReleased` не доказывает долговременное освобождение.
Не снимайте маркер по видимой записи и не повторяйте apply вслепую.

Различайте **хвост этой же попытки** и **исторические записи**. Повтор
идентичного APPLY, последняя подходящая запись журнала которого
относится к этой миграции (`Applied` либо `FenceReleased`), отклоняется
с `schema_migration_in_progress`, пока fence закреплён за этой попыткой:
совпавшее приобретение fence удерживает её поколение, и хотя запись
освобождения фиксируется с увеличенным поколением, слот остаётся
закреплённым за попыткой, пока жив раннер или резерв освобождения.
Дословный повтор уже
применённого ординала из более ранней истории (например, после restore) —
это no-op без повторного DDL: он не требует **дополнительной
синхронизации хвоста журнала**, когда проверенная более поздняя история
доказывает, что fence той попытки уже освобождён; поиск идентичности уже
синхронизирует системное хранилище. Следующее приобретение fence само
предварительно синхронизирует журнал. Если раннера нет, сервер перед
подтверждением клиента обеспечивает долговременность хвоста этой
попытки (в том числе `FenceReleased`) явной синхронизацией; сама по
себе успешная явная синхронизация не доказывает, что данные стали
долговременными только при ней: фоновая запись могла сделать их
долговременными и до завершения явной синхронизации. Сохранившийся
маркер это не снимает: маркер после `Applied`
идентичный APPLY может закрыть через путь освобождения, но маркер,
сохранённый после видимой записи `FenceReleased`, не снимается даже
успешным повтором с синхронизацией — требуется контролируемое
переоткрытие хранилища.

Перед выбором пути проверьте, осталась ли незавершённая операция или
только не снят fence. Если последняя запись журнала — `Applied` без
`FenceReleased` и fence всё ещё удерживается, `status` может одновременно
показать миграцию в применённом префиксе и запись `in_flight` со
`status: "applied"`. Наличие миграции в префиксе само по себе не
означает, что fence снят.

Для освобождения отправьте прямой типизированный запрос
`ApplyApplicationSchemaMigration` с теми же метаданными и идентичным
сохранённым телом уже применённой миграции. Если серверу удаётся
зарезервировать освобождение соответствующего поколения fence, он
дописывает только `FenceReleased`, не повторяя DDL. Клиентский `apply_next`
выбирает следующий неприменённый ординал и не отправляет запрос только
на освобождение fence. После прямого запроса снова прочитайте `status`:
успешный ответ apply может вернуть сохранённый результат и без снятия
fence. Прежде чем продолжать, убедитесь, что fence снят; не запускайте
следующую миграцию вслепую.

Для незавершённой операции есть два пути:

* **Сервер reconcile сам** при повторном apply **точно того же тела**:
  resume использует тот же `apply_next` (идентичное сохранённое тело,
  ожидаемый predecessor и поколение fence, сообщённое status для
  незавершённой миграции). Сервер отказывает живому владельцу, берёт
  новое поколение, закрывает журнал для прежнего владельца и сверяет
  каждую операцию из её записанного intent (create/rename/alter/drop/check,
  bind/unbind — по binding-специфичному evidence): завершившуюся операцию
  сверка доказывает без повторного исполнения; операции, которую сверка не
  может доказать или безопасно завершить, отклоняется — «все
  идентичные resumes безопасны» не гарантируется; при невоспроизводимой
  истории сверка останавливается с отказом. Отдельной «reconcile»-операции в первом
  релизе нет.
* **Вмешательство оператора** требуется, когда reconciliation не может
  доказать или безопасно завершить эффект (например, постусловие операции
  не выполнилось — `schema_migration_postcondition_failed`, либо
  `EvidenceUnavailable`/`UnmatchedUnresolved` от резолвера: голова журнала
  обогнала попытку, не доказав её). Если apply был отправлен и исход его
  неизвестен (`MigrationError::UnknownOutcome { attempt, .. }`), сначала
  вызовите `resolve_unknown_apply(&attempt, &set)` — один read-only
  `status` с типизированным вердиктом (`Applied`, `InDoubt`,
  `ReadyToApply`, `EvidenceUnavailable`, `UnmatchedUnresolved`);
  вмешательство — только по его результату. Действия оператора — только
  диагностика по status/plan/логам и restore из консистентного backup;
  **не** правьте тела и не выполняйте DDL руками. Не считайте отказанный
  apply случившимся: исход доказывает только ответ сервера.

### Restore до/вместо обновления схемы

Журнал миграций живёт в durable-системном каталоге и восстанавливается
вместе с backup. Применённый префикс **восстановленного** журнала —
авторитет: ординалы, которых в нём больше нет, могут быть применены
заново **дословно** (то же самое тело) — это не нарушает
неизменяемость, потому что журнал-авторитет их не содержит, и повторный
verbatim-apply применённого ординала завершается как no-op/успех.
Дословность при этом обязательна и на сервере: серверный `plan` (как и
apply) отвергает подмену тела уже применённого в журнале-авторитете
ординала кодом `schema_migration_checksum_mismatch`.
Восстановление mid-flight-снимка воспроизводит то же незавершённое
состояние; поколение fence после restore без владельца — следующий apply
берёт новое поколение и делает сверку, а не продолжает старого владельца.
Состояние «данные откатились, журнал нет» (или наоборот) — это
Corruption: дрейф тел журнала против множества следующий `status`
сообщит как `PrefixCoverage::Drift` (диагностика — сама команда не
падает), но **смешанный
backup-дрейф (физическая схема
расползлась с журналом при совпадающих телах) `status` не обязан
обнаруживать** — он сверяет тела журнала, не физическую схему; при
подозрении на такой дрейф — restore из согласия и верифицированная
проверка данных, а не вслепую apply.

Логический import существует, но он переносит только логические данные и
schema: журнал application-schema миграций R1c и durable-идентичность базы
через него не переносятся. Смешивание каталогов через import/restore в этом
релизе даёт несогласованное состояние; проектное ограничение будущего R3 —
отказывать при конфликте журнала или идентичности
(`schema_migration_identity_mismatch`).

### Допущенные виды операций (версия 1)

Версия 1 допускает **только DDL, без DML**. Закрытый набор видов операций
тела миграции (`model/operation.rs` в `shamir-query-types`):

* таблицы: `CreateTable`, `DropTable`, `RenameTable`;
* индексы: `CreateIndex`, `DropIndex`, `RenameIndex`;
* схема таблицы: `SetTableSchema`, `AddSchemaRule`, `RemoveSchemaRule`;
* функции: `CreateFunction`, `DropFunction`, `RenameFunction`,
  `CheckFunction`, `AlterFunction`;
* валидаторы: `CreateValidator`, `DropValidator`, `RenameValidator`,
  `BindValidator`, `UnbindValidator`.

Каждое тело валидируется локально при построении множества и повторно на
сервере при apply; недопущенная операция — типизированный отказ ещё до
сети. Функции и валидаторы серверно-глобальны: операция над ними может
затрагивать и другие базы этого сервера, а такой apply сериализуется на
весь сервер. Замена регистрации (`replace`) в теле миграции при этом не
исполняется: она требует проверяемого HMAC и отклоняется preflight
миграции с `hmac_required` до первой записи в журнал, так что до
рантайма такая операция не доходит.

Допущение в модели не гарантирует исполнения на рантайме: операции,
требующие HMAC по канонической политике, отказываются ещё в admission с
`hmac_required`; каждая операция также проходит обычную авторизацию цели
(`BindValidator` обязан целиться в ограждённую базу). `BindValidator`
допущен моделью, но на рантайме **всегда откладывается** с
`schema_migration_operation_deferred:` — зафиксировать существующие
строки до бинда безопасно нельзя. `AlterFunction` меняет **только**
режим optimize (замены тела/кода нет).

<!-- ORCHESTRATOR FOLLOW-UP (functional-index admission fix): limits of the
     functional-index subset are under review on branch rel-r1c-fix-funcidx
     (narrowing to `lower`/`upper`/`trim`/`length` without `functional_args`).
     Not merged into master at the time of writing; this guide intentionally
     does not enumerate the current limits. When the fix lands, state here:
     the built-in function allowlist and the rule on `functional_args`. -->

### Таблица стабильных кодов ошибок

| Код | Значение | Действие оператора |
|---|---|---|
| `schema_migration_in_progress` | Обычная запись, DDL или `CallOp`: целевая база в `reserved` или `live` (как и прежде; `Lifecycle` само по себе это правило не меняет). Для `STATUS`/`PLAN` и свежего валидного `APPLY` на новую миграцию `Lifecycle` также даёт этот код; тот же отказ даёт удерживаемый live-раннер или резерв. Специальный повтор уже применённого ordinal обрабатывается отдельно; пока поколение закреплено за живым раннером/резервом, повтор также отказывается с этим кодом | Дождитесь завершения apply; затем повторите запрос |
| `schema_migration_in_doubt` | Обычная запись, DDL или `CallOp`: у цели маркер `in_doubt` (как и прежде). Для `STATUS`/`PLAN` vacant-слот с собственным хвостом `Applied` без `FenceReleased` также означает `in_doubt`: fence всё ещё принадлежит применённой миграции. Свежий `APPLY` новой миграции при таком хвосте получает тот же код; идентичный повтор уже применённого ordinal идёт отдельным путём подтверждения/освобождения. Для неразрешённой миграции несовпадающий `APPLY` может быть отклонён раньше как `predecessor_mismatch` или `checksum_mismatch`; совпадающий resume не обходит валидацию и foreign preflight. Для `PLAN` отказ также возможен при консервативной гонке, когда освобождение завершилось между чтением журнала и снимком слота: повторите `status`/`plan`. Исключение жизненного цикла — только чужая диспетчеризация: `RENAME`/`DROP` ограждённой цели, отличной от обслуживающей базы, возвращает этот код и при `reserved`/`live`; если цель — сама обслуживающая база, отказ приходит раньше, на входе в батч, с её текущим кодом состояния (`in_progress` при `reserved`/`live`, тот же `in_doubt` при маркере) | Дождитесь активного apply; иначе проверьте `status`. Если исход коммита не доказан или маркер сохраняется после неподтверждённого освобождения, выполните контролируемое переоткрытие и сравните `status`. Следуйте процедуре восстановления выше |
| `schema_migration_checksum_mismatch` | Тело применённой миграции изменилось (drift) | Forward-миграция вперёд; тела не править; при дрейфе данных — restore |
| `schema_migration_gap` | В применённых ординалах дыра | Локальная ошибка множества; пересоберите set, проверьте ординалы |
| `schema_migration_predecessor_mismatch` | Ожидаемый predecessor не совпал с журналом | `status`, сверить префикс; apply не повторять вслепую |
| `schema_migration_follower_read_only` | Любая application-schema операция (status/plan/apply) на follower/replica | Выполнять на primary |
| `schema_migration_durability_unavailable` | При открытии: повреждение durable-журнала миграций, обнаруженное валидацией истории при старте — как отнесённое к базе, так и неатрибутируемое, либо физический ключ строки журнала не является 16-байтовым record id (строгий скан раздела журнала) — прерывает открытие всего ShamirDb/сервера; код виден в диагностике старта, не как wire-ответ. В рантайме: тот же код от `status`/`plan`/`apply` при повреждении, маршрутизируемом на одну базу (конверт с пригодным ключом), при неатрибутируемом конверте (могут отказать все читатели), а также при обычных отказах чтения/записи/синхронизации хранилища — сам по себе он не означает повреждение | При открытии: полный restore `data_dir` из верифицированного консистентного backup (журнал, схема и данные восстанавливаются вместе) и повторный запуск; каталог вручную не править. В рантайме: не спешите с restore — диагностируйте доступность хранилища, затем заново прочитайте `status`; apply не повторять вслепую; при незавершённых durable-намерениях состояние может вместо этого быть in-doubt |
| `schema_migration_postcondition_failed` | Постусловие операции не подтвердилось | Вмешательство оператора: диагностика, restore при расхождении |
| `schema_migration_identity_mismatch` | Присланная целевая идентичность базы не совпала с обслуживаемой базой | Проверьте, в ту ли базу подключены (`reported_target`); import/restore смешанных состояний каталога |
| `schema_migration_operation_deferred:` | Операция отложена сервером (см. сообщение) | Читать серверное сообщение, действовать по нему |

### Старый сервер / новый клиент

Сервер без R1c отвечает отказом на неизвестную типизированную операцию —
клиент возвращает `MigrationError::UnsupportedServer { message }`:
обновите сервер. Отдельный случай: пир отвечает на `status`, но опускает
`applied`/`fence_generation` при непустой голове журнала — это тоже
`UnsupportedServer`, а не «пустой журнал». Никакого fallback на DDL без
журнала не существует ни на одном пути.

### Что добавит административный шелл (позже)

Шелл (release-1 R5, проект
[docs/dev-artifacts/design/admin-shell.md](../../dev-artifacts/design/admin-shell.md))
получит те же команды apply/status/check поверх этого протокола; R1c
поставляет только клиентские API.

## Административный шелл: состояние и узкие просмотры данных (R5 PR3)

Административный шелл (`shamir-server shell`) поддерживает интерактивный
режим (только когда и stdin, и stdout — TTY) и батч-режим: ровно одна
команда после `--`, разбор из argv, один ответ. Батч-вывод — всегда JSON
Lines (схема v1), в том числе на TTY; stderr — только для человека и
никогда не содержит сырых серверных сообщений или peer-supplied текста.
Секреты в argv запрещены: источников ровно четыре — `--secret-prompt`,
`--secret-stdin`, `--secret-env` и `--secret-fd N`. Четвёртый доступен
**только на Unix** и передаёт уже открытый файловый дескриптор, а не сам
секрет в argv; на Windows флаг отвергается типизированным отказом до
чтения секрета и до любой сетевой активности. Ровно один источник за
вызов: несколько явных селекторов — типизированный конфликт источников.
`--secret-env` принимает только `SHAMIR_SHELL_PASSWORD` (SCRAM) или
`SHAMIR_SHELL_TOKEN` (токен). Никакой истории в R5 нет — поверхности
утечки через историю не существует.

### Команды состояния и просмотра данных

| Команда | Что печатает | Права |
|---|---|---|
| `status` | снимок состояния сервера (см. ниже) | superuser-класс |
| `version show` | версия бинарника, наблюдённая при загрузке эпоха формата и исход штампа; штамп при запросе не перечитывается (сохранённые переопределения durable-политики читаются из системного каталога во время запроса — см. ниже) | superuser-класс |
| `replication subscriptions` | записи подписок репликации (идентичности, состояние, закладки, версии), но не «живой» лаг апстрима | **только superuser-класс**: хендлер отвергает не-суперпользователя ещё до запуска batch; у суперпользователя проверки `List(Root)` (`authorize_repl_read`) и ACL базы маршрутизации проходят через общий admin-обход; БД маршрутизации из профиля обязательна (batch-путь) |
| `data count DB REPO TABLE` | число записей таблицы и признак пути (O(1)-счётчик или скан); если статистика ответа не различает путь — консервативное `unknown` с оговоркой | Read на поименно названной базе команды (floor профиля не требуется); затем обычный ACL базы/таблицы сервера (batch-путь) |
| `data head DB REPO TABLE N` | первые N записей в порядке серверного скана (см. ниже) | Read на поименно названной базе команды (floor профиля не требуется); затем обычный ACL базы/таблицы сервера (batch-путь) |
| `data get DB REPO TABLE KEY` | точечное чтение по точному ключу: тело одной записи (см. поля результата ниже) или типизированный `not_found`. KEY — Base58 `RecordId` длиной **16–22** символа (22 — каноническая полная форма), проверяется локально до соединения; некорректный ключ — типизированный отказ `invalid_record_id` (exit 1), не паника и не скан | сначала Database Read, затем Table Read на поименно названной базе (floor профиля не требуется), с проверкой предка до любого поиска; та же двухшаговая проверка повторяется после ожидания бюджета ответа (см. ниже); курсор не создаётся, курсорный путь не задействуется |

**Единый отказ чтения: таблица не выдаёт своего существования.** Если
вызывающий не может прочитать таблицу — потому ли, что она закрыта ACL, или
потому, что её нет, — все пути чтения данных возвращают **один и тот же**
типизированный код `unknown_table` (exit 5): точечное чтение `data get`,
batch-чтение (`data count` / `data head`), создание и чтение курсора. Тот же
код получает и опрос статуса DDL-операции. Закрытая таблица и отсутствующее
имя по коду неразличимы. Правило действует в одной точке на каждом пути:
отказ `Read` на `Table` в авторизации batch-запроса, проверка существования
таблицы перед выполнением batch-чтения и двухшаговая проверка
`Database`→`Table` у точечного чтения, создания/чтения курсора и опроса
статуса DDL. Отказ уровня **базы** (`Database Read`) по-прежнему
`access_denied` (exit 4): он выполняется до любого разрешения таблицы и
поэтому о таблице ничего не сообщает. Отсутствующие родители сохраняют свои
коды `unknown_db` / `unknown_repo`. Той же маской закрыты `DescribeTable` и
`GetTableSchema`: закрытая и отсутствующая таблица дают `unknown_table` с
фиксированным сообщением `table is unavailable` (родительская проверка базы
остаётся `access_denied`).

Важно не обобщать это правило на все операции READ: паритет отказа для
чтения данных действует как на здоровых, так и на quarantined репозиториях,
но порядок admission и проверки доступности различается. При slot admission
раньше availability таблицы в обоих состояниях получают одинаковый
фиксированный `server_busy`. На `GetRecord` availability раньше slot:
absent/closed таблицы получают `unknown_table`, quarantine проверяется позже.
Остаётся узкое окно name-to-handle между повторной проверкой регистрации и
получением handle; terminal-state гонка курсора также follow-up. `List(Tables)`
через store List перечисляет настроенные имена; `List(Indexes)` различает
состояния. `GetBufferConfig` и `ListValidators` через stored function всё ещё
различают availability. Для batch missing repository остаётся legacy untagged
error, не typed `unknown_repo`.
`Subscribe` с любым нечитаемым или отсутствующим source целиком отклоняется
до создания потока; нет частичного исключения источников. Утверждённая
совместимость `runLive` — отклонить весь promise без выдачи stream.

Практическое следствие: раньше пути расходились — закрытая таблица давала
`access_denied` (exit 4), а отсутствующая на batch-пути приходила общим кодом
`query` (в шелле — `server_error`). Теперь и закрытая, и отсутствующая
таблица дают `unknown_table` (exit 5) на всех перечисленных путях. Неудачная
попытка прочитать таблицу больше не подтверждает и не опровергает её
существование.

**Вторая авторизация и сравнение регистрации.** Точечное чтение — не batch-
и не курсорный путь, поэтому он сам выполняет ту же двухшаговую проверку
(Database `Read`, затем Table `Read`, с проверкой предка до любого поиска)
**дважды**: до ожидания ёмкости байтового бюджета ответа и ещё раз после
него — иначе отзыв прав за время ожидания был бы обойдён. После второй
проверки сервер заново разрешает имя родителя и сравнивает **регистрацию**
таблицы с той, которую авторизовала первая проверка; сравнение различает
любые две регистрации, включая legacy-таблицы без store id. Три исхода:

* регистрация не изменилась — чтение выполняется обычным путём;
* регистрация заменена (drop + create, переименование, замена
  legacy-таблицы другой legacy-таблицей, имя маршрута занято другой
  регистрацией) — типизированный `access_denied` с фиксированным серверным
  текстом «the authorized table registration changed during the request»;
  шелл печатает собственный фиксированный отказ (exit 4) и серверный текст
  не воспроизводит. Если идентичность регистрации установить нельзя, сервер
  отказывает так же (fail closed), а не читает вслепую;
* таблицы по этому имени больше нет — `unknown_table` (exit 5): отсутствие
  сообщается как отсутствие, а не как замена.

Если за время ожидания исчез родитель (база или репозиторий), ответ — тот
же типизированный отказ разрешения имени, что и без ожидания
(`unknown_db` / `unknown_repo`), а не `unknown_table`.

**Остаточное DDL-окно.** Между последней проверкой регистрации и получением
хендла таблицы остаётся окно: DDL, целиком попавшая в него, сравнением не
покрыта. Повторная проверка сужает окно ожидания, но не закрывает его — это
документированное ограничение этой версии.

**Границы ответа `data get`: имена и карантин.** `db`, `repo` и `table`
принимаются в пределах **1..=255 UTF-8 байт**. Это **физический** предел
имени ключевого пространства хранилища, а не логический предел DDL:
логические валидаторы создания базы и репозитория длину не ограничивают
(только пустое имя, `.`/`..` и символы вне `[A-Za-z0-9_-]`), поэтому база с
логическим именем длиннее 255 байт может существовать, но точечным чтением
не адресуется — сервер отвечает фиксированным отказом `invalid_request`
(шелл: exit 5) без эха имени. Карантин репозитория проверяется и до
ожидания, и при получении хендла таблицы; на этом пути `resync_required`
несёт **фиксированный** текст, а не записанную или наблюдённую идентичность
репликации (у batch-пути текст не менялся). Шелл в любом случае печатает
собственный фиксированный текст.

Припаркованный (ожидающий ёмкости байтового бюджета ответа, а **не**
видимости) `data get` при этом **не виден в просмотре активности запросов и
не может быть отменён оттуда** — известное ограничение этой версии:
точечное чтение занимает слот in-flight запросов сессии, но не регистрирует
строку активности (в отличие от припаркованного batch-запроса, который
виден и отменяем).

**Права `replication subscriptions` и admin-обход.** Команда идёт
существующим admin-путём `ReplicationStatus`. Это admin-операция: хендлер
**отвергает не-суперпользователя ещё до запуска batch** (типизированный
`permission_denied`, exit 4), поэтому «обычная сессия с Root List» эту
команду выполнить не может. У суперпользователя обе оставшиеся проверки —
`List(Root)` (`authorize_repl_read`) и ACL базы маршрутизации из профиля —
проходят через общий admin-обход движка (актор уровня admin/system проходит
гейт, не сравнивая биты), так что выдавать суперпользователю Root List
дополнительно не нужно.

**Конверт ответа и поле `node`.** Батч-вывод — всегда ровно одна запись
JSON Lines (схема v1), в том числе на TTY. Интерактивный режим
JSON-конверта не печатает вовсе: человекочитаемая таблица или блок ошибки
идёт только на stderr, stdout молчит. Поле `node` (наблюдённые роль и id
узла) заполнено **только** в **успешной** записи (`kind: result`) команд
`status` и `version show` — каждая выдаёт **собственный живой** `Status`
RPC (тот же RPC и те же смыслы полей); лаунчер при загрузке/подключении
захватывает исход штампа формата и идентичность узла (роль и id), а
uptime, наблюдения, WAL-блок и сохранённые переопределения
durable-политики собираются **во время запроса**. Любая **ошибка** несёт
`node: null` и `data: null`. Все остальные команды — `data count`, `data head`,
`data get`, `replication subscriptions` — печатают `node: null`, как и
локальные/поясняющие команды и отказы до подключения: роль подключённого
узла наблюдаема **только** через `status`/`version show`, а шелл никогда
не отправляет скрытый лишний запрос, чтобы её заполнить. `null` здесь
означает «не наблюдалось», а не «нет роли»; выдуманное наблюдение не
печатается никогда.

`status` печатает:

* версию серверного бинарника;
* **наблюдённую** эпоху формата и исход штампа — захваченные лаунчером
  при загрузке (через существующий on-disk `inspect` под boot-lock);
  константа процесса не используется, и штамп при запросе НЕ
  перечитывается. Утверждение «`status` не читает диск» было бы
  неверным: **сохранённые** переопределения durable-политики читаются из
  системного каталога **во время запроса** (через работающее
  хранилище системы), поэтому `status` и `version show` могут
  завершиться типизированным кодом, если это чтение не удалось.
  Каталожные команды (`db list`, `table describe` и т.п.) отличаются не
  этим, а тем, что каталог — сам предмет их ответа: `db list` перечисляет
  **живой реестр** процесса (зарегистрированные имена баз), а не сканирует
  каталог на диске, и так же `repo list`/`table list` отдают живые
  регистрации при каждом вызове;
* монотонный uptime (монотонные часы; не откатывается);
* роль репликации **из настроенного при запуске режима узла**, не
  выведенную из телеметрии; отсутствует — `unknown`;
* разрыв репликации по репозиториям: поле `gap_versions_merged` —
  **слитный неавторитетный агрегат** (реестр объединяет наблюдения
  лидера и фолловера). Всегда печатается оговорка; слово «лаг» не
  используется никогда. Возраст выборки всегда неизвестен (нет
  timestamp-проведения), ноль может означать «не сэмплировано».
  Возраст по квитанции (receipt) — отдельное поле, если присутствует
  в снимке, иначе неизвестен;
* WAL-телеметрию: watermark/отложенные версии/коммиты. Это **диапазон
  W–R (watermark — recoverable), а не число записей**;
* настройки durable-политики по репозиториям: **эффективные** (после
  всех переопределений) и **сохранённые**; отсутствующее
  переопределение — `null`.

Принятые ограничения `status` (сформулированы явно, чтобы их не принимали
за дефекты):

1. **Наблюдения репликации берутся из ПРОЦЕССНО-ГЛОБАЛЬНОГО реестра**:
   два лаунчера в одном процессе видят наблюдения друг друга; реестр
   ограничен числом различных ключей `(db, repo)`, и при переполнении
   наблюдения всех непоместившихся источников сводятся в одну строку
   `(other)/(other)` — это предел **ключей реестра**, а не источников на
   репозиторий; идентичности репозитория для этой строки не сохраняются,
   поэтому она печатается с оговоркой.
2. **Роль выводится из настроенного режима**, а не из локального потока:
   `ReadOnly` → `follower`, любая другая конфигурация (включая
   `ReadWrite` и выключенную репликацию) → `unknown`. Локальный поток
   репликации роль никогда не определяет.
3. **Пара WAL о первом отложенном**: `first_pending_age_ns` присутствует
   ровно тогда, когда присутствует `first_pending_version` (и никогда
   отдельно). Нулевой возраст — **нижняя граница**, а не доказательство,
   что отложенная версия только что появилась: экспортёр метрик отдаёт
   ноль и когда отложенных версий нет, и когда параллельное или запоздалое
   наблюдение сделало выборку несравнимой (более новое наблюдение не
   заменяется).

Бюджеты и обещания `data head` (оболочечные, поверх серверных):

* **порядок не обещан** — записи приходят в порядке серверного скана,
  `ORDER BY` не используется и порядок полагать нельзя;
* не более **100 записей** (больший N обрезается с видимым маркером
  `rows_capped`);
* не более **256 КиБ** — это бюджет **выдаваемого массива записей**, а не
  полученной от сервера полезной нагрузки и не транзитных аллокаций:
  первая запись, которая переполнила бы массив, останавливает проекцию и
  выставляет маркер `bytes_truncated`;
* **дедлайн 5 секунд** по **монотонным часам** — ОДИН общий дедлайн на
  весь запрос **после установления сессии**: и на ожидание ответа, и на
  локальную проекцию (монотонные часы не зависят от перевода системного
  времени и не откатываются). Точный контракт:
  * дедлайн проверяется **между записями** и при ожидании;
  * **одна запись сначала проецируется целиком**, поэтому стоимость одной
    записи ограничена только серверным лимитом результата — это **не**
    жёсткая граница wall-time на одну запись;
  * **уже истёкший дедлайн важнее ошибки проекции**: если бюджет
    исчерпан, отказ — типизированный `head_wall_time` (**exit 7**),
    даже когда проекция записи вернула ошибку (`projection_limit` и
    т.п.) — никогда не вводящий в заблуждение exit 5;
  * дедлайн перепроверяется **и после** синхронной проверки вывода на
    отражение секрета (у токен-сессии) — этот последний замер входит в
    тот же бюджет: если к этому моменту он истёк, ответ —
    `head_wall_time` (exit 7), а не успех (exit 0) и не отказ проверки
    (exit 5);
  * истечение — «исход неизвестен, операция может ещё выполняться»;
    повторная отправка не выполняется никогда.

Поля результата `data head` (схема v1): `rows` — массив спроецированных
записей; `rows_requested` — запрошенное N десятичной строкой;
`rows_returned` — число выданных записей десятичной строкой;
`rows_truncated` — `true` только когда сервер сообщил, что записи
продолжаются (`has_next`), И запрос превысил жёсткий предел в 100;
`bytes_truncated` — `true`, если проекцию остановил байтовый бюджет;
`rows_capped` — `true`, присутствует только при N > 100 (сам факт
усечения запроса); `caveat` — фиксированная оговорка о порядке.

Поля результата `data count` (схема v1): `count` — десятичная строка;
`count_path` — `counter` (метка `__record_counter__`, O(1)-счётчик),
`scan` (статистика показала просканированные записи) или `unknown`
(статистика не различает путь); `caveat` — фиксированная оговорка,
присутствует **только** при `count_path = unknown`. Шелл никогда не
выдумывает маркер скана, а фильтры всё равно сканируют — это бюджет, а не
обещание физического осмотра.

Поля результата `status` (схема v1): `server_version` — строка;
`format_epoch`, `uptime_ms` — десятичные строки; `stamp_outcome` — одно из
`created`, `adopted`, `unchanged`, `version_updated`, `inspected_stampless`,
`inspected_older`; `role` — `follower` или `unknown` (только из настроенного
режима узла); `node_id` — строка или `null`; `gap_caveat`, `age_caveat` —
фиксированные оговорки; `replication` — массив объектов `{db, repo,
gap_versions_merged, age, receipt_age_ns}`, где `gap_versions_merged` —
десятичная строка, `age` — всегда `null`, `receipt_age_ns` — десятичная
строка или `null`; `repositories` — массив объектов `{db, repo, effective,
stored, wal}`: `effective` и `stored` — `{wal_commit,
wal_retention_max_bytes, on_wal_retention_exceeded, replication_serve}`
(`wal_commit`: `buffered`/`synced`; `wal_retention_max_bytes`: десятичная
строка; `on_wal_retention_exceeded`: `refuse_writes`/`declare_gap`;
`replication_serve`: `durable`/`completed_reseed`; отсутствующее
переопределение — `null`, `stored` целиком `null`, когда ничего не
сохранено), `wal` — `{backend_mem, watermark_w, recoverable_through,
pending_versions, first_pending_version, first_pending_age_ns,
sync_now_calls, sync_rounds, sync_coalesced, sync_errors, sync_duration_ns}`
(`backend_mem` — булев, остальные — десятичные строки, оба `first_pending_*`
— `null` или строка). `wal` — это диапазон W–R, а не число записей.

Поля результата `version show` (схема v1): `server_version`, `format_epoch`,
`stamp_outcome`, `role`, `node_id` — те же значения и типы, что у
одноимённых полей `status` (тот же RPC и те же смыслы полей, без uptime и телеметрии).

Поля результата `replication subscriptions` (схема v1): `subscriptions` —
массив объектов, и в каждой записи только allowlist-поля — `db`, `repo`,
`name`, `state`, `installation_state` (строки), `recorded_identity` и
`observed_identity` (`null` либо `{lineage_id, history_epoch}`, где
`lineage_id` — ровно 16 неотрицательных целых (в выводе — десятичные
строки), а `history_epoch` — **обязательное** поле идентичности: оно
должно присутствовать и быть НЕ `null` (версия; в выводе — десятичная
строка)), `upstream_bookmark`, `next_pull_version`,
`local_consistent_version`, `certified_gap_first`, `certified_gap_through`,
`seed_source_version`, `local_baseline_version`, `source_history_epoch`
(`null` или версия — в выводе десятичная строка), `source_lineage_id`
(`null` или ровно 16 неотрицательных целых, в выводе — десятичные
строки), `artifact_digest` (`null` или строка).

Запись проецируется **поле за полем**: поля, отсутствующие в серверной
карте, **ПРОПУСКАЮТСЯ** (ключа просто нет в объекте — это не `null`);
прочие поля сервера (сырой `status_message`, терминальные причины, пути
источников) отбрасываются; имя из allowlist, для которого у шелла нет
типизированной формы, — типизированный отказ (exit 5), а не «данные».
Так же отказом (exit 5), а не данными, считается несовпадение формы
значения.

Три случая различаются. Поля, отсутствующие в серверной карте, оболочка
опускает целиком (это случай выше, а не `null`). Присутствующий лист
версии/эпохи/закладки, который движок типизирует как `u64`, принимается
только в канонической форме: **присутствующее** значение `null` (движок
закодировал `Null` для необязательного листа) печатается как `null`;
неотрицательное целое; `Big`, укладывающийся в `u64`; либо канонический
десятичный текст **строго выше** `i64::MAX` (значения, помещающиеся в
`i64`, приходят целым). Любой другой текст — `"7"`, `"007"`, `"+1"`,
`"-1"`, текст с пробелами и всё выше `u64::MAX` — отказ, а не данные.
Список lineage-байт с элементом больше 255 или неверной длины — тоже
отказ, а не данные. Присутствующая идентичность
(`recorded_identity`/`observed_identity`) как карта обязана нести
`history_epoch` **присутствующим и не `null`**: `history_epoch: null` —
типизированный отказ (exit 5), а не данные; идентичность целиком `null`
печатается как `null`.

Поле результата `data get` (схема v1): `data` — **только тело записи** (без
обёртки `record`), спроецированное конвенциями схемы v1: целые и
`Big`/`Dec` — десятичные строки, `bin` — hex, строки санитизированы,
нефинитные float помечены тегом, карта — объект, список — массив. Точное
чтение возвращает ровно одну запись или типизированный `not_found`
(exit 5); отсутствующая таблица — `unknown_table`, карантин —
`resync_required` (оба exit 5).

### Токен-сессии (read-only подмножество)

Токен-сессия (`--auth token`) может вызывать только фиксированное
read-only подмножество (включая `data head` и `data count`). Из команд
этого раздела **только** они разрешены; `status`, `version show`,
`data get` и `replication subscriptions` в подмножество не входят:
шелл отвечает фиксированным типизированным отказом
`token_request_denied` с **exit 4 ещё до отправки какого-либо запроса**
на сервер (на сервер не уходит ничего; текст отказа фиксирован:
«token sessions expose only the read-only shell subset; log in with
SCRAM for this command»). Серверный периметр при этом
шире — сужение это политика шелла поверх обычной авторизации сервера:
никаких клиентских привилегий шелл не добавляет.

### Примеры вызова (безопасные, секреты вне argv)

```sh
shamir-server shell --profile prod --secret-prompt -- status
shamir-server shell --profile prod --secret-stdin --output-version 1 -- version show
shamir-server shell --profile prod --secret-prompt -- replication subscriptions
shamir-server shell --profile prod --secret-prompt -- data count billing main invoices
shamir-server shell --profile prod --secret-prompt -- data head billing main invoices 10
shamir-server shell --profile prod --secret-prompt -- data get billing main invoices "<KEY>"
```

`"<KEY>"` — плейсхолдер В КАВЫЧКАХ: подставьте валидный Base58 `RecordId`
(от **16 до 22** символов), тоже в кавычках. 16–22 — это диапазон длин: id
с ведущими нулевыми байтами кодируется короче, **22 символа — максимум**
(это каноническая полная форма), но не единственная допустимая форма.

`--output-version 1` — локальное утверждение схемы v1 (на сервер не
отправляется); неподдерживаемое значение отклоняется с exit 1 до
соединения. Нулевое число строк `data head` и пустой/переполненный или
невалидный ключ `data get` отвергаются локально, до набора. Коды
завершения: 0 — успех; 1 — ошибка использования/разбора; 2 — сбой
соединения; 3 — отказ аутентификации; 4 — типизированный отказ
(включая `token_request_denied`); 5 — ошибка сервера; 6 —
неподдерживаемая комбинация **или отказ политики оболочки** (в том числе
неподтверждённая разрушающая команда и попытка изменить ресурс `root`);
7 — таймаут после отправки (исход неизвестен); 130 — прерывание. Сырой wire-JSON в документации и в
выводе шелла не появляется: значения строятся через builder, вывод —
типизированные записи схемы v1 с экранированием.

**Типизированные серверные коды для ВСЕХ команд.** Общий серверный
маппер (`connect::scram::server_code`) типизирует для каждой команды
шелла как минимум эти коды сервера, каждый — с фиксированным локальным
сообщением; сырой серверный текст не печатается никогда:

* **exit 4** (`permission_denied`): `permission_denied`, `access_denied`,
  `token_database_denied`, `token_read_only`, `token_channel_denied`,
  `token_request_denied`;
* **exit 5** (`server_error`): `not_found`, `unknown_table`,
  `resync_required`, `result_too_large`, `invalid_owner`, `hmac_mismatch`,
  `hmac_required`, `unknown_db`, `read_only_replica`, `cancelled`.

Следствие, которое стоит знать при обновлении: у `table describe`
отсутствующая таблица сообщает типизированный `not_found`, тогда как
более старые версии сообщали общую серверную ошибку (`server_error`); код
выхода при этом не изменился — 5. Типизированное сообщение `not_found`
теперь нейтрально: «the addressed resource was not found» — маппер не
различает отсутствующую базу, хранилище, таблицу и функцию, поэтому текст
и не обещает различия. Кроме перечисленных, отдельно типизированы `not_supported` (код выхода
6) и коды окончания сессии (`session_expired`, `session_invalidated`,
`token_revoked`, `token_expired` — `authentication_failed`, exit 3).
Любой **не** перечисленный код сервера по-прежнему схлопывается в
`server_error` (exit 5): список закрытый, и новый стабильный код сервера
не появляется в оболочке, пока его не добавят в маппер.

## Административный шелл: права и активность

Просмотр прав, изменение прав и активность — три группы команд; правила,
общие для всех трёх, — в конце раздела.

### Команды прав (SCRAM only)

Все команды прав доступны **только в SCRAM-сессии**; токен-сессия
отклоняет их на клиенте (см. общее примечание ниже). Все они идут
пакетным путём, поэтому профиль **обязан** задавать базу маршрутизации
(`database:`): без неё команда отказывает кодом 1 до соединения, а сама
база должна существовать и быть доступной на чтение. Для `rights show`
этого мало: сервер дополнительно требует `Manage(root)` — как и для
`user list`, и для `group tree`/`group members`.

```sh
shamir-server shell --profile prod --secret-prompt -- user list
shamir-server shell --profile prod --secret-prompt -- group tree
shamir-server shell --profile prod --secret-prompt -- group tree --db billing
shamir-server shell --profile prod --secret-prompt -- group members
shamir-server shell --profile prod --secret-prompt -- rights show billing
shamir-server shell --profile prod --secret-prompt -- rights show billing/main/invoices
shamir-server shell --profile prod --secret-prompt -- rights show function:tax_total
```

* `user list` — список пользователей. Сервер требует право `Manage` на
  ресурс `root`: это не отдельное право «список пользователей». Поля:
  имя, числовой идентификатор принципала, признак суперпользователя,
  область делегирования владельца (`database`) или `null`. Это
  **не** «база по умолчанию» пользователя: `database` — необязательная
  область, в границах которой владелец делегирует права; `null` значит,
  что область не задана.
* `group tree [--db DB]` — дерево ресурсов (root → база → хранилище →
  таблица), функции (с признаком `builtin`) и принципалы (пользователи;
  группы с участниками). Право `Manage(root)` требуется **всегда**;
  `--db` передаётся серверу как фильтр дерева и никогда не меняет
  требуемое право — фильтрация и авторизация остаются на сервере.
  Целиком дерево проецируется в пределах оболочки: не глубже 8 уровней и
  не больше **2000 узлов**. Дерево больше этого предела даёт собственный
  типизированный отказ `tree_too_large` (код 5) с фиксированным
  сообщением, которое называет предел и средство — фильтр `--db`. Фильтр
  сужает только базы: функции сервер возвращает всегда, поэтому он не
  поможет, если сами функции или одна база превышают предел.
* `group members` — только группы и их участники: тот **же проводной**
  ответ (`AccessTree` без фильтра), но своя проекция: оболочка печатает
  только список групп. Это не «тот же результат», что `group tree`:
  ресурсное дерево и функции в выводе отсутствуют, поэтому и предел в
  2000 узлов к `group members` не применяется.
* `rights show PATH` — владелец, группа, режим (восьмеричный, 4 цифры),
  признак setuid и, у функции, признак `builtin` одного ресурса из уже
  разрешённого дерева. Поддержанные формы пути:

  | Форма | Что адресует |
  |---|---|
  | `root` или `/` | синтетический корень ресурсов |
  | `db:ИМЯ` | **всегда** базу `ИМЯ`, даже если она называется `root` |
  | `function:ИМЯ` | хранимую функцию по точному имени; `ИМЯ` может содержать `/` |
  | `БАЗА` | базу |
  | `БАЗА/ХРАНИЛИЩЕ` | хранилище |
  | `БАЗА/ХРАНИЛИЩЕ/ТАБЛИЦА` | таблицу |

  `db:` и `function:` — единственный способ адресовать базу с именем
  `root` и функцию; само слово `root` (без префикса) всегда означает
  синтетический корень, а не базу. **Селектор никогда не уходит на провод
  как ссылка на ресурс**: оболочка запрашивает разрешённое дерево и
  выбирает узел локально, одним запросом и без второго обращения.
  Разбор локальный, до любого dial: `root`/`/` и `function:ИМЯ` — не
  ресурсные пути; два первых компонента пути (база, хранилище) обязаны
  подчиняться собственному правилу имён движка — только `[A-Za-z0-9_-]`,
  никогда `.` или `..`; у этого правила **нет ограничения длины**, и
  зеркало оболочки его тоже не добавляет. Третий компонент (таблица)
  проверяется по самому мягкому правилу физического имени хранилища:
  имя хранилища `__history__` + имя таблицы (у таблицы без суффикса `--<id>`)
  не длиннее 255 байт, поэтому имя таблицы не длиннее **244** байт. Таблица,
  созданная сегодня, имеет ещё и суффикс `--<id>`, поэтому настоящий предел
  движка ниже и зависит от длины идентификатора. Оба потолка — **244** байта
  у таблицы и **1024** байта у функции — это собственные границы
  выполнимости оболочки (табличная выведена из предела физического имени
  хранилища, функциональная — разумный предел самой оболочки), а **не**
  правило валидации движка: оболочка не зеркалит серверную проверку. На
  имени между гарантированно допустимой длиной и потолком оболочки
  авторитет остаётся за сервером, и он может отказать в имени, которое
  оболочка приняла; имя длиннее потолка оболочки до запроса не доходит
  вовсе.
  Оболочка отказывает локально (`usage`, код 1, ничего не отправляется) только
  имени, которое не может существовать ни у какой таблицы физического
  хранилища Fjall. Имя
  `function:ИМЯ` движок хранит как есть и никакой проверки имени или
  сегментов не делает (папки создаются отдельной операцией), поэтому
  оболочка отказывает только пустому имени, управляющему байту и имени
  длиннее 1024 байт (собственный предел оболочки: у движка предела нет);
  `math/abs`, `math.v2/abs` и `math/abs:v2` адресуются одинаково. Форма
  `function:` выбирает и встроенные функции (узел несёт признак `builtin`).
  Путь, отсутствующий в разрешённом дереве, даёт `not_found` (код 5) с
  нейтральным текстом — база это, хранилище, таблица или функция, маппер
  не различает. `rights show` проецирует **только выбранный узел**:
  потомки не проецируются и в предел дерева не входят, поэтому здоровый
  каталог с тысячами таблиц не мешает `rights show /`; запрос при этом
  по-прежнему получает ВСЁ дерево, и каталог, не помещающийся в предел
  ответа пакета (10 МиБ по умолчанию), даёт `result_too_large` ещё до
  проекции. Папки и
  пространства имён функций оболочкой не адресуются. Оболочка не
  вычисляет действующие права и не синтезирует ACL.

### Команды изменения прав (SCRAM only, подтверждение, подпись)

Только SCRAM-сессия; каждая команда требует подтверждения и подписи.

```sh
shamir-server shell --profile prod --secret-prompt -- chmod billing/main/invoices 0750 --yes
shamir-server shell --profile prod --secret-prompt -- chown billing/main/invoices alice --yes
shamir-server shell --profile prod --secret-prompt -- chgrp billing/main/invoices devs --yes
```

* Цели: база, хранилище, таблица, функция (`function:ИМЯ` со слэшами в
  имени). Формы пути — те же, что у `rights show`, включая `db:ИМЯ` (база
  с именем `root`) и `root`/`/` для синтетического корня. `root` изменить
  нельзя — отказ на клиенте, код 6, запрос не отправляется.
* Владелец и группа задаются именем, и оболочка **сама** переводит имя в
  числовой идентификатор из уже полученного ответа сервера: для владельца
  из `user list`, для группы из ответа `AccessTree` (тем же ответом, что
  печатают `group tree` и `group members`; отдельной команды `group
  members` для этого не выполняется). Оболочка никогда не придумывает и
  не разбирает числовые идентификаторы из аргументов. Неизвестное имя —
  `unknown_principal` (код 5); малформированное имя — `usage` (код 1) до
  соединения.
* Имя группы, содержащее пробел, **невыразимо в интерактивном REPL**:
  REPL разбивает строку по пробелам и кавычек не понимает. Такое имя
  работает только как один аргумент пакетного режима
  (`-- chgrp БАЗА/ХРАН/ТАБЛ "sales ops" --yes`). Ограничение осознанное:
  токенизатор REPL в этой версии не меняется. Имена групп длиннее 256
  байт оболочка отвергает и в пакетном режиме.
* `MODE` — восьмеричный, 1–4 цифры, только 0–7, не больше 7777 (`0755`);
  иначе `invalid_mode` (код 1), ничего не отправляется. Значение, похожее
  на флаг (`-1`, `--x`), отвергается ещё разбором аргументов как `usage`
  (код 1) — до `invalid_mode`.
* Подтверждение: интерактивно оболочка печатает вопрос в stderr и ждёт
  `yes` (с обрезкой окружающих пробелов); в пакетном режиме нужен
  `--yes`. Флаг `--yes` пропускает вопрос целиком: он не печатается и
  ответа никто не ждёт (оболочке не нужен терминал). Без подтверждения — `confirmation_required` (код
  6), до вычисления подписи и до любой отправки.
* Подпись: сервер требует суперпользователя, право `Manage` на
  изменяемый ресурс и HMAC-тег этой сессии. Тег — HMAC-SHA256 над
  каноническими байтами операции: фиксированный префикс домена, затем
  дискриминатор операции (`chmod` / `chown` / `chgrp`), затем
  типизированная цель (база/хранилище/таблица/функция) и новое значение
  (режим, идентификатор владельца, идентификатор группы). Одноразового
  входа (nonce) у этих трёх операций **нет** и база маршрутизации в
  канонические байты **не** входит: подпись той же операции дважды тем же
  ключом сессии даёт тот же тег. Тег вычисляется **после** подтверждения
  из ключа живой сессии; он никогда не печатается, не попадает в
  аргументы командной строки и не читается из файла. Точка входа подписи
  в builder-крейте существует только вместе с его фичей `crypto` (она
  входит в набор фич по умолчанию, но это именно фича `crypto`: сборка
  **без** `crypto` — в том числе с `--no-default-features` и без явного
  включения `crypto` — метода подписи не имеет, и команды изменения прав
  в такой сборке недоступны).
* Отказы типизированы, сообщения фиксированные, сырой текст сервера не
  печатается: `permission_denied` — 4 (право `Manage` проверяется
  раньше существования цели: вызывающему без `Manage` отказ приходит
  даже для несуществующей цели, и цель никогда не создаётся);
  `unknown_db` — 5 (вызывающий с достаточными правами адресовал
  несуществующий ресурс; фиксированное сообщение не уточняет, чего
  именно нет — базы, хранилища, таблицы или функции, — потому что
  серверный отказ эту разницу не различает; тот же код приходит, когда
  не существует **база маршрутизации** профиля);
  `hmac_mismatch` — 5 (тег не совпал с каноническими байтами);
  `hmac_required` — 5 (сервер не получил тега, который принимает для
  разрушающего изменения; это же сообщение приходит, если сервер не смог
  закодировать канонические байты, — типизированные builder'ы такую
  операцию вообще не собирают); `invalid_owner` — 5 (сервер не знает
  такого идентификатора владельца или группы; на проводе этот код значит
  не только «неизвестный идентификатор»);
  `unknown_principal` — 5 (имени нет в справочнике, который оболочка
  только что прочитала); `invalid_mode` — 1;
  `read_only_replica` — 5 (узел — реплика только для чтения и отказывает
  в записи: повторите изменение на лидере; оболочка не перенаправляет и
  не повторяет запрос); `cancelled` — 5 (ваш собственный пакет отменил
  другой оператор); `root_mutation_refused` — 6.
  **Не каждый** стабильный отказ сервера доживает до вывода в типичном
  виде: список типизированных кодов в маппере закрытый (см. общее
  примечание выше), остальные коды схлопываются в `server_error` (код 5)
  с тем же успехом-нейтральным сообщением.
* Результат: путь и применённый режим (восьмерично) / имя и идентификатор
  владельца / имя и идентификатор группы. Путь в результате — та форма,
  которую вы адресовали: `db:ИМЯ` остаётся `db:ИМЯ`, `/` остаётся `/`.

### Команды активности (SCRAM only)

Только SCRAM-сессия.

```sh
shamir-server shell --profile prod --secret-prompt -- activity sessions
shamir-server shell --profile prod --secret-prompt -- activity requests --limit 20
shamir-server shell --profile prod --secret-prompt -- activity sessions --all
shamir-server shell --profile prod --secret-prompt -- activity cancel 42 --yes
```

* `activity sessions` / `activity requests` — одна страница за запрос;
  оболочка дочитывает продолжение до своего предела: страница не больше
  50 строк (`--limit`, по умолчанию 50, значение больше 50 усекается до
  50), всего не больше 100 строк, не больше 10 страниц. Достижение
  предела в 100 строк завершает обход **немедленно**: следующий запрос
  не отправляется, даже если сервер выдал продолжение; такой результат
  помечен `rows_truncated: true`. `--limit 0` — `usage` (код 1) до
  соединения. Признак усечения не равен «строк ровно 100»: он истинен
  только когда строки были отброшены или продолжение осталось
  невыбранным. На десятой странице обход прекращается, и результат может
  содержать меньше 100 строк с `rows_truncated: true` — там усекает
  предел страниц, а не предел строк.
* Область по умолчанию — `own`: только собственные строки вызывающего,
  строки другого актора в неё не попадают никогда. `--all` расширяет
  область до других акторов, и решает это сервер (`ActivityView` с
  действием `Manage`); иначе сервер отказывает, и оболочка печатает
  `permission_denied` (код 4). `--all` — единственный способ **увидеть**
  чужую строку; оболочка это сама не решает.
* `activity cancel REQUEST_ID` — попытка отменить запрос по выданному
  сервером идентификатору; нужен `--yes` или интерактивное
  подтверждение (иначе `confirmation_required`, код 6, ничего не
  отправляется). Идентификатор — ASCII-десятичное число: без знака,
  пробелов и префикса, не больше 20 цифр, в пределах `u64`; иначе
  `usage` (код 1). Результат — одно из четырёх значений: `cancelled` —
  сервер **принял переход** запроса в состояние «отменён» и подал сигнал
  его исполнителю; ответ оператору **не ждёт** сворачивания цели, и
  успешный ответ **сам по себе не доказывает** завершения перехода:
  собственный обработчик запроса дожидается завершения своей работы и
  только потом отвечает клиенту запроса типизированной ошибкой
  `cancelled` (отмена всё равно не доказывает, что побочные эффекты цели
  не успели начаться); `already_finished` — идентификатор больше не
  живой запрос: запрос завершён, никогда не существовал или уже был
  отменён; `already_committing` — запрос вошёл в фиксацию и не
  отменяется; `not_cancellable` — его состояние не допускает отмены.
  Отмена должна приходить от **другого пользователя**, чем тот, кто
  выполняет запрос, и требует серверного права `ActivityView:Manage` на
  цель: сервер сравнивает принципала отменяющего с принципалом целевого
  запроса. Отдельная сессия оболочки — операционный способ выполнить это
  условие (соединение оболочки обычно вторая сессия по отношению к
  запросу клиентской программы), а не серверное требование о сессиях.
  Отмена кооперативная:
  успешный ответ не означает, что запрос уже отменён, а `not_cancellable`
  может означать и политику регистрации запроса, даже если он ещё
  выполняется или фиксируется. Сессии и запросы — **глобальные для узла**
  величины, а строки запросов всё равно ограничены базой маршрутизации:
  запросы другой базы не попадают в список даже с `--all`.
* Строки активности содержат только типизированные значения, выданные
  сервером: чеканенные метки сессий и запросов (десятичные строки), имя
  принципала, имя транспорта, имя базы, вид операции, слово состояния и
  тайминги, у сессий — ещё счётчики открытых транзакций и курсоров;
  каждая такая строка экранируется для показа. Четыре свободных поля
  (`user`, `db`, `transport`, `op_kind`) проходят каждое через собственный
  список разрешённого в
  `crates/server/shamir-server/shell/commands/activity.rs` (`user_field`,
  `name_field`, `transport_field`, `op_kind_field`); значение вне списка
  заменяется одной фиксированной подстрокой `<invalid>`. Это **правило
  показа оболочки**, а не правило движка. `user` — непустое имя: не
  длиннее **255** байт (`MAX_USER_NAME_BYTES` = `USERNAME_MAX_BYTES`,
  `crates/net/shamir-connect/src/common/model/types.rs`) и без символа,
  который является управляющим (`char::is_control`), пробельным
  (`char::is_whitespace`) или одним из символов показа, которые
  экранирует вывод (bidi/zero-width, `is_display_control`). Входной
  профиль сервера — PRECIS `UsernameCaseMapped`
  (`crates/net/shamir-connect/src/common/user/username.rs`,
  `NormalizedUsername::from_raw`), поэтому не-ASCII буквы печатаются как
  есть после обычного экранирования (как и 65-символьный ASCII-логин);
  саму нормализацию — width/case mapping, NFC и Bidi — оболочка **не**
  повторяет: это разрешающий список формы, а не копия профиля. `db` —
  только `[A-Za-z0-9_-]` (`name_field`) с пределом 255 байт
  (`MAX_DB_NAME_BYTES`; у правила движка для имени базы предела длины
  нет; собственный профиль оболочки ограничивает только базу
  **маршрутизации** — 64 байта); `transport` — только из закрытого набора
  `tcp` / `ws` / `ipc` (`TRANSPORTS`); `op_kind` — только имя операции
  из `[a-z0-9_:]` не длиннее 64 байт (`MAX_OP_KIND_BYTES`; двоеточие
  входит в собственные метки сервера, а самая длинная из них —
  `admin:apply_application_schema_migration`, 40 байт, поэтому строка
  реально идущей миграции печатается, а не заменяется). Вне ASCII-набора
  проходит только `user`: `db`, `transport` и `op_kind` отвергают любой
  не-ASCII символ, а управляющие символы, пробелы и слишком длинные
  значения отвергают все четыре поля; обратный слэш в `user` список
  пропускает, но вывод его экранирует. `op_kind` проверяет **форму**,
  а не принадлежность закрытому словарю: выдуманная, но формально
  корректная метка (`admin:bogus`) от повреждённого авторизованного
  источника пройдёт. `admin:list_activity` — реальная внутренняя метка,
  но строку собственного листинга сервер из вывода исключает, так что
  увидеть её в живом списке нельзя.
  Значение, допустимое по правилу (например, длинная шестнадцатеричная
  строка в роли имени пользователя), печатается как есть: правило
  гарантирует форму имени, а не отсутствие в нём чего-то, похожего на
  секрет; сама оболочка в эти поля ничего секретного не кладёт. Оболочка
  никогда не повторяет значения фильтров, литералы и тела сообщений, и ни
  в один поток не попадают ключ сессии, тег, токен или сырой текст
  сервера. В
  интерактивном REPL список выводится **одной строкой на запись**
  (`rows[0]`, `rows[1]`, ...), чтобы поздние метки оставались видны;
  предел в 200 символов относится к **значению** записи, а не к строке:
  значение обрезается до 200 символов, затем к нему добавляется `...`, а
  метка (`rows[i]`) печатается перед значением отдельным столбцом, так
  что общая длина строки больше 200 (усечение никогда не разрывает
  escape-последовательность `\uXXXX`). Пакетный вывод — по-прежнему
  ровно одна запись JSON Lines v1; в интерактивном режиме JSON Lines не
  печатается вообще.
* Коды выхода интерактивного отказа: интерактивно отклонённое
  подтверждение печатает `confirmation_required`, и сессия сохраняет код
  выхода 6 до конца своей жизни — обычные `exit` или EOF его не
  превращают в 0. Уточнение: «до конца жизни» означает именно нормальное
  завершение сессии; последующее прерывание (130) или ошибка чтения
  терминала дают свой код. В пакетном режиме тот же отказ тоже даёт код
  6.
* Отдельных видов для активных транзакций, курсоров и статистики форм в
  оболочке нет: они вне этой версии.

### Общее для всех трёх групп

* Все команды этого раздела требуют входа по SCRAM. В сессии с
  API-токеном они отклоняются на клиенте фиксированным сообщением
  `token_request_denied` с кодом 4, и ни один запрос не отправляется.
  Токенный набор — ровно это, и ничего больше: `connect`, `ping`,
  `db list`, `repo list`, `table list` / `table describe` /
  `table schema`, `index list`, `function list`, `data head`,
  `data count`. `repo describe`, `status`, `version show`,
  `data get`, `replication …`, весь раздел прав и весь раздел
  активности — вне набора. Быть в наборе оболочки **не** значит быть
  разрешённым сервером: у токена со ограниченной областью сервер
  откажет и в `db list` (код 4), потому что периметр сервера уже
  клиентского набора.
* Вывод — та же схема JSON Lines v1 (одна запись на команду), коды
  возврата — та же таблица.
* Ничего из этого раздела не печатает ключи, теги, токены и сырые
  сообщения сервера. Уточнение к более раннему правилу «шелл не
  печатает ничего, присланного узлом»: в таблицах этого раздела
  печатаются **авторизованные данные узла** (имена, режимы, метки,
  счётчики) — всегда с экранированием; гарантия «никогда» относится к
  необработанному тексту ошибок сервера, который не воспроизводится
  никогда. `node` в записи результата этого раздела остаётся `null`:
  оболочка не запрашивает скрытый статус узла.
* `group tree --db` — единственное место раздела, где значение фильтра
  возвращается в выводе самим шеллом (поле `database_filter`); это
  значение аргумента, а не данные сервера. В остальном шелл не повторяет
  значения фильтров.
* Исчерпанный после отправки локальный 60-секундный бюджет времени даёт
  `timeout_outcome_unknown` (код 7): исход неизвестен, и повторять
  команду автоматически нельзя. Бюджет проверяется **последним**, после
  проекции и после проверки вывода: если время вышло, код 7 приходит
  даже вместо отказа проекции, ошибки сканирования вывода или
  успешного результата. Точнее: проверка идёт после проекции и после
  проверки вывода (это не проверка сериализации и не проверка записи в
  поток). Проверка вывода в SCRAM-сессии ничего не сканирует (секретов в
  выводе этих команд нет), поэтому на практике бюджет решает только
  тайминг. Исчерпание бюджета на подготовительном чтении (`user list` для
  `chown`, дерево для `chgrp`) даёт код 7 и **не** говорит, был ли
  изменяющий запрос отправлен: при таком исходе команду нельзя повторять
  автоматически.
* Что покрыто тестами именно через оболочку: живые строки активности
  (собственная сессия, чужая живая сессия в `--all` и её отсутствие в
  `own`), полнота набора строк при обходе (`--limit 1` при двух живых
  сессиях возвращает обе строки, каждую один раз, и
  `rows_truncated: false`; маркер усечения, размер страницы и передача
  продолжения проверяются на подставном листинге в модульных тестах),
  отображение `<invalid>`, отказ токенной сессии без единого входящего
  соединения и отмену по неизвестному идентификатору
  (`already_finished`). Что **не** покрыто: строка запроса в живом
  листинге `activity requests` (листинг исключает собственный запрос, а
  припаркованный запрос в фикстуре оболочки детерминированно не
  создаётся), поэтому проекция строки запроса — включая все реальные
  метки операций — закреплена модульным тестом проекции, а проверка
  «ровно сколько запросов ушло на провод» недоступна: у фикстуры нет
  шва, наблюдающего исходящий `limit`. Отмена **припаркованного** запроса (ждёт
  блокировку записи) доказана на уровне обработчика, а не через
  оболочку: `tests/admin/activity/cancel_lock_tests.rs` держит блокировку
  старой транзакцией, ждёт `waiting_lock` в списке активности, отменяет
  запрос по выданной метке и проверяет типизированную ошибку `cancelled`,
  исчезновение строки и освобождение блокировки. Сервер внутри
  shell-фикстуры не отдаёт тесту свой `ShamirDb`, а клиент не умеет
  держать интерактивную транзакцию по проводу, поэтому в shell-тесте
  припаркованный запрос не создаётся детерминированно.

## Административный шелл: копии, версии, диагностика (R5c)

Пять тем: online-копия без остановки сервера, версии и штампы формата,
статус миграций прикладной схемы, поясняющие мануалы вместо запуска и
диагностика; отдельно — автономная проверка копии, которая командой
шелла не является. Все команды раздела — только SCRAM. Шелл остаётся
тонким клиентом: каталог данных он не читает, пути не разбирает и
серверную валидацию не переизобретает — каждая удалённая команда
становится либо одной admin-операцией существующего batch-протокола,
либо одним типизированным запросом
([`command.rs:1-14`](../../../crates/server/shamir-server/shell/commands/maintenance/command.rs#L1-L14)).

### Копии без остановки сервера

```sh
shamir-server shell --profile prod --secret-prompt -- backup start --to /srv/copies --yes
shamir-server shell --profile prod --secret-prompt -- backup status
shamir-server shell --profile prod --secret-prompt -- backup cancel --yes
```

* Три команды — оболочки над уже существующими admin-операциями
  online-копии `StartBackup` / `BackupStatus` / `CancelBackup`
  ([`MaintenanceCommand::op`](../../../crates/server/shamir-server/shell/commands/maintenance/command.rs#L64-L77)):
  новых серверных операций ради шелла нет. Семантика снимка, стадии,
  кооперативная отмена и границы копии — в
  [разделе «Online-копия: когда и как»](#online-копия-когда-и-как);
  здесь только поведение командной строки.
* Операции **node-local**: копируется `data_dir` этого узла, а не база.
  Batch-путь шелла при этом требует базу маршрутизации из профиля
  (`database:`): без неё — `usage`, код 1, до чтения секрета и до
  соединения
  ([`MaintenanceCommand::requires_routing_db`](../../../crates/server/shamir-server/shell/commands/maintenance/command.rs#L79-L88),
  [`session.rs:118-127`](../../../crates/server/shamir-server/shell/parser/session.rs#L118-L127)).
* Все три — admin-операции, то есть **только для суперпользователя**:
  серверный хендлер отвергает не-суперпользователя до запуска работы
  (грубый admin-гейт,
  [`execute.rs:176-194`](../../../crates/server/shamir-server/src/engine/db_handler/repo_admin/execute.rs#L176-L194)),
  а классификатор сессии даёт `StartBackup`/`CancelBackup` глобальный
  `Alter`, `BackupStatus` — глобальный `Read`
  ([`session.rs:582-587`](../../../crates/engine/shamir-engine-query/src/query/auth/session.rs#L582-L587)).
* `--to` — **серверный** абсолютный путь, переданный без изменений:
  шелл его не разрешает, не создаёт и не проверяет
  ([`MaintenanceCommand::execute`](../../../crates/server/shamir-server/shell/commands/maintenance/command.rs#L166)),
  и прикладывает к ответу фиксированную оговорку `destination_note`
  (`backup.rs:16`). Относительный путь отвергает **сервер** —
  типизированным `backup_invalid_destination` (код 5), а не оболочка
  ([`validate_destination`](../../../crates/server/shamir-server/src/boot/config/diagnostics/backup_online/schedule/destination.rs#L54)).
* `backup start` печатает `backup_id`, `destination` и
  `destination_note`. Второй прогон на узле отвергается
  `backup_already_running` (код 5): одна online-копия за раз.
* `backup status` — read-only и **не доходит до follower-гейта записи**.
  Поля результата: `backup_id` (`null`, если прогонов ещё не было),
  `destination`, `dest_dir`, `started_at_unix_ns`, `running`,
  `cancel_requested`, `stage`, `failed`, `failed_stage`,
  `excluded_repos`, `repos_in_snapshot`, `repos_created_during`, `repos`
  (по репозиторию: `repo`, `bytes_hashed`, `files_hashed`),
  `progress_stage`, `progress_started_at_unix_ns` и фиксированный
  `current_run_note`; при отказе прогона добавляется `failure_note`
  (текст серверной ошибки не воспроизводится) — `backup.rs:41-174`.
* `backup cancel` не несёт id и всегда адресует **текущий** прогон на
  узле (`current_run_note`, `backup.rs:12-13`). `cancelled: false` —
  отменять было нечего (прогон не идёт); это ответ, а не отказ.
* Подтверждение: `backup start` и `backup cancel` его требуют —
  интерактивно `yes`, в пакетном режиме `--yes`
  ([`dispatch.rs`](../../../crates/server/shamir-server/shell/parser/dispatch.rs#L550),
  вызов `ctx.confirm` внутри `maintenance_outcome`).
  Без подтверждения — `confirmation_required`, код 6, **до соединения и
  до отправки**: пакетный прогон без `--yes` не отправляет ничего.
* На follower `backup start` и `backup cancel` отвергаются
  `online_backup_follower_read_only` (код 5), а `backup status`
  по-прежнему отвечает
  ([`backup_handlers.rs:13-30`](../../../crates/server/shamir-server/src/engine/db_handler/handlers/backup_handlers.rs#L13-L30)).
* `backup schedule show` не поддержан: `unsupported_prerequisite`, код 6,
  ничего не отправляется
  ([`MaintenanceCommand::local`](../../../crates/server/shamir-server/shell/commands/maintenance/command.rs#L113)).
  Это типизированный отказ, а
  не заглушка-успех.

### Версии и миграции схемы

```sh
shamir-server shell --profile prod --secret-prompt -- version stamp
shamir-server shell --profile prod --secret-prompt -- version migrate
shamir-server shell --profile prod --secret-prompt -- schema-migration status --database-id "<ID>"
```

* `version stamp` — вид P1 status DTO: поля `server_version`,
  `format_epoch`, `stamp_outcome`, `role`, `node_id` — те же значения и
  типы, что у `version show`, потому что это буквально та же проекция и
  тот же запрос `Status` (`state.rs:727-737`,
  [`MaintenanceCommand::op`](../../../crates/server/shamir-server/shell/commands/maintenance/command.rs#L69)). Эпоху
  формата и исход штампа захватил лаунчер при загрузке существующим
  on-disk `inspect`; сам шелл каталог данных не читает и при запросе
  штамп не перечитывает. Оговорка `status` про чтение **сохранённых**
  переопределений durable-политики во время запроса относится и сюда:
  запрос тот же, поэтому типизированный код при неудаче этого чтения
  возможен. База маршрутизации не нужна. Запись результата успешна и её
  op — `Status`, поэтому поле `node` (наблюдённые роль и id) заполнено и
  здесь
  ([`observed_node`](../../../crates/server/shamir-server/shell/output.rs#L456-L469)).
* `version migrate` — **только объяснение**, шелл миграцию никогда не
  применяет: печатаются две ручные строки (`format-migrate --plan` и
  `format-migrate --yes --from-backup SNAPSHOT`, обе с `--config CONFIG`),
  см. [«Объяснения вместо запуска»](#объяснения-вместо-запуска).
* `schema-migration status --database-id ID` — ровно одна запись статуса
  одной базы. `--database-id` — Base58 `RecordId` (короткая форма
  допустима, предел — 22 байта), проверяется **локально, до любого
  dial**: пустой, слишком длинный или неразбираемый id — типизированный
  `invalid_record_id`, код 1, запрос не уходит
  ([`versions.rs:19-29`](../../../crates/server/shamir-server/shell/commands/maintenance/versions.rs#L19-L29)).
* Операция — существующая под-операция `ApplicationSchema` Status
  ([`MaintenanceCommand::op`](../../../crates/server/shamir-server/shell/commands/maintenance/command.rs#L70),
  `versions.rs:41-52`). Сервер авторизует `Read` на базе
  ([`authorize_database_read`](../../../crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/schema_migration.rs#L67-L80))
  и не проходит грубый admin-гейт: операция в списке исключений
  ([`admin.rs:632-658`](../../../crates/server/shamir-server/src/engine/db_handler/handlers/admin.rs#L632-L658)),
  то есть суперпользователь не обязателен, а право на базу обязательно.
* На follower статус отвергается `schema_migration_follower_read_only`
  (код 5): у follower нет согласованного вида журнала и схемы
  ([`execute.rs:199-215`](../../../crates/server/shamir-server/src/engine/db_handler/repo_admin/execute.rs#L199-L215)).
* Запись несёт `database_identity`, `protocol_version`,
  `last_applied_id` (`null`, если применённых нет), `last_applied_ordinal`,
  `last_applied_checksum` (hex или `null`), `checksum_validation`
  (`valid` / `missing_body` / `mismatch` / `unknown_version` /
  `not_checked`), `fence_generation`, `in_flight` (`null` или объект),
  `applied_count`, `applied` (по записи: `id`, `ordinal`, `checksum`) и
  серверный `error` из закрытого набора либо `null`; счётчики —
  десятичные строки
  ([`project`](../../../crates/server/shamir-server/shell/commands/maintenance/versions.rs#L76-L260)).
* Важное различие: `schema_migration_identity_mismatch` приходит с
  **нулевым** `last_applied_ordinal`, нулевым `fence_generation` и
  `applied_count` «0» — это **не** пустой журнал, а ответ «identity в
  запросе не та, что обслуживает база»; `database_identity` в записи —
  обслуживающая, не запрошенная
  ([`status_result`](../../../crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/schema_migration.rs#L85-L100)).
* Проекция строгая: неизвестный ключ или несовпадение формы —
  `server_error`, код 5, а не «данные».

### Объяснения вместо запуска

Семь команд шелла отвечают **локально** напечатанным мануалом и никогда
ничего не запускают; их список и текст — одна таблица `EXPLAINED`
([`EXPLAINED`](../../../crates/server/shamir-server/shell/commands/maintenance/explain.rs#L84-L151)).
Печатаются ровно такие строки:

```sh
shamir-server backup-verify SNAPSHOT
shamir-server --config CONFIG restore --from SNAPSHOT
shamir-server --config CONFIG backup --to DIR
shamir-server --config CONFIG format-migrate --plan
shamir-server --config CONFIG format-migrate --yes --from-backup SNAPSHOT
shamir-server --config CONFIG logical-export --db DATABASE --output PATH
shamir-server --config CONFIG logical-import --input PATH --target DIR
shamir-server --config CONFIG doctor
```

* `SNAPSHOT`, `DIR`, `PATH`, `DATABASE`, `REPO`, `TABLE`, `CONFIG` —
  литеральные плейсхолдеры: шелл не подставляет значения и **не
  повторяет** переданный argv (для `backup verify` печатается
  `backup-verify SNAPSHOT`, а сам аргумент проверяется только на
  непустоту,
  [`MaintenanceCommand::validate`](../../../crates/server/shamir-server/shell/commands/maintenance/command.rs#L137)).
* Каждая запись несёт `os_rights`, `destructive_effects`,
  `implementation_limits`, `caveats` и `executed: false`
  ([`Explained::payload`](../../../crates/server/shamir-server/shell/commands/maintenance/explain.rs#L167-L180)).
* Права ОС и предусловия: `backup verify` — чтение каталога снимка и
  каждого перечисленного файла; `backup restore` — владение каталогом
  данных и его родителем, сервер остановлен; `backup snapshot` — запись в
  `DIR` и владение каталогом данных; `version migrate` — владение
  каталогом данных, сервер остановлен; `export logical` — владение
  каталогом данных, сервер остановлен (только офлайн); `import logical` —
  запись в родителя `DIR`, сам `DIR` отсутствует или пуст и никем не
  занят; `diag integrity` — владение каталогом данных, сервер
  остановлен, потому что проверка открывает каталог напрямую.
* Разрушительные эффекты: `backup restore` подменяет живой каталог
  данных, оставляя текущий рядом как `.pre_restore_backup_<timestamp>`
  (сохраняется, не удаляется); `doctor --apply` чинит нездоровые таблицы
  на месте, пересоздавая все их индексы; `format-migrate --yes`
  перезаписывает on-disk штампы формата, а без `--yes` только печатает
  план; `logical-import` создаёт **новый** каталог данных и никогда не
  заменяет существующий.
* Шелл не выполняет ничего и не открывает ничего: эти ответы строятся до
  соединения, поэтому им не нужны ни профиль, ни секрет, ни dial
  ([`dispatch.rs:534-564`](../../../crates/server/shamir-server/shell/parser/dispatch.rs#L534-L564)).
* Печатные флаги закреплены тестом в двух половинах. Реальная:
  `logical-export`, `logical-import` и `format-migrate` разбирают токены
  своих мануалов настоящими `clap::Args` сервера, включая флаги, которых
  нет в командной строке. Исходная: объявления `backup-verify`,
  `backup`, `restore`, `doctor` и их флагов ищутся в тексте
  `src/main.rs`, где они объявлены inline, а не как `clap::Args`. Первая
  половина ловит переименование, удаление и перенос флага внутри
  `clap::Args`; вторая не отличает флаг, **перемещённый** между двумя
  inline-подкомандами
  ([`printed_flags_exist_in_the_server_argument_parser`](../../../crates/server/shamir-server/tests/admin/shell/maintenance/explain.rs#L358)).
  `--force` не печатается никогда. Ответ несёт ровно `manual`
  (напечатанные строки), `os_rights`, `destructive_effects`,
  `implementation_limits`, `caveats` и `executed: false` — и ничего
  больше.

### backup-verify: проверка копии

Это команда бинарника, а не шелла:

```sh
shamir-server backup-verify SNAPSHOT
```

* Диспетчеризация — **до загрузки конфига** и до `Cli::parse()`: команда
  должна быть первым аргументом, `--config` не нужен
  ([`run_main`](../../../crates/server/shamir-server/src/main.rs#L306-L334)).
  Аргумент ровно один; иначе — фиксированная строка
  `usage: shamir-server backup-verify SNAPSHOT` и код 1, причём argv не
  эхом (`main.rs:319-333`).
* Ничего не загружается: сервер не стартует, каталог данных не
  открывается, сеть не используется. Команда работает с правами ОС
  вызывающего пользователя и только читает.
* Она проверяет `manifest.json` и **хеширует каждый перечисленный файл**,
  поэтому дешёвой не является: время пропорционально размеру снимка.
* Вывод — строки `label` и значение: `result` (`verified` / `failed`),
  `kind` (`offline` / `online`), `files_checked`, `total_bytes`,
  необязательный `format_epoch` и фиксированный `note`
  ([`backup_verify`](../../../crates/server/shamir-server/src/main.rs#L396-L422)).
* При отказе: `result failed`, `error_class`, `note`. Класс берётся из
  закрытого набора — `manifest_missing`, `manifest_invalid`,
  `checksum_mismatch`, `unmanifested_file`, `unsafe_manifest_path`,
  `unsupported_manifest_format_version`, `managed_snapshot`, `io`,
  `source_missing`, `source_not_dir`, `cache_manifest_path`,
  `duplicate_manifest_entry`, `wal_sequence_gap`; любой неназванный
  вариант сообщается как `corrupt` (`main.rs:442-460`).
* Коды: 0 — снимок проверен, 1 — ошибка использования, 5 — снимок не
  прошёл проверку. Текст сообщений библиотеки не печатается никогда:
  только класс.

### Диагностика

```sh
shamir-server shell --profile prod --secret-prompt -- diag wal
shamir-server shell --profile prod --secret-prompt -- diag durability main
shamir-server shell --profile prod --secret-prompt -- diag integrity
```

* `diag wal` — телеметрия WAL по репозиториям из того же P1 status DTO
  (запрос `Status`): `backend_mem`, `watermark_w`,
  `recoverable_through`, `pending_versions`, первый отложенный
  (`first_pending_version`, `first_pending_age_ns`) и счётчики
  синхронизации (`sync_now_calls`, `sync_rounds`, `sync_coalesced`,
  `sync_errors`, `sync_duration_ns`) — `diag.rs:24-60`.
  `pending_versions` — **граница**, а не точное число незаписанных
  записей; это фиксированная оговорка `caveat`. Строка `source` говорит
  прямо: это экспорт сервера, а шелл файл WAL не открывает. Нужен
  суперпользователь (запрос `Status`,
  [`operator_state.rs:536-541`](../../../crates/server/shamir-server/src/engine/db_handler/repo_admin/operator_state.rs#L536-L541));
  база маршрутизации не нужна.
* `diag durability REPO` — проекция существующей admin-операции
  `DescribeRepo`: поле `durability` — действующая политика с учётом
  настроенного переопределения, `durability_override` — записанная в
  каталоге, плюс фиксированная оговорка `caveat` (`diag.rs:63-78`).
  Нужны суперпользователь (admin-гейт) и `Read` на
  репозитории
  ([`admin_repo_durability.rs:188-197`](../../../crates/engine/shamir-db/src/shamir_db/execute/repo_admin/admin_repo_durability.rs#L188-L197));
  только SCRAM; база маршрутизации из профиля нужна.
* `diag integrity` — только объяснение (`doctor`). Проверка офлайн по
  построению: скан живого хранилища может увидеть переходные
  miscount'ы, поэтому команда печатает мануал, а не запускает проверку.

### Коды возврата и токенные сессии

* Все команды раздела — только SCRAM. В сессии с API-токеном каждая
  отвергается на клиенте `token_request_denied` (код 4) до отправки: на
  провод не уходит ничего
  ([`dispatch.rs:30-86`](../../../crates/server/shamir-server/shell/parser/dispatch.rs#L30-L86)).
* 0 — успех или напечатанный мануал; 1 — ошибка использования (включая
  `invalid_record_id`); 4 — отказ токена или прав; 5 — серверный отказ
  (`backup_already_running`, `backup_invalid_destination`,
  `backup_io_error`, `backup_unavailable`,
  `online_backup_follower_read_only`,
  `schema_migration_follower_read_only`, `not_found`, …); 6 —
  неподтверждённая разрушающая команда или `unsupported_prerequisite`;
  7 — таймаут после отправки: исход неизвестен, повторять автоматически
  нельзя; 130 — прерывание.
* Ни один отказ не выдаётся за успех: таблица кодов не отображает ошибку
  в 0
  ([`ShellError::exit`](../../../crates/server/shamir-server/shell/output.rs#L316-L370)).

### Что покрыто тестами, а что нет

Приёмочный набор для команд раздела лежит в
`crates/server/shamir-server/tests/admin/shell/maintenance/`
(`backup.rs`, `versions.rs`, `explain.rs`, `verify.rs`, `diag.rs`,
`policy.rs`, `migrations.rs`) и объявлен в
[`mod.rs`](../../../crates/server/shamir-server/tests/admin/shell/mod.rs);
он входит в интеграционную цель `admin`. Зелёный CI этой главой не
обещан: имена тестов приведены, чтобы их можно было найти, а
утверждения раздела сверены с кодом, а не с прогоном.

Покрыто — online-копии (`maintenance/backup.rs`):
`backup_status_reports_no_running_copy`,
`backup_start_without_yes_is_refused_before_sending`,
`backup_start_with_yes_starts_a_copy`,
`backup_cancel_without_yes_is_refused`,
`backup_cancel_with_yes_answers_the_current_run`,
`relative_destination_is_refused_by_the_server_unmodified` (относительный
`--to` отвергает сервер, а не шелл),
`follower_refuses_start_and_cancel`,
`token_session_is_refused_with_no_request` (все пятнадцать команд
раздела под токенной сессией: код 4, ничего не отправлено).

Покрыто — версии и статус миграций (`maintenance/versions.rs`,
`maintenance/migrations.rs`):
`version_stamp_reports_the_boot_captured_stamp`,
`malformed_database_id_is_refused_locally`,
`identity_mismatch_is_not_an_empty_journal`,
`follower_refuses_the_status_read`.
`schema_migration_status_reports_an_applied_migration` применяет одну
миграцию по настоящему проводу и затем проверяет через шелл
`last_applied_id`, ненулевой `last_applied_ordinal`, `applied_count`,
единственную запись в `applied` с её hex-`checksum`,
`last_applied_checksum`, равный ей, и `in_flight` — null или объект.
Предел остаётся честным: применение требует SDK-клиента в тесте (сам
шелл миграцию не применяет), а отказ сервера в применении провалит
тест, а не пропустит его.

Покрыто — объяснения (`maintenance/explain.rs`):
`explained_commands_print_the_documented_sections` (точные печатные
строки), `explained_commands_change_nothing_on_disk` (sha256 каждого
обычного файла каталога данных до и после),
`schedule_show_is_unsupported_not_a_placeholder_success`,
`explained_command_options_are_refused`,
`printed_flags_exist_in_the_server_argument_parser`.

Покрыто — `backup-verify` (`maintenance/verify.rs`):
`valid_v1_snapshot_verifies`,
`corrupted_content_reports_checksum_mismatch`,
`missing_manifest_reports_manifest_missing`,
`unlisted_file_reports_unmanifested_file`,
`unsafe_manifest_path_is_refused`,
`malformed_manifest_reports_manifest_invalid`,
`missing_argument_is_a_usage_error`,
`backup_verify_rejects_a_flag_token`,
`extra_argument_is_a_usage_error`,
`pre_existing_dispatch_still_requires_config`.

Покрыто — диагностика и справка (`maintenance/diag.rs`,
`maintenance/policy.rs`): `diag_wal_reports_the_status_wal_projection`
(голден проекции против загруженного сервера: одиннадцать полей `wal`,
`source`, `caveat`), `diag_durability_reports_the_describe_repo_projection`
(проекция `DescribeRepo` плюс `caveat`) и
`help_lists_every_command_family` (каждое семейство команд раздела
присутствует в `help`, `--force` — нет).

Покрыто — только юнит-тестами (`shell/commands/maintenance/tests/`):
`backup_status_never_reflects_a_hostile_server_value`,
`diag_wal_never_reflects_a_hostile_repository_name` и
`schema_migration_status_never_reflects_a_hostile_server_value`.
Управляющие символы, ANSI-escape, bidi и длинный текст с канарейкой не
отражаются; последний тест заодно закрепляет ровно одиннадцать ключей
проекции статуса миграций. Живой тест `diag wal` закрепляет одиннадцать
имён полей проекции; отдельного общего помощника у шелла и `state.rs` нет
(проекционные помощники `state.rs` приватны, расширять их запрещено).

Не покрыто — прямо: (a) в харнессе нет fake-server seam, поэтому защита
от «злонамеренного пира» — это юнит-тест проекций, а не живой враждебный
сервер; (b) `explained_commands_change_nothing_on_disk` сравнивает
СОДЕРЖИМОЕ файлов (в Windows файл, который запущенный сервер фикстуры
держит под блокировкой диапазона байт и потому не читается, сверяется по
длине): он доказывает отсутствие постоянной записи, а не «ни один файл не
открыт»; (c) интерактивный человеческий рендер (он обрезает
длинные ячейки на 200 символах) для длинных текстов объяснений не
проверяется; (d) начатую копию тест завершает через `backup cancel` —
естественное завершение оракула не имеет; (e) исходная половина флагового
пина не отличает флаг, **перемещённый** между двумя inline-подкомандами
(реальная половина покрывает три `clap::Args`-подкоманды).

## Поиск и отмена зависшего запроса

Откройте **второе аутентифицированное SCRAM-соединение**, чтобы надёжно отправить
команду отмены, пока целевой запрос занимает первое. Соединение не является
монопольным: на нём могут выполняться параллельные запросы (до 32 in-flight
на соединение), но собственный лимит клиента/соединения или занятые слоты могут
задержать управляющий запрос. `CancelRequest` не занимает слот запроса,
не расходует байтовый бюджет ответа и не подпадает под заданный вызывающим
лимит результата.
При этом [транспортная диспетчеризация](../../../crates/net/shamir-connect/src/server/infra/dispatch.rs#L139-L152)
по-прежнему проверяет частоту запросов аутентифицированного сеанса до
вызова обработчика и при превышении возвращает `rate_limited`.
Для своих запросов достаточно аутентификации; для просмотра и отмены
запросов других пользователей нужно право `ActivityView:Manage` (для
просмотра используйте `ActivityScope::All`).

На управляющем соединении запросите список активных запросов. Сверьте базу,
сеанс, вид операции и время выполнения, прежде чем выбрать строку. Если
нужной строки нет на первой странице, передайте `page.continuation` в
следующий `ListActivityReq`. Для отмены берите серверный `request_exec_id`,
а не клиентский `rid`.

```rust
use shamir_client::{Client, ClientError};
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityPage, ActivityRow, ActivityScope, CancelOutcome, ListActivityReq,
};

async fn list_own_requests(control: &Client, db: &str) -> Result<ActivityPage, ClientError> {
    control
        .list_activity(
            db,
            ListActivityReq {
                scope: ActivityScope::Own,
                kind: ActivityKind::Requests,
                limit: Some(64),
                continuation: None,
            },
        )
        .await
}

// selected — проверенная оператором строка из page.rows.
async fn cancel_selected_request(
    control: &Client,
    db: &str,
    selected: &ActivityRow,
) -> Result<Option<CancelOutcome>, ClientError> {
    match selected {
        ActivityRow::Request { request_exec_id, .. } => {
            let id = *request_exec_id;
            control.cancel_request(db, id).await.map(Some)
        }
        _ => Ok(None),
    }
}
```

| Исход (`CancelRequest`, `CancelTx`, `CancelCursorAdmin`) | Что означает и что делать |
|---|---|
| `cancelled` | Отмена принята. `CancelRequest`: целевой запрос остановится в контрольной точке или проснётся из поддерживаемого ожидания, его клиент получит обычную ошибку с кодом `cancelled`; ответ не означает, что запрос уже остановился. `CancelTx`: транзакция простаивала, дескриптор удалён из реестра; задача освобождения блокировок запущена и ожидается внутри вызова отмены. Исход означает, что освобождение было ожидаемо, но не гарантирует его завершение: сбой задачи журналируется (`error` при panic, `warn` при отмене из-за остановки runtime), но только пока вызов отмены ещё ожидает задачу: если соединение вызывающего уже разорвано, задача выполняется отсоединённо и этот вызов её сбой не запишет; подготовленные изменения не публикуются. `CancelCursorAdmin`: курсор удалён из реестра; закреплённый снимок освобождается, когда исчезает последняя ссылка на курсор (уже начавшийся `FetchNext` владельца, получивший курсор до удаления, может вернуть страницу и держать снимок до своего возврата). |
| `already_finished` | Цель уже завершилась, была отменена ранее или идентификатор неизвестен. Для `CancelRequest` проверьте ответ исходного запроса; этот исход не говорит, чем тот завершился. Для `CancelTx` и `CancelCursorAdmin` дескриптор уже не существует либо не имеет строки в списке активности; для `CancelTx` — также если проверенную транзакцию успела заменить другая под тем же дескриптором, а для `CancelCursorAdmin` — если проверенный курсор успели заменить другим под тем же идентификатором (замена не затрагивается). |
| `already_committing` | Только `CancelRequest` и `CancelTx`. `CancelRequest`: запрос уже защищён от отмены — он вошёл в фазу фиксации или дошёл до запуска `Call` либо записывающей административной операции. `CancelTx`: на момент проверки транзакция уже начала фиксацию; дождитесь нормального исхода фиксации у владельца (если фиксация успела завершиться раньше отмены, ответ будет `already_finished`). Для курсоров не встречается. |
| `not_cancellable` | `CancelRequest`: запрос зарегистрирован без контрольных точек отмены — это `TxBegin` (`tx_open`) и `TxExecute`; `CancelRequest` его не остановит. `TxBegin` открывает транзакцию, но сам не отменяем. `CancelTx`: контекст транзакции занят запросом владельца (`TxExecute`) либо чтением фиксации; отмена ничего не меняет, не ждёт этот запрос и не прерывает его — дождитесь его возврата и повторите `CancelTx`. `CancelCursorAdmin` этого исхода не возвращает. |

Отмена кооперативная: сервер проверяет её в контрольных точках и может
разбудить запрос в следующих ожиданиях, пока запрос не защищён от отмены:

| Статус | Какое ожидание можно прервать |
|---|---|
| `waiting_lock` | Ожидание блокировки строки, мьютекса записи в уникальный индекс до фиксации или мьютекса воспроизведения журнала (drainer). |
| `waiting_visibility` | Ожидание подтверждения видимости версии при чтении. |
| `waiting_budget` | Ожидание свободной ёмкости байтового бюджета. |

В drainer прерывается только ожидание мьютекса. Если drainer уже получил
мьютекс и начал проход воспроизведения с изменением данных, он сначала
завершит этот проход: его асинхронная операция не прерывается. Правила
ожидания видимости и байтового бюджета не изменились.

Само наличие `Call` или записывающей административной операции в пакете
не защищает запрос от отмены при допуске. Непосредственно перед первым
фактическим запуском такой операции сервер выполняет тот же атомарный
переход (`CAS`) в `committing`, что и при входе в фазу долговечной фиксации.
Если отмена выиграла гонку, операция не начнётся; если выиграл переход в
`committing`, отмена вернёт `already_committing`. После перехода отмена не
включается заново до конца пакета, даже между операциями или фиксациями.
Уже выполняющийся вызов WASM принудительно не прерывается. Бюджет инструкций
(fuel) ограничивает код гостя, но не ограничивает время ввода-вывода хоста.

`CancelRequest` охватывает только выполняющиеся пакеты. Открытые
интерактивные транзакции и курсоры отменяются собственными операциями
`CancelTx` и `CancelCursorAdmin`, описанными ниже. У отдельной команды
протокола `TxCommit` по-прежнему нет собственного дескриптора в реестре
запросов, поэтому `CancelRequest` никогда не отменяет фиксацию.

На реплике (follower) применение потока репликации не регистрируется в
списке запросов и не отменяется. Незарегистрированный ID даст
`already_finished`, а не `not_cancellable`. Свои обычные пакеты на реплике
можно отменять по тем же правилам.

### Отмена открытой транзакции и курсора

**Отдельные операции.** `CancelTx` отменяет открытую интерактивную
транзакцию по её дескриптору `tx_handle`, а `CancelCursorAdmin` — курсор
по `cursor_id`. Обе, как и `CancelRequest`, отправляются единственной
операцией самостоятельного нетранзакционного пакета: они не занимают слот
выполняющегося запроса сеанса, не расходуют байтовый бюджет ответа и не
подпадают под заданный вызывающим лимит размера результата. Сеансовая
wire-команда `DbRequest::CancelCursor` не изменилась и закрывает только
собственный курсор вызывающего.

**Второе соединение.** Используйте отдельное SCRAM-соединение как управляющий
канал, чтобы избежать транспортного backpressure, если все 32 in-flight
разрешения заняты медленными запросами: читатель не сможет принять отмену,
пока одно из них не освободится. Сам dispatch конкурентный — на каждый
допущенный запрос создаётся отдельная задача, и простаивающий запрос сам по
себе не блокирует отмену на том же соединении. На одном соединении могут быть
до 32 одновременно выполняющихся запросов; это лимит in-flight запросов, а
не соединений. Общий лимит активных соединений по умолчанию 1000
(`security.connection.max_active_connections`). Для `CancelTx` нужен
`tx_handle`, который владелец получает в `TxOpened`: простаивающую транзакцию
можно отменить с того же или другого соединения, но транзакцию с выполняющимся
`TxExecute` `CancelTx` не прерывает и отвечает `not_cancellable`. `CancelRequest`
отменяет лишь поддерживаемые cancellable-запросы; `TxBegin` (`tx_open`) и
`TxExecute` зарегистрированы без контрольных точек отмены. `TxExecute` оставлен
неотменяемым: текущая регистрация запроса не даёт ему безопасных точек отмены,
а расширение этого поведения потребовало бы менять модель регистрации и
проверять корректность прерывания плана; здесь это не делается. Источники:
`crates/server/shamir-server/src/boot/config.rs:295-327`,
`crates/foundation/shamir-tunables/src/lib.rs:46-53`,
`crates/server/shamir-server/src/engine/server/server_launcher/support/helpers.rs:164-185`,
`crates/server/shamir-server/src/engine/server/server_launcher/http/launcher.rs:402`,
`crates/client/shamir-client/src/client/api/request.rs:815-875`.

Порядок для управляющего соединения и работы, выполняющейся на другом:

1. Откройте второе аутентифицированное SCRAM-соединение.
2. Вызовите `list_activity` с `ActivityKind::Transactions` или
   `ActivityKind::Cursors`; для чужих строк нужен `ActivityScope::All`.
3. Возьмите `tx_handle` (`ActivityRow::Transaction`) или `cursor_id`
   (`ActivityRow::Cursor`) из строки.
4. Отправьте `cancel_tx` или `cancel_cursor_admin` и разберите
   `CancelOutcome`. Владелец может использовать полученный при создании
   handle/id и вызвать отмену со своего соединения без просмотра активности.

```rust
use shamir_client::{Client, ClientError};
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityPage, ActivityRow, ActivityScope, CancelOutcome, ListActivityReq,
};
use shamir_query_types::wire::CursorId;

async fn list_transactions(control: &Client, db: &str) -> Result<ActivityPage, ClientError> {
    control
        .list_activity(
            db,
            ListActivityReq {
                scope: ActivityScope::Own,
                kind: ActivityKind::Transactions,
                limit: Some(64),
                continuation: None,
            },
        )
        .await
}

// selected — проверенная оператором строка из page.rows.
async fn cancel_selected_tx(
    control: &Client,
    db: &str,
    selected: &ActivityRow,
) -> Result<Option<CancelOutcome>, ClientError> {
    match selected {
        ActivityRow::Transaction { tx_handle, .. } => {
            control.cancel_tx(db, *tx_handle).await.map(Some)
        }
        _ => Ok(None),
    }
}

async fn cancel_selected_cursor(
    control: &Client,
    db: &str,
    selected: &ActivityRow,
) -> Result<Option<CancelOutcome>, ClientError> {
    match selected {
        ActivityRow::Cursor { cursor_id, .. } => {
            control
                .cancel_cursor_admin(db, CursorId(*cursor_id))
                .await
                .map(Some)
        }
        _ => Ok(None),
    }
}
```

| Состояние строки `Transactions` | Что означает |
|---|---|
| `active` | Транзакция зарегистрирована и ещё не вошла в фиксацию. Строка не показывает, выполняется ли на ней запрос: пока контекст занят, `CancelTx` ответит `not_cancellable`; простаивающую транзакцию можно отменить. `TxBegin` (`tx_open`) отображается как строка `Requests`, а не `Transactions`; список `Transactions` показывает только зарегистрированные интерактивные транзакции, открытые через `TxBegin`. |
| `awaiting_commit` | Это наблюдение текущего состояния: фиксация началась, и транзакция остаётся в списке до конца фиксации. Оно не гарантирует исход более позднего `CancelTx`: при гонке отмена может увидеть `Committing` и ответить `already_committing` либо увидеть уже удалённую запись и ответить `already_finished`; фиксация сообщит владельцу свой обычный исход. |
| `expired_pending_reap` | Транзакция превысила абсолютный срок жизни или idle-TTL и ещё не собрана reaper'ом; состояние вычисляется при чтении, а reaper удаляет строку на ближайшем тике, только если при повторной проверке она всё ещё просрочена. Выполняющийся запрос не считается idle. Успешный `TxExecute` обновляет lease, поэтому даже чтение может обновить idle-expired транзакцию до её сборки reaper'ом; истечение idle также может быть снято при захвате lease (`TxRegistry::acquire_owned` обновляет активность). |

**Что видит владелец.** После успешного `CancelTx` дескриптора больше не
существует: следующий `TxExecute`, `TxCommit` или `TxRollback` с ним
отклоняется типизированным кодом `tx_not_found` — тем же, что даёт
откаченный или собранный reaper'ом дескриптор. Ничего из подготовленного
транзакцией не становится видимым никакому читателю, включая свежий
снимок. Успешная отмена удаляет транзакцию из реестра и отбрасывает её; задача
освобождения блокировок запускается отдельно и ожидается внутри вызова
отмены. Исход `cancelled` означает, что дескриптор удалён и ожидание
освобождения выполнено, но не гарантирует успешное освобождение: сбой
задачи журналируется (`error` при panic, `warn` при отмене из-за остановки
runtime), пока вызов отмены ещё ожидает задачу; при уже разорванном
соединении вызывающего задача выполняется отсоединённо и её сбой этим
вызовом не записывается. Поэтому в обычном пути блокировки освобождаются, но при таком
сбое этого обещать нельзя. Вторая транзакция, ждавшая на этих блокировках,
продолжает работу после их освобождения. Слот транзакции сеанса освобождается при удалении записи из реестра,
а закреплённый MVCC-снимок — когда исчезает последняя ссылка на
транзакцию: тот же сеанс может сразу начать новую транзакцию, а снимок
может пережить слот, если на транзакцию остаётся другая ссылка. Отменённая строка исчезает из следующего
списка.

**Какой конец транзакции снимает её блокировки.** Откат владельцем
(`TxRollback`), сбор reaper'ом по idle-TTL или абсолютному дедлайну,
закрытие сеанса, отказ `tx_too_large` и исчерпание лимитов выполнения
освобождают пессимистические блокировки записей, которые транзакция ещё
удерживала; фиксация снимает их сама — и на успешном пути, и на каждом
пути отказа конвейера фиксации. Освобождение привязано к самому
репозиторию, разрешённому при открытии транзакции, поэтому `RenameDb` и
`RenameRepo`, выполненные, пока транзакция простаивала, его не теряют:
блокировки снимаются в том хранилище, где они взяты, даже если база или
репозиторий теперь называются иначе. Освобождение ставит фоновая задача,
которую запускает исчезновение последней ссылки на транзакцию: если в этот
момент на транзакции ещё выполняется запрос владельца (он держит
собственную ссылку и ждёт блокировку), блокировки снимаются сразу после
возврата этого запроса. Повторное освобождение того же дескриптора
безвредно: снимаются только блокировки этой транзакции, поэтому поздно
пришедшее освобождение не отнимает запись у транзакции, захватившей её
позже. Найти держателя блокировки можно в списке `Transactions` (состояние
`active` или `expired_pending_reap`); чтобы снять блокировку сразу, не
дожидаясь reaper'а, отмените транзакцию через `CancelTx`.

**Два случая, когда освобождения не будет.** Отказ фиксации, который
движок возвращает до входа в конвейер (например `access_denied`, когда у
сеанса отозвали `Write` на саму базу, пока транзакция простаивала),
освобождает блокировки сам — но если к этому моменту база или репозиторий
уже удалены и хранилище недостижимо, снимать нечего и нечем. И транзакция,
чьё последнее ссылочное значение исчезает вне живого runtime (например
сервер снят вместе с runtime, а `ShamirDb` пережил его), блокировки не
освобождает: освобождение — асинхронная задача, которой некому работать;
этот случай пишется в журнал на уровне `warn` с именем базы и репозитория,
и блокировки уходят вместе с процессом.

Известный предел пути фиксации: контекст транзакции изымается из реестра в
`tx_commit_with_options` (`crates/server/shamir-server/src/engine/db_handler/repo_admin/tx_handlers.rs:538`)
до начала конвейера фиксации. Функция `tx_commit_resolved`
(`crates/engine/shamir-db/src/shamir_db/execute/core/db_tx.rs:387-425`)
разрешает базу и репозиторий по имени, ожидает авторизацию и при отказе
освобождает блокировки встроенно. Из чтения кода следуют два случая (не наблюдались при запуске).
Первый — отказ авторизации: блокировки освобождаются встроенно, и если
соединение разорвано между изъятием контекста и этим освобождением,
контекст может быть отброшен до освобождения всех ключей —
`InteractiveTx::drop` уже не владеет контекстом. Второй — сбой разрешения
по имени (например, репозиторий переименован, а фиксация идёт под прежним
именем): он возвращается ДО освобождения, и блокировки могут остаться
без какого-либо разрыва соединения. Это известные ограничения, не
исправление; общая причина — у контекста нет владельца-уборщика между
изъятием и началом конвейера.

Ответ `cancelled` относится только к простаивающей транзакции и ничего
не подтверждает про выполняющийся запрос владельца: при таком запросе
отмена отвечает `not_cancellable` и не возвращает `cancelled`.

**Курсоры.** У `CancelCursorAdmin` возможны только `cancelled` и
`already_finished`; `not_cancellable` и `already_committing` к курсорам
не применяются. Отменённый курсор удаляется из реестра, а его
закреплённый снимок освобождается обычным путём drop, когда исчезает
последняя ссылка на курсор: `FetchNext`, уже получивший курсор до
удаления, может вернуть страницу и держать снимок до своего возврата.
Новый `FetchNext` владельца, начатый после удаления, получает
типизированный код `cursor_not_found`:
явное закрытие не оставляет tombstone, поэтому это не `cursor_expired`.
Сеансовая команда владельца `DbRequest::CancelCursor` на уже закрытый
или неизвестный курсор ошибкой не считается — сервер идемпотентно
отвечает `CursorClosed`, и клиентский `CursorStream::close()` возвращает
`Ok(())`. Курсор, который ни разу не появлялся в списке (у него нет
строки активности), не привязывается к пользователю и даёт
`already_finished`.

**Чего отмена не делает.** Транзакцию, уже вошедшую в фазу фиксации
(строка в состоянии `awaiting_commit`), отменить нельзя: `CancelTx`
вернёт `already_committing` (или `already_finished`, если фиксация успела
завершиться) и ждать фиксацию не будет — она завершится и
сообщит владельцу свой обычный исход. Если на транзакции выполняется
`TxExecute` владельца, `CancelTx` вернёт `not_cancellable` и ничего не
изменит: она
не ждёт чужой выполняющийся запрос и не прерывает его. Сам этот вызов
тоже неотменяем — `CancelRequest` по его строке отвечает
`not_cancellable`, потому что `TxExecute` регистрируется без контрольных
точек отмены; дождитесь его возврата и повторите `CancelTx`. Вызов
завершается на собственной копии транзакции, его подготовленные записи
остаются в транзакции, и следующая успешная `CancelTx` отбрасывает их
вместе с ней.

**Права.** Отмена работы собственного пользователя (любого его сеанса)
требует только аутентификации; отмена работы другого пользователя — права
`ActivityView:Manage`, которое проверяется обычным путём авторизации, а не
через superuser-гейт: операции отмены исключены только из грубого
административного гейта, и это исключение само по себе ничего не
разрешает. Порядок проверки такой: неизвестный или уже закрытый
дескриптор отвечает `already_finished` до проверки прав, а существующая
транзакция или курсор другого пользователя — `permission_denied`, если у
вызывающего нет `Manage`. Эти два ответа различимы, поэтому
непривилегированный вызывающий узнаёт, указывает ли дескриптор на живую
работу другого пользователя; чья это работа и что она делает, ответ не
раскрывает. Транзакционные и курсорные дескрипторы — не секреты: владелец получает
`tx_handle` в `TxOpened`, а `cursor_id` — в `CursorPage`; активность — не единственный возможный источник их знания. Дескрипторы транзакций — не секреты: это не секретные счётчики на репозиторий, и клиент, видевший предыдущий `TxOpened`, может знать форму счётчика и угадывать следующий. У курсора `CursorPage` может содержать `cursor_id`, которого нет в зеркале активности: если все строки помещаются на первую страницу, `CreateCursor` возвращает id, не регистрируя курсор. Кроме того, `AlreadyFinished` от `CancelCursorAdmin` может означать, что курсор заменён под тем же id: удаление с проверкой идентичности возвращает `AlreadyFinished` при несовпадении строки активности. Для чужой работы дескриптор можно узнать из активности с правом `ActivityView:Manage`; однако знание дескриптора не даёт права отмены. Метаданные активности курсора фиксируются при `CreateCursor`.


Если аудит включён, сервер по принципу «лучшее из возможного» пытается
записать в аудит отказы `already_committing` и `not_cancellable` всех трёх
операций, а для `cancel_tx` и `cancel_cursor_admin` — успешную отмену
(`cancelled`) работы другого пользователя; исход `already_finished` и отказ
`permission_denied` не аудируются. Успешная отмена своей работы не
аудируется, а успешный `CancelRequest` не аудируется вовсе — ни свой, ни
чужой. Имена операций в записи — `cancel_tx` и `cancel_cursor_admin`; для
отказов самой `cancel_request` используется `cancel_request`. Детали
содержат только целевой дескриптор или id и исход — никогда
пользовательский текст, литералы или материал токена. Очередь ограничена
256 незавершёнными заданиями глобально и 32 заданиями на principal; отказ
в приёме не задерживает отмену: возвращается тот же типизированный
`CancelOutcome`, а задание аудита отбрасывается и учитывается. Если
трёхсекундный drain истекает, отбрасывается только blocking audit job,
который ещё не начал запись: `discard_queued` пытается прервать
отслеживаемые задания, и прервать можно лишь то, чьё blocking-замыкание
ещё не начало выполняться. Уже запущенное задание, в том числе ожидающее
persistence gate писателя, не прерывается.

**Метрика.** Каждый `CancelOutcome`, который вернула любая из трёх
операций отмены, увеличивает ровно одну серию счётчика
`shamir_cancellations_total{result="cancelled|already_finished|already_committing|not_cancellable"}`.
Отказ `permission_denied` и ошибки допуска и проверки запроса не считаются:
счётчик увеличивает только возвращённый `CancelOutcome`.
У счётчика ровно четыре фиксированных значения `result` и нет меток
пользователя или сеанса; он ведётся на экземпляр сервера, экспортируется
на том же маршруте `/metrics`, что и гейджи `shamir_active_*`, и
монотонен в течение жизни сервера.

## Статистика по формам запросов

Движок считает не тексты запросов, а **формы**: одна форма — один
канонический ключ из 128 бит, в который свёрнуто то, что определяет
работу операции, и ничего из того, что определяет её данные. В ответе
ключ приходит как 32 шестнадцатеричных символа нижнего регистра и три
счётчика: `executions`, `total_nanos`, `errors`. Ни имени таблицы, ни
имени поля, ни значения фильтра, ни пользователя в ответе нет и быть не
может: ключ — хеш.

### Что учитывается и что нет

Учитывается исполнение **одной операции над данными**: только `Read`,
`Insert`, `Update`, `Delete` и `Set`. `Call`, административные операции и
все остальные виды в статистику не попадают вовсе — учёт стоит там же,
где снимается длительность операции над данными.

Операция, не дошедшая до своего исполнительного плеча с уже разрешённой
таблицей и интернером, формой не становится: неизвестная таблица, отказ в
правах, ошибка разрешения репозитория, отказ проверки запроса (глубина
фильтра, шаблон, форма значений — она идёт до запуска любой операции) не
оставляют ни строки, ни счётчика. Ошибки уже внутри исполнения
(например, несовпадение типа значения с полем), наоборот, попадают в
`errors`. Поэтому «в списке нет строки» **не** означает «такого трафика
не было»: это может быть трафик, падавший до таблицы, трафик другого
узла, трафик до перезапуска или до сброса, либо трафик, который
сворачивается в другую форму. Учёт — одна запись на исполнение,
синхронно, в той же точке, что и гистограмма длительности: `total_nanos`
— то же измерение, что попадает в `shamir_query_duration_seconds`.

### Что сворачивается в форму

* `Read` — выборка (поля, агрегаты с функцией и полем, `count(*)`,
  функции с числом аргументов, признак `DISTINCT` у агрегатных функций
  (`Aggregate` и `AggregateFn`), выражения) и флаг `distinct`; дерево
  `where` (теги операторов, ключи путей полей, вложенность, количество
  элементов у `In`/`NotIn`/`Contains*`); `group by` с `having`;
  `order by` с направлением и размещением `nulls`; **наличие** (не
  значения) limit/offset/page. Соединений в форме нет по построению: их
  нет и в `ReadQuery`, поэтому нет и тега «join».
* `Update` — фильтр `where` и **только ключи** записываемых полей
  (порядок значим, значения нет); `select` и `expected_version` не входят.
* `Delete` — только фильтр `where`; `Insert` и `Set` — вид операции и
  таблица.

Литералы, шаблоны (`LIKE`/`regex`/полнотекст), псевдонимы, имена
параметров, значения limit/offset и ключа seek-пагинации, `count_total`,
темпоральный селектор, `with_version` и `explain` в форму не входят:
первые превращаются в один тег типа (int / float / string / …). Поэтому
`age = 5` и `age = 999` — одна форма, `age = 5` и `age > 5` — две.

Дети `And` и `Or` сворачиваются **по порядку**, поэтому `And[a, b]` и
`And[b, a]` — две разные формы (так же значим порядок ключей в `set`):
форма описывает работу движка, а не логическую эквивалентность. Имена
полей сворачиваются как идентификаторы интернера, а имя, которого в
интернере нет, — в **один общий тег «неизвестное поле»**, и интернер не
пополняется: неизвестные имена не растят ни интернер, ни число форм.
Имя таблицы входит в форму своим хешем — разные таблицы дают разные
формы, но имени таблицы в ответе нет.

### Строка `(other)` и вытеснение

Реестр форм — память узла, не больше **256** строк на репозиторий. Когда
строка нужна, а места нет, исполнение не теряется: оно попадает в
аккумулятор `(other)`, после чего запускается уборка. Она вытесняет
строки с самой старой отметкой до **224** и каждую вытесненную складывает
в `(other)` при удалении; идёт она в один поток, второй вызов сразу
возвращается. Поэтому, пока ни одно исполнение не учитывается
одновременно, сумма `executions` по строкам плюс `other.executions` равна
числу учтённых исполнений, а `tracked` — число строк без `(other)`, не
больше 256; при живом трафике конкретный снимок может быть меньше:
исполнение, добавленное в строку — прежнюю или новую — уже после того,
как снимок её скопировал, не попадает ни в строки, ни в `(other)`.
Гарантирует снимок другое: ни один счётчик не учитывается дважды через
вытеснение — счётчики вытесненной строки переезжают в `(other)` и не
дублируются.

Практическое следствие: клиент с правом на запросы может «крутить»
реестр — каждая различимая форма это отдельная строка, пока её не
вытеснили, поэтому поток разнообразных запросов вытесняет ранние формы в
`(other)`. Счётчики не теряются, теряется детализация. Считаются и
невалидные имена: недопустимое имя функции, оператор выражения или имя
сравнения хешируются в ключ до того, как ветка их отвергнет, и тоже
занимают строку — но не больше одной и в тех же 256; а поскольку ключ —
односторонний 128-битный хеш и страница показывает только его,
невалидное имя ничего не раскрывает и ничего сверх этой одной
ограниченной строки не растит.

### Сброс

`reset_shape_stats` стирает всё, включая `(other)`: строки, счётчики и
`tracked` обнуляются. Наблюдение после сброса попадает в свежую строку;
гоняющееся со сбросом может быть потеряно, а чтение во время сброса видит
либо прежнее состояние, либо пустое — «наполовину стёртого» реестра не
бывает. Сброс уничтожает доказательства, поэтому при включённом аудите он
пишется в аудит-цепочку с одним именем репозитория — без счётчиков, строк
и ключей форм; ответ записи аудита не ждёт, а сам листинг в аудит не
пишется.

### Листинг и сброс из клиента

Обе операции — единственная операция самостоятельного нетранзакционного
пакета; ответ — типизированное значение (`ShapeStatsPage` у листинга,
`ShapeStatsReset` у сброса), а не `Batch`. Строки страницы идут по `total_nanos` по убыванию, затем `executions` по
убыванию, затем `shape_hex` по возрастанию; рядом `other` (`(other)`),
`tracked` и `truncated`. `limit` по умолчанию 50 и усекается до 256, а
`limit: 0` — типизированный отказ `invalid_request`; продолжения нет, а
`truncated` истинен, когда предел вызывающего обрезал страницу.

```rust
use shamir_client::{Client, ClientError};
use shamir_query_types::admin::shape_stats::{
    ListShapeStatsReq, ResetShapeStatsReq, ShapeStatsPage,
};

async fn top_shapes(
    control: &Client,
    db: &str,
    repo: &str,
) -> Result<ShapeStatsPage, ClientError> {
    let request = ListShapeStatsReq { repo: repo.to_string(), limit: Some(20) };
    control.list_shape_stats(db, request).await
}

async fn clear_shapes(control: &Client, db: &str, repo: &str) -> Result<(), ClientError> {
    let request = ResetShapeStatsReq { repo: repo.to_string() };
    control.reset_shape_stats(db, request).await
}
```

### Права, аудит и границы

* Только SCRAM-сессия. В токенной сессии обе операции отклоняются на
  сервере фиксированным `token_request_denied`, stateless REST-вызовы —
  тем же правилом периметра, что и просмотр активности; клиентский метод
  требует SCRAM ещё до отправки.
* Обе операции требуют право `ActivityView` с действием `Manage`,
  проверяемое обычным путём авторизации; из грубого superuser-гейта они
  исключены, и это исключение само по себе ничего не разрешает. Своей
  «области своих строк» у статистики форм нет: агрегат смешивает работу
  всех пользователей. Права проверяются раньше `limit` и раньше разрешения
  репозитория, поэтому отказ в правах одинаков при любом запрошенном имени
  и не служит оракулом существования репозитория.
* Неизвестный репозиторий — существующий типизированный отказ
  (`unknown_db`, если нет базы; `unknown_repo`, если в ней нет
  репозитория), а не пустая страница.
* Метрика `shamir_shapes_tracked{db,repo}` — гейдж числа отслеживаемых
  строк без `(other)`, не больше 256; серий по формам и по пользователям
  нет.
* Статистика живёт в памяти узла: она не реплицируется и не сохраняется,
  перезапуск узла её обнуляет, каждый узел показывает только свой трафик.
  Обе операции классифицированы как read-only, поэтому грубый гейт реплики
  их не отклоняет — но реплика покажет и сбросит только свои счётчики.
* TypeScript-клиента и команд `shapes ls` / `shapes reset` в
  административной оболочке пока нет: они вне этой версии.

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
The budget excludes inner commit (including asynchronous index-visibility durability-error tail
awaits), admission and network backpressure; post-ack soft pressure is not a
hard memory admission guarantee. Reconcile Pending with WaitForVisibility and
open a new snapshot for reads. Materialization is independent of visibility.
Protocol v4 gating is current; query-v3 requests are rejected. See [PR1c migration](./PR1C-MIGRATION.md). Supported SDK adoption remains VIS-5 work.
Ktav uses `transactions: { visibility_ack_mode: strict ... }` with the same keys.

VIS-4c exposes this policy as metrics: acknowledgement duration histograms
(`shamir_visibility_ack_wait_seconds_*`, labelled `mode` × `result`), pending
counters by `reason` and write `kind`, AsOf read-timeout and
`WaitForVisibility` reconciliation counters, and the `shamir_visibility_ack_waiters`
gauge — see `/metrics` above. Structured WARN logs fire at every wait expiry,
rate-limited per repository. `shamir_tx_committed_total` keeps counting
commits: Pending acknowledgement is a committed outcome, not an abort.
