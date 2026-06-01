בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ShamirDB — Project State

**Snapshot date:** 2026-06-01. The canonical "where we are" document — what
ShamirDB is, what shipped, and where it goes next. Roadmap index lives in
[`roadmap/NEXT_PHASES.md`](roadmap/NEXT_PHASES.md); per-feature plans under
[`roadmap/`](roadmap/).

Since the last snapshot: a **WASM function engine** ("M") was built
end-to-end ([`roadmap/FUNCTIONS.md`](roadmap/FUNCTIONS.md)) and the
behavior-preserving substrate of the **Shomer access fabric** landed
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

## 2. Architecture — 15 crates, layered bottom-up

| Crate | Role |
|---|---|
| `shamir-types` | Value model, RecordId, codecs, **sort_codec**, string→u64 interner |
| `shamir-storage` | `Store`/`Repo` trait + **6 backends** (Sled, Redb, Fjall, Nebari, Persy, Canopy), feature-gated |
| `shamir-wal` | WAL V2 (crash recovery) |
| `shamir-tx` | **MVCC over dumb-KV** (`<key>::<version_be>`), `RepoTxGate`, `TxContext`, SSI, **predicate locks** |
| `shamir-query-types` | Wire DTOs (filter/read/write/batch + `DbRequest`/`DbResponse`) |
| `shamir-engine` | Table engine, JSON batch query, planner, secondary indexes, HNSW vectors, FTS, **commit pipeline** |
| `shamir-db` | Facade: `ShamirDb`, SystemStore, durable DDL catalogue |
| `shamir-connect` | Wire protocol: TLS 1.3 + SCRAM-Argon2id + Ed25519 channel binding, session tickets |
| `shamir-transport-tcp` / `-ws` | TCP (TLS) and WebSocket (native + browser) |
| `shamir-server` | ServerLauncher: bootstrap, RBAC, audit HMAC chain, rate-limit, **interactive-tx registry** |
| `shamir-client` | Client SDK |
| `shamir-client-node` | napi binding (MSVC-only, excluded from the workspace) |
| `shamir-sdk` | function authoring SDK (guest): `Ctx`/`Batch`/`Params`/`Value`, host-call shims; builds to wasm32 |
| `shamir-sdk-macros` | the `#[shamir::function]` proc-macro — hides the whole guest ABI from the author |

The **function engine** itself lives in `shamir-engine` (`function/`:
runtime, registry, Wasmtime backend, host imports, gateways) + `shamir-db`
(durable catalogue, lifecycle API). The **Shomer access fabric** primitives
live in `shamir-types` (`access`: `Actor`/`ResourcePath`/`Action`/the gate).

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
- JSON batch query API: WHERE / SELECT (projections+aggregations) / GROUP BY
  / ORDER BY / LIMIT / pagination; cross-query refs (`{"$query": "@alias[].field"}`).
- Secondary indexes + query planner; sorted indexes; **HNSW vector** search;
  **FTS**; functional indexes; index2 backend; online migration.
- Admin DDL (Create/Drop Db/Repo/Table/Index, List), auth ops
  (User/Role/Grant/Revoke); multi-database / multi-repo system store with
  durable metadata.
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
- **Access fabric (Shomer)** — hierarchical POSIX-style DAC
  (owner/group/mode over a resource tree + setuid-style delegation +
  capability bits). The behavior-preserving substrate is in place (`Actor`
  threaded everywhere; one transparent `authorize` door); enforcement +
  metadata are the next non-refactor steps. See
  [`roadmap/ACCESS_FABRIC.md`](roadmap/ACCESS_FABRIC.md).
- Quality: **15 crates, ~1050+ engine lib tests** + integration; property tests
  (`proptest`: version codec + SSI read-set validation); green gate
  (`fmt --all --check` · `clippy --workspace --all-targets -D warnings` ·
  `test --workspace --lib` · `test --workspace --test '*'`).

---

## 4. What shipped this cycle (transactional layer → production-grade)

Commit arc roughly `2cfb7f6 → dfaed28`:

1. **Phase A hardening** (3 adversarial audit waves): read-your-own-writes,
   batched MVCC inserts, HNSW promote outside `commit_lock`, C6 empty-tx
   fast-path, honest multi-table deferral contract, idempotent recovery, D12
   atomic rid-slot-claim, observable `materialized` flag, alloc-free RYOW.
2. **Phase B — interactive multi-call transactions**: wire DTOs →
   `TxRegistry` → engine glue → facade → handler dispatch (ownership +
   single-repo pin), idle/absolute-deadline reaper, per-tx staging budget
   (`tx_too_large`).
3. **Phase C — true serializability (phantom protection)**: predicate/range
   SIREAD locks, `Filter → IndexRange` bridge (sort-codec), commit-time
   write-key log on `RepoTxGate`, Phase 2-bis in `pre_commit`
   (`PhantomConflict`), 22 anomaly/precision/zero-overhead tests.
