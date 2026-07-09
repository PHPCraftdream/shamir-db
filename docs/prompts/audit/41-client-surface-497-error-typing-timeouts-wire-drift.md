Task: HIGH-client residual cluster — from
`docs/audits/2026-07-06-client-surface-parity.md`. Task #497. This
audit covers the TypeScript SDK (`crates/shamir-client-ts` — actually a
TS package, not a Rust crate despite the path; confirm actual location
by grep, the audit's paths may be relative to a different root),
the napi/node binding (`crates/shamir-client-node` — MSVC-only, built
separately per this repo's workspace convention, see root CLAUDE.md),
and the Rust client (`crates/shamir-client`) and server
(`crates/shamir-server`) wire contract they must match.

These are INDEPENDENT findings — fix each on its own merits. Given the
breadth (Rust + TypeScript + a separate node binding), expect to use
the scope-down escape valve liberally for anything requiring a large
codegen/tooling investment (see finding 1.8) — document investigation +
follow-up rather than attempting a huge cross-language fixture-manifest
rebuild in one pass.

## Finding 1.1 (CRITICAL) — TS `BatchLimits` missing `max_nesting_depth` — GUARANTEED request failure

Rust: `crates/shamir-query-types/src/batch/batch_limits.rs:47` (confirm
line) — `BatchLimits` has 5 REQUIRED fields, none `#[serde(default)]`.
TS: `crates/shamir-client-ts/src/core/types/batch.ts:89-94` and
`builders/batch.ts:34-39`'s `DEFAULT_LIMITS` — only 4 fields. Any
`Batch.limits({max_queries: 20})` call sends an object missing
`max_nesting_depth` → the server rejects the ENTIRE batch with
`invalid_request: missing field 'max_nesting_depth'`. A TS unit test
(`builders/__tests__/batch.test.ts:143`, confirm line) currently
CEMENTS the wrong (4-field) shape as the expected golden output — this
test itself needs to be corrected as part of the fix, not preserved.

### Fix — 1.1

1. Add the missing field to the TS `BatchLimits` type AND to
   `DEFAULT_LIMITS` in `builders/batch.ts`.
2. Fix the existing unit test (`batch.test.ts:143`) that currently
   asserts the WRONG (4-field) shape — update it to expect the correct
   5-field shape. This is a required test-correction, not scope creep.
3. Longer-term (do this too if straightforward): add `#[serde(default)]`
   on each field of the Rust `BatchLimits` struct so a client that omits
   a field gets the server's default instead of a hard rejection —
   check this doesn't change any EXISTING Rust-side behavior/tests that
   rely on the current all-required-fields strictness.
4. Add a regression test that would have caught this: a TS-side
   round-trip test (or, if wire fixtures exist per finding 1.8, use
   those) confirming a `Batch.limits({max_queries: N})` call produces a
   wire payload the server actually accepts (a parity/fixture test, not
   just a TS-internal unit test).

## Finding 1.2 (HIGH) — TS client lies about `query_version`, silently downgrades after resume

Wire: `CURRENT_QUERY_LANG_VERSION = 2`
(`crates/shamir-query-types/src/wire/db_message.rs:19`, confirm line).
TS hardcodes `query_version: 1` in `execute` (`client.ts:484`),
`txBegin` (`:736`), `txExecute` (`:763`) while ACTUALLY using v2 features
(`records_idmsgpack`, `result_encoding:'id'`). The server currently
accepts both versions without gating fields by version
(`shamir-server/src/version.rs:45-54`), so this is latent, not yet
broken — but the FIRST real version-gate will break TS. Separately:
`ShamirClient.resume()` (`client.ts:194-227`) never reads
`server_query_version` from the resume_ok response (the wire DOES carry
it: `crates/shamir-client/src/wire_frames.rs:63`), so after a reconnect
`_serverQueryVersion` resets to 0 and the client silently loses the
entire id-on-wire optimization path. The Rust client does this
correctly (`client.rs:701` sends CURRENT; resume reads the field) —
use it as the reference implementation.

### Fix — 1.2

1. Export `CURRENT_QUERY_LANG_VERSION` from ONE shared location in the
   TS codebase (investigate whether a codegen/shared-constants file
   already exists, or whether this needs to be a new small module) and
   use it in `execute`/`txBegin`/`txExecute` instead of the hardcoded
   `1`.
2. Fix `resume()` to read `server_query_version` from the resume_ok
   response and set `_serverQueryVersion` accordingly, matching the
   Rust client's `client.rs` behavior.
3. Add an e2e test: resume a session, confirm `_serverQueryVersion`
   reflects v2 post-resume (not reset to 0), and confirm the id-on-wire
   path is genuinely still active after resume (not silently degraded).

## Finding 1.3 (HIGH) — Wire-field drift in TS types

Multiple independent drifts — fix each:
1. `ReadQuery.explain` (Rust: `read_query.rs:44-45`) is missing from TS
   `ReadQuery` (`types/query.ts:152-162`) and has no builder method
   (`builders/query.ts` lacks `.explain()`); `QueryResult.explain:
   ExplainPlan` (`query_result.rs:80-82`) is missing from TS
   `QueryResult` (`types/batch.ts:168-173`). EXPLAIN is entirely
   unavailable to TS users. Add the field + builder method + result
   type.
2. `DurabilityLevel` in TS (`types/batch.ts:84`) is missing
   `'async_index'` (present in Rust: `batch_request.rs:76-79`,
   `durability.rs`'s `AsyncIndex` variant). Add it.
3. `records_idmsgpack` is incorrectly typed at the `BatchRequest` level
   in TS (`types/batch.ts:131`) when in Rust it's a field of `InsertOp`
   (`write/types.rs:93`) — the TS type's own jsdoc comment contradicts
   its placement ("Present per query-entry, not at batch level").
   Runtime code (`client.ts:703`) already places it correctly — only the
   TYPE lies. Fix the type declaration to match actual runtime placement
   and the Rust source of truth.
4. `DbResponse::Error`'s doc-comment list of error codes
   (`db_message.rs:146-148`) is missing several codes actually sent by
   the server (`handler.rs:456-483`): `read_only_replica`,
   `access_denied`, `nesting_too_deep`, `tx_*`, `bad_role`,
   `unsupported_query_version`. Update the doc-comment to be accurate
   (this feeds into finding 2.1's typed-error work too — coordinate).

## Finding 1.4 (HIGH) — Rust vs TS behavioral divergence in `executeWithTouch`

Rust ALWAYS sets `result_encoding = Id` on v2
(`shamir-client/src/interner_cache_ops.rs:412`, confirm line); TS SKIPS
id-encoding whenever a batch contains `$query`/`$param` references or a
sub-batch (`client.ts:707-712`, with a comment claiming those rely on
server-side intermediate results staying name-keyed). ONE of these is
wrong — either Rust has a latent bug (a batch with a `$query`-ref +
`result_encoding=Id` breaks path resolution), or TS is unnecessarily
degrading performance. No cross-client test currently exists to settle
this.

### Fix — 1.4

1. Write a SERVER-SIDE test: a batch containing a `$query` reference AND
   `result_encoding=Id`, confirm whether path/reference resolution works
   correctly or breaks. This settles which client has the bug.
2. Based on that result, align the two clients: either fix Rust (if it
   has the latent bug) or remove TS's unnecessary degradation (if Rust
   is actually fine and TS was being overly conservative).
3. Add a regression test locking in the correct, now-verified behavior
   for both clients.

## Finding 2.1 (HIGH) — Error-surface: typed error codes die at the JS boundary

The server sends a rich error-code vocabulary (`permission_denied`,
`access_denied`, `read_only_replica`, `limits`, `timeout`,
`lock_timeout`, `tx_conflict`, `bad_hmac`, `fk_*`,
`unsupported_query_version`, etc.), preserved typed in the Rust client
(`ClientError::Db{code,message}`, `shamir-client/src/error.rs:24-25`).
But the node binding collapses everything to a plain string
(`lib.rs:264-266`, `Error::from_reason(e.to_string())`), and the TS
ws-client does the same (`client.ts:353-360`, `new Error(...)`
interpolating code+message into a single string). Callers can only
distinguish retryable (`timeout`, `lock_timeout`, `tx_conflict`,
`read_only_replica`→redirect) from fatal (`validation`,
`permission_denied`) errors via REGEX on the message string — an
existing e2e test literally documents this as a defect
(`tests/e2e/tests/09-errors.test.js:18,58`).

### Fix — 2.1

1. In TS: define `class ShamirDbError extends Error { code: string;
   retryable: boolean }` (or similar), populate it from the server's
   actual error code, and throw/reject with this typed error instead of
   a plain `Error` with an interpolated string.
2. In the node binding: use napi-rs's error-with-code support (confirm
   the exact API — check napi-rs version/docs, don't guess) so JS
   consumers of the native binding also get a `code` property instead of
   a plain string.
3. Export a single shared table of error codes (with a
   retryable/non-retryable classification) from ONE location — decide
   where this canonical source should live (a shared TS module? codegen
   from the Rust error enum? investigate before choosing) and have both
   the TS ws-client and the node binding consume it.
4. Fix the e2e test (`09-errors.test.js`) that currently documents the
   regex-based workaround as expected behavior — it should assert on the
   new typed `code`/`retryable` properties instead.

## Finding 2.2 (HIGH) — No timeouts/reconnect: a client can hang forever

Rust: `roundtrip` awaits a oneshot with NO timeout (`client.rs:827`).
TS: `sendDbRequest` registers a pending request with no deadline
(`client.ts:404-430`); `readLoop` only rejects on socket close. The
server's `max_execution_time_secs` only bounds `Execute` — `Ping`,
`CreateScramUser`, `TxCommit`, or a server-side lost response id leave
the client waiting forever. There's also no connect-timeout
(`platform/node.ts:107-125` — a TCP-level hang on connect just hangs),
no heartbeat, no auto-resume (the ticket exists but using it is left
entirely to the caller).

### Fix — 2.2

1. Add a `requestTimeoutMs` option (default something sensibly larger
   than the server's `max_execution_time_secs`, e.g. ~35s if the server
   default is 30s — confirm the actual server default before picking a
   number) to the TS client's pending-request tracking — a timer per
   pending request that rejects with a typed, retryable timeout error
   (coordinate with finding 2.1's typed-error work) if no response
   arrives.
2. Add a `connectTimeoutMs` option bounding the initial connection
   attempt (currently `ws.once('open'|'error')` with no timeout at all).
3. Investigate whether adding this to the Rust client's `roundtrip` is
   in scope for this pass too — if the Rust-side fix is straightforward
   (same shape: wrap the oneshot await in a timeout), do it; if it
   requires deeper changes to the Rust client's architecture, scope it
   down as a follow-up and focus on the TS side, which the audit
   describes in more operational detail.
4. Do NOT attempt heartbeat/auto-resume in this pass — the audit
   explicitly separates these as design decisions left to the caller;
   adding request/connect timeouts is the concrete, scoped fix here.
5. Add regression tests: a mock server that never responds to a Ping /
   never opens a connection, confirming the client now times out with a
   typed error instead of hanging.

## What to scope down / defer entirely in this pass

Findings 1.5, 1.6, 1.7, 1.8, 2.3, 2.4, and section 3 (DX) are lower
priority (MED severity or DX-only) and NOT in this task's scope — do
NOT attempt them here. If you find any of them trivially fixable as a
side effect of the above work (e.g., you're already touching a file
that also has a 1-line fix for one of these), a small opportunistic fix
is fine, but do not go looking for extra scope. Document anything
you notice but don't fix as a follow-up task suggestion in your report.

## TDD/regression requirement

For EACH finding you fix: add a regression test that would FAIL without
the fix. Findings 1.1/1.2/1.3 need TS-side (and where relevant,
cross-client parity) tests; 1.4 needs a server-side test settling the
Rust-vs-TS question first; 2.1/2.2 need tests proving the NEW typed
error / timeout behavior actually fires under the described conditions.

## Test scope

Confirm the actual test-running convention for the TS/node packages
(this repo's CLAUDE.md's `./scripts/test.sh` convention is for Rust
crates — the TS SDK and node binding likely have their own
`npm test`/`vitest` convention; grep `package.json` files under
`crates/shamir-client-ts` and `crates/shamir-client-node` for the
actual test commands and use those). For any Rust-side changes (e.g.
finding 1.1's optional `#[serde(default)]` follow-up, or 1.4's
server-side test, or 2.2's Rust client timeout):

```
./scripts/test.sh -p shamir-query-types
./scripts/test.sh -p shamir-server
./scripts/test.sh -p shamir-client
```

## Gate

For Rust changes:
```
cargo fmt -p <touched-crates> -- --check
cargo clippy -p <touched-crates> --all-targets -- -D warnings
```
For TS changes: use whatever lint/typecheck command this repo's TS
packages define (check `package.json` scripts — likely
`npm run lint`/`npm run typecheck` or similar).

If clippy/lint flags PRE-EXISTING issues in code you did not touch, do
not fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

For EACH of the findings (1.1, 1.2, 1.3, 1.4, 2.1, 2.2):
```
[Finding X] Status: fixed / partially-fixed / deferred
  > What changed + regression test added (if fixed/partial)
  > OR: deferral reason + follow-up task description (if deferred)
```
Full test/lint/gate results (exact commands + pass/fail) for whichever
packages/crates were actually touched.
