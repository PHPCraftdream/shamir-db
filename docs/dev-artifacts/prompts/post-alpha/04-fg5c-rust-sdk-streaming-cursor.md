# Brief: FG-5c — Rust SDK streaming cursor (#757)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem

FG-5a (wire protocol, commit `c5da8e12`) and FG-5b (server-side cursor
engine, commit `f15dda8c`) are landed: a client can already send
`DbRequest::{CreateCursor,FetchNext,CancelCursor}` and get back
`DbResponse::{CursorPage{cursor_id,page,has_more},CursorClosed}` over the
wire. But the Rust client SDK (`shamir-client`) has no ergonomic wrapper —
a caller today would have to hand-drive the request/response cycle
themselves. This task adds an idiomatic `impl futures::Stream` that
internally issues `FetchNext` as the consumer polls, hiding pagination
entirely — this is what actually fixes the original P0 problem (results no
longer need to materialize as one big `Vec` on the CLIENT side either, not
just the server side FG-5b fixed).

## Code anchors (re-verified against the tree 2026-07-22 — do not re-derive)

- **`crates/shamir-client/src/client.rs`**: `Client::execute` (~line 770)
  and `Client::repl` (~line 855) are the two existing patterns for "build a
  `DbRequest`, call `self.roundtrip(&req).await?`, match the expected
  `DbResponse` variant, error via `ClientError::Protocol` on mismatch."
  Mirror this exactly for `CreateCursor`/`FetchNext`/`CancelCursor`.
- **`Client::roundtrip`** (~line 876) is the sole request/response
  primitive — allocates a request id, registers a oneshot **before**
  writing, sends, awaits. It already converts `DbResponse::Error{code,message}`
  into `Err(ClientError::Db{code,message})` (~line 930) — server-side
  cursor errors (`cursor_not_found`/`cursor_expired`/`cursor_limit_exceeded`/
  `cursor_temporal_not_supported`, all wired in FG-5b) surface through this
  path automatically; no special-case error handling needed.
  **`roundtrip` is currently a private (no `pub`) method** — widen it to
  `pub(crate)` so a new sibling module can call it.
- **`crates/shamir-query-builder/src/cursor.rs`** (FG-5a): free functions
  `create_cursor(db, query, page_size) -> DbRequest`,
  `fetch_next(cursor_id, page_size) -> DbRequest`,
  `cancel_cursor(cursor_id) -> DbRequest`. `shamir-client` already
  re-exports the whole crate as `pub use shamir_query_builder as builder`
  (`client.rs` ~line 59) — these are reachable as
  `shamir_client::builder::cursor::{create_cursor,fetch_next,cancel_cursor}`.
  **Use these, never construct `DbRequest::CreateCursor{..}` etc. by hand**
  (repo rule: query construction is builder-only).
- **`crates/shamir-client/src/subscription.rs`** (`SubscriptionHandle`,
  lines 36-65) is the closest existing "streaming client handle" precedent
  — study its `Drop` impl closely (line 59): it does **ONLY local**
  cleanup (removes its channel from a client-side registry) — it does
  **NOT** attempt a network round-trip on drop. Follow this same
  philosophy (see "Cleanup" section below); do not invent a
  spawn-on-drop network call.
- **No `futures`/`tokio-stream` dependency exists yet** in
  `shamir-client/Cargo.toml` — add `futures = "0.3"` (this pulls in both
  the `Stream` trait and `futures::stream::unfold`, used below).
- **Test fixture**: `crates/shamir-client/tests/smoke.rs` — the
  `ServerLauncher` + `TempDir` + `Client::connect` boot pattern every
  existing client integration test copies. Reuse it verbatim for this
  task's own integration test(s); FG-5e (a later, separate task) is where
  the FULL cross-SDK e2e matrix (idle-timeout, per-session cap, cancel
  mid-stream, snapshot stability) lives — this task's own test scope can
  stay narrower (see Tests below).

## Design

### 1. Implementation vehicle: `futures::stream::unfold` — do NOT hand-roll `Pin`/`Poll`

A manual `impl Stream for CursorStream` with a hand-rolled `poll_next`
(storing a boxed in-flight future, manually pinning it) is a well-known
footgun class in async Rust — easy to get subtly wrong (lost wakers,
incorrect `Pin` projection). **Use `futures::stream::unfold(initial_state,
|state| async move { ... })` instead** — it is built exactly for "generate
a stream via repeated async computation over evolving state," which is
precisely this task. Sketch:

```rust
enum CursorStreamState<'a> {
    // Buffered records from the current page not yet yielded, plus
    // whether another FetchNext should be attempted once the buffer
    // drains.
    Buffered { client: &'a Client, cursor_id: CursorId, page_size: u32,
               remaining: std::vec::IntoIter<QueryRecord>, has_more: bool },
    Exhausted,
}
```
`unfold` yields one `QueryRecord` at a time out of `remaining`; when
`remaining` is empty and `has_more` was true, it issues ONE `FetchNext`
(via `builder::cursor::fetch_next` + `client.roundtrip`), refills
`remaining`/`has_more` from the new `CursorPage`, and continues; when
`remaining` is empty and `has_more` is false, the stream ends (`unfold`
returns `None`).

