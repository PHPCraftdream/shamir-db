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

Пример production-юнита (`deploy/shamir-db.service`):

```ini
[Unit]
Description=ShamirDB — production database server
After=network-online.target

[Service]
Type=simple
User=shamir
ExecStart=/usr/local/bin/shamir-server --config /etc/shamir/server.ktav
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

# Hardening
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/lib/shamir-db /var/log/shamir-db
PrivateTmp=yes
PrivateDevices=yes

MemoryMax=4G
TasksMax=4096
```

### Windows Service

```powershell
shamir-server service install
```

Регистрирует сервис в Windows SCM. Остановка — через `sc stop` или
`Services MMC`. Recovery policy настраивается через SCM.

### macOS / BSD

`service install` генерирует launchd plist (macOS) или rc.d-скрипт (BSD).

## 2. Конфиг: `.ktav`

Конфиг — файл в формате Ktav (подмножество JSON5: комментарии, trailing
commas). Минимальный конфиг (для e2e-тестов):

```ktav
{
    data_dir: "/tmp/shamir-test",
    listeners: [{
        kind: "tcp",
        addr: "127.0.0.1:7000",
        profile: "tls_exporter"
    }],
    tls: {
        cert_path: "/tmp/shamir-test/cert.pem",
        key_path: "/tmp/shamir-test/key.pem"
    }
}
```

Production-конфиг (`deploy/server.example.ktav`) — 99 строк с
аннотациями:

```ktav
data_dir: /var/lib/shamir-db

logging: {
    level: info
    slow_query_threshold_ms: 1000
}

kdf_defaults: {
    memory_kb: 131072
    time: 4
    parallelism: 1
    argon2_version: 19
}

argon2_concurrent_max: 64

listeners: [
    { kind: tcp,  addr: "0.0.0.0:7331", profile: tls_exporter }
    { kind: ws,   addr: "0.0.0.0:7332", profile: tls_exporter,
      path: "/shamir/v1" }
    { kind: ws,   addr: "0.0.0.0:7333", profile: tls_no_export,
      path: "/shamir/v1/browser",
      browser_origin_allowlist: ["https://app.example.com"] }
]

tls: {
    cert_path: /var/lib/shamir-db/cert.pem
    key_path:  /var/lib/shamir-db/key.pem
}

security: {
    connection: {
        auth_init_timeout_ms: 5000
        max_active_connections: 10000
    }
    query_limits: {
        max_result_size_bytes:        1073741824    # 1 GiB
        max_execution_time_secs:      60
        max_queries_per_batch:        100
        max_inflight_response_bytes:  4294967296    # 4 GiB (RI-15, optional)
    }
}

audit: {
    max_file_size_mb: 100
    retention_days: 30
}

observability: {
    addr: "127.0.0.1:9090"
}
```

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

### Валидация при старте

`Config::validate()` проверяет: `data_dir` существует, `listeners` не
пустой, `tls` пути — файлы, `kdf_defaults` в диапазонах, порты ≥ 0.
Ошибка → процесс не стартует, понятное сообщение.

`security.query_limits.max_inflight_response_bytes` (RI-15) — если задан,
должен быть `>= max_result_size_bytes`: иначе ни один максимально большой
батч-ответ не смог бы пройти глобальный бюджет памяти. По умолчанию
`None` (не задан) — глобальный бюджет не ограничивает суммарный размер
одновременно висящих в памяти ответов; действует только per-batch cap
`max_result_size_bytes`.

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

14 неймспейсов с индивидуальными уровнями: `shomer`, `wal`, `tx`,
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

In-process online-snapshot без downtime (через существующий redb-handle
`system_store`) — запланированное улучшение **P2**, пока не реализовано
(см. комментарий-модуля `crates/shamir-server/src/backup.rs`, «Future enhancement
(P2)»).

**Scheduled backup** — cron / systemd timer, но задание **обязано** сначала
остановить сервер, затем снять копию, затем поднять его обратно. Пример обёртки
для systemd: один `.service` с `ExecStartPre=systemctl stop shamir-server`,
`ExecStart=shamir-server ... backup --to ...`, `ExecStopPost=systemctl start
shamir-server`; либо wrapper-скрипт `stop → backup → restart`. Наивный cron,
запускающий `backup` против всё ещё живого сервера, — в точности тот риск,
которого избегает это правило.

**Restore (runbook-обязанность).** После восстановления `SystemStore` из любого
снимка оператор **обязан** выполнить `revokeAllTickets` (и `revokeAllLockouts`),
чтобы откатнутые resumption-tickets / lockout-счётчики не допустили replay.
Автоматического принуждения этой команды сейчас нет — это ручной шаг; см.
`docs/guide-docs/client-server-protocol-spec/IMPLEMENTATION_GUIDE.md` §5.7 и
`SECURITY_MODEL.md` §4.4.

## 9. Capacity planning

Подробная таблица — в `docs/dev-artifacts/ops/CAPACITY_PLANNING.md`. Кратко:

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
* **Observability — opt-in.** Без `observability.addr` — нет HTTP-метрик.
  В проде — почти всегда нужен (load balancer / health checks).
* **Graceful shutdown — bounded.** 30 s deadline. Если застрявшее
  соединение не успело — процесс выходит, OS reclaim'ает сокеты.
* **Single-instance guard.** Файловая блокировка (`fs4`) на `data_dir`.
  Два процесса на одних данных → второй не стартует.
* **Logging — non-blocking.** Никогда не блокирует горячий путь. Overflow
  — lossy (лучше потерять лог, чем запрос).

## Куда дальше

||| Упёрся в… | Поднимайся на |
||---|---|---|
|| «нужна децентрализация / P2P» | [Этаж 8 — Interconnect](./08-interconnect.md) |
