Task: #520 — Rust client (`shamir-client`) has no request/connect timeout.
Found during task #497's `@fl` review. The TS client SDK already got this
(`requestTimeoutMs`/`connectTimeoutMs` in `crates/shamir-client-ts/src/core/client.ts`,
task #497) — this brief brings the Rust client to parity.

## Context (confirmed by reading current code)

`crates/shamir-client/src/client.rs`:
- `Client::connect` (~line 294) and `Client::resume` (~line 513, confirm
  exact name) both call `TcpStream::connect(opts.addr).await?` with NO
  timeout — an unresponsive/firewalled endpoint hangs the connect call
  forever.
- `Client::execute` (~line 684) sends a request and awaits a response via
  the reader-task channel — confirm the exact await point, but there is
  no timeout wrapper: a server that accepts the connection but never
  responds (or a network partition after the request is sent) hangs the
  caller forever.
- `ConnectOptions` (~line 54) has NO timeout-related fields.

## Fix

1. Add two new fields to `ConnectOptions`:
   `pub connect_timeout: Option<std::time::Duration>` and
   `pub request_timeout: Option<std::time::Duration>`. `None` = no
   timeout (preserves EXACT current behavior — this is critical for
   backward compatibility; do not silently change default behavior for
   existing callers).
2. Wrap `TcpStream::connect(opts.addr)` in `Client::connect` (and the
   equivalent in `Client::resume`/whatever the second connect-site is
   named) with `tokio::time::timeout(d, ...)` when `connect_timeout` is
   `Some(d)`; on timeout, return a clear `ClientError` variant (check
   `crates/shamir-client/src/error.rs` for the existing error enum shape
   and add a `ConnectTimeout` variant if one doesn't already exist,
   matching this codebase's existing error-variant naming style).
3. Wrap `Client::execute`'s response-await with the same
   `tokio::time::timeout` pattern when `request_timeout` is `Some(d)`;
   add a `RequestTimeout` (or similarly-named) `ClientError` variant on
   timeout.
4. **Mechanical call-site update (13 files, confirm via
   `grep -rln "ConnectOptions {" crates/`):** `ConnectOptions` has no
   `Default` impl (its `addr: SocketAddr` field has no sensible default),
   and none of the existing 13 construction sites use `..Default::default()`
   spread — so adding the two new fields WILL require adding them to
   every existing struct literal. Add `connect_timeout: None,
   request_timeout: None,` (or an equivalent sensible default matching
   whatever this codebase's convention turns out to be for
   internal/test call sites — check if `shamir-server`'s own internal
   replication client (`crates/shamir-server/src/replication/prod_factory.rs`)
   should set an actual non-None default given it's production
   inter-node traffic, vs. test/example call sites which can stay
   `None`) to every one of the 13 sites. This is a mechanical, additive
   change — no logic change beyond the timeout wiring itself.

## Naming/semantics parity with the TS client (for consistency, not required to be identical)

Check `crates/shamir-client-ts/src/core/client.ts`'s `requestTimeoutMs`/
`connectTimeoutMs` (task #497) for the semantics already established in
this codebase (what exactly each timeout bounds — connection
establishment vs. a single request/response round-trip) and mirror that
scoping in the Rust client, even though the Rust API naturally uses
`Duration` instead of a millisecond integer.

## TDD

1. A test proving `connect_timeout` actually fires — e.g. connect to an
   address that accepts TCP connections but never completes the
   application-level handshake (or a non-routable address with a short
   timeout), assert the call returns the new timeout error within
   roughly the configured duration (not instantly, not hanging).
2. A test proving `request_timeout` fires similarly for `execute` against
   a server that accepts the connection/handshake but never responds to
   a specific request.
3. A test proving `None` (the default) preserves current unbounded-wait
   behavior — e.g. existing tests that already pass with real servers
   must continue to pass unchanged.
4. Confirm all 13 existing call sites still compile after adding the two
   new fields.

## Test scope

```
./scripts/test.sh -p shamir-client -p shamir-server
```

(shamir-server is in scope because its own replication client uses
`ConnectOptions` directly.)

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-client -p shamir-server
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Implementation] Status: fixed
  > New ConnectOptions fields + ClientError variants added
  > Which call sites got a non-None default (if any) and why
  > New tests, confirmed timeout actually fires (not just compiles)
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-client -p shamir-server: pass/fail
```
