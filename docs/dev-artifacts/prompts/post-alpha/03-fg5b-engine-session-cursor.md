# Brief: FG-5b — engine/session cursor (#756)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem

FG-5a (commit `c5da8e12`, already in the tree) landed the wire protocol:
`DbRequest::{CreateCursor,FetchNext,CancelCursor}`,
`DbResponse::{CursorPage,CursorClosed}`, `CursorId(u64)`
(`crates/shamir-query-types/src/wire/`), and
`BatchError::{CursorNotFound,CursorExpired,CursorLimitExceeded}`
(`crates/shamir-query-types/src/batch/batch_error.rs`, already mapped to
wire error codes in `crates/shamir-server/src/db_handler/handler.rs`'s
`error_code()` function). The server currently answers all three cursor
ops with a placeholder `DbResponse::Error{code:"cursor_not_yet_implemented"}`
— 3 stub match arms in `handler.rs`'s `RequestHandler::handle`, each
tagged `// FG-5b`. **This task replaces those 3 stubs with the real
cursor object.**

## Design — reuse existing machinery, do not build new streaming infra

Two mechanisms already in this codebase make this MUCH smaller than "build
a streaming cursor from scratch." Use them; do not reinvent either.

### 1. Fetch-next reuses existing keyset (seek) pagination — no live `Stream` storage

`crates/shamir-query-types/src/read/limit.rs`'s `Pagination::After { key,
limit, after_id }` (already used by regular non-cursor keyset-paginated
queries) is the right shape for "resume where the last page left off." A
cursor does NOT need to hold a live `futures::Stream` across async calls
(which would fight `TableManager`'s `'a`-borrowed stream lifetimes in
`table_manager_streaming.rs` for no benefit). Instead, cursor state is just
a **bookmark**:

- At `CreateCursor`: take the caller's `ReadQuery`. If `query.order_by` is
  `None`, default it to order by `_id` ascending (a keyset seek needs a
  total, stable order — see `read_query.rs:27` `order_by: Option<OrderBy>`).
  Store the (possibly-defaulted) query, `page_size`, and an initial
  `Pagination::After { key: vec![], limit: None, after_id: None }` bookmark
  (or `Pagination::None` for the very first page, then switch to `After`
  once you have a real seek key — check which the planner actually expects
  for "first page" vs. "resume").
- At `FetchNext`: clone the stored query, set `query.pagination =
  Pagination::After { key: <bookmark>, limit: Some(page_size), after_id:
  <bookmark_id> }` and `query.temporal = Temporal::AsOf { at:
  At::Version(pinned_version) }` (see §2), then call
  `TableManager::read(&query, &ctx) -> DbResult<QueryResult>`
  (`crates/shamir-engine/src/table/read_exec.rs:223` — the same entry
  point a plain non-tx `Query` batch op already uses). Extract the new
  bookmark from the LAST record in the returned page (its order-by
  column(s) + its `_id`, base58-encoded per `Pagination::After::after_id`'s
  doc comment at `limit.rs:65-77`) and from `QueryResult.pagination`'s
  `has_next`/`PaginationInfo` (`limit.rs:265-336`) to answer the wire
  `has_more` flag.
- Storing a bookmark instead of a stream means cursor state is `Send +
  'static` trivially — no lifetime plumbing against `TableManager`.

### 2. Snapshot pinning reuses `shamir_tx::SnapshotGuard` — `Temporal::Latest` cursors ONLY (deliberate scope cut)

`shamir_tx::repo_tx_gate::RepoTxGate::open_snapshot()` (async, returns
`SnapshotGuard`) pins the CURRENT `last_committed()` floor for the guard's
lifetime — `SnapshotGuard::version()` reports which version got pinned.
This is the exact mechanism `InteractiveTx` already uses
(`crates/shamir-server/src/tx_registry.rs:80,120` — `_snapshot:
SnapshotGuard`) to keep MVCC GC/WAL truncation from reclaiming a version a
live reader still needs; a cursor's registration is architecturally
identical to an interactive tx's.

**Constraint you must respect:** `open_snapshot()` only pins "whatever is
currently committed" — there is no API to pin an arbitrary ALREADY-PAST
version on demand (a historical version may already be past the GC floor
by the time someone asks for it). Meanwhile `Temporal::AsOf`/`Temporal::History`
on a plain read go through entirely separate one-shot, non-resumable code
paths (`read_exec.rs:289` `read_as_of`, `read_exec.rs:295` `read_history`)
that are not designed for incremental keyset pagination at all.