4. **Quality & pipelines**: `proptest` property tests; three flaky tests
   de-flaked at the root (argon2 semaphore, sled transact observer,
   vector-migration approximate-top-k); roadmap planning docs + `NEXT_PHASES`
   index.
5. **CI hardened & green**: the `clippy` job had been red since 2026-05-29
   from `@stable` drift (new lints on untouched code). Fixed durably by
   **pinning the toolchain** (`rust-toolchain.toml` + `dtolnay/rust-toolchain@1.93.0`
   in all CI jobs) so local and CI lint identically — green here == green in
   CI. Bumped `actions/checkout@v4 → @v5` (Node 20 EOL). Added
   [`../CONTRIBUTING.md`](../CONTRIBUTING.md): the exact four-command gate +
   the toolchain-bump procedure. Benches never run in CI (`--lib` +
   `--test '*'` exclude `[[bench]]`).

Method: multi-agent workflows (smart `aoh` research, parallel → `ao46l`
implementation, sequential → verify) with a zero-trust backstop review (diffs
and semantics confirmed by independent gate runs, never by agent claims).

---

## 5. Next steps

**Small tails (optional, non-blocking):**
- Phase B/C benches (zero-overhead already test-proven; measurement is nice
  to have) — see [`roadmap/PERF_OPPORTUNITIES.md`](roadmap/PERF_OPPORTUNITIES.md).
- Nightly `cargo-fuzz` target for the version codec (proptest covers the bulk).
- `.gitignore` housekeeping: `server-cert.pem`, `crates/shamir-client-node/target/`.
- Periodically bump the pinned toolchain (currently `1.93.0`) + fix any new
  lints — procedure in [`../CONTRIBUTING.md`](../CONTRIBUTING.md).
- Task #17 ("transactions within a batch — MemBuffer + WAL") is effectively
  **closed** by Phase A + Phase B.

**Priority recommendation:** the foundation (storage + transactions +
protocol + security) is solid, and the **WASM function engine ("M") shipped**
end-to-end. The highest-value next steps are now:
1. **Access enforcement (Shomer P4)** — flip the transparent door to the real
   POSIX check; add the metadata envelope (owner/group/mode) + provenance
   (`created_by`/`modified_by`). Substrate is already in place.
2. **Function wire-DDL (slice 10)** — manage functions via the JSON request
   API + a wire-level e2e.
3. **Replication / P2P (the "I")** or **query language v2** — the remaining
   charter pillars.
Each taken the same way: smart-agent research → implementation → zero-trust
verify → green CI. Discipline throughout: "don't over-build" — pull each
slice by real need, not ahead of it.

**Large directions** (per [`roadmap/`](roadmap/)):

| Direction | Plan |
|---|---|
| WASM modules (user logic — the "M") | not started |
| P2P / interconnected (chat — the "I") | not started |
| Replication / sharding / backup tooling | [`roadmap/ROADMAP.md`](roadmap/ROADMAP.md) |
| Query language v2 (SQL frontend); default→Serializable now phantoms are closed | [`roadmap/ROADMAP.md`](roadmap/ROADMAP.md), [`roadmap/TRANSACTIONS.md`](roadmap/TRANSACTIONS.md) |
| Browser WASM client (Argon2id in a Web Worker) | [`roadmap/BROWSER_WASM_PLAN.md`](roadmap/BROWSER_WASM_PLAN.md) |
| Vectors / embeddings hardening | [`roadmap/EMBEDDINGS_AND_VECTORS.md`](roadmap/EMBEDDINGS_AND_VECTORS.md) |
| Full-text search hardening | [`roadmap/FULL_TEXT_SEARCH.md`](roadmap/FULL_TEXT_SEARCH.md) |
| Transports: QUIC, UDP, Unix socket | [`roadmap/ROADMAP.md`](roadmap/ROADMAP.md) |
| Auth v1.1+ (HIBP, WebAuthn 2FA, DPoP); PQ identity (Ed25519+ML-DSA) | [`roadmap/ROADMAP.md`](roadmap/ROADMAP.md) |
| Production hardening / server plan | [`roadmap/PRODUCTION_HARDENING_ROADMAP.md`](roadmap/PRODUCTION_HARDENING_ROADMAP.md), [`roadmap/PRODUCTION_SERVER_PLAN.md`](roadmap/PRODUCTION_SERVER_PLAN.md) |

The transactional foundation is complete and solid (SI → SSI → true
serializability, single-batch + interactive, crash-safe, property-covered),
all on one backend-agnostic dumb-KV foundation. The natural next major step is
**WASM modules** ("M"), **replication / P2P** ("I"), or **query language v2**
(usability over the finished engine).

---

_Maintained as the project's state snapshot. Last updated 2026-05-31 after
Phase A hardening + Phase B (interactive tx) + Phase C (phantom protection)._
