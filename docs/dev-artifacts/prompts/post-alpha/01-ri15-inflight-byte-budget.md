# Brief: RI-15 — global inflight response-memory budget (#754)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem

Today the server has a **per-batch** response-size clamp
(`max_result_size_bytes`, `crates/shamir-server/src/db_handler/handler.rs:398-401`)
but **no server-wide gate on the SUM of in-flight response bytes** across
all concurrently-executing batches/connections. At
`max_active_connections = 1000` × the 64 MiB per-batch default, worst case
is ~64 GiB of in-flight response memory — unbounded relative to a typical
4–8 GiB container. A burst of max-size result sets across many connections
can OOM the server even though every individual batch is within its own
limit.

## Design constraints (re-verified against the working tree 2026-07-22 — do not re-derive)

1. `tokio::sync::Semaphore` counts permits, not bytes. Implement a custom
   async byte-budget primitive: `AtomicUsize` CAS-loop for the fast
   acquire/release path + `tokio::sync::Notify` to wake waiters when bytes
   free up. (Alternative — `Semaphore::acquire_many(n_bytes as u32)` against
   a huge permit pool — is simpler but caps the budget at `u32::MAX` and
   couples byte granularity to permit granularity; prefer the custom
   primitive.)
2. Response size is unknown until the planner/executor has actually run.
   The gate must measure the **actual serialized response size** post-execution
   and hold the permit through the write (not reserve `max_result_size_bytes`
   upfront — that under-utilizes the budget by the cap-to-actual ratio).
3. **The permit must be released in the WRITER task, not the dispatch task.**
   Replies travel through a bounded mpsc as `WriterMsg::{Reply, ReplyAndClose}`
   (`crates/shamir-server/src/connection/request_loop.rs`, enum at ~line 86,
   channel construction at ~line 143). The permit must ride INSIDE the
   `WriterMsg` payload and drop after the socket write completes — and
   equally on every write-error break path, or the budget leaks permanently.
4. Plumbing path:
   - `Arc<ByteBudget>` constructed in
     `crates/shamir-server/src/server/server_launcher.rs`, next to the
     existing `QueryLimitsCap` construction (~line 388).
   - New field on `ShamirDbHandler`, next to `query_limits`/`tx_limits`
     (`crates/shamir-server/src/db_handler/handler.rs` ~lines 152-154).
   - `RequestHandler::handle` in `shamir-connect` carries no per-request
     resource context, so the acquire must happen inside
     `ShamirDbHandler::execute`.
5. Config surface:
   `security.query_limits.max_inflight_response_bytes: Option<usize>`,
   default `None` = unbounded (preserves current behavior). Validation: if
   set, must be `>= max_result_size_bytes` (otherwise no single max-size
   batch could ever pass) — reject config at startup otherwise. Document
   the new field in the config reference alongside the existing
   `query_limits` fields.

## Scope

Entirely inside `shamir-server`, ~250–400 LOC:
- new `byte_budget.rs` module: the `ByteBudget` primitive (acquire/release,
  async wait on exhaustion, `Notify`-based wakeup).
- `ShamirDbHandler` gains the budget field; `execute` acquires post-execution,
  before handing the reply to the writer channel.
- `WriterMsg::Reply`/`ReplyAndClose` carry the acquired permit/guard; it
  drops after the write (success AND error paths).
- `server_launcher.rs` constructs the `Arc<ByteBudget>` from config.
- config field + startup validation + doc.

## Tests (TDD — write failing tests first)

- Unit test(s) for `ByteBudget`: acquire under budget succeeds immediately;
  acquire over budget blocks until a release frees enough bytes; multiple
  waiters wake correctly (FIFO or at-least-one-progress, whichever the
  implementation naturally gives — document the actual guarantee).
- Behavioral test in `shamir-server` exercising the exhaustion path: N
  concurrent max-size-response batches saturate the budget, the (N+1)-th
  blocks/queues until one of the first N's write completes and releases.
- Behavioral test for the release path on a write error (simulate a closed
  socket / write failure) — budget must still recover, not leak.
- Config validation test: `max_inflight_response_bytes < max_result_size_bytes`
  is rejected at startup.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All three must pass before returning. Do not touch code outside
`shamir-server` (and, if genuinely needed, the config-reference doc file).