**Scope cut (do this, do not attempt full AsOf/History cursor support):**
- `CreateCursor` on a query whose `ReadQuery.temporal` is `Temporal::Latest`
  (the default): call `open_snapshot()`, store the resulting
  `SnapshotGuard` in the cursor's registry entry (its `Drop` releases the
  MVCC hold — same as `InteractiveTx`), and pin `AsOf(Version(guard.version()))`
  as the effective temporal for every subsequent `FetchNext` (giving
  snapshot-consistent pagination across the cursor's whole lifetime even
  though the query's own field said `Latest`).
- `CreateCursor` on a query whose `temporal` is `AsOf { .. }` or
  `History { .. }`: reject with a clear, distinct error (extend
  `BatchError` with e.g. `CursorTemporalNotSupported` if the existing 3
  variants don't fit, or reuse a validation-style error already in that
  enum if one fits better — check before adding a 4th). Document this as
  an intentional, named scope reduction in the code comment (cite this
  brief / FG-5b), not a silent gap. A future task can revisit full
  historical-cursor support if ever needed.

### 3. Registry — mirror `TxRegistry` almost exactly

`crates/shamir-server/src/tx_registry.rs` (the WHOLE file — `InteractiveTx`,
`TxRegistry`, `spawn_reaper_task`, `ReaperTask`, `TxRegistryError`) is the
template. Build a sibling `crates/shamir-server/src/cursor_registry.rs`
(new file, same crate — this is a server-side concept, not an engine one)
with:

- `Cursor` struct (≈ `InteractiveTx`): the stored `ReadQuery` + bookmark +
  `page_size` default, the `SnapshotGuard`, `owner_sid: [u8; 32]`, `db`/`repo`
  strings, `created_at: Instant`, `last_activity_nanos: AtomicU64`,
  `deadline_nanos` (reuse the same idle-TTL + absolute-lifetime shape as
  `InteractiveTx::is_expired`), and a `tokio::sync::Mutex<...>` around the
  mutable bookmark/pagination-cursor state (a `FetchNext` mutates it,
  mirroring why `InteractiveTx.ctx` needs the across-`.await` mutex).
- `CursorRegistry` (≈ `TxRegistry`): `open: DashMap<u64, Arc<Cursor>>`.
  **Difference from `TxRegistry`:** `TxRegistry.by_session` enforces
  ONE tx per session; a cursor registry must allow MANY cursors per
  session, capped by `CursorLimitsCap::max_cursors_per_session`. Do NOT
  store a `Vec<u64>` per session and check `.len()` for the cap (banned
  O(N) pattern per `CLAUDE.md`'s O(x→0) pillar / `clippy.toml`
  `disallowed-methods`) — instead keep `by_session: DashMap<[u8; 32],
  Arc<AtomicUsize>>` (a live count) that `register` increments (rejecting
  with `CursorLimitExceeded` and dropping the just-built `Cursor` if
  already at cap — same "reject and let RAII drop the unused resource"
  pattern as `TxRegistry::register`'s `TxAlreadyOpen` path) and `remove`
  decrements.
- `spawn_reaper_task` (≈ the tx one): same shape, new idle TTL / reap
  interval constants (see config below — do not hardcode the interactive-tx
  ones, cursors likely want a longer idle TTL since fetch-next round-trips
  can be client-paced; pick a sensible default and justify it in a doc
  comment, e.g. 60s idle / 10s sweep, adjustable via config).
- `CursorRegistryError` (≈ `TxRegistryError`): `CursorNotFound` (handle
  never existed or already reaped/canceled — maps to the EXISTING
  `BatchError::CursorNotFound`), `CursorLimitExceeded` (maps to the
  EXISTING `BatchError::CursorLimitExceeded`). Note `CursorExpired`
  (already defined in `batch_error.rs`) is for a fetch against a
  reaped-for-being-idle handle specifically — decide whether your registry
  can distinguish "never existed" from "existed but was reaped" (e.g. by
  not reusing `u64` cursor ids for a while, or by keeping a short-lived
  tombstone) and return the more specific error where you can; falling
  back to `CursorNotFound` for both is acceptable if a clean
  never-existed/reaped distinction isn't cheap — document the choice.

### 4. Config — mirror `TxLimitsCap`/`QueryLimitsCap`/(RI-15's) `ByteBudget` wiring exactly

`crates/shamir-server/src/db_handler/config.rs`: add
```rust
pub struct CursorLimitsCap {
    pub max_cursors_per_session: usize,
    pub idle_timeout_secs: u64,
}
```
Thread it: new field on `SecurityConfig` in `crates/shamir-server/src/config.rs`
(`security.query_limits`/`security.tx` are the existing siblings — add
`security.cursors` or fold into an existing section, your call, but pick
ONE and follow the existing nesting convention exactly); new
`pub(super) cursor_limits: CursorLimitsCap` field + `with_cursor_limits`
builder on `ShamirDbHandler` (`db_handler/handler.rs`, next to
`tx_limits`/`byte_budget`); construct + wire in
`crates/shamir-server/src/server/server_launcher.rs` next to where
`tx_registry_for_reaper`/`spawn_reaper_task` for `TxRegistry` already
happens (~line 407-449) — add the cursor registry + its reaper right next
to it, same pattern. Reasonable defaults: `max_cursors_per_session = 16`,
`idle_timeout_secs = 60` (document why, don't just copy tx's 30s — cursors
are read-only and often paced by client consumption speed, not a single
round-trip).

### 5. Replace the 3 FG-5a stub arms

`crates/shamir-server/src/db_handler/handler.rs`'s `RequestHandler::handle`
— find the 3 arms tagged `// FG-5b` (`DbRequest::CreateCursor`,
`::FetchNext`, `::CancelCursor`) and replace each placeholder with the real
call into your new registry. Follow the EXACT existing error-conversion
pattern used by `tx_begin` (`crates/shamir-server/src/db_handler/tx_handlers.rs:12-38`):
on a registry `Err(e)`, return
`DbResponse::Error { code: error_code(&e).to_string(), message: e.to_string() }`
(reusing the `error_code()` classifier already extended by FG-5a for the 3
`BatchError` cursor variants) rather than inventing a new error-shaping
path.

## Tests (TDD — write failing tests first)

- `Cursor`/`CursorRegistry` unit tests (new
  `crates/shamir-server/src/tests/cursor_registry_tests.rs`, mirroring
  `tx_registry.rs`'s own test conventions if any exist, or the closest
  analog in `crates/shamir-server/src/tests/`): register respects the
  per-session cap and rejects past it; `remove` frees the session's slot
  so a new cursor can be created; `expired_handles`-equivalent correctly
  identifies idle-past-TTL cursors.
- Behavioral tests through `ShamirDbHandler::execute`/`handle` (mirror
  `db_handler/tests/node_mode_tests.rs`'s harness style, or FG-5a's
  `db_handler/tests/` additions): `CreateCursor` → `FetchNext` (repeatable,
  multiple pages) → `CancelCursor` happy path over a real in-memory table
  with more rows than one page; verify `has_more` transitions correctly
  from `true` to `false` on the last page; verify a `FetchNext` after
  `has_more == false` returns a clean `CursorNotFound`/`CursorClosed`-style
  response, not a panic.
- Snapshot stability test: open a cursor, then commit a write to the same
  table via a SEPARATE regular batch call, then `FetchNext` the cursor —
  the newly committed row must NOT appear in any subsequent page (proves
  the pinned snapshot actually isolates the cursor from concurrent
  writes — this is the core correctness property FG-5e will also check
  end-to-end later, but it must be proven here at the unit/integration
  level first).
- Per-session cap rejection test: create `max_cursors_per_session` cursors
  on one session, verify the next `CreateCursor` returns
  `cursor_limit_exceeded` (the wire code FG-5a already defined).
- Idle-timeout eviction test: create a cursor, don't touch it, advance
  past the idle TTL (use the SAME `#[cfg(test)]` `OnceLock` override
  pattern already established in `crates/shamir-engine/src/tx/commit.rs`'s
  `TEST_MAX_TX_LIFETIME_OVERRIDE` for shrinking a timeout under test
  rather than sleeping the real production duration — do not sleep 60
  real seconds in a test), run the reaper sweep, verify the cursor is gone
  and a subsequent `FetchNext` against its id returns the not-found/expired
  error.
- Rejected-temporal test: `CreateCursor` with `Temporal::AsOf{..}` in the
  query returns the documented scope-cut error, not a panic or silent
  fallback to `Latest`.

## Gate

```
cargo fmt -p shamir-server -p shamir-query-types -- --check
cargo clippy -p shamir-server -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -p shamir-query-types --full
```

All must pass before returning. Stay inside `shamir-server` (new
`cursor_registry.rs`, `handler.rs`, `config.rs`, `server_launcher.rs`,
tests) and, only if you need a 4th `BatchError` variant for the
rejected-temporal case, `shamir-query-types`. Do NOT touch
`shamir-engine`'s streaming internals (`table_manager_streaming.rs`) — you
are calling the existing `TableManager::read` entry point, not modifying
it — and do NOT touch the Rust/TS SDK streaming wrappers (FG-5c/FG-5d,
later tasks).
