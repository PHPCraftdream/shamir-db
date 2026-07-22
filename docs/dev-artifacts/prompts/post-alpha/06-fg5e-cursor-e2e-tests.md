# Brief: FG-5e — cursor e2e tests, Rust + TS (#759)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background test/build invocations.

## Status — what's already covered, do NOT re-implement it

FG-5a/b/c/d (commits `c5da8e12`, `f15dda8c`, `927a8b33`, `8fe00cab`) are
fully landed, and their OWN test suites already prove most of the "full
cursor contour" this task's original scope lists:

- **Happy-path multi-page pagination** — proven through the real server
  AND both SDKs: `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs::create_fetch_cancel_happy_path_paginates_all_rows`,
  `crates/shamir-client/tests/cursor_stream.rs::stream_cursor_paginates_all_rows_in_order`,
  `crates/shamir-client-ts/src/__tests__/e2e-cursors.test.ts::'for await collects all rows...'`.
- **Explicit cancel/close mid-stream, proven to reach the server** —
  `crates/shamir-client/src/tests/cursor_stream_tests.rs::close_mid_stream_reaches_the_server`
  (Rust) and `e2e-cursors.test.ts`'s `'break mid-iteration...'` +
  `'explicit close()...'` tests (TS).
  **`crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`
  MVCC snapshot stability** — `cursor_does_not_observe_a_write_committed_after_creation`
  proves a concurrent write during a cursor's lifetime is never observed,
  at the handler/registry level.

**Do not duplicate any of the above.** This task's actual value-add is the
TWO gaps every one of those test files explicitly deferred to "a later,
separate task" in their own doc comments:

1. **Idle-timeout eviction observed through the real SDKs** (not just the
   server-side registry/reaper, which FG-5b already tested directly).
2. **Per-session open-cursor cap rejection observed through the real
   SDKs** (FG-5b tested the registry's cap logic directly; neither SDK's
   own test suite exercises hitting the cap through `Client::stream_cursor`/
   `client.streamCursor`).

Write ONE new test per gap per SDK (4 new tests total: Rust×2, TS×2) — not
a sprawling new suite re-proving what already passes.

## Design constraint you must know before writing these — the reaper interval is NOT configurable

`crates/shamir-server/src/cursor_registry.rs`'s `DEFAULT_CURSOR_REAPER_INTERVAL`
(5s) is hardcoded in `server_launcher.rs`'s `spawn_reaper_task` call — only
`idle_timeout_secs` comes from config (`security.cursors.idle_timeout_secs`).
An idle-timeout e2e test therefore CANNOT observe eviction the instant
`idle_timeout_secs` elapses — it must wait for the NEXT reaper sweep too.
**Set `idle_timeout_secs` to a small value (e.g. `1`) and sleep at least
~7 real seconds** (1s idle + one full 5s sweep interval + slack) before
asserting the cursor is gone. This is a real-time e2e test, not a unit
test — a multi-second sleep here is expected and consistent with this
crate's existing e2e tier (several existing `e2e-*.test.ts` files already
take 4-16s each).

## Rust (`crates/shamir-client/tests/`)

