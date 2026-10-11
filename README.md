בס״ד

לכבוד הקדוש ברוך הוא — *for the glory of the Holy One, blessed be He*

# S.H.A.M.I.R. Database

[![CI](https://github.com/PHPCraftdream/shamir-db/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/PHPCraftdream/shamir-db/actions/workflows/ci.yml)
[![Supply-chain checks](https://github.com/PHPCraftdream/shamir-db/actions/workflows/supply-chain.yml/badge.svg?branch=master)](https://github.com/PHPCraftdream/shamir-db/actions/workflows/supply-chain.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust 1.95.0](https://img.shields.io/badge/rust-1.95.0-orange.svg)](rust-toolchain.toml)

**S**ecure, **H**igh-performance, **A**synchronous, **M**odular, **I**nterconnected, **R**epository

An alpha, single-node document database server written in Rust — MVCC transactions over a WAL, interned field names, authenticated transports, WASM user functions, and built-in full-text and vector search in one binary.

> **Status: alpha preview — not recommended for production data.** The public API, wire protocol, and on-disk format will change incompatibly before the first stable release, and there is no in-place upgrade path. Backups are offline-only (stop the server first).

The repository is currently source-first: it provides a Cargo workspace, a server binary, Rust client crates, a TypeScript client, protocol specifications, and development documentation. Published binaries and stable release guarantees are not available yet.

## 🧭 What S.H.A.M.I.R. is (and isn't)

S.H.A.M.I.R. is an alpha-stage, single-node Rust database for greenfield self-hosted applications and experiments. It combines transactional document storage, an authenticated network protocol, WASM user functions, and full-text and vector search in one server binary. It has not been validated for production workloads.

It is **not** a replacement for PostgreSQL, MySQL, MongoDB, SQLite, Redis, or Memcached, and does not claim to be one. Where it overlaps with them (table below), the overlap is in data model or deployment shape — not in maturity, ecosystem, tooling, or operational guarantees.

| System | Overlap with S.H.A.M.I.R. | Verdict |
|---|---|---|
| SQLite | Embeddable, transactional | Different shape — S.H.A.M.I.R. is primarily a network server with multi-user auth, WASM functions, and vector search; SQLite is a mature in-process SQL library with a long-term file-format guarantee S.H.A.M.I.R. does not offer |
| MongoDB | Document data model, flexible schema | Partial overlap — no sharding/clustering, no aggregation-pipeline parity, alpha-stage query surface |
| PostgreSQL / MySQL | Mature relational engines, decades of ecosystem/tooling, mature query optimizers | **Not a replacement** — no SQL, no comparable optimizer maturity, alpha-only guarantees |
| Redis / Memcached | In-memory cache semantics, sub-millisecond hot-path latency | **Not a replacement** — S.H.A.M.I.R. is disk-durable transactional storage, not a cache |

**Alpha status.** See the [Project Status](#-project-status) section below and the [versioning/compatibility statement in CHANGELOG.md](CHANGELOG.md#versioning-scheme) — there is no supported in-place upgrade path between alpha releases yet. For a citation-backed list of current architectural limitations (transactions, schemas, indexes, subscriptions, replication, results, numbers), see [`docs/guide-docs/KNOWN_LIMITATIONS.md`](docs/guide-docs/KNOWN_LIMITATIONS.md).

---

*Русский эквивалент:*

S.H.A.M.I.R. — однонодовая база данных на Rust в стадии alpha для новых self-hosted приложений и экспериментов. Она объединяет транзакционное документное хранилище, аутентифицированный сетевой протокол, WASM-функции, полнотекстовый и векторный поиск в одном серверном бинарнике. Для production-нагрузок не валидирована.

Это **не** замена PostgreSQL, MySQL, MongoDB, SQLite, Redis или Memcached, и проект на это не претендует. Пересечение с ними — в модели данных или форме развёртывания, а не в зрелости, экосистеме и эксплуатационных гарантиях.

**Alpha-статус.** См. секцию [Project Status](#-project-status) ниже и [заявление о совместимости версий в CHANGELOG.md](CHANGELOG.md#versioning-scheme) — гарантированного пути обновления между alpha-версиями пока нет.

## 🎯 Project Status

**Version:** 0.1.0-alpha.1

**Current Features:**
- ✅ Feature-gated storage abstraction with the current durable Fjall backend
- ✅ Key interning system (strings → u64 for memory efficiency)
- ✅ Async streaming with batch generators
- ✅ High-level Table engine with UserValue/InnerValue transformations
- ✅ Multi-database / multi-repo system store with durable metadata
- ✅ Batch query API (MessagePack/QueryValue): WHERE / SELECT / GROUP BY / ORDER BY / LIMIT / pagination
- ✅ Cross-query references (`filter.queryRef('@alias', '[].field')`)
- ✅ Admin DDL (Create/Drop Db / Repo / Table / Index, List)
- ✅ Auth ops (Create/Drop User, Grant / Revoke Role)
- ✅ Secondary indexes + query planner
- ✅ Wire protocol: TLS 1.3 + SCRAM-Argon2id; TLS-exporter channel binding and Ed25519 server-identity pinning in the Rust client (the TypeScript client currently uses the browser profile: no channel binding, no identity pinning)
- ✅ Transports: TCP, WebSocket (native + browser), local IPC (Unix socket / Windows named pipe)
- ✅ Session resumption tickets (AES-256-GCM, anti-downgrade, multi-device families)
- ✅ Audit log with HMAC chain (authentication events only; DDL/ACL/admin operations are not yet audited)
- ✅ ACID Transactions: single-batch + **interactive multi-call** (`begin → execute* → commit/rollback`) Snapshot Isolation (SI) + Serializable (SSI) with **read-set validation guarding point/match-scan reads against concurrent writes** (see [`docs/guide-docs/KNOWN_LIMITATIONS.md`](docs/guide-docs/KNOWN_LIMITATIONS.md#1-transactions) for the streaming-scan phantom-protection scope cut — full SSI predicate/range locking over a stream is not yet covered), crash recovery via WAL V2
- ✅ MVCC versioned reads, history store GC, max-tx-lifetime enforcement, interactive-tx idle/lifetime reaper + per-tx staging budget
- ✅ Broad workspace test coverage, including property tests for the version codec and SSI read-set validation
- ✅ Offline `backup` / `restore` CLI (sha256 manifest, server must be stopped) and `doctor` integrity check

**Experimental (may change incompatibly or be removed):**
- 🧪 Leader→follower async read replicas — schema is not replicated, no automatic reseed, no failover
- 🧪 Storage-engine migration API — disabled by default, known data-loss gaps
- 🧪 Server-side Rust→WASM compilation — development only (needs a toolchain and a repository checkout on the host)

**Not available yet:** online backup, point-in-time recovery, in-place upgrades between alpha versions, published binaries/packages, comparative benchmarks.

**Planned Features (see [docs/dev-artifacts/roadmap/ROADMAP.md](docs/dev-artifacts/roadmap/ROADMAP.md)):**
- 🔜 Browser WASM client (Argon2id in Web Worker)
- 🔜 QUIC transport
- 🔜 Post-quantum hybrid handshake

## 🏗️ Architecture

Cargo workspace, layered from foundation upward. The default workspace currently contains 24 Rust crates; `shamir-client-node` is built separately with the MSVC toolchain and the TypeScript client is managed by npm.

```
crates/
├── shamir-types/         # Value model, identifiers, codecs, interner
├── shamir-storage/       # Store/Repo traits and storage backends
├── shamir-engine/        # Table engine, query language, and batch executor
├── shamir-query-types/   # Wire-shareable query DTOs
├── shamir-query-builder/ # Client-side typed query builder and macros
├── shamir-db/            # Top-level database facade
├── shamir-connect/       # Authenticated connection protocol
├── shamir-transport-*/   # TCP, WebSocket and local IPC transports
├── shamir-server/        # Server binary, listeners, access control, audit
├── shamir-sdk/           # WASM-facing SDK and macros
└── shamir-wasm-host/     # WASM compilation and execution boundary
```

Documentation:
- **[docs/guide-docs/architecture/LOGIC_FLOW.md](docs/guide-docs/architecture/LOGIC_FLOW.md)** — view from above: how a request travels through the crates
- **[docs/guide-docs/architecture/ARCHITECTURE.md](docs/guide-docs/architecture/ARCHITECTURE.md)** — DB internals (storage, types, indexes)
- **[docs/guide-docs/client-server-protocol-spec/](docs/guide-docs/client-server-protocol-spec/README.md)** — wire protocol (auth, session, transports)
- **[docs/guide-docs/guide/](docs/guide-docs/guide/README.md)** — progressive user and operator guide
- **[docs/dev-artifacts/roadmap/](docs/dev-artifacts/roadmap/ROADMAP.md)** — production hardening, server plan, feature roadmap
- **[docs/dev-artifacts/ops/](docs/dev-artifacts/ops/)** — capacity planning, perf tuning

## 🚀 Quick Start

Start with the [five-minute server and TypeScript client walkthrough](docs/guide-docs/guide/00-quickstart.md). For a local build:

```bash
cargo build --workspace
cargo run -p shamir-server -- --help
```

## 📦 Storage Backends

| Backend | Status | Notes |
|---------|--------|-------|
| **Fjall** | ✅ Supported | Durable LSM-style backend used by the server |

## 🧪 Testing

Tests run through the `cargo-nextest` wrapper — raw `cargo test` is blocked
by a perimeter guard in `.cargo/config.toml` (see `CLAUDE.md`'s "Centralised
test entry point" section for why).

```bash
# Full workspace test sweep (lib tests, all crates — fastest signal)
./scripts/test.sh

# Specific crate
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-server
```

The repository's required pre-commit checks are documented in [CONTRIBUTING.md](CONTRIBUTING.md). The Node.js end-to-end suite has separate prerequisites; see [tests/e2e/README.md](tests/e2e/README.md).

## 📊 Performance

- **Benchmarks**: no comparative benchmarks against other databases have been published; internal benchmarks live under `crates/*/benches/` and are host-specific, not a performance claim
- **Interning**: field names are stored as interned `u64` ids (size reduction depends on the schema; not yet measured)
- **Streaming**: bounds wire/client-side memory for unsorted result streams; `ORDER BY`/`GROUP BY`/`DISTINCT` queries and cursor bookmarks still materialize/sort the full matching set server-side before the first page returns (see [`docs/guide-docs/KNOWN_LIMITATIONS.md`](docs/guide-docs/KNOWN_LIMITATIONS.md#6-results))
- **Scale envelope**: unique/sorted/FTS/functional/vector `CREATE INDEX` blocks writers for the whole backfill (~140–160 s at 100k rows) and FTS/functional indexes are rebuilt on every restart — see [`docs/guide-docs/KNOWN_LIMITATIONS.md`](docs/guide-docs/KNOWN_LIMITATIONS.md) §11

## 🤝 Contributing

Issues and pull requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) before making changes. For security vulnerabilities, follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

## 📚 Documentation

- [Documentation index](docs/README.md)
- [Guided usage documentation](docs/guide-docs/guide/README.md)
- [Architecture](docs/guide-docs/architecture/ARCHITECTURE.md)
- [Client/server protocol](docs/guide-docs/client-server-protocol-spec/README.md)
- [Roadmap](docs/dev-artifacts/roadmap/ROADMAP.md)
- [Security model and data protection](docs/guide-docs/security/data-protection.md)

## 📝 License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

---

**Made with ❤️ in Rust**
