Task: CRIT-9 — `ShamirClient.resume()` in the TS client decodes the
server's `resume_ok` response as a NAMED msgpack MAP, but the server
actually sends it as a POSITIONAL msgpack ARRAY. Every named-key read
(`resp.session_id`, `resp.expires_at_ns`, etc.) reads `undefined`
against a real server, so `resume()` ALWAYS throws
`"resume response: session_id must be 32 bytes"` today. This is a
completely broken, shipped feature — CRITICAL, not just a staleness
bug.

## How this was found

Discovered while implementing task #446 (a narrower query_version
fix), independently confirmed by a second reviewer pass. Both
confirmations trace the exact same evidence chain below — this brief
repeats it so the fixing agent doesn't have to re-derive it from
scratch.

## Confirmed wire truth (do not re-litigate — verify quickly then move
to the fix)

- Server encode: `crates/shamir-server/src/connection/wire.rs:80-92`
  defines `ResumeOkWire` as a plain struct with NO
  `#[serde(rename_all)]`, and its own doc comment states: *"Always
  present on the wire (positional msgpack — see
  `AuthOk::server_query_version`)"*. `crates/shamir-server/src/connection/handshake.rs:156`
  sends it via `rmp_serde::to_vec(&wire_ok)` — the SAME call used for
  `Challenge` and `AuthOk` (both already correctly decoded positionally
  in TS). `rmp_serde::to_vec` (not `to_vec_named`) serializes a struct
  as a positional array, field order = declaration order.
- Rust client's own decode: `crates/shamir-client/src/wire_frames.rs:47-62`
  — `WireResumeOk` (plain `#[derive(Deserialize)]`, no rename
  attribute), field order:
  ```rust
  pub struct WireResumeOk {
      pub session_id: Vec<u8>,              // index 0
      pub expires_at_ns: u64,               // index 1
      pub resumption_ticket: Vec<u8>,       // index 2 (default empty)
      pub resumption_expires_at_ns: u64,    // index 3 (default 0)
      pub server_query_version: u8,         // index 4 (default 0)
  }
  ```
  This only works via `rmp_serde::from_slice` because the server truly
  sends a positional array (a struct-with-no-rename decoding from a
  MAP would require named keys matching field names, which `to_vec`
  does not produce).
- TS's own established pattern for the SAME handshake module:
  `crates/shamir-client-ts/src/core/protocol.ts:150-180` already
  decodes `auth_ok` (same connection, same server module, same
  positional convention) as `const okRaw = decode(...) as unknown[] | {error?: string};`
  and reads fields by INDEX (`okRaw[OK_SESSION_ID]`, `okRaw[7]` for
  `server_query_version`), with index constants `OK_SERVER_SIG = 0`,
  `OK_SERVER_PUB = 1`, `OK_SESSION_ID = 3`, `OK_EXPIRES_AT_NS = 4`
  (`protocol.ts:47-51`), plus a helper `asBytes(v, what)` for
  bytes-or-throw extraction, and an explicit array-vs-error-map
  discriminator: `if (!Array.isArray(okRaw)) { check .error string; throw }`.

`resume()` in `crates/shamir-client-ts/src/core/client.ts` (~line
167-233) is the ONLY place in this codebase that decodes one of these
handshake responses as a named map instead of a positional array —
confirmed by grepping for all callers of `.resume(` (only 2 test
files touch it, both fake-socket unit tests with a hand-built MAP
mock — `resume.test.ts` and `client.test.ts` — which is exactly why
this bug has never been caught: nothing in the test suite exercises
`resume()` against a real server or a genuinely array-shaped mock).

## Fix

Rewrite `resume()`'s response decode to be positional, mirroring
`runHandshake`'s `auth_ok` decode pattern EXACTLY (same helper
functions, same array-vs-error-map discriminator, same style of index
constants):

```rust
// Field order (from crates/shamir-client/src/wire_frames.rs's
// WireResumeOk / crates/shamir-server/src/connection/wire.rs's
// ResumeOkWire):
//   0: session_id (bytes, 32)
//   1: expires_at_ns (u64)
//   2: resumption_ticket (bytes, optional/empty default)
//   3: resumption_expires_at_ns (u64, default 0)
//   4: server_query_version (u8, default 0)
```

