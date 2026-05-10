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
| Audit-цепочка | HMAC-chain + JSON-line + checkpoint | `audit_appender.rs` |
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
| 1 | Health checks `/healthz`, `/readyz` | ~1 ч | LB / K8s probe знает живой ли сервер. Без этого rolling deploy слепой |
| 2 | Backups | ~3 ч | `Database::backup()` + CLI `shamir-server backup --to /path`. Кэш regenerate'ит, но user data → терять нельзя |
| 3 | Global max-connections cap | ~2 ч | Atomic counter, отказ accept'а при лимите. Без этого DDoS = OOM |
| 4 | Pre-handshake read/write timeout | ~1 ч | Slow-loris защита. Одна `tokio::time::timeout` на auth_init read |
| 5 | Audit log rotation | ~2 ч | Файл растёт бесконечно → диск переполнится. Простой rotation by size/age |
| 6 | Prometheus `/metrics` | ~3 ч | Без него production слепой. `auth_attempts`, `active_sessions`, `argon2_busy`, `connections_active`, p99 latency |
| 7 | Slow query logging | ~1 ч | `if execution_time_us > N { tracing::warn!(..) }`. Тривиально, спасает от безумия в продакшене |
| 8 | systemd unit + Dockerfile | ~2 ч | Без них ops-команда не задеплоит |
| 9 | `changePassword` SCRAM | ~3 ч | Базовый user flow. Уже реализован в `shamir-connect::changepw`, нужно подключить через wire. Без него юзер не может сменить пароль = security incident response невозможен |
| 10 | Capacity planning docs | ~1 ч | Не код. README: "1 session = ~2 KB RAM, audit entry = ~200 bytes, redb growth ~X/день при Y запросов/сек" |

**Итого P0 ≈ 19 часов.**

### P1 — нужно если multi-user / долгосрочная эксплуатация

| # | Задача | Время | Что и зачем | Когда нужно |
|---|--------|-------|-------------|-------------|
| 11 | Real RBAC | ~4 ч | Per-table permissions. Сейчас бинарный superuser-or-not | Когда >1 типа пользователей с разными правами |
| 12 | Resume PATH (server-side) | ~3 ч | Issue'им ticket но не принимаем — мёртвый функционал | Когда долгие mobile-сессии (re-auth дорогой Argon2id) |
| 13 | Per-session resource limits на сервере | ~3 ч | Сейчас `BatchLimits` в payload — клиент сам себе верит. Server-side override | Когда >1 organization sharing tenant |
| 14 | CLI admin tool | ~5 ч | `shamir-server-admin user/audit/backup`. Иначе ops пишет custom client | Когда есть отдельная ops-команда |
| 15 | Connection drain mode (SIGUSR1) | ~2 ч | Blue/green deploy без потерянных соединений | Только если blue/green — иначе SIGTERM с timeout достаточен |

**Итого P1 ≈ 17 часов** (если все нужны).

### P2 — отдельные крупные проекты, не roadmap item

| Задача | Когда делать |
|---|---|
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
| **Audit query API** | `jq` + `tail -f` + `grep` работают на JSON-line файле. Admin API — сахар. Если есть Splunk/ELK — туда отправлять важнее чем API |
| **Query result streaming (cursor)** | `BatchRequest::limits.max_result_size = 10MB` уже limits. Если ответ 100MB — это проектная проблема, не недостаток сервера. LIMIT/pagination существуют |
| **Connection draining metrics** | Часть P0 #6 (Prometheus metrics) |
| **Replication / HA** | Не "10 часов задача" — это **месяц** работы (Raft consensus или conflict resolution). Отдельный major feature, не пункт в roadmap |
| **Connection drain SIGUSR1** для всех | SIGTERM с graceful timeout (уже есть) покрывает 95%. SIGUSR1 — только для blue/green. Перенесено в P1 как опциональное |

## Рекомендуемый порядок до production-готовности

```
== Sprint 1 (~15 часов): ship-able state ==
1. Health checks (/healthz, /readyz)              ~1 ч
2. Pre-handshake timeout                          ~1 ч
3. Global max-connections cap                     ~2 ч
4. Audit log rotation                             ~2 ч
5. Slow query logging                             ~1 ч
6. Backup task + CLI                              ~3 ч
7. Prometheus /metrics                            ~3 ч
8. systemd unit + Dockerfile                      ~2 ч

== Sprint 2 (~4 часов): user lifecycle ==
9. changePassword wire-side                       ~3 ч
10. Capacity planning docs                        ~1 ч

== Optional sprint 3 (~17 часов): multi-user maturity ==
11. Real RBAC                                     ~4 ч
12. Resume PATH (server-side)                     ~3 ч
13. Per-session server-side limits                ~3 ч
14. CLI admin tool                                ~5 ч
15. Connection drain SIGUSR1                      ~2 ч
```

**Минимально достаточно: Sprint 1 + 2 = ~19 часов.**
Полная зрелость для multi-tenant: + Sprint 3 = ~36 часов.

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
было P0    ~11 ч    →   стало P0     ~19 ч  (включил часть P1+P2 что реально важно)
было P1    ~22 ч    →   стало P1     ~17 ч  (выкинул IP-allowlist, cert-rotation, drain mode)
было P2    ~49 ч    →   стало P2      0 ч   (всё в "проекты" или "не делать")
─────────────────────────────────────────
было total ~82 ч    →   стало total  ~36 ч  (-56%)
```

**Production-ready минимум: ~19 часов** (Sprint 1+2).
**Multi-user maturity: +17 часов** = ~36 часов всего.
**HA/replication, encryption-at-rest, HSM**: отдельные проекты,
не roadmap items.