### 2. Lifetime: borrow `&'a Client` — matches `execute`/`repl`, do not require `Arc<Client>`

`Client` is not `Clone` and every existing method (`execute`, `repl`)
takes `&self`. The new entry point should too:
```rust
pub fn stream_cursor<'a>(
    &'a self,
    db: &str,
    query: ReadQuery,
    page_size: u32,
) -> impl futures::Stream<Item = Result<QueryRecord, ClientError>> + 'a
```
This ties the returned stream's lifetime to the borrowed `&Client` — the
same tradeoff every other `Client` method already has (caller keeps the
client alive for as long as they use the stream). Do not introduce a new
`Arc<Client>`-based API surface just for this — it would be inconsistent
with the rest of the crate for no real benefit.

### 3. Cleanup — mirror `SubscriptionHandle`, do NOT spawn a network call in `Drop`

There is no client-side registry entry for a cursor to clean up locally
(unlike subscriptions, which route pushes through a client-side channel
registry) — a bare-dropped stream has nothing useful to do synchronously,
and `Client: !Clone` plus `tokio::spawn` needing `'static` ownership makes
a fire-and-forget cancel-on-drop RPC awkward and fragile (needs an
ambient Tokio runtime at drop time, no guarantee of one, and no
error-reporting path if it fails). **Do not implement `Drop` for
network cleanup.** Instead:
- Provide an explicit **`async fn close(self) -> Result<(), ClientError>`**
  (or a method on a thin wrapper type around the `unfold` stream, if
  `impl Stream` alone can't carry an extra method — see "API shape" below)
  that sends `CancelCursor` deterministically for early, intentional
  release.
- A consumer who drains the stream to `has_more == false` never needs to
  call `close()` — the server already auto-closes an exhausted cursor
  (FG-5b: `fetch_next` removes it from the registry on the last page).
- A consumer who drops the stream early WITHOUT calling `close()` leaves
  the cursor open server-side until the idle-timeout reaper reclaims it
  (FG-5b default: 60s) — this is the SAME backstop philosophy FG-5b's own
  design already documents (idle-timeout is the safety net for abandoned
  resources, not an afterthought). Document this tradeoff in the new
  module's doc comment explicitly — do not leave it implicit.

### 4. API shape — a thin wrapper struct, not a bare `impl Stream` fn, so `close()` has somewhere to live

Since `impl Stream<Item=...>` returned from a function is an opaque type
with no other inherent methods, wrap the `unfold`-produced stream in a
named struct that implements `Stream` (delegating `poll_next` to the
inner `unfold` stream via `pin_project`-free manual delegation, or by
storing the inner stream in a `Pin<Box<dyn Stream<...> + 'a>>` field for
simplicity — boxing one stream per cursor is a negligible cost, and it
sidesteps needing `pin_project`/`pin_project_lite` as a new dependency)
and exposes `pub async fn close(mut self) -> Result<(), ClientError>`
alongside it. Name it `CursorStream<'a>`.

## Tests (TDD — write failing tests first)

- Unit-level test(s) against a fake/mock is NOT the right shape here (the
  whole point is proving the real `FetchNext` round-trips work) — write
  integration tests in `crates/shamir-client/tests/` using the
  `smoke.rs` `ServerLauncher`/`Client::connect` fixture pattern:
  - Happy path: seed N rows (N large enough to span 3+ pages at a small
    `page_size`), `client.stream_cursor(...)`, collect via
    `futures::StreamExt::collect::<Vec<_>>()` (or a manual `while let
    Some(...) = stream.next().await` loop), assert the full record set
    matches what was seeded, in order (if the query has an ORDER BY).
  - `close()` mid-stream: start iterating, stop partway, call `close()`,
    verify a subsequent server-side `FetchNext` against the same
    `cursor_id` (constructed directly via `builder::cursor::fetch_next` +
    a raw `roundtrip`-equivalent, or by checking the server's cursor
    registry is empty if that's exposed to the test process) reports
    `cursor_not_found`/`cursor_closed`-consistent behavior — proving
    `close()` actually reached the server.
  - Error propagation: request a page from a nonexistent table / an
    `AsOf` query (rejected per FG-5b's scope cut) and assert the stream's
    first `Item` is `Err(ClientError::Db{code,..})` with the expected
    code, not a panic.
  - Empty result set: a query matching zero rows yields a stream that
    ends immediately with zero items (not an error, not a panic).

## Gate

```
cargo fmt -p shamir-client -- --check
cargo clippy -p shamir-client --all-targets -- -D warnings
./scripts/test.sh -p shamir-client --full
```

All must pass before returning. Stay inside `shamir-client` (new
`cursor_stream.rs` module + `client.rs`'s `roundtrip` visibility widening +
`Cargo.toml`'s new `futures` dependency + new integration test file). Do
NOT touch `shamir-server`/`shamir-engine`/`shamir-query-types` — the wire
protocol and server behavior are already complete (FG-5a/FG-5b) — and do
NOT touch the TS SDK (FG-5d, a separate later task) or write the full
cross-SDK e2e matrix (FG-5e, also separate).