New file, e.g. `cursor_lifecycle_e2e.rs`. No harness changes needed — copy
FG-5c's own `tests/cursor_stream.rs` fixture (`ServerLauncher` + `TempDir` +
`Client::connect`, see that file's `boot()`-equivalent `make_config`/setup)
and simply override the `Config`'s `security.cursors` field per test
(it's a plain public struct — `CursorLimitsConfig { max_cursors_per_session,
idle_timeout_secs }`, `crates/shamir-server/src/config.rs`), no plumbing
needed since the field is already wired end-to-end.

- **Idle-timeout eviction test**: `security.cursors.idle_timeout_secs = 1`.
  Open a cursor via `client.stream_cursor(...)`, fetch the first page (so a
  `cursor_id` exists), then do NOT poll it again for ~7s (`tokio::time::sleep`
  in the test — a REAL sleep, this is an e2e test proving real wall-clock
  behavior). Then poll again (or issue a raw `roundtrip`-equivalent
  `fetch_next`, whichever is more natural against `CursorStream`'s API) and
  assert the error surfaces as `ClientError::Db{code: "cursor_expired", ..}`.
- **Per-session cap test**: `security.cursors.max_cursors_per_session = 2`.
  Open 2 cursors via `client.stream_cursor(...)` (poll each once so
  `CreateCursor` actually round-trips). Open a 3rd — assert its first
  polled item is `Err(ClientError::Db{code: "cursor_limit_exceeded", ..})`.
  A different session (a second `Client::connect` against the same server)
  must be unaffected — open one cursor on it successfully to prove the cap
  is per-session, not global.

## TS (`crates/shamir-client-ts/src/__tests__/`)

### Harness extension needed first (small, additive)

`e2e-harness.ts`'s `writeKtavConfig(dir, opts: {host, port, origin})`
hardcodes the `security` block (no `cursors` sub-block at all — the
server defaults to 16/60s when the block is omitted, since
`security.cursors` is `#[serde(default)]` on the Rust side). Extend:
- `writeKtavConfig`'s `opts` type: add an optional
  `cursors?: { maxCursorsPerSession?: number; idleTimeoutSecs?: number }`.
- When present, emit an additional `cursors: { max_cursors_per_session: N
  idle_timeout_secs: N }` block inside the existing `security: { ... }`
  KTAV block (follow the existing block's exact indentation/style).
- `startServer(opts?: { port?: number })`: add `cursors?: {...}` to its
  options type too, threading it through to `writeKtavConfig`.
- Keep this backward-compatible: every EXISTING call site that doesn't
  pass `cursors` must produce byte-identical config to today (the
  omitted-block-means-defaults behavior already tested by every other
  passing e2e file must not regress).

### New file `e2e-cursor-lifecycle.test.ts` (distinct name from FG-5d's own `e2e-cursors.test.ts`)

Mirror `e2e-cursors.test.ts`'s fixture structure exactly (`beforeAll`/
`afterAll`, `seedItems` helper), but call `startServer({ cursors: {...} })`
per the extension above.

- **Idle-timeout eviction test**: `idleTimeoutSecs: 1`. Open a cursor via
  `client.streamCursor(...)`, call `.next()` once (so `cursorId` exists),
  wait ~7s (real `setTimeout`/`await new Promise(r => setTimeout(r, 7000))`),
  then call `client.probeCursorOp({ op: 'fetch_next', cursor_id: cursorId!,
  page_size: N })` (the narrow test-support method FG-5d already added)
  and assert it throws `ShamirDbError` with `code === 'cursor_expired'`.
- **Per-session cap test**: `maxCursorsPerSession: 2`. Open 2 cursors via
  `client.streamCursor(...)` (call `.next()` on each so `create_cursor`
  actually round-trips). Open a 3rd — assert its first `.next()` (or the
  `for await` loop's first iteration) throws `ShamirDbError` with
  `code === 'cursor_limit_exceeded'`. Connect a SECOND client (`connectAdmin`
  again, or whatever the harness's multi-client pattern is — check
  `e2e-harness.ts` for an existing precedent before inventing one) and
  prove its own cursor cap is independent (opens successfully).

## Gate

```
# Rust
cargo fmt -p shamir-client -- --check
cargo clippy -p shamir-client --all-targets -- -D warnings
./scripts/test.sh -p shamir-client --full

# TS
npm --prefix crates/shamir-client-ts run typecheck
npm --prefix crates/shamir-client-ts run test
```
(Verify exact TS script names in `package.json` before assuming — they
were `typecheck`/`test` as of the FG-5d brief.) All four must pass. The TS
run requires a FRESH release `shamir-server` binary (rebuild via
`cargo build --release -p shamir-server` if the e2e tests report the
stale-binary guard — this can take several minutes, run it in the
foreground and wait for it to actually finish before running `npm test`).

Stay inside `crates/shamir-client/tests/` (new file), `crates/shamir-client-ts/src/__tests__/`
(new file + the additive `e2e-harness.ts` extension). Do not touch
`shamir-server`/`shamir-engine`/`shamir-query-types` — the wire protocol,
server behavior, and both SDKs are already complete.
