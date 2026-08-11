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

## Index & cursor sizing (engine-level)

The numbers above are server/auth-level. These cover engine-level costs
that scale with table size and index count — release-review follow-up
(#1086), all EXTRAPOLATED from real, already-measured benchmarks (no
fresh bench run backs these directly; each row below cites the actual
source measurement).

### Cursor rescans (offset-fallback / non-keyset-eligible pages)

A cursor `FetchNext` page that ISN'T eligible for the F-53b keyset-seek
fast path — a multi-column `ORDER BY`, an unindexed/computed `ORDER BY`,
or a page where a concurrent write to the indexed field tripped the
per-index mutation high-water gate — re-runs a FULL pinned-snapshot
table scan on EVERY call (`KNOWN_LIMITATIONS.md` §6). Cost scales with
total table size, not `page_size`.

No dedicated cursor-rescan benchmark exists yet. The nearest measured
analog is the (pre-online-build) regular/hash `CREATE INDEX` backfill
scan — it reads every row at the same O(N) cost class, though it also
writes postings, so treat these as a conservative UPPER bound on a
pure-read rescan:

| Table size | Full-scan-class cost | Source |
|---|---|---|
| 5k rows | ~150–170 ms | `f78_writer_latency` bench, measured |
| 100k rows | ~140–160 s | `f78_writer_latency` bench, measured (P1-4/#969) |
| 1M rows | hours (extrapolated — scan is superlinear) | RFC `2026-08-07-online-index-build-rfc.md` §1.2 |

The CR-C3 fix (batched MVCC version resolution) measured 13–25× faster
over a real fjall-backed store across 1k–100k keys for the per-row
version-lookup part of a page's scan — it meaningfully reduces, but does
not eliminate, the O(N) shape above.

**Guidance:** for tables beyond low tens of thousands of rows, use a
single-column indexed `ORDER BY` with no `WHERE` clause (the only
currently keyset-seek-eligible shape) to avoid this cost entirely.
Anything else pays the full-table-scan-per-page cost at the scale shown.

### FTS / functional index rebuild (restart, and `CREATE INDEX`)

Every `index2` backend (FTS, functional, vector) EXCEPT vector's own
snapshot-restore path does a full data-store rebuild on `restore_on_open`
— i.e. on every table open / server restart while such an index exists
(`table_manager.rs`, default `IndexBackend::restore_on_open`). Cost is
O(rows × indexes): each FTS/functional index on a table independently
re-scans and re-derives the FULL table on open — nothing is shared
across indexes.

`CREATE INDEX` for these families (and a restart-rebuild, which reruns
the identical backfill code) is NOT on the barrier-free "online build"
path landed for regular/hash in #1054–#1062 — it remains on the
ORIGINAL whole-write-barrier backfill, so the pre-online-build
regular/hash numbers apply directly as a per-index cost floor (FTS/
functional per-row cost is typically HIGHER than plain hash indexing —
tokenization + BM25 stats for FTS, function invocation for functional —
so these are a floor, not a ceiling):

| Table size | Build/rebuild duration (per index) | Source |
|---|---|---|
| 5k rows | 147–168 ms | `f78_writer_latency` bench, measured |
| 100k rows | ~140–160 s | `f78_writer_latency` bench, measured (P1-4/#969) |
| 1M rows | hours (extrapolated — scan is superlinear) | RFC `2026-08-07-online-index-build-rfc.md` §1.2 |

**Guidance:** N FTS/functional indexes on one table multiply restart
time roughly linearly (O(rows × indexes)) — budget accordingly for
tables with several such indexes at 100k+ rows. `doctor::repair()`'s
multi-family rebuild loop pays the same underlying cost per family.

### Vector index quality (HNSW recall) — #1070, 2026-08-11

60 fresh CI runs (20 × 3 OS: ubuntu/windows/macos) against both
statistical recall tests replaced outdated/incorrect doc claims:

- `restart_preserves_recall_at_10_against_brute_force` (**3K** vectors —
  docs previously and incorrectly said 10K): min 0.968 (ubuntu) / 0.970
  (windows) / 0.978 (macos), mean 0.983–0.987 across all three. Recall
  floor recalibrated to **0.90** (had been lowered to 0.60 chasing two
  historical single-run CI outliers that did not reproduce even once
  across the fresh 60-run sweep).
- `recall_at_10_on_1k_vectors` (single-query, so recall only lands on
  multiples of 0.1): every one of 60 runs landed at exactly 0.80 or 0.90
  across all 3 platforms, never lower. Floor raised to **0.75**. The
  docs' old "~95–99%" claim was never actually true — corrected to the
  real observed 80–90% range.

See `docs/guide-docs/guide/06-search.md` for the current, corrected
vector-index documentation and `.github/workflows/hnsw-recall-matrix.yml`
for the cross-platform matrix workflow that produced these numbers.

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
