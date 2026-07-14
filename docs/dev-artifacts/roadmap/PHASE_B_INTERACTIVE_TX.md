בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase B — Interactive (Multi-Call) Transactions — Design

**Status: IMPLEMENTED (2026-05-31).** Stages 1-10 landed: wire DTOs
(`TxBegin/TxExecute/TxCommit/TxRollback`), server-side `TxRegistry`,
engine glue (`open/execute/commit_interactive_tx`), facade methods, handler
dispatch (ownership + single-repo pin + per-tx staging budget `tx_too_large`),
a background idle/absolute-deadline reaper, and the full test matrix
(wire-level e2e, SSI write-skew across calls, two-tx SI race, crash-mid-tx
leaves no durable footprint). Builds on **Phase A** (single-batch SI/SSI ACID
with WAL crash-recovery) — see [`docs/dev-artifacts/pre-transactional/REVIEW.md`](../pre-transactional/REVIEW.md)
for the Phase-A snapshot and [`TRANSACTIONS.md`](./TRANSACTIONS.md)
§"Phase B — interactive transactions (later)" for the original two-phase
split. The design below records the *how*; it matches what shipped.

This document is the Phase-B counterpart of [`TRANSACTIONS_IMPL.md`](./TRANSACTIONS_IMPL.md):
it gives the *how* — concrete recon of today's single-batch machinery, the
minimal wire/server/engine changes to make a `TxContext` survive across
client round-trips, the rule that keeps the existing single-batch path at
zero overhead, the honest list of where it will hurt, and the test
scenarios that will prove it works under contention.

> **Companion / sibling planning docs** (forward links — these are the
> sibling files this plan slots into; some may land in a parallel
> documentation pass):
> - [`NEXT_PHASES.md`](./NEXT_PHASES.md) — overview / index of the
>   post-Phase-A roadmap (Phase A tails → Phase B → Phase C).
> - [`PHASE_C_SERIALIZABLE.md`](./PHASE_C_SERIALIZABLE.md) — phantom /
>   predicate locks. **Phase B stresses the SSI read-set lifetime across
>   round-trips but defers phantom protection to Phase C.**
> - [`PHASE_A_TAILS.md`](./PHASE_A_TAILS.md) — Phase-A hardening tail
>   (the §11 follow-ups in `REVIEW.md`: real-crash subprocess harness,
>   `table_by_token` commit-hot-path cache, property/fuzz coverage).
> - [`TRANSACTIONS.md`](./TRANSACTIONS.md) — Phase A design.
> - [`TRANSACTIONS_IMPL.md`](./TRANSACTIONS_IMPL.md) — Phase A
>   implementation analysis (this doc mirrors its depth + house style).
> - [`docs/dev-artifacts/pre-transactional/REVIEW.md`](../pre-transactional/REVIEW.md)
>   §11 — honest open boundaries on top of Phase A.

---

## 1. Зачем это нужно — по-русски

### Что разблокирует interactive tx

Сегодня транзакция живёт **ровно один batch** — один вызов
`ShamirDb::execute(db, batch)`. Это видно в исполнителе:
`execute_transactional` сам открывает tx (`repo.begin_tx`,
`crates/shamir-engine/src/query/batch/executor.rs:267`), прогоняет план
(`execute_plan_tx`, `:194`) и **тут же** коммитит или абортит
(`repo.commit_tx(tx)`, `:291`) — всё внутри одного вызова. Клиент не
может «подержать» транзакцию открытой между сетевыми round-trip'ами.

Phase B снимает это ограничение. Жизненный цикл становится:

```
begin → N независимых batch'ей (каждый — отдельный сетевой запрос) → commit | rollback
```

Это разблокирует два класса сценариев, которые single-batch не покрывает:

1. **Read-modify-write через round-trip.** Клиент читает запись, считает
   что-то у себя (или показывает человеку), затем пишет обратно — и всё
   это на **одном консистентном snapshot'е**. С single-batch так нельзя:
   между двумя `execute` нет общего snapshot'а. (С Phase A read-after-write
   работает только *внутри* одного batch'а — см. тест
   `execute_batch_transactional_si_happy_path`, ссылка в
   `REVIEW.md:504`.)
2. **Client-driven multi-step.** Сложная бизнес-логика, где следующий шаг
   зависит от результата предыдущего, и нельзя заранее уложить весь план в
   один batch (например, потому что форма ветвления решается на клиенте
   или человеком в цикле).

### Архитектурный лейтмотив

Тот же принцип, что вёл Phase A: **истина живёт в одном месте — в
версионированном MVCC-store; всё производное (индексы, HNSW-граф,
счётчики, interner) — это overlay над ним, восстановимый и lock-free; WAL
остаётся гарантом материализации.**

Interactive tx — это **просто более долго живущий overlay.** Архитектурно
тут нет новой сущности: `TxContext`
(`crates/shamir-tx/src/tx_context.rs:51`) уже **есть** правильным
overlay'ем — `write_set`/`index_write_set`/`staged_vectors`/
`counter_deltas`/`interner_overlay`/`read_set`, и Drop = RAII rollback
без I/O (док-коммент `:50`). Phase A создавал и потреблял его внутри
одного batch'а; Phase B даёт ему пожить дольше. Гарант долговечности не
меняется: ничего не durable до commit'а, а commit'ом по-прежнему является
успешный `wal.begin` в Phase 4 (`crates/shamir-engine/src/tx/commit.rs:732`).
Crash в середине interactive tx = чистый abort (см. §8).

MVCC построен **над** «тупым» KV-`Store` (физический ключ =
`<key>::<version_be>`), backend-agnostic — Phase B **ничего** не добавляет
к `Store` trait и не протекает идентичностью бэкенда. Вся новизна — в
двух слоях выше: wire-протокол и серверный реестр живых tx.

