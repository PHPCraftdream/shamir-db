# Capacity planning — ShamirDB server

Sizing reference for the production server. Numbers below are derived
from the structures in code (per-session / per-connection memory)
combined with the configured caps in `server.ktav`. They are accurate
to within ±20 % for typical workloads; verify against `process_*`
metrics from `/metrics` for your specific deployment.

## Memory

| Subsystem | Per unit | Notes |
|-----------|----------|-------|
| Idle process | ~50 MB | tokio multi-thread runtime + redb mmap regions for the open databases |
| Active session | ~2 KB | `Session` struct (32 KB user_id, 16 KB username, ~96 KB permission cache, parking-lot mutex slot, channel binding) |
| Active connection | ~8 KB | tokio task stack (~4 KB) + per-connection scratch buffers (`frame_buf` 4 KB, `write_scratch` 4 KB, RAII guard for `ConnLimiter`) |
| Argon2id verify (peak) | ~`memory_kb` × 1.05 | Held for ~50–500 ms per verify depending on cost params; capped concurrent by `argon2_concurrent_max` |
| Audit chain in-memory | ~1 KB | `AuditChain` HMAC state + last seq |
| Audit batched buffer | up to ~20 KB | Drained every 5 s in `Durab::Batched` mode |
| `RedbConsumedCounters` | ~16 bytes per (user × family) entry | GC'd by scheduler |
| Lockout entries | ~80 bytes per (subnet × user) pair | GC'd every 5 min |
| Rate-limiter buckets | ~24 bytes per subnet | GC'd every 5 min |

**Worst-case auth-time RAM** = `argon2_concurrent_max × kdf_defaults.memory_kb`.
With the defaults `64 × 128 MB = 8 GB`. Lower the cap if peak RAM is the
binding constraint; lower `kdf_defaults.memory_kb` only if you're
willing to weaken password security.

**Steady-state RAM** ≈ `idle + (active_sessions × 2 KB) + (active_connections × 8 KB)`.
For 10 000 concurrent sessions × 8 000 connections that's roughly
`50 MB + 20 MB + 64 MB = 134 MB` in addition to whatever Argon2 is
doing right now.

## CPU

| Operation | Cost | Notes |
|-----------|------|-------|
| Argon2id verify | ~50 ms (defaults) | Linear in `memory_kb × time × parallelism`. Bound by `argon2_concurrent_max` |
| TLS 1.3 handshake | ~5 ms | rustls + aws-lc-rs; one-off per connection |
| Post-auth request | ~20 µs | RequestEnvelope decode → dispatch → ShamirDb query (fast path) |
| Batch query (cold) | ~1 ms / query | Filter compile + index lookup + serde encode |
| `/metrics` poller | ~30-50 µs every 5 s | `metrics-process` reads `/proc/self/*` (~0.001 % CPU) |
| Scheduler tasks | ~100 µs per tick | counter_gc / lockout_gc / etc. |

**Auth-handshake throughput** ≈ `argon2_concurrent_max × (1000 ms / kdf_time_ms)`.
Defaults: `64 × (1000 / 50) ≈ 1280 successful auths/sec`. Lockout +
rate-limit bound a single attacker to far fewer attempts.

**Post-auth request throughput** ≈ `cores × 50 000 req/sec` for fast-
path queries (dominated by msgpack encode). Slow queries with index
scans drop this by 1-2 orders of magnitude.

## Disk

| File | Growth rate | Notes |
|------|-------------|-------|
| `server_meta.redb` | <1 MB lifetime | Static identity / ticket keys / audit chain key |
| `users.redb` | ~200 bytes per user | Linear in user count |
| `counters.redb` | ~64 bytes per (user × family × rotation) | GC'd by scheduler |
| `shamir_db_meta.redb` | <100 KB per (db, repo) | DB / repo metadata only |
| `shamir_db_default_main.redb` | application data | The big one. Plan disk based on row count × row size × ~1.5 (redb overhead) |
| `audit.log` (active) | ~200 bytes per audit event | Rotated at `max_file_size_mb` |
| `audit.log.<ts>` (rotated) | up to `max_file_size_mb × retention_days × events_per_day / max_size` | Retention managed manually today (logrotate / cron) |
| `wire_tables.json` | ~50 bytes per (db, repo, table) | Negligible |
| TLS PEM (`cert.pem`, `key.pem`) | ~3 KB total | One-off |

**Audit log sizing**: 1000 events/sec × 200 bytes = 17 GB/day. Rotation
at 100 MB → ~170 rotated files/day. With 30-day retention that's ~510
files / ~17 GB on disk for audit alone. Adjust `max_file_size_mb` and
`retention_days` accordingly.

## Recommended sizing

For three workload tiers — these are starting points, validate against
your `/metrics` data after a week of production traffic.

### Small (developer / single-tenant pilot)

- 2 vCPU, 2 GB RAM, 20 GB SSD
- `argon2_concurrent_max: 16` (worst-case 2 GB Argon2 RAM at 128 MB cost)
- `max_active_connections: 1000`
- Up to ~100 concurrent users, ~10 RPS sustained

### Medium (small/mid SaaS)

- 4 vCPU, 4 GB RAM, 100 GB SSD
- `argon2_concurrent_max: 32`
- `max_active_connections: 5000`
- Up to ~5 000 concurrent users, ~500 RPS sustained

### Large (production tenant or multi-tenant)

- 8 vCPU, 8 GB RAM, 500 GB SSD
- `argon2_concurrent_max: 64` (default)
- `max_active_connections: 10 000` (default)
- Up to ~50 000 concurrent users, ~5 000 RPS sustained

### Very large

Scale by replicating + fronting with a load balancer that pins a
session_id to a single server (sessions are in-memory, not shared
between replicas). Replication of the durable redb files is a separate
P2 feature — see `../roadmap/PRODUCTION_HARDENING_ROADMAP.md`.

## Things that will hurt you if you ignore them

* **Argon2id RAM × concurrency = peak server RAM.** A KDF tuning that
  looks great for security (`memory_kb: 512000`) combined with
  `argon2_concurrent_max: 64` reserves 32 GB for KDF alone. Don't.
* **Audit log without rotation** fills the disk in days. We rotate by
  default (100 MB / 30 days); leave it on.
* **Slow queries serialise on Tokio workers** (because `block_in_place`
  doesn't release the worker for synchronous DB calls — it just lets
  the runtime schedule other I/O on a *different* worker). With
  `worker_threads: 4` (default) and 5 simultaneous slow queries, the
  fifth waits. Either bump worker count or shorten queries.
* **`wire_tables.json` is single-writer.** Schema changes during heavy
  load may serialise on the registry mutex. Negligible at most rates,
  worth knowing if you create thousands of tables/sec.
* **Identity seed is plaintext** in `server_meta.redb`. Use disk-level
  encryption (LUKS / EBS encryption) to mitigate; HSM-grade isolation
  is a P2 item.

## Where to look

* `process_resident_memory_bytes` (Prometheus) — running RAM usage
* `process_cpu_seconds_total` rate — CPU load
* `process_io_*_bytes_total` rate — disk I/O
* `process_threads` — should hover at `worker_threads + ~5`
* `process_open_fds` — should be roughly `connections_active × 2`