Translate to TS:
1. Define index constants (e.g. `RESUME_OK_SESSION_ID = 0`,
   `RESUME_OK_EXPIRES_AT_NS = 1`, `RESUME_OK_RESUMPTION_TICKET = 2`,
   `RESUME_OK_RESUMPTION_EXPIRES_AT_NS = 3`,
   `RESUME_OK_SERVER_QUERY_VERSION = 4`) — either co-located near
   `resume()` in `client.ts`, or in `protocol.ts` alongside the
   existing `OK_*` constants if that's a more natural home (check
   which fits the existing file organization better; don't invent a
   third location).
2. Decode the response as `unknown[] | { error?: string }` (matching
   `runHandshake`'s exact type union and the `!Array.isArray(...)`
   error-map check — an error response is presumably still sent as a
   map with an `error` string field, mirroring `auth_ok`'s error path;
   verify this assumption holds for `resume_ok`'s error path too by
   checking how the SERVER sends a resume REJECTION — same file,
   `crates/shamir-server/src/connection/handshake.rs`, search for
   where a resume failure is sent back, likely as `{error: "..."}` —
   confirm before assuming).
3. Extract `session_id` via the existing `asBytes` helper (reuse it —
   don't reinvent — import from `protocol.ts` if it's not already
   exported, or export it if needed).
4. Extract `expires_at_ns`, `resumption_ticket`,
   `resumption_expires_at_ns`, `server_query_version` positionally,
   matching the exact optional/default semantics already used for the
   analogous `auth_ok` fields (`okRaw[5]`/`okRaw[6]`/`okRaw[7]` handling
   in `protocol.ts:171-180`) — same undefined/absent handling, same
   type coercions.
5. Keep the SAME 32-byte `session_id` length validation and the SAME
   overall control flow/error messages `resume()` currently has — only
   the DECODE mechanism changes (positional instead of named), not the
   validation logic or the public API/behavior when the response is
   well-formed.

## TDD requirement (mandatory — this is a correctness-critical fix)

1. **Red**: the CURRENT test mock in `client.test.ts`/`resume.test.ts`
   builds the fake `resume_ok` response as a named JS object
   (`encode({session_id: ..., ...})`) — this is WRONG, it doesn't
   match the real wire format and is exactly why the bug went
   undetected. **Rewrite the mock to encode a genuine POSITIONAL
   ARRAY** (`encode([sessionId, expiresAtNs, resumptionTicket ?? Buffer.alloc(0), resumptionExpiresAtNs ?? 0, serverQueryVersion ?? 0])`
   — matching exactly how `auth_ok`'s mock is presumably ALSO array-
   encoded elsewhere in the test suite; check how `runHandshake`/
   `auth_ok`'s existing tests mock ITS response for the established
   idiom and mirror it precisely). Confirm this rewritten mock, run
   against the CURRENT (buggy) `resume()` code, FAILS (throws
   `session_id must be 32 bytes` or similar) — proving the test now
   actually exercises the real bug.
2. **Green**: apply the fix; confirm the SAME array-shaped mock now
   passes.
3. **Ideally, add or point at a genuine end-to-end test** that
   round-trips a real `resume()` call against the actual Rust server
   binary (the project's e2e test harness under
   `crates/shamir-client-ts/src/__tests__/e2e*.test.ts` already spins
   up a real server for other tests — check whether it's feasible to
   add a resume-specific e2e case there, or whether that's a larger
   undertaking better tracked as its own follow-up; use your judgment
   and state your decision in the report). This is the test that would
   have caught the ENTIRE class of bug (any future wire-shape drift on
   this or a similar handshake frame), not just this one instance.
4. Confirm all existing `resume`/`connect`/`client` tests still pass.

## Test scope command

```
cd crates/shamir-client-ts && npx vitest run
cd crates/shamir-client-ts && npx tsc --noEmit
```

## Gate (must be clean before finishing)

```
cd crates/shamir-client-ts && npx tsc --noEmit
cd crates/shamir-client-ts && npx vitest run
```

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- The exact positional decode implemented (index constants, helper
  reuse) and where it lives.
- Confirmation the error-vs-success discriminator (array vs `{error}`
  map) was verified against the server's actual resume-rejection wire
  format, not just assumed.
- The rewritten test mock (array-shaped, matching real wire truth) and
  its Red→Green evidence.
- Whether you added a genuine live-server e2e round-trip test for
  `resume()`, or deferred it (state why if deferred).
- Full test suite + typecheck results (exact commands + pass/fail).