---

## 2. The lifecycle — state machine

A handle is a server-minted opaque `TxId` (today `shamir_tx::TxId(pub u64)`,
`crates/shamir-tx/src/types.rs:12`, allocated by
`RepoTxGate::fresh_tx_id`, `crates/shamir-tx/src/repo_tx_gate.rs:86`). The
live `TxContext` is parked server-side, keyed by that handle and bound to
the authenticated session (§5).

```
                              client round-trip boundary = ─ ─ ─
                              server-resident state       = █████

   ┌────────────┐  BEGIN(repo, iso)
   │  (no tx)   │ ─────────────────────────►  mint TxId, open snapshot,
   └────────────┘                              park TxContext + SnapshotGuard
        ▲                                            │  reply { tx_handle, snapshot_version }
        │                                            ▼
        │                                  ┌──────────────────────┐
        │   COMMIT(handle) ── ok ─────────│   ████  OPEN  ████    │◄──┐
        │   (commit_tx pipeline runs)      │  TxContext accumulates │   │ EXECUTE(handle, batch)
        │                                  │  state across calls    │───┘ (no commit;
        │   ROLLBACK(handle) ── ok ────────│                        │      RYOW within tx)
        │   (drop TxContext = RAII)        └──────────────────────┘
        │                                            │
        │   idle-timeout / DEFAULT_MAX_TX_LIFETIME   │   disconnect (session gone)
        └───────────── abort ◄───────────────────────┘   ──► abort (drop TxContext)
```

State transitions, normatively:

- **`BEGIN(repo, isolation)`** — server calls the existing
  `RepoInstance::begin_tx(iso)`
  (`crates/shamir-engine/src/repo/repo_instance.rs:367`), which returns a
  `(TxContext, SnapshotGuard)` pair. Both are moved into the per-session
  registry (PROPOSED, §5). The reply carries the minted `tx_handle` and
  `snapshot_version` (`TxContext.snapshot_version`,
  `crates/shamir-tx/src/tx_context.rs:60`). **The `SnapshotGuard` must be
  parked alongside the `TxContext`** — its `Drop`
  (`crates/shamir-tx/src/repo_tx_gate.rs:61`) removes the snapshot from
  `active_snapshots`, which is what holds `min_alive` back for GC
  (`min_alive`, `:140`). Dropping it early would let GC reclaim versions
  the open tx still needs.
- **`EXECUTE(handle, batch)*`** — zero or more times. Each call threads the
  *existing* parked `TxContext` through `execute_plan_tx`
  (`executor.rs:194`) **without committing**. State accumulates: writes
  into `write_set`, index ops into `index_write_set`, SSI reads into
  `read_set`, etc. Read-your-own-writes (RYOW) holds across calls because
  the `TxContext` is the same object (the read path already consults the
  staging overlay — see `read_tx`, `executor.rs:413`).
- **`COMMIT(handle)`** — server removes the `TxContext` from the registry
  and runs the full Phase-A commit pipeline `commit_tx(tx, repo)`
  (`crates/shamir-engine/src/tx/commit.rs:341`). The `SnapshotGuard` is
  dropped only *after* `commit_tx` returns (snapshot must stay alive
  through commit so SSI validation and history reads remain correct). The
  reply is the existing `TransactionInfo`
  (`crates/shamir-query-types/src/batch/types.rs:491`), carrying
  `status`, `commit_version`, and `materialized`.
- **`ROLLBACK(handle)`** — server removes the `TxContext` and drops it.
  Drop = RAII rollback, no storage side-effects
  (`tx_context.rs:50`). The `SnapshotGuard` drops with it.
- **timeout / disconnect** — abort. Detected by the idle-sweep task and the
  connection close path (§7). Identical effect to ROLLBACK.

