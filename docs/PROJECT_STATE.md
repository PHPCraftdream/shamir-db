בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ShamirDB — Project State

**Snapshot date:** 2026-06-10. The canonical "where we are" document — what
ShamirDB is, what shipped, and where it goes next. The actionable forward
plan lives in [`roadmap/PLAN.md`](roadmap/PLAN.md); the (now historical)
transactional index in [`roadmap/NEXT_PHASES.md`](roadmap/NEXT_PHASES.md);
per-feature plans under [`roadmap/`](roadmap/).

Since the last snapshot (`a620115`..`f80070b`, 152 commits), a dense
**second arc** landed across six fronts: **temporal reads + single-log
MVCC** (#237/#238/#232 — the old TOCTOU is structurally dissolved);
**covering index** (Opt O / #218/#236 — Postgres-class range latency);
**Level-3 pessimistic locking** (#234/#235 — wound-wait, deadlock-free by
construction); **stored procedures** (BatchOp::Call first-class);
**nested batches** (#282 — sub-tx scope, $param in write values,
NestingTooDeep guard); **duplex/multiplexing** (#292–#298 — splittable
Framer, rid-demux, resume fast-path); a **full TypeScript client**
(TS-T0–T17 — platform-agnostic core, OQL/Batch/Call builders, live-server
e2e); a **shamir-tunables** crate (zero-overhead atomic runtime knobs);
**WASM/SDK slimming** (−34 % wasm size, guest-lean query-types, builder
compiles to wasm32, runs in guest); **runtime perf** (InstancePre cache
−40 %, AOT disk cache ~2×, M1/M2 fast paths); and an **internal
refactor** that extracted `shamir-wasm-host` and `shamir-index` from the
engine monolith and migrated all inline tests to `tests/` directories. See
[`roadmap/PLAN.md`](roadmap/PLAN.md),
[`roadmap/TEMPORAL.md`](roadmap/TEMPORAL.md),
[`roadmap/MVCC_CELL.md`](roadmap/MVCC_CELL.md).

Earlier: a **write-lifecycle arc** (`016d68b`..`a620115`): DDL completed,
Shomer access fabric enforced, validators + CAS, changefeed. Before that:
**WASM function engine** ("M")
([`roadmap/FUNCTIONS.md`](roadmap/FUNCTIONS.md)) and the substrate of the
**Shomer access fabric**
([`roadmap/ACCESS_FABRIC.md`](roadmap/ACCESS_FABRIC.md),
[`roadmap/ACCESS_REFACTOR.md`](roadmap/ACCESS_REFACTOR.md)).

---

## 1. What it is

**ShamirDB** — a production-grade, self-contained, decentralized database in
Rust. One binary (< 50 MB), no external runtime dependencies. The name is the
charter:

**S**ecure (Rust) · **H**igh-performance · **A**synchronous ·
**M**odular (WASM) · **I**nterconnected (P2P / chat) · **R**epository.

Design principles: self-contained (one binary); hybrid storage (records are
MessagePack, field names interned to `u64`); WASM-first user logic;
reliability (checksums everywhere, backends own durability, WAL handles crash
recovery). Dual-licensed **MIT OR Apache-2.0**.

---

## 2. Architecture — 21 crates, layered bottom-up

21 crates in the workspace. Plus 2 non-workspace packages: `shamir-client-node`
(napi/MSVC binding, excluded) and `shamir-client-ts` (pure-TypeScript package,
no Cargo.toml).

| Crate | Role |
|---|---|
| `shamir-collections` | TMap/TSet leaf, guest-lean, re-exported by `shamir-types` |
| `shamir-types` | Value model, RecordId, codecs, **sort_codec**, string→u64 interner |
| `shamir-storage` | `Store`/`Repo` trait + **6 backends** (Sled, Redb, Fjall, Nebari, Persy, Canopy), feature-gated |
| `shamir-tunables` | Runtime tunables — zero-overhead atomic knob reads |
| `shamir-wal` | WAL V2 (crash recovery) |
| `shamir-tx` | **MVCC over dumb-KV** — now a **single append-only version-log**; `RepoTxGate`, `TxContext`, SSI, predicate locks, **Level-3 pessimistic locking** |
| `shamir-query-types` | Wire DTOs (filter/read/write/batch + `DbRequest`/`DbResponse`); guest-lean via `server` feature-gate |
| `shamir-query-builder` | Typed Rust query builder — `q!`/`filter!`/`doc!`; guest-lean, compiles to wasm32 |
| `shamir-query-builder-macros` | Proc-macro support for `shamir-query-builder` |
| `shamir-funclib` | Built-in scalar + aggregate function library |
| `shamir-wasm-host` | WASM function host — Wasmtime runtime, registry, host imports, gateways, compile-from-source; **extracted from engine this cycle** |
| `shamir-index` | Index subsystem — legacy IndexManager/SortedIndexManager + FTS/Functional/Vector (HNSW) backends + **covering index** + meta-envelope; **extracted from engine this cycle** |
| `shamir-engine` | Table engine, OQL batch query, planner, **commit pipeline**, **temporal reads**; function engine and index subsystem live in their own crates and are re-exported |
| `shamir-db` | Facade: `ShamirDb`, SystemStore, durable DDL catalogue |
| `shamir-connect` | Wire protocol: TLS 1.3 + SCRAM-Argon2id + Ed25519 channel binding, session tickets |
| `shamir-transport-tcp` / `-ws` | TCP/TLS and WebSocket native+browser; **splittable Framer** (FrameReader/FrameWriter) |
| `shamir-server` | ServerLauncher: bootstrap, RBAC, audit HMAC chain, rate-limit, interactive-tx registry, **duplex request loop** |
| `shamir-client` | Client SDK; **rid-demux multiplexer**, resume fast-path |
| `shamir-sdk` | Function authoring SDK (guest): typed `#[scalar]`/`#[procedure]` kinds, builder-in-guest; builds to wasm32 |
| `shamir-sdk-macros` | Proc-macros for `shamir-sdk` |

The **Shomer access fabric** primitives live in `shamir-types` (`access`:
`Actor`/`ResourcePath`/`Action`/the gate).

**Architectural leitmotif** (held throughout): *the truth lives in one place
— the versioned MVCC store; everything derived (indexes, the HNSW graph,
counters, the interner, predicate locks) is a recoverable, lock-free overlay;
the WAL is the materialization guarantor.* The transactional layer is built
**above** the dumb-KV trait — so it behaves identically on every backend and
never leaks backend identity.

Concurrency invariants (engine hot paths): `scc::HashMap` / `ArcSwap` /
atomics; `tokio::sync::Mutex` only across `.await` with bounded contention;
`spawn_blocking` for CPU-bound work. Avoid `std`/`parking_lot` locks in hot
paths.

---

## 3. Capabilities (implemented)

- 6 storage backends behind one trait; key interning (~70% memory cut on
  string-heavy data); async streaming with constant memory.
- MessagePack batch query API (OQL): WHERE / SELECT (projections+aggregations) / GROUP BY
  / ORDER BY / LIMIT / pagination; cross-query refs (`{"$query": "@alias[].field"}`).
- Secondary indexes + query planner; sorted indexes; **HNSW vector** search;
  **FTS**; functional indexes; index2 backend; online migration.
- **Admin DDL — complete:** Create/Drop Db/Repo/Table/Index/Function/
  Validator/FunctionFolder, bind/unbind validators, List/introspection,
  auth ops (User/Role/Grant/Revoke), access-control (chmod/chown/chgrp,
  groups); idempotency (`if_not_exists`/`cascade`), referential integrity
  (refuse drop of non-empty), structured error codes (`exists`/
  `not_found`/`access_denied`/`still_referenced`); DDL↔DML ordering via
  `after` edges; first-class builder methods (DDL reads like DML).
  Multi-database / multi-repo system store with durable metadata.
- **Validators + sequenced writes:** WASM CHECK/BEFORE-write validators
  (per-table, priority-ordered, op-bound, fail-closed) seeing old+new
  record; optimistic **CAS** via a canonical, key-order-independent
  record hash (`crypto/canonical_hash`) — "blockchain-of-order" guard.
- **Changefeed (CDC):** hybrid **live-push** (`tokio::broadcast`, never
  blocks the commit) + **durable journal** (per-repo, keyed by
  `commit_version`, resumable `read_changelog_from`); fires on tx and
  non-tx writes; event version equals the data's MVCC version
  (replication-ready). The AFTER half of the write lifecycle.
- **Transactions — full isolation spectrum:**
  - Snapshot Isolation (SI) + Serializable Snapshot Isolation (SSI,
    write-skew) — **Phase A**.
  - **Phantom protection → true serializability** via predicate/range SIREAD
    locks — **Phase C**.
  - **Interactive multi-call** (`begin → execute* → commit/rollback`) with
    session-scoped state, idle/lifetime reaper, per-tx staging budget —
    **Phase B**.
  - Crash recovery via WAL V2 (WAL-as-commit-point; recovery is the
    materialization guarantor); MVCC versioned reads, history GC,
    max-tx-lifetime.
- Wire protocol: TLS 1.3 + SCRAM-Argon2id + Ed25519 channel binding; session
  resumption tickets (AES-256-GCM, anti-downgrade, multi-device); audit log
  HMAC chain; RBAC; password-at-rest; rate-limit + persistent lockout
  snapshots; WS pre-auth frame cap; exponential auth backoff.
- Transports: TCP, WebSocket (native + browser).
- **Functions (WASM, the "M")** — user-defined `async fn(ctx, batch, params)
  -> Result<Value>` (author sees user data only; the ABI/memory/fuel/msgpack
  are macro-generated and hidden). Compile-from-source (Rust→wasm via cargo,
  toolchain-gated) or submit `.wasm`; durable catalogue with load-on-open
  (a runner without cargo still runs functions); sandboxed (fuel + memory
  limits); per-batch scratch context + process globals + env seeding;
  function-calls-function (`ctx.call`, depth-bounded); DB read/write
  (`ctx.db()`, autocommit); outbound HTTP (`ctx.http_fetch`, curl wrapper,
  allowlist deny-default); per-function secret grants. See
  [`roadmap/FUNCTIONS.md`](roadmap/FUNCTIONS.md).
- **Access fabric (Shomer) — enforced.** Hierarchical POSIX-style DAC
  (owner/group/mode over a resource tree). The gate is live: ancestor
  Execute-traversal + `permits` first-match on the target, `System`
  bypass; applied to every **DDL/admin op** and every **DML op**
  (table-level, on both the batch and interactive-tx paths). Default mode
  is open (`0o777`) so enforcement breaks nothing until a resource is
  `chmod`-ed — no global flag. **Owner delegation** (a DB owner manages
  users scoped to their DB) and **getter-only** users (setuid = SECURITY
  DEFINER: Execute a function with no table Read → a data-firewall through
  procedures) both proven end-to-end. See
  [`roadmap/ACCESS_FABRIC.md`](roadmap/ACCESS_FABRIC.md).
- **Temporal OQL** (additive, default-invisible): `History` (per-record version
  timeline), `AsOf` (point-in-time read), `PurgeHistory` (imperative
  time-predicate purge), `ChangesSince` (one-shot delta since version V);
  builder methods `as_of`/`history`/`with_version`. Retention: per-table
  `max_count`/`min_count`/`max_age` knobs, eager vacuum for `CurrentOnly`
  default. Single append-only version-log — the old MVCC-2 TOCTOU is
  dissolved by construction. See [`roadmap/TEMPORAL.md`](roadmap/TEMPORAL.md),
  [`roadmap/MVCC_CELL.md`](roadmap/MVCC_CELL.md).
- **Covering index** (Opt O): DDL `include:[...]`; index entry stores bincode
  projected fields; write-path maintenance; planner recognises covered queries;
  index-only reads reusing the M2 streaming serialiser — zero data-store touch
  for covered range queries.
- **Level-3 pessimistic locking**: wound-wait on the monotonic version (total
  order → deadlock-free by construction); block-conflictors isolation level;
  complements SI (L1) + SSI (L2).
- **Stored procedures**: `BatchOp::Call` (wire + core); join the dependency
  graph (params + result as `$query`); `Batch::call` + `q!(call ...)`.
- **Nested batches**: sub-batches with their own tx scope; `$param` in
  write-op values; recursive own-tx execution; `NestingTooDeep` guard;
  builder `sub_batch`/`param`.
- **Duplex / multiplexing**: splittable Framer (`FrameReader`/`FrameWriter`);
  duplex request loop (concurrent per-connection dispatch); rid-demux
  multiplexer (concurrent requests over a single connection) in both Rust and
  TS clients; resume fast-path (reconnect via ticket, skip Argon2id).
- **TypeScript client** (`shamir-client-ts`): platform-agnostic core + Node/
  Browser adapters + WS/SCRAM transport; OQL read/write/DDL/ACL/RBAC builders;
  Batch + Call fluent builders; interactive-tx + `db.tx(fn)` auto-managed
  wrapper; `$query`/`$ref` helpers; bound `Db` handle; rid-demux; live-server
  e2e; README.
- **shamir-tunables**: zero-overhead atomic runtime knobs; storage/engine/tx
  scan batches + server frame-buffer/poll-interval routed through knobs.
- **WASM/SDK slimming + thin-waist**: wasm size −34 %; typed
  `#[scalar]`/`#[procedure]` macros; query-types `server` feature-gate
  (guest-lean); `shamir-collections` leaf; query builder compiles to wasm32
  and runs in the guest (`ctx.db().execute(&Batch)` via `db_execute` host
  import). Example guest 337 KB.
- **Runtime perf**: InstancePre cache (per-call −40 %); AOT disk cache (~2×
  restart compile); pooling allocator + CoW (+12 % concurrent); M1
  single-column columnar ORDER BY fast path; M2 streaming msgpack projection for
  SELECT * (3.4×); H₂ `Persistable` trait + `PersistRegistry`.
- Quality: **21 crates, ~2912 lib tests** + integration; property tests
  (`proptest`: version codec + SSI read-set validation); **29 benchmarks**
  across engine/tx/storage/connect/server; green gate (`fmt --all --check`
  · `clippy --workspace --all-targets -D warnings` · `test --workspace
  --lib` · `test --workspace --test '*'`).

---

## 4. What shipped this cycle (temporal + single-log MVCC, covering index, duplex, TS client, tunables, refactor)

Commit arc `a620115`..`f80070b` (152 commits):

1. **Temporal + single-log MVCC** (#237/#238/#232) — the headline arc. MVCC
   store is now ONE append-only version-log (dual-write main/archive
   eliminated). `History`/`AsOf`/`PurgeHistory`/`ChangesSince` OQL ops;
   builder temporal methods; per-table retention knobs; eager vacuum. The old
   MVCC-2 TOCTOU dissolved by construction.
2. **Covering index** (Opt O / #218/#236) — DDL `include:[...]`; index entry
   stores bincode projected fields; write-path maintenance; planner covers
   queries; index-only reads reuse M2 streaming serialiser — zero data-store
   touch for covered range queries. Backed by versioned covering-posting
   envelope + RecordCell high-water mark.
3. **Level-3 pessimistic locking** (#234/#235) — wound-wait on monotonic
   version; deadlock-free by construction; block-conflictors isolation level.
4. **Stored procedures** — `BatchOp::Call`; join dependency graph; `Batch::call`
   + `q!(call ...)`.
5. **Nested batches** (#282) — sub-tx scope; `$param` in write-op values;
   recursive execution; `NestingTooDeep` guard; builder `sub_batch`/`param`.
6. **Duplex / multiplexing** (#292–#298) — splittable Framer; duplex request
   loop; rid-demux multiplexer in both Rust and TS clients; resume fast-path.
7. **TypeScript client** (TS-T0–T17) — full pure-TS client: platform-agnostic
   core + Node/Browser adapters; OQL/Batch/Call builders; interactive-tx;
   rid-demux; live-server e2e; README.
8. **shamir-tunables** (new crate) — zero-overhead atomic runtime knobs;
   storage/engine/tx/server parameters routed through knobs; TUNABLES.md index.
9. **WASM/SDK slimming + thin-waist** — wasm −34 %; typed `#[scalar]`/
   `#[procedure]` macros; `shamir-collections` leaf; query builder runs in
   guest. Example guest 337 KB.
10. **Runtime perf** — InstancePre cache −40 %; AOT disk cache ~2×; M1/M2
    fast paths (columnar ORDER BY, streaming SELECT *); H₂ `Persistable` trait.
11. **Consolidation / security** (Movement A) — 13 unauthorized admin/DDL ops
    gated; fail-closed `effective_fn_actor`; `canonical_hash` round-trip;
    non-tx writes join SSI ledger; durable-journal watermark + gap signal;
    trust-boundary truncations closed.
12. **Internal refactor** — `shamir-wasm-host` and `shamir-index` extracted
    from the engine monolith; all inline tests migrated to `tests/` dirs; monolith
    trimmed ~64 k → ~46 k lines; `shamir-index` consolidated the legacy
    IndexManager/SortedIndexManager.
13. **CI** — cross-platform matrix (ubuntu/windows/macos).

Method: multi-agent workflows (smart research, parallel implementation,
sequential verify) with zero-trust backstop review (diffs and semantics
confirmed by independent gate runs, never by agent claims).

---

## 5. Next steps

**Charter status.** S (Secure), H (High-performance), A (Async), M (Modular
WASM), R (Repository) are all closed. The one open pillar is **I
(Interconnected)**. Movement A (consolidate) ✅ DONE; Movement B (perf, incl.
covering index) ✅ DONE; **Movement C (the "I") is the live frontier.**

The foundation for "I" is fully laid: changefeed with event version ==
`commit_version`, `ChangesSince` one-shot query, durable journal + watermark +
gap signal, duplex transport + rid-demux, resume tickets, written subscriptions
design doc (#201). The ladder:

1. **Network changefeed** — wire-streaming over the journal; `ChangesSince` is
   the one-shot precursor.
2. **Live subscriptions / server-push** (#201) — design written; the duplex
   channel is the pipe.
3. **Leader-follower replication** — apply by `commit_version`.
4. **P2P / gossip → chat.**

Pull-on-demand parallels (by real need, not ahead): browser-WASM client, QUIC/
UDP/Unix transports, auth v1.1+ / PQ identity, vectors/FTS hardening, backup/
restore tooling, non-blocking batched namespaced logging.

**Large directions** (per [`roadmap/`](roadmap/)):

| Direction | Plan |
|---|---|
| WASM modules (user logic — the "M") | ✅ **shipped** ([`roadmap/FUNCTIONS.md`](roadmap/FUNCTIONS.md)) |
| Temporal reads + single-log MVCC | ✅ **shipped** ([`roadmap/TEMPORAL.md`](roadmap/TEMPORAL.md), [`roadmap/MVCC_CELL.md`](roadmap/MVCC_CELL.md)) |
| Covering index (Opt O) | ✅ **shipped** |
| Level-3 pessimistic locking | ✅ **shipped** |
| TypeScript client | ✅ **shipped** |
| P2P / interconnected (chat — the "I") | Movement C — ladder above; live subscriptions design in [`roadmap/PLAN.md`](roadmap/PLAN.md) |
| Replication / sharding / backup tooling | [`roadmap/PLAN.md`](roadmap/PLAN.md) (replication), [`roadmap/ROADMAP.md`](roadmap/ROADMAP.md) |
| Query language: **OQL is final — no textual/SQL frontend, ever** | [`roadmap/PLAN.md`](roadmap/PLAN.md), [`roadmap/TRANSACTIONS.md`](roadmap/TRANSACTIONS.md) |
| Browser WASM client (Argon2id in a Web Worker) | [`roadmap/BROWSER_WASM_PLAN.md`](roadmap/BROWSER_WASM_PLAN.md) |
| Vectors / embeddings hardening | [`roadmap/EMBEDDINGS_AND_VECTORS.md`](roadmap/EMBEDDINGS_AND_VECTORS.md) |
| Full-text search hardening | [`roadmap/FULL_TEXT_SEARCH.md`](roadmap/FULL_TEXT_SEARCH.md) |
| Transports: QUIC, UDP, Unix socket | [`roadmap/ROADMAP.md`](roadmap/ROADMAP.md) |
| Auth v1.1+ (HIBP, WebAuthn 2FA, DPoP); PQ identity (Ed25519+ML-DSA) | [`roadmap/ROADMAP.md`](roadmap/ROADMAP.md) |
| Production hardening / server plan | [`roadmap/PRODUCTION_HARDENING_ROADMAP.md`](roadmap/PRODUCTION_HARDENING_ROADMAP.md), [`roadmap/PRODUCTION_SERVER_PLAN.md`](roadmap/PRODUCTION_SERVER_PLAN.md) |

The foundation (storage + MVCC + transactions + protocol + security + WASM +
DDL + access + changefeed + temporal + covering index + duplex + TS client) is
complete and solid. The natural next step is **Movement C — build the "I"
(subscriptions → replication → P2P)** — ordered in
[`roadmap/PLAN.md`](roadmap/PLAN.md).

---

_Maintained as the project's state snapshot. Last updated 2026-06-10 after
the temporal + single-log MVCC, covering index, Level-3 pessimistic locking,
stored procedures, nested batches, duplex/rid-demux, TypeScript client,
shamir-tunables, WASM slimming, perf, and internal-refactor arc
(`a620115`..`f80070b`): Movement A (consolidate) and Movement B (perf) are
DONE; Movement C (the "I" — subscriptions → replication → P2P) is the live
frontier. Prior: DDL → access → write-lifecycle arc (`016d68b`..`a620115`)._
