# Post-alpha execution plan — RI-15 + FG-5 (cursors/streaming)

Status: **planned, deferred post-alpha** (user decision 2026-07-21/22 —
neither item blocks the v0.1.0-alpha.1 gate, which completed 2026-07-22:
full local gate + push + green remote CI on `master`).

TaskList ids: **#754** (RI-15), **#755–#759** (FG-5a–e). This file is the
durable narrative behind those task entries — the task descriptions carry
the same anchors in compressed form; this file carries the reasoning.

Recommended order: **RI-15 first, then FG-5** (see §3).

All code anchors below re-verified against the working tree on 2026-07-22
(post-RI-13 frozen commit).

---

## 1. RI-15 — global inflight response-memory budget (#754)

### Problem

Found during RI-8 (#737) verification. The server has a **per-batch**
response-size clamp — `batch.limits.max_result_size` is min-clamped against
the operator cap in `crates/shamir-server/src/db_handler/handler.rs:398-401`
— but **no server-wide gate on the SUM of in-flight response bytes** across
all concurrently-executing batches/connections.

Worst case at defaults: `max_active_connections = 1000` × 64 MiB per-batch
cap ≈ **64 GiB** of in-flight response memory — unbounded relative to a
typical 4–8 GiB container. A coordinated (or merely unlucky) burst of
max-size result sets can OOM the server despite every individual batch
being within its limit.

### Design constraints (from the RI-8 crush session, re-verified — do not re-derive)

1. **`tokio::sync::Semaphore` counts permits, not bytes.** Two viable
   shapes:
   - a custom async byte-budget primitive — `AtomicUsize` CAS-loop for the
     fast path + `tokio::sync::Notify` for waiters (~80–120 LOC + a
     dedicated stress test), or
   - `Semaphore::acquire_many(n_bytes as u32)` against a large permit pool
     (simpler, but caps the budget at `u32::MAX` permits and couples byte
     granularity to permit granularity).
2. **Response size is unknown until the planner has run.** The gate must
   either (a) reserve `max_result_size_bytes` upfront per batch —
   pessimistic, under-utilizes the budget by up to the cap-to-actual ratio
   — or (b) measure the actual serialized size post-execution and hold the
   permit through the write. Option (b) is the useful one, which forces:
3. **The permit must be released in the WRITER task, not the dispatch
   task.** The reply travels through a bounded mpsc as
   `WriterMsg::{Reply, ReplyAndClose}`
   (`crates/shamir-server/src/connection/request_loop.rs` — enum at ~:86,
   channel construction at ~:143). The permit therefore has to ride INSIDE
   the `WriterMsg` payload and drop after the socket write completes — and
   equally on every write-error break path, or the budget leaks.
4. **Plumbing path.** `Arc<ByteBudget>` constructed in
   `crates/shamir-server/src/server/server_launcher.rs` (next to the
   `QueryLimitsCap` construction, ~:388 — NOTE: this file moved into the
   `server/` subdir since the original writeup cited a flat path); new
   field on `ShamirDbHandler` (next to `query_limits`/`tx_limits`,
   `db_handler/handler.rs` ~:152-154). The `RequestHandler::handle` trait
   in `shamir-connect` carries no per-request resource context, so the
   acquire must live inside `ShamirDbHandler::execute`.
5. **Config surface.**
   `security.query_limits.max_inflight_response_bytes: Option<usize>`,
   default `None` = unbounded (preserves current behavior); validation:
   if set, must be ≥ `max_result_size_bytes` (otherwise no single max-size
   batch could ever pass). Documented in the config reference; behavioral
   tests on both the exhaustion path (N concurrent max-size batches, N+1-th
   blocks/queues) and the release path (budget returns after write AND
   after a write error).

### Scope & gate

~250–400 LOC entirely inside `shamir-server`: new `byte_budget.rs`
primitive, handler field, `WriterMsg`-carries-permit, launcher
construction, config field + validation, concurrency stress test.
**Single self-contained task — no decomposition needed.**

Gate: `./scripts/test.sh -p shamir-server --full` + fmt + clippy, standard
prompt-first brief + zero-trust verification discipline.

---

## 2. FG-5 — server-side cursors / streaming results (#755–#759)

### Problem

2026-07-21 review, P0#5: `QueryResult` materializes all records into a
`Vec`, and both client APIs return arrays. Large result sets / exports are
memory-bounded at BOTH ends — the server buffers the full serialized
response (see RI-15 above — the two problems compound), and the client
holds the full array.

### What already exists (the synergy that makes this tractable)

- **The engine is already internally streaming.**
  `list_stream` / `filter_stream` / `list_stream_tx` in
  `crates/shamir-engine/src/table/table_manager_streaming.rs` iterate in
  batches; FG-3's read-your-own-writes overlay is already wired into the
  tx-aware variants, so a tx-scoped cursor gets RYOW for free. The work is
  therefore mostly at the **wire/session** layer, not the engine core.
- **Temporal infrastructure exists.** `ReadQuery` already carries a
  `Temporal` selector (`crates/shamir-query-types/src/read/read_query.rs`
  ~:34-36, defaults to `Latest`). A stable cursor snapshot should pin a
  specific MVCC version at cursor creation and pass it as the Temporal
  selector on every fetch-next — no new temporal machinery expected.

### Sub-task decomposition (dependency chain)

```
#755 FG-5a  wire protocol + spec
   └─► #756 FG-5b  engine/session cursor (MVCC pin + caps + idle reaper)
          ├─► #757 FG-5c  Rust SDK (async Stream)
          └─► #758 FG-5d  TS SDK (async iterator)
                 └──┬──┘
                    ▼
              #759 FG-5e  Rust + TS e2e
```

#### FG-5a (#755) — wire protocol + spec

Define, in `shamir-query-types` + the protocol spec docs
(`docs/guide-docs/client-server-protocol-spec/`):

- cursor-creating request variant (a read with a `cursor: true`-style
  flag or a dedicated op) returning a **cursor id** + first page;
- **fetch-next** request (cursor id, page-size limit) → page + `has_more`;
- **close/cancel** request (idempotent);
- server-side **idle-timeout** semantics on the wire (a fetch against an
  evicted cursor returns a defined error code, distinguishable from
  "unknown id");
- a defined error code for **"cursor limit exceeded"** (cap itself is
  enforced in FG-5b, but the wire shape is fixed here);
- both query builders extended (repo rule: never hand-assemble wire ops).

#### FG-5b (#756) — engine/session cursor

The server-side cursor object:

- pins an MVCC snapshot version at creation; every fetch-next re-issues
  the underlying streaming read with the pinned `Temporal` selector;
- **resource safety is the core risk**: a pinned old snapshot blocks MVCC
  GC/WAL truncation (same class of concern as the truncation-ceiling
  mechanism — see `crates/shamir-engine/src/tx/tests/truncation_tests.rs`
  for how the repo reasons about held-back truncation). Idle-timeout
  eviction (background reaper or lazy check-on-access) is therefore
  mandatory, not a nicety;
- per-session/per-user **open-cursor cap**, config alongside RI-8's
  resource limits (`crates/shamir-server/src/db_handler/config.rs` area),
  rejecting creation past the cap with FG-5a's error code;
- interaction with RI-15 if it has landed: each fetch-next page passes
  through the same global byte budget as any other response.

#### FG-5c (#757) — Rust SDK

`impl futures::Stream` wrapper in `shamir-client`/`shamir-sdk`: lazy
fetch-next on poll, end-of-stream on `has_more == false`, best-effort
close on `Drop` + explicit async `close()`. Via the Rust builder.

#### FG-5d (#758) — TS SDK

Async-iterator wrapper in `shamir-client-ts` (`Symbol.asyncIterator`,
`for await … of`): lazy fetch-next, close on early `break`/`return()` +
explicit `close()`. Via the TS builder.

#### FG-5e (#759) — e2e (Rust + TS)

Against a real server process (existing harness conventions —
`crates/shamir-client/tests/`, `crates/shamir-client-ts/src/__tests__/`,
incl. the stale-binary guard in `e2e-harness.ts`):

- happy-path pagination over a large set (both SDKs);
- idle-timeout eviction;
- explicit cancel mid-stream;
- open-cursor cap rejection;
- **snapshot stability**: a write committed mid-cursor-lifetime must NOT
  appear in subsequent fetch-next pages.

---

## 3. Ordering rationale: RI-15 before FG-5

1. **Size/risk**: RI-15 is one bounded task in one crate; FG-5 is a
   five-task chain across six crates + spec + both SDKs.
2. **Reuse**: FG-5b's fetch-next responses should flow through the same
   `ByteBudget` gate — landing RI-15 first means cursors are born
   budget-aware instead of retrofitted.
3. **Non-substitution**: FG-5 REDUCES the pressure that motivates RI-15
   (streaming clients hold smaller responses) but does not replace it —
   non-cursor clients can still collectively exhaust memory; the global
   gate is needed regardless.