**One tx per session** (YAGNI, matches `TRANSACTIONS.md:360` "keep
nesting / parallel tx out"). A second `BEGIN` on a session that already
owns an open tx is rejected (PROPOSED error `tx_already_open`). This keeps
the registry a 1:1 map and sidesteps nested-tx semantics entirely.

---

## 3. Wire-protocol changes

### 3.1 Today's wire surface (recon)

The post-auth application payload is `DbRequest`
(`crates/shamir-query-types/src/wire/db_message.rs:23`), an internally-
tagged enum (`#[serde(tag = "op", rename_all = "snake_case")]`, `:22`).
The only data path is `DbRequest::Execute { query_version, db, batch }`
(`:30`) carrying a `BatchRequest`
(`crates/shamir-query-types/src/batch/types.rs:403`). Responses are
`DbResponse` (`db_message.rs:60`) — `Batch { response: BatchResponse }`
(`:64`) or `Error { code, message }` (`:78`). The whole thing rides inside
`RequestEnvelope`/`ResponseEnvelope`
(`crates/shamir-connect/src/common/envelope.rs:23` / `:156`); the envelope
already carries `session_id` (`:25`) and an optional `request_id` (`:29`).

`BatchRequest` already has `transactional: bool` (`types.rs:416`) and
`isolation: Option<String>` (`types.rs:427`). `BatchResponse` already
returns `transaction: Option<TransactionInfo>` (`types.rs:486`), and
`TransactionInfo` now carries `materialized: bool` (`types.rs:527`) with a
serde default (`default_materialized`, `:530`).

### 3.2 The change: new top-level request variants, not new BatchOps

Begin / commit / rollback are **session-lifecycle verbs**, not data
operations on a table. The cleanest mapping is **new `DbRequest`
variants**, not new `BatchOp` variants — because `BatchOp`
(`types.rs:32`) is dispatched by a per-op table-key discriminator
(`BatchOp::deserialize`, `types.rs:111`) and every variant either targets a
table or is admin DDL; a tx-lifecycle verb fits neither. Adding `BatchOp`
variants would also pollute the planner, the cross-repo guard
(`distinct_repos`, `types.rs:283`), and `is_admin` (`types.rs:248`).

**PROPOSED** `DbRequest` additions (all new variants → forward-compatible:
an internally-tagged enum simply fails to decode an unknown `op` on old
servers, which is the correct "unsupported" behaviour, exactly like the
`query_version` gate at `db_handler.rs:264`):

```rust
// PROPOSED — crates/shamir-query-types/src/wire/db_message.rs
pub enum DbRequest {
    Ping,
    Execute { query_version: u32, db: String, batch: BatchRequest },
    CreateScramUser { /* unchanged */ },

    // --- Phase B (PROPOSED) ---
    /// Open an interactive transaction. `repo` names the single repo the
    /// tx is scoped to (cross-repo stays out of scope — §3.4).
    TxBegin {
        #[serde(default = "default_query_version")] query_version: u32,
        db: String,
        repo: String,
        /// "snapshot" (default) | "serializable" — same vocabulary as
        /// BatchRequest.isolation (types.rs:427).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        isolation: Option<String>,
    },
    /// Run a batch inside an already-open interactive tx.
    TxExecute {
        #[serde(default = "default_query_version")] query_version: u32,
        db: String,
        tx_handle: u64,          // the TxId minted by TxBegin
        batch: BatchRequest,     // batch.transactional is IGNORED here (the
                                 // handle already establishes tx mode)
    },
    /// Commit an open interactive tx.
    TxCommit { db: String, tx_handle: u64 },
    /// Roll back (abort) an open interactive tx.
    TxRollback { db: String, tx_handle: u64 },
}
```

**PROPOSED** `DbResponse` additions:

```rust
// PROPOSED — crates/shamir-query-types/src/wire/db_message.rs
pub enum DbResponse {
    Pong,
    Batch { response: BatchResponse },
    UserCreated { /* … */ },
    Error { code: String, message: String },

    // --- Phase B (PROPOSED) ---
    /// Reply to TxBegin.
    TxOpened { tx_handle: u64, snapshot_version: u64, isolation: String },
    /// Reply to TxExecute — carries the SAME BatchResponse a non-tx
    /// Execute returns, EXCEPT `BatchResponse.transaction` stays `None`
    /// (the tx is still open; there is no per-call commit outcome yet).
    TxBatch { response: BatchResponse },
    /// Reply to TxCommit — the existing TransactionInfo, now produced at
    /// COMMIT time rather than per-batch.
    TxCommitted { transaction: TransactionInfo },
    /// Reply to TxRollback.
    TxRolledBack { tx_handle: u64 },
}
```

### 3.3 Alternative considered: `tx_handle` on `BatchRequest`

A lighter wire shape is to add `#[serde(default, skip_serializing_if =
"Option::is_none")] tx_handle: Option<u64>` to `BatchRequest`
(`types.rs:403`) and keep a single `DbRequest::Execute`. `begin`/`commit`/
`rollback` would still need *some* verbs, so this does not eliminate the
new variants — it only fuses `TxExecute` back into `Execute`. **Rejected
as the primary shape** because it overloads one request type with two
lifecycles (start-and-commit vs. mid-tx) and forces `db_handler::execute`
(`crates/shamir-server/src/db_handler.rs:256`) to branch on a payload field
to decide whether to touch the registry. The explicit-variant shape keeps
the dispatch in `RequestHandler::handle` (`db_handler.rs:231`) a clean
`match`. The `tx_handle`-on-batch idea is recorded here as a viable
fallback if wire-surface minimalism is later prioritised.

### 3.4 Backward-compat & invariants

- **Additive only.** No existing field changes type or meaning. The
  `materialized` precedent (`types.rs:526` `#[serde(default = ...)]`,
  proven by `transaction_info_missing_materialized_defaults_true`,
  `types.rs:900`) is the template: every new field is `#[serde(default)]`
  / `skip_serializing_if`.
- **`query_version`** is echoed and gated exactly like `Execute`
  (`check_query_lang`, `db_handler.rs:264`). No version bump is required to
  *add* variants (an old server rejects the unknown `op`); a bump of
  `CURRENT_QUERY_LANG_VERSION` (`db_message.rs:13`) is the maintainer's
  call if the team wants explicit negotiation. **This document does not
  bump it.**
- **Single-repo guard preserved.** `TxBegin` takes one `repo`; every
  `TxExecute` batch must target that same repo. The existing guard
  (`distinct_repos` + `BatchError::CrossRepoNotSupported`,
  `types.rs:283` / `:694`, fired at `executor.rs:52`) is re-used, now
  evaluated against the handle's pinned repo (§6). 2PC across repos stays
  out of scope (`REVIEW.md:28`).
- **`materialized` flows through `TxCommitted`** unchanged — the commit
  pipeline already returns it via `TxOutcome::materialized()`
  (`crates/shamir-engine/src/tx/commit.rs:76`), threaded into
  `TransactionInfo::committed(..)` at `executor.rs:299`.

---

## 4. Session-scoped `TxContext` lifetime

### 4.1 Where the live tx is parked

The `TxContext` + its `SnapshotGuard` must outlive a single dispatch and
be retrievable by `tx_handle` on the next call. Two options:

1. **Per-connection** (parked in `ConnectionContext`,
   `crates/shamir-server/src/connection.rs:117`, or the request loop's
   stack). **Rejected.** Dispatch runs on `spawn_blocking`
   (`connection.rs:712`) and the handler is invoked through a `dyn
   RequestHandler` (`connection.rs:739`) that receives only
   `(&Session, &[u8])` (`dispatch.rs:24`) — it has **no** handle to the
   connection task's locals. Tying tx lifetime to the TCP socket also
   breaks resume (a session can re-bind on a new connection within grace,
   `connection.rs:273`).
2. **Server-side registry keyed by `tx_handle`, bound to the session**
   (PROPOSED). **Chosen.** It survives across dispatches and across the
   `spawn_blocking` boundary, and it binds naturally to the authenticated
   identity the handler already receives (`&Session`, with
   `session.session_id` at `crates/shamir-connect/src/server/session.rs:93`
   and `session.user_id` at `:101`).

### 4.2 PROPOSED registry shape

The registry lives where `ShamirDbHandler` can reach it (it already owns
`Arc<ShamirDb>`, `crates/shamir-server/src/db_handler.rs:177`). It must
obey the repo's concurrency invariants (CLAUDE.md §Concurrency):
`scc::HashMap` for the shared registry, atomics for counters, and
`tokio::sync::Mutex` *only* where a guard must live across `.await` with
bounded contention.

```rust
// PROPOSED — new module, e.g. crates/shamir-server/src/tx_registry.rs
pub struct InteractiveTx {
    /// The live overlay. tokio::sync::Mutex because a TxExecute mutates it
    /// across `.await` (execute_plan_tx is async) and we must serialise
    /// concurrent TxExecute calls *for the same handle*. Contention is
    /// bounded: one tx per session, and a well-behaved client issues calls
    /// on a handle serially. This is the sanctioned use of
    /// tokio::sync::Mutex (CLAUDE.md: "across an .await … bounded contention").
    ctx: tokio::sync::Mutex<shamir_tx::TxContext>,
    /// Pins the MVCC snapshot for GC. Dropped at commit/rollback/timeout.
    snapshot: shamir_tx::SnapshotGuard,
    /// Owning session — abort-on-disconnect + cross-session-theft guard.
    owner_sid: [u8; 32],
    owner_user_id: [u8; 16],
    /// (db, repo) the handle is pinned to — every TxExecute must match.
    db: String,
    repo: String,
    /// Idle-timeout bookkeeping (monotonic). Bumped on each TxExecute.
    last_activity: std::time::Instant,
    /// Absolute deadline = created_at + DEFAULT_MAX_TX_LIFETIME.
    deadline: std::time::Instant,
}

// keyed by tx_handle (TxId.0). scc::HashMap = CAS-based, lock-free,
// no RwLock poisoning — exactly the registry primitive the repo mandates.
pub struct TxRegistry {
    open: scc::HashMap<u64, std::sync::Arc<InteractiveTx>>,
    /// One-tx-per-session enforcement: session_id → tx_handle.
    by_session: scc::HashMap<[u8; 32], u64>,
}
```

Notes:

- `TxContext` is **not** `Clone` and holds `scc::HashMap`/`AtomicU64`
  fields (`tx_context.rs:84`, `:89`, `:110`); parking it behind a
  `tokio::sync::Mutex` inside an `Arc` lets `TxExecute` take `&mut` across
  the async plan without moving it out of the registry. `commit`/`rollback`
  *remove* the `Arc<InteractiveTx>` from the map and then take ownership of
  the inner `TxContext` (via `Mutex::into_inner` after the last `Arc` ref
  is dropped, or by storing the ctx in an `Option` the remover swaps out).
- The `version_provider` for SSI is already attached at `begin_tx`
  (`repo_instance.rs:379`) when isolation is Serializable — parking the
  whole `TxContext` preserves it for the eventual commit-time
  `validate_read_set` (`tx_context.rs:250`, called in `pre_commit`,
  `commit.rs:603`).

### 4.3 Binding to the authenticated session

Every `TxExecute`/`TxCommit`/`TxRollback` carries `tx_handle`. The handler
(which holds `&Session`) MUST verify the handle's `owner_sid ==
session.session_id` (and/or `owner_user_id == session.user_id`) before
touching the tx. This prevents a different session — even the same user on
another connection — from driving or committing someone else's open tx.
The §7.5 validity check (`dispatch.rs:68`) already runs *before*
`handle`, so a kicked/expired session never reaches the registry; the
ownership check is the second, tx-scoped gate.

### 4.4 Concurrency-invariant implications

- **No `std::sync::*` / `parking_lot::*` in the registry hot path** —
  `scc::HashMap` for `open`/`by_session`, atomics for any counters
  (CLAUDE.md §Concurrency). `parking_lot::Mutex` appears on `Session`
  (`session.rs:122`) only for the changePassword challenge — not a tx
  path; we do not extend it.
- **The per-handle `tokio::sync::Mutex`** is the one across-await lock, and
  it is justified exactly as the repo permits: the guard lives across the
  `execute_plan_tx` await, and contention is bounded to a single client
  serially driving its own handle. Two *different* handles never contend
  (distinct map entries).
- **Commit still serialises through `RepoTxGate::commit_lock`**
  (`repo_tx_gate.rs:123`) — interactive commit reuses the identical
  critical section as Phase A. The registry mutex protects the *overlay*;
  the gate mutex protects the *commit*. They are orthogonal locks with no
  ABBA risk (the registry lock is released before `commit_tx` acquires the
  gate — the remover takes ownership of the ctx, drops the per-handle
  mutex, then calls `commit_tx`).

---

## 5. Executor changes

### 5.1 Today: create-and-commit inside one batch

`execute_transactional` (`crates/shamir-engine/src/query/batch/executor.rs:230`)
is a closed cycle:

1. resolve the single repo (`:253`),
2. `repo.begin_tx(iso)` → `(tx, _guard)` (`:267`),
3. `execute_plan_tx(plan, queries, resolver, admin, &mut tx)` (`:278`),
4. on `Ok` → `repo.commit_tx(tx)` (`:291`); on `Err` → drop tx = RAII
   rollback, return `TransactionInfo::aborted` (`:283`).

The `SnapshotGuard` (`_guard`, `:267`) is bound to the function scope and
dropped at return. The per-query routing in `QueryRunner::run`
(`:364`) already branches on `self.tx` for both reads (`read_tx`, `:413`)
and writes (`execute_*_tx`, `:423`/`:435`/`:447`/`:459`) — **this is the
exact machinery Phase B reuses, unchanged.**

### 5.2 Phase B: thread an existing handle's `TxContext`

Phase B splits the closed cycle into three reusable pieces. **No new
execution engine is needed** — `execute_plan_tx` already takes
`tx: &mut TxContext` (`:194`) and does not commit. The new code is glue
that (a) holds the `TxContext` across calls and (b) decouples the commit
from the plan run.

**PROPOSED** thin helpers (engine side, alongside `execute_transactional`):

```rust
// PROPOSED — executor.rs (or a sibling tx-session module)

/// BEGIN: factor out steps 1–2 of execute_transactional. Returns the
/// parked overlay + its snapshot guard for the server registry to hold.
pub async fn open_interactive_tx(
    repo: &RepoInstance,
    iso: shamir_tx::IsolationLevel,
) -> DbResult<(shamir_tx::TxContext, shamir_tx::SnapshotGuard)> {
    repo.begin_tx(iso).await   // already exists — repo_instance.rs:367
}

/// EXECUTE(handle): step 3 only — run one batch's plan against an EXISTING
/// tx, NO commit. Mirrors execute_plan_tx but driven from the registry's
/// parked &mut TxContext. Returns the BatchResponse with `transaction:
/// None` (the tx is still open).
pub async fn execute_in_open_tx(
    request: &BatchRequest,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    tx: &mut shamir_tx::TxContext,        // borrowed from registry mutex
) -> Result<BatchResponse, BatchError> {
    // cross-repo guard (executor.rs:52) re-evaluated against this batch;
    // additionally assert distinct_repos ⊆ {handle.repo} (caller passes it).
    // plan + validate_tables exactly as execute_batch (executor.rs:62/:65),
    // then execute_plan_tx(plan, &request.queries, resolver, admin, tx).
    // transaction field stays None.
}

/// COMMIT(handle): step 4's commit arm — consume the removed TxContext.
pub async fn commit_interactive_tx(
    repo: &RepoInstance,
    tx: shamir_tx::TxContext,             // moved out of the registry
) -> Result<TxOutcome, crate::tx::CommitError> {
    repo.commit_tx(tx).await              // already exists — repo_instance.rs:403
}
// ROLLBACK is just `drop(tx)` — RAII (tx_context.rs:50). No engine call.
```

The server's `ShamirDbHandler` (`db_handler.rs:177`) gains the registry
(§4) and a `match` arm per new `DbRequest` variant in `handle`
(`db_handler.rs:231` → dispatched from `:235`). The existing
`run_blocking` async-bridge (`db_handler.rs:619`) carries the
`open_interactive_tx`/`execute_in_open_tx`/`commit_interactive_tx`
futures exactly as it carries `db.execute` today (`db_handler.rs:323`).

### 5.3 What stays identical

- **Read/write routing** — `QueryRunner` (`executor.rs:351`) and its
  `tx`-aware arms are untouched.
- **SSI read-set population** — `read_tx` already records reads through a
  shared `&TxContext` (Vector I.1, `executor.rs:413`; `record_read_shared`,
  `tx_context.rs:226`). Across multiple `TxExecute` calls the read-set
  simply keeps growing in the same parked `TxContext` — **this is exactly
  the SSI-lifetime stress Phase C must reason about** (see §7 and
  `PHASE_C_SERIALIZABLE.md`).
- **Commit pipeline** — `commit_tx` (`commit.rs:341`) and all seven phases
  are reused byte-for-byte. The only difference is *when* it runs (on
  `TxCommit`, not at batch end).

---

## 6. Где станет больно (risks)

Honest, in the spirit of `TRANSACTIONS_IMPL.md` §"Где станет больно" and
`REVIEW.md` §11. Each item names the real mechanism it stresses.

### 6.1 Unbounded staging growth across many EXECUTEs

A single batch is implicitly bounded by `BatchLimits.max_queries`
(default 50, `types.rs:597`) and the server-side cap
(`QueryLimitsCap`, `db_handler.rs:151`, applied at `db_handler.rs:276`).
An interactive tx has **no such bound** — N `TxExecute` calls can each
write rows, growing `write_set` (`tx_context.rs:67`), `index_write_set`
(`:72`), `staged_vectors` (`:80`), and the read-set (`:110`) without limit.
`StagingStore` is an in-memory buffer; the WAL entry built at commit
(`wal_ops_from_tx`, `commit.rs:236`) materialises every staged op, and
`REVIEW.md` Q2 (`:400`) already flags "no `WalEntryV2` size cap".

**Mitigation (PROPOSED):** a per-tx byte/row budget enforced on every
`TxExecute`, checked against the accumulated `TxContext`. Exceeding →
abort the tx with `tx_too_large` (the name `REVIEW.md:410` already
reserves). Budget is config-driven (PROPOSED `[security.tx] max_tx_bytes`,
default e.g. 64 MiB per `REVIEW.md:410`). No new dependency; the check is
a running sum maintained as ops are staged. **Do not** bump any crate to
add this.

### 6.2 Long-held SSI read-set vs. version GC (the load-bearing tension)

The `SnapshotGuard` pins `min_alive`
(`min_alive`, `repo_tx_gate.rs:140`; GC respects it, `run_gc`,
`repo_instance.rs:584`; periodic via `spawn_gc_task`, `:544`). A
*long-lived* interactive tx therefore **pins old versions for its entire
wall-clock life** — exactly the "Long-running tx blocks GC" risk
(`TRANSACTIONS.md:384`) and "min_alive held back by the oldest open tx".
With Phase A a tx lived milliseconds; with Phase B a human-in-the-loop tx
could live minutes. History (the `MvccStore` history store,
`crates/shamir-tx/src/mvcc_store.rs`) grows for as long as the snapshot is
held, because every overwrite below an open snapshot archives the old
version (the zero-overhead branch flips on once `active_snapshots` is
non-empty — `mvcc_store.rs:61`/`:117`).

For SSI specifically, the `read_set` grows monotonically across calls and
is validated wholesale at commit (`validate_read_set`, `tx_context.rs:250`;
first-read-wins keeps the *earliest* version, `:226` doc) — a long tx that
read widely has a large, stale read-set and is *more* likely to hit
`SsiConflict` at commit (`commit.rs:611`). That is correct behaviour, but
operationally it means long interactive SSI txs will abort under
contention; clients must be prepared to retry.

**Mitigation:** the idle-timeout + `DEFAULT_MAX_TX_LIFETIME` cap (§6.4)
bounds how long any one tx can wedge GC. Telemetry already exists
(`TxMetrics`, `REVIEW.md:216`); a `gc_lag_versions` gauge
(`TRANSACTIONS.md:385`) is the operator's early warning. **Phantom
protection is explicitly deferred to Phase C** — Phase B only guarantees
the snapshot wall and commit-time read-set validation it inherits from
Phase A.

### 6.3 Abort-on-disconnect detection

When the TCP/WS connection drops, the request loop simply `break`s
(`connection.rs:690`) and closes the framer (`:277`); the **session
survives** in `SessionStore` for grace/resume (`:273`). So a disconnect is
*not* directly observable to the tx registry. Two PROPOSED detection paths,
used together:

1. **Session-GC hook.** When `SessionStore` evicts a session by idle TTL
   (referenced at `connection.rs:277`), fire a callback that aborts any tx
   owned by that `session_id` (drop `InteractiveTx` → drop `TxContext` +
   `SnapshotGuard`). This is the authoritative cleanup.
2. **Idle-tx sweep** (§6.4) as the backstop — even if the session lingers,
   an idle tx is reaped on its own timer.

A disconnect therefore degrades to "the tx idles out", which is safe:
nothing was durable (no `wal.begin` ran), so the abort is free (RAII).

### 6.4 Idle-tx timeout vs. `DEFAULT_MAX_TX_LIFETIME`

Phase A already enforces an **absolute** lifetime: `commit_tx_inner`
rejects a tx older than `DEFAULT_MAX_TX_LIFETIME` (5 min,
`commit.rs:81`/`:369`) via `TxContext::is_expired`
(`tx_context.rs:175`, using `started_at`, `:125`), surfaced as
`TxError::Expired` (`commit.rs:174`) and mapped to a `tx expired` reason in
the executor (`executor.rs:314`).

Phase B needs **two** clocks, both enforced by a PROPOSED background sweep
(mirroring `spawn_gc_task`, `repo_instance.rs:544`):

- **Absolute deadline** = `created_at + DEFAULT_MAX_TX_LIFETIME` — reuse
  the existing 5-min constant so a stuck interactive tx cannot pin GC
  forever. (At commit, `is_expired` is the final backstop even if the
  sweep missed it.)
- **Idle timeout** (PROPOSED, shorter, e.g. 30 s per `TRANSACTIONS.md:357`)
  = `last_activity + idle_ttl`, bumped on each `TxExecute`. Catches a
  client that began a tx and vanished without disconnecting cleanly.

The sweep walks `TxRegistry.open`, aborts entries past either deadline
(drop = RAII), and removes them from `by_session`. **No new dependency** —
`tokio::time` + `Instant` are already in use (`commit.rs:81`,
`repo_instance.rs:557`).

### 6.5 Holding unique-write locks across round-trips — MUST NOT

This is the single most important "do not" of Phase B. The per-table
`unique_write_lock` is acquired **only inside the commit critical
section** — Phase 2.5 takes it (`commit.rs:684`) and Phase 5c releases it
(`commit.rs:914`), all under `commit_lock` (`repo_tx_gate.rs:123`) which is
itself held for the duration of one commit. Phase A deliberately makes
unique validation **optimistic at stage time and authoritative at commit
time**: `validate_unique_*` runs without a lock during the batch, and the
decisive byte-equal re-check happens under the lock at Phase 2.6
(`commit.rs:707`), backed by `TxContext::unique_guards`
(`tx_context.rs:135`).

**Phase B changes nothing here, and that is the point.** If an interactive
tx held a unique-write lock from `BEGIN` to `COMMIT` (across human-scale
round-trips), it would serialise every other unique writer to that table
for the tx's entire lifetime — wedging the database. Because Phase A
already defers the lock to commit, an interactive tx accumulates only
`unique_guards` (cheap, in-memory) across its EXECUTEs and acquires the
real lock for the few milliseconds of `commit_tx`. **No lock is held
across a round-trip. Ever.** Any Phase-B implementation that "optimises" by
pre-acquiring unique locks at `BEGIN` is a correctness *and* availability
regression and must be rejected in review.

### 6.6 Partial-failure semantics within a multi-call tx

If `TxExecute` #3 fails (e.g. a query error), what happens to the writes
from `TxExecute` #1 and #2 already in `write_set`?

**PROPOSED semantics:** a failed `TxExecute` does **not** auto-abort the
whole tx; it returns `DbResponse::Error` and leaves the tx OPEN with the
state from the successful prior calls intact. The client decides whether to
continue, `TxCommit`, or `TxRollback`. Rationale: this matches the
interactive contract (the client is in control across round-trips) and
keeps the failure local. The alternative (auto-abort on any error) is
recorded as a config-gated option (PROPOSED `tx_abort_on_error`), but the
default is "stay open" — the human/driver decides.

Within a *single* `TxExecute` batch, failure semantics are inherited from
Phase A's `execute_plan_tx`: the batch's plan runs stage by stage and an
error short-circuits *that batch* (`executor.rs:218` returns `Err`); the
partial writes that batch staged remain in the `TxContext` (they were
staged, not committed). If the team wants per-`TxExecute` atomicity
("either the whole batch stages or none of it"), that is a PROPOSED
savepoint/checkpoint extension — out of scope for the first cut and noted
as a Phase-B tail.

---

## 7. Recovery interaction

The model is unchanged from Phase A and *favours* Phase B: **nothing an
interactive tx does is durable until `COMMIT`.** Concretely:

- An open interactive tx exists only as in-memory `TxContext` state in the
  registry. No `wal.begin` has run — the WAL entry is written in commit
  Phase 4 (`commit.rs:732`/`:746`), and "a successful `wal.begin` IS the
  commit point" (`commit.rs:416`, type-doc `commit.rs:14`).
- Therefore **a crash mid-interactive-tx leaves nothing tx-related on
  disk** → clean abort. This is precisely the `pre_commit` crash seam
  contract: a HARD crash before Phase 4 "must find nothing → clean abort"
  (`commit.rs:727` and its doc `:118`). Recovery (`recover_v2_inflight`,
  CRIT-A, `REVIEW.md:540`) replays only *committed* inflight WAL entries;
  an uncommitted interactive tx has no entry to replay.
- The MVCC version floor is restored from durable markers on open (CRIT-B,
  `repo_instance.rs:242`/`:282`), independent of any open interactive tx.
  An interactive tx's `snapshot_version` was just a read view; losing it on
  crash costs nothing.

The only durable footprint an interactive tx ever leaves appears at the
instant of `COMMIT`, at which point it is — by construction — identical to a
Phase-A single-batch commit. So `materialized` semantics
(`MaterializationState`, `commit.rs:43`; reported via `TransactionInfo`,
`types.rs:527`) carry over verbatim: a `Deferred` commit is durable and
recovery-reconciled exactly as in Phase A. **No new recovery code is
required for Phase B** beyond ensuring the registry is *not* persisted
(it must not be — an open tx must die on crash).

---

## 8. Order of work

Numbered, staged, SI-first then SSI — mirroring `TRANSACTIONS_IMPL.md`
§"Order of work". Estimates are focused-engineering hours.

1. **Wire DTOs** — add `TxBegin/TxExecute/TxCommit/TxRollback` to
   `DbRequest` and `TxOpened/TxBatch/TxCommitted/TxRolledBack` to
   `DbResponse` (`db_message.rs`), with serde-default round-trip tests in
   the style of `types.rs:833`–`916`. (2 h)
2. **`TxRegistry`** — new server module (`scc::HashMap` registry +
   per-handle `tokio::sync::Mutex`, one-tx-per-session map, ownership
   check). Unit tests for insert/lookup/remove and the session-binding
   guard. (3 h)
3. **Engine glue** — `open_interactive_tx` / `execute_in_open_tx` /
   `commit_interactive_tx` thin wrappers around the existing
   `begin_tx`/`execute_plan_tx`/`commit_tx` (`executor.rs`,
   `repo_instance.rs`). No new execution logic. (2–3 h)
4. **Handler dispatch** — `ShamirDbHandler::handle` (`db_handler.rs:231`)
   gains a `match` arm per new variant; route through `run_blocking`
   (`db_handler.rs:619`); enforce single-repo against the pinned handle and
   the ownership check. (3 h)
5. **SI happy path E2E** — begin → execute(write) → execute(read RYOW) →
   commit → read-back from a second connection sees the writes. (2 h)
6. **Idle/lifetime sweep** — background task (mirror `spawn_gc_task`,
   `repo_instance.rs:544`) enforcing idle-timeout + reusing
   `DEFAULT_MAX_TX_LIFETIME`; abort = RAII drop + registry removal. (3 h)
7. **Abort-on-disconnect** — session-GC eviction hook → abort owned tx;
   wire into the `SessionStore` idle-eviction path referenced at
   `connection.rs:277`. (3 h)
8. **Staging budget** — per-tx byte/row cap on `TxExecute` →
   `tx_too_large` (§6.1). (2 h)
9. **SSI mode across calls** — confirm `read_set` accumulates across
   `TxExecute` and `validate_read_set` fires at `TxCommit`; conflict →
   `tx_conflict` (reusing `executor.rs:309`). The machinery exists
   (`record_read_shared`, `tx_context.rs:226`; `pre_commit` SSI block,
   `commit.rs:603`) — this stage is wiring + tests, not new isolation
   logic. (3 h)
10. **Concurrency + recovery tests** — see §9. (5–6 h)
11. **Docs** — flip this file's status, update `NEXT_PHASES.md`, root
    capability list. (1 h)

**Total: roughly 1 focused week** — materially smaller than Phase A
because the entire engine (MVCC, commit pipeline, SSI, GC, WAL recovery,
tx-aware read/write routing) already exists; Phase B is wire + session
lifetime + sweep, sitting *above* the proven Phase-A core.

---

## 9. Test strategy

Layered, matching the existing crash-recovery / concurrency harness style
(`crates/shamir-engine/tests/crash_recovery.rs` referenced at
`commit.rs:84`; the `tests/e2e/` JS harness referenced in
`TRANSACTIONS_IMPL.md:444`).

### 9.1 Rust unit (fast, in-memory)

- `TxRegistry`: insert/lookup/remove; one-tx-per-session rejection;
  ownership guard rejects a foreign `session_id`/`user_id`.
- Lifetime: `InteractiveTx` past idle deadline / absolute deadline is
  reaped; reuse the `is_expired` boundary (`tx_context.rs:175`).
- Glue: `execute_in_open_tx` accumulates into the same `TxContext` across
  two calls (write then read-back); `transaction` field stays `None`
  per call.

### 9.2 Rust integration (engine, all backends)

Built on `RepoInstance::begin_tx`/`commit_tx` directly (no wire), so they
run uniformly on every backend like the Phase-A acceptance suite
(`REVIEW.md:174`):

- **RYOW across calls** — stage a write via one `execute_in_open_tx`, read
  it back via a second; assert the staged value is visible (RYOW), while a
  *separate* `begin_tx` snapshot does not see it until commit
  (snapshot-wall, cf. scenario 6/7 in `TRANSACTIONS_IMPL.md:393`/`:404`).
- **Commit publishes** — after `commit_interactive_tx`, a fresh snapshot
  sees all writes at `commit_version`.
- **Rollback discards** — drop the parked tx; a fresh snapshot sees
  nothing; history/registry are clean.

### 9.3 Concurrency scenarios (multi-session)

Spawn two registries/sessions (or two JS connections per
`TRANSACTIONS_IMPL.md:317`):

- **Two interactive SI txs racing** — both begin, both read `X`, both
  write `X` across separate `TxExecute` calls, both commit. SI+LWW:
  last commit wins (documents behaviour, cf. scenario 1,
  `TRANSACTIONS_IMPL.md:330`).
- **Two interactive SSI txs racing** — same shape, `isolation:
  "serializable"`. One commits OK, the other aborts with `tx_conflict`
  (read-set accumulated across calls; cf. scenario 2,
  `TRANSACTIONS_IMPL.md:345`). **This is the headline SSI-lifetime test
  for Phase B.**
- **Write skew across calls** (doctor-on-call, scenario 4,
  `TRANSACTIONS_IMPL.md:367`) — proves the boundary that only SSI defends,
  now exercised over multiple round-trips. (Phantom protection still
  deferred → Phase C.)
- **Disconnect-mid-tx** — begin + one write, then kill the client
  connection; assert the tx is aborted (session-GC hook or idle sweep),
  nothing durable, snapshot unchanged.
- **Idle timeout** — begin, then no activity past the idle TTL; assert
  abort + registry cleanup; a subsequent `TxExecute` on the dead handle
  returns `tx_not_found`/`tx_expired`.
- **Absolute lifetime** — drive a tx past `DEFAULT_MAX_TX_LIFETIME`;
  assert commit (or sweep) aborts with `tx expired` (`executor.rs:314`).
- **Foreign-session theft** — session A begins; session B (same user, new
  connection) attempts `TxExecute`/`TxCommit` on A's handle → rejected by
  the ownership guard (§4.3).

### 9.4 Crash recovery

- **Crash mid-interactive-tx** — begin + several writes (state only in the
  registry, no `wal.begin`), HARD-crash the server (the `process::abort`
  seam style, `commit.rs:150`), reopen. Assert: **nothing** from the tx is
  visible (clean abort), the version floor is intact (CRIT-B,
  `repo_instance.rs:282`), and no inflight WAL entry exists for it.
- **Crash mid-COMMIT** — once `TxCommit` is driving `commit_tx`, the
  existing Phase-A crash seams (`commit.rs:727`/`:752`/`:830`/…) and their
  recovery contract apply verbatim; reuse the Phase-A crash-recovery
  harness with the commit triggered through the interactive path.

### 9.5 E2E (wire, JS SDK)

Following the file layout in `TRANSACTIONS_IMPL.md:444`, a new
`tests/e2e/tests/14-interactive-transactions.test.js` (PROPOSED):
begin/execute*/commit happy path; rollback; disconnect-mid-tx (close the
socket); two-connection SSI conflict. The recovery case extends the
real-crash subprocess harness tracked in `PHASE_A_TAILS.md` /
`REVIEW.md:573`.

---

## 10. Cross-references

- [`NEXT_PHASES.md`](./NEXT_PHASES.md) — roadmap overview / index this
  doc is part of.
- [`PHASE_C_SERIALIZABLE.md`](./PHASE_C_SERIALIZABLE.md) — phantom /
  predicate locks. Phase B deliberately stresses but does **not** solve
  the SSI read-set lifetime across round-trips (§6.2); full
  phantom/predicate protection is Phase C's job.
- [`PHASE_A_TAILS.md`](./PHASE_A_TAILS.md) — Phase-A hardening tail
  (real-crash subprocess harness, commit-hot-path `table_by_token` cache,
  property/fuzz coverage — the `REVIEW.md` §11 follow-ups Phase B builds
  on).
- [`TRANSACTIONS.md`](./TRANSACTIONS.md) — Phase A design (the original
  Phase-B sketch lives in its §"Phase B — interactive transactions").
- [`TRANSACTIONS_IMPL.md`](./TRANSACTIONS_IMPL.md) — Phase A
  implementation analysis; this document mirrors its depth and bilingual
  house style.
- [`docs/dev-artifacts/pre-transactional/REVIEW.md`](../pre-transactional/REVIEW.md) —
  §11 honest open boundaries on top of Phase A (CRIT/HIGH closures, I.1/I.2
  follow-ups, MED-A, the size-cap / membuffer open questions Phase B
  inherits).

---

*Status: IMPLEMENTED — stages 1-10 + lifecycle reaper + staging budget
shipped. Depends on Phase A (done). Last updated 2026-05-31.*
