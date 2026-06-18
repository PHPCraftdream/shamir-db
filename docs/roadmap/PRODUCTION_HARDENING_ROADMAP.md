# Production hardening roadmap

После закрытия 5 MVP-gaps (commits `5bda64a`, `7b0130a`, `210c360`,
`47c5b36`, ...) сервер реально принимает клиентов через TCP и WS,
переживает рестарт и поддерживает полный язык запросов. Этот документ —
честная карта того, что **ещё нужно** для production-grade развёртывания,
с приоритетами и оценками.

> **TL;DR ревью первоначального плана**: из 25 пунктов реально нужно
> ~12. Половина — перфекционизм или решается уровнем инфраструктуры
> (firewall, FS-level encryption, K8s rolling restart). Реальная
> production-готовность достижима за **~25 часов** работы, не 82.
>
> Что **выкинуто** как избыточное и почему — в разделе "Что НЕ делать"
> ниже.

## Что уже есть (validated, в коде)

| Категория | Что | Где |
|---|---|---|
| Логи | `tracing` + `EnvFilter` из config | `main.rs`, `server.rs` |
| Audit-цепочка | HMAC-chain + audit-line + checkpoint | `audit_appender.rs` |
| Per-subnet rate limit | `auth_init` token-bucket + restart warmup | `shamir-connect::rate_limit` |
| Lockout | exponential backoff + 5 min idle GC | `shamir-connect::lockout` |
| Argon2id concurrency cap | semaphore `try_acquire` | config `argon2_concurrent_max` |
| Frame size cap | 16 MiB | `MAX_FRAME_SIZE_DEFAULT` |
| MAX_SESSIONS_PER_USER | 16 LRU | `session.rs` |
| Latency padding | spec §8.5 — обе ветки auth | `LatencyPadGuard` |
| Durable secrets | redb `Durability::Immediate` | `ServerMetaStore` |
| Bootstrap superuser | password / random-token | `bootstrap.rs` |
| TLS cert auto-gen | self-signed на первом старте | `tls.rs` |
| Versioning | hardcoded list handshake + query language | `version.rs` |
| Graceful shutdown | broadcast-channel → drain audit + scheduler | `server.rs` |
| Durable persistence | redb `default.main` + tables_registry | `server.rs`, `tables_registry.rs` |
| WS endpoint | native + browser + Origin allowlist | `framer.rs`, `server.rs` |
| Resumption ticket issuance | в `auth_ok`, TTL=24h | `connection.rs` |
| §7.5 ticket invalidation | secondary index user_id→name | `user_directory.rs` |

## Что частично

- **Лимит подключений**: per-subnet rate-limit есть, **глобального cap
  на одновременные connections — нет**. 100K параллельных handshake'ов
  положат сервер по памяти.
- **Авторизация по IP**: subnet-based lockout есть для bruteforce, но
  **IP allowlist для admin endpoint'ов нет**. Любой может попробовать
  `CreateScramUser` если знает superuser-credentials.
