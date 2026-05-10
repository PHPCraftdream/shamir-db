# Production hardening roadmap

После закрытия 5 MVP-gaps (commits `5bda64a`, `7b0130a`, `210c360`,
`47c5b36`, ...) сервер реально принимает клиентов через TCP и WS,
переживает рестарт и поддерживает полный язык запросов. Этот документ —
честная карта того, что **ещё нужно** для production-grade развёртывания,
с приоритетами и оценками.

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

## Дорожная карта

### P0 — нельзя в production без этого

| # | Задача | Время | Что и зачем |
|---|--------|-------|-------------|
| 1 | Health checks `/healthz`, `/readyz` | ~1 ч | K8s readiness probe + L4 health check |
| 2 | Backups (`Database::backup()`) | ~3 ч | `BackupScheduler` task + CLI `shamir-server backup --to /path`. Без этого — потеря всего при corrupted redb |
| 3 | Глобальный max-connections cap | ~2 ч | counter в state, отказ accept'а при лимите. Защита от resource-exhaustion |
| 4 | Pre-handshake read/write timeout | ~1 ч | Slow-loris защита: TLS-accepted клиент не должен висеть бесконечно без `auth_init` |
| 5 | Connection drain mode | ~2 ч | `SIGUSR1` → перестать accept'ить, дождаться N секунд активных. Для blue/green |
| 6 | Audit log rotation | ~2 ч | Сейчас файл растёт бесконечно. По size/age, либо `logrotate`-friendly hook |

**Итого P0 ≈ 11 часов.**

### P1 — production-grade, deploy + добавить параллельно

| # | Задача | Время | Что и зачем |
|---|--------|-------|-------------|
| 7 | Prometheus `/metrics` | ~3 ч | `auth_attempts_total{result}`, `active_sessions`, `argon2_busy`, `audit_chain_seq`, `connections_active`, `frame_size_bytes_p99`. Labels по listener |
| 8 | IP allowlist/denylist | ~2 ч | Config `[security.network] allow_subnets = [..] deny_subnets = [..]`. Reject до TLS accept'а — экономия CPU |
| 9 | Real RBAC | ~4 ч | Wire `shamir_db::db::query::auth::SessionPermissions` в session.rs. Сейчас бинарный superuser-or-not |
| 10 | Resume PATH (server-side) | ~3 ч | Мы issue'им ticket, но не принимаем. Нужен `resume_init` frame + `resume_session(...)` |
| 11 | TLS cert rotation без рестарта | ~3 ч | File watcher на `cert.pem` → reload `TlsAcceptor`. Без этого Let's Encrypt expiry = downtime |
| 12 | Per-session resource limits | ~3 ч | Max queries/sec, max in-flight requests, max result size. Один клиент не должен задушить сервер |
| 13 | Slow query logging | ~1 ч | Log при `execution_time_us > N`. Critical для отладки production |
| 14 | systemd unit + Dockerfile + Helm chart | ~3 ч | Ops-команда не сможет деплоить без этого |

**Итого P1 ≈ 22 часа.**

### P2 — strongly recommended, не блокер

| # | Задача | Время | Что и зачем |
|---|--------|-------|-------------|
| 15 | OpenTelemetry distributed tracing | ~4 ч | Propagate `trace_id` через RequestEnvelope. Spans accept→handshake→batch→response |
| 16 | Encryption at rest | ~6 ч | redb-файлы зашифрованы (SQLCipher-style). Для shared infra / cloud disks |
| 17 | HSM/KMS integration | ~5 ч | Ed25519 server identity seed сейчас plaintext в `server_meta.redb` |
| 18 | Replication / HA | ~10+ ч | Read-replica или Raft. Single-point-of-failure без этого |
| 19 | Schema migrations | ~5 ч | При изменении `BatchRequest` schema нужна миграция durable данных |
| 20 | Audit query API | ~3 ч | Admin op для поиска по audit log без `tail`-инга файла |
| 21 | `changePassword` SCRAM | ~3 ч | Реализован в `shamir-connect::changepw`, через wire не подключён |
| 22 | Query result streaming | ~5 ч | Cursor для 100MB+ результатов вместо одного сообщения |
| 23 | CLI admin tool | ~5 ч | `shamir-server-admin user/audit/backup/migrate` |
| 24 | Capacity planning docs | ~2 ч | RAM per session, RAM per audit entry, redb growth rate |
| 25 | Connection draining metrics | ~1 ч | Counter активных per-connection tasks для graceful shutdown |

**Итого P2 ≈ 49 часов.**

## Рекомендуемый порядок до production-MVP-2

Чтобы можно было реально ставить за nginx и принимать клиентов:

```
1. Health checks (/healthz, /readyz)              ~1 ч  [P0]
2. Pre-handshake timeout (slow-loris)             ~1 ч  [P0]
3. Global max-connections cap                     ~2 ч  [P0]
4. Audit log rotation                             ~2 ч  [P0]
5. Backup task + CLI                              ~3 ч  [P0]
6. /metrics (Prometheus)                          ~3 ч  [P1]
7. IP allowlist в config                          ~2 ч  [P1]
8. Connection drain mode (SIGUSR1)                ~2 ч  [P0]
9. systemd unit + Dockerfile + healthcheck       ~2 ч  [P1]
```

**Итого: ~18 часов.** После этого получится:

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

## Что НЕ нужно для production (но полезно потом)

- **Multi-tenancy isolation** — если все клиенты одной организации, не критично.
- **GDPR data deletion API** — зависит от юрисдикции, не universal.
- **Real-time admin dashboard UI** — оперативно покрывает Grafana.
- **Compliance attestations (SOC2, ISO 27001)** — отдельный non-engineering трек.

## Известные ограничения архитектуры

| Ограничение | Воркараунд |
|---|---|
| ShamirAdminExecutor.create_repo принимает только `engine: "in_memory"` | Pre-create durable репо в boot path (как `default.main`); wire-side вторичные репо in-memory only до патча shamir-db |
| `db_handler` blocks Tokio worker через `block_in_place` | Это **не блокирует** runtime (block_in_place именно для этого), но один long-running query занимает worker'а целиком — нужен query timeout |
| `BatchRequest::limits.max_queries=50` — global default | Настраивается per-request, но защиты на сервере нет (доверяем `BatchLimits` из payload) |
| Identity Ed25519 seed в plaintext | Перенести в HSM/KMS (P2 #17) |
| TLS exporter не работает на windows-msvc target | Не блокер: используем windows-gnu или Linux в проде |

## Сводная оценка до full-production

```
P0 (must)        ~11 часов
P1 (production)  ~22 часов
P2 (recommended) ~49 часов
─────────────────────────
Total            ~82 часа
```

С одним инженером — 2-3 спринта. С 2-3 параллельно работающими над
независимыми пунктами — 3-4 недели.