- **Server identity rotation**: реализована в `ServerIdentityState::rotate`
  (overlap-окно, finalize по timer'у), но **admin op для триггера
  rotation не подключён в db_handler**.

## Дорожная карта (после ревью)

### P0 — реально нужно для production

| # | Задача | Время | Что и зачем |
|---|--------|-------|-------------|
| 1 | Observability HTTP-сервер: `/healthz`, `/readyz`, `/metrics`, `/info` | ~4 ч | **P0 #1 и #6 объединены** — один маленький HTTP-сервер (axum/hyper, обычно `127.0.0.1:9090`). `/healthz` boolean alive (для K8s liveness, держим простым), `/readyz` boolean готовности (для traffic gating), `/metrics` стандартный Prometheus формат с **process metrics** (`process_cpu_seconds_total`, `process_resident_memory_bytes`, `process_threads`, `process_io_read_bytes_total`, `process_io_write_bytes_total`) + application metrics (sessions, connections, auth attempts). `/info` опциональный pretty-printed msgpack для curl-debug'инга. Используем crate `metrics-process` — кроссплатформенно, эмитит стандартные имена которые node_exporter / Grafana уже понимают. Background poller раз в 5 сек обновляет кэш (~30-50 μs работы), probes читают атомики (~ns). Overhead: 0.001% CPU |
| 2 | Backups | ~3 ч | `Database::backup()` + CLI `shamir-server backup --to /path`. Кэш regenerate'ит, но user data → терять нельзя |
| 3 | Global max-connections cap | ~2 ч | Atomic counter, отказ accept'а при лимите. Без этого DDoS = OOM |
| 4 | Pre-handshake read/write timeout | ~1 ч | Slow-loris защита. Одна `tokio::time::timeout` на auth_init read |
| 5 | Audit log rotation | ~2 ч | Файл растёт бесконечно → диск переполнится. Простой rotation by size/age |
| ~~6~~ | ~~Prometheus `/metrics`~~ | — | **Слит с #1** (один HTTP-сервер обслуживает все 4 endpoint'а: healthz/readyz/metrics/info) |
| 7 | Slow query logging | ~1 ч | `if execution_time_us > N { tracing::warn!(..) }`. Тривиально, спасает от безумия в продакшене |
| 8 | systemd unit + Dockerfile | ~2 ч | Без них ops-команда не задеплоит |
| ~~9~~ | ~~`changePassword` SCRAM~~ | — | **Выкинуто из roadmap.** Изначально стояло как production-must "без него юзер не может сменить пароль". На практике admin может пересоздать SCRAM-юзера через уже-работающий wire-op `CreateScramUser` (delete старого через прямой доступ к `RedbUserDirectory` + create нового). Self-service changePassword — nice-to-have, не critical. Возвращаемся когда (а) compliance-driver требует self-service rotation, или (б) appears regular-user-flow с тысячами пользователей которым нужна smena без admin-вмешательства |
| 10 | Server-side query limits cap | ~1 ч | `[security.query_limits] max_result_size_bytes`, `max_execution_time_ms`, `max_queries_per_batch`. Клиент может **уменьшить**, не **увеличить**. Сейчас 10MB-default — произвольный, нужен явный operator-knob |
| 11 | Capacity planning docs | ~1 ч | Не код. README: "1 session = ~2 KB RAM, audit entry = ~200 bytes, redb growth ~X/день при Y запросов/сек" |

**Итого P0 ≈ 15 часов.**

### P1 — нужно если multi-user / долгосрочная эксплуатация

| # | Задача | Время | Что и зачем | Когда нужно |
|---|--------|-------|-------------|-------------|
| 12 | Real RBAC | ~4 ч | Per-table permissions. Сейчас бинарный superuser-or-not | Когда >1 типа пользователей с разными правами |
| 13 | Resume PATH (server-side) | ~3 ч | Issue'им ticket но не принимаем — мёртвый функционал | Когда долгие mobile-сессии (re-auth дорогой Argon2id) |
| 14 | CLI admin tool | ~5 ч | `shamir-server-admin user/audit/backup`. Иначе ops пишет custom client | Когда есть отдельная ops-команда |
| 15 | Connection drain mode (SIGUSR1) | ~2 ч | Blue/green deploy без потерянных соединений | Только если blue/green — иначе SIGTERM с timeout достаточен |

**Итого P1 ≈ 14 часов** (если все нужны).

### P2 — отдельные крупные проекты, не roadmap item

| Задача | Когда делать |
|---|---|
| **True streaming responses** (multi-frame `ResponseChunk` + cursor + cancellation) | Когда конкретный use-case с измеренными требованиями (например "ETL пайплайн отправляет 1 GB datasets раз в час через сеть"). Это **breaking change wire-протокола** + изменения в shamir-db (Vec<Value> → Stream<Value>) + новая query_version. Реалистично ~15-20 ч. Промежуточный workaround — поднять `max_result_size_bytes` в config (P0 #10) и LIMIT/pagination на уровне query |
| Replication / HA (Raft / master-replica) | Когда uptime требование >99.9%. Это **месяц работы**, не 10 часов из исходного плана. Не roadmap, а отдельный design doc |
| HSM/KMS для identity seed | Когда compliance требует (SOC2, FedRAMP). Иначе chmod 600 + encrypted disk достаточно |
| Schema migrations | Когда **впервые** придётся бампать `BatchRequest` version. Сейчас version=1 hardcoded — преждевременно |
| Encryption at rest на app уровне | Только если FS-level encryption недоступна. На AWS EBS/Azure managed disk избыточно |

## Что НЕ делать (выкинуто из исходного плана)

| Было | Почему выкинуто |
|------|-----------------|
| **OpenTelemetry distributed tracing** | `tracing` (уже есть) + slow query log покрывают 95% debugging. OTEL setup-complexity не оправдана для системы с одним сервисом. Когда появится 5+ микросервисов — добавить |
| **TLS cert rotation без рестарта** | Let's Encrypt = 90 дней. Rolling restart раз в 90 дней OK для 99% случаев. File-watcher reload — bug-prone, сложный (race с in-flight handshakes). Овчинка не стоит выделки |
| **IP allowlist/denylist в config** | Делается на firewall (iptables/cloud security groups/nginx). На application уровне — дублирование. Если за nginx — он лучше это умеет |
| **Encryption at rest** | LUKS / cloud-managed encryption (EBS, GCP PD) делают это лучше + transparent. SQLCipher-style — много кода, slow, ключи всё равно нужно где-то хранить → та же проблема |
| **HSM/KMS** для большинства | Если seed файл с `chmod 600` + диск зашифрован — этого достаточно для всего кроме FedRAMP-grade compliance. Откладываем до compliance-driver |
| **Schema migrations infrastructure** | Сейчас одна версия. Преждевременная оптимизация. Сделаем когда впервые понадобится |
| **Audit query API** | `jq` + `tail -f` + `grep` работают на audit-line файле. Admin API — сахар. Если есть Splunk/ELK — туда отправлять важнее чем API |
| ~~**Query result streaming (cursor)**~~ ← **изменено** | Изначальный аргумент "10 MB достаточно" был патерналистским — за пользователя нельзя решать. Промежуточное решение: configurable `max_result_size_bytes` (P0 #10). Полный streaming с cursor/cancellation/multi-frame — отдельный major feature в P2, делать когда появится конкретный use-case (ETL с GB-датасетами и backpressure-требованиями) |
| **Connection draining metrics** | Часть P0 #6 (Prometheus metrics) |
| **Replication / HA** | Не "10 часов задача" — это **месяц** работы (Raft consensus или conflict resolution). Отдельный major feature, не пункт в roadmap |
| **Connection drain SIGUSR1** для всех | SIGTERM с graceful timeout (уже есть) покрывает 95%. SIGUSR1 — только для blue/green. Перенесено в P1 как опциональное |

## Рекомендуемый порядок до production-готовности

```
== Sprint 1 (~16 часов): ship-able state ==
1. Health checks (/healthz, /readyz)              ~1 ч
2. Pre-handshake timeout                          ~1 ч
3. Global max-connections cap                     ~2 ч
4. Audit log rotation                             ~2 ч
5. Slow query logging                             ~1 ч
6. Backup task + CLI                              ~3 ч
7. Prometheus /metrics                            ~3 ч
8. Server-side query limits cap (configurable)   ~1 ч
9. systemd unit + Dockerfile                      ~2 ч

== Sprint 2 (~1 час): docs ==
11. Capacity planning docs                        ~1 ч

== Optional sprint 3 (~14 часов): multi-user maturity ==
12. Real RBAC                                     ~4 ч
13. Resume PATH (server-side)                     ~3 ч
14. CLI admin tool                                ~5 ч
15. Connection drain SIGUSR1                      ~2 ч
```

**Минимально достаточно: Sprint 1 + 2 = ~20 часов.**
Полная зрелость для multi-tenant: + Sprint 3 = ~34 часа.

True streaming (multi-frame ResponseChunk) — отдельный feature на ~15-20 ч,
делать когда конкретный use-case с GB-датасетами появится.

После Sprint 1+2 получится:

```yaml
# docker-compose.yml
services:
  shamir-db:
    image: shamir-db:latest
    volumes:
      - shamir-data:/var/lib/shamir-db
    ports:
      - "7331:7331"  # tcp
      - "7332:7332"  # ws native
      - "7333:7333"  # ws browser
      - "9090:9090"  # metrics (loopback only внутри pod'а)
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:9090/healthz"]
      interval: 10s
      timeout: 3s
      retries: 3
    deploy:
      resources:
        limits:
          memory: 2G
          cpus: "2"
```

## Что НЕ нужно для production (никогда или очень нескоро)

- **Multi-tenancy isolation** — если все клиенты одной организации.
- **GDPR data deletion API** — пишется когда придёт юрист.
- **Real-time admin dashboard UI** — Grafana покрывает всё.
- **Compliance attestations (SOC2, ISO 27001)** — отдельный нон-инженерный трек.
- **gRPC / GraphQL gateway** — у нас свой бинарный протокол, конвертеры — overkill.
- **Built-in load balancing** — nginx/envoy решают.
- **Plugin система** — преждевременная гибкость.

## Известные ограничения архитектуры

| Ограничение | Воркараунд / план |
|---|---|
| `ShamirAdminExecutor.create_repo` принимает только `engine: "in_memory"` | Pre-create durable репо в boot path (как `default.main`); вторичные репо in-memory only |
| `db_handler` blocks Tokio worker через `block_in_place` | Это **не блокирует** runtime (block_in_place именно для этого), но один long-running query занимает worker'а целиком — нужен query timeout (P1 #13) |
| `BatchRequest::limits.max_queries=50` — global default | Настраивается per-request, но server-side cap нет (доверяем payload). Закроется P1 #13 |
| Identity Ed25519 seed в plaintext | `chmod 600` + encrypted disk достаточно. HSM только если compliance требует |
| TLS exporter не работает на windows-msvc target | Не блокер: windows-gnu или Linux в проде |

## Сводная оценка после ревью

```
было P0    ~11 ч    →   стало P0     ~15 ч  (+ slow-query log, query-limits cap, capacity docs)
было P1    ~22 ч    →   стало P1     ~14 ч  (выкинул IP-allowlist, cert-rotation, отдельный drain mode)
было P2    ~49 ч    →   стало P2 на потребность  (HA, streaming, HSM — feature-driven, не roadmap)
─────────────────────────────────────────
было total ~82 ч    →   стало total  ~34 ч  (-58%)
```

**Production-ready минимум: ~20 часов** (Sprint 1+2).
**Multi-user maturity: +14 часов** = ~34 часа всего.
**HA/replication, true streaming, HSM, encryption-at-rest**: отдельные
feature-driven проекты, делать когда конкретный use-case появится.
