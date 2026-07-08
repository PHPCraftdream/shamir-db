Task: HIGH-client — TS client hardcodes `query_version: 1` while
actually using v2 wire features, and `ShamirClient.resume()` silently
downgrades to query-version 0 by never reading the server's
`server_query_version` field (audit top-5 #2,
`docs/audits/2026-07-06-client-surface-parity.md` §1.2).

## Where

`crates/shamir-client-ts/src/core/client.ts`:

1. **Hardcoded `query_version: 1`** at THREE call sites:
   - line 484 (`execute` request envelope)
   - line 736 (`txBegin` request envelope)
   - line 763 (`txExecute` request envelope)

   Wire truth: `crates/shamir-query-types/src/wire/db_message.rs:19` —
   `pub const CURRENT_QUERY_LANG_VERSION: u32 = 2;`. The Rust client
   (`crates/shamir-client/src/client.rs:701`) correctly sends
   `CURRENT_QUERY_LANG_VERSION`. The TS client sends a hardcoded `1`
   at all three sites while ALREADY relying on v2-only wire features
   (`records_idmsgpack`, `result_encoding: 'id'` — see
   `client.ts:707-712` area, the `executeWithTouch` id-encoding path).
   Today the server doesn't gate fields by declared version
   (`crates/shamir-server/src/version.rs:45-54`), so this is latent —
   but the client is LYING about its own capability, and the first
   real version-gate added server-side would break every TS client
   silently (or loudly, with `unsupported_query_version`, depending on
   gate direction).

2. **`ShamirClient.resume()` never reads `server_query_version`.**
   Compare the two construction paths in `client.ts`:
   - `connect()` (~line 114-146): destructures `serverQueryVersion`
     from `runHandshake(...)`'s return value and passes it as the
     final constructor argument.
   - `resume()` (~line 167-230): decodes the resume response as
     `const resp = decode(rawBytes) as Record<string, unknown>;` and
     reads `resp.session_id`, `resp.expires_at_ns`,
     `resp.resumption_ticket`, `resp.resumption_expires_at_ns` — but
     **never reads `resp.server_query_version`**, and its
     `new ShamirClient(...)` call OMITS the `serverQueryVersion`
     constructor argument entirely, so it falls back to the
     constructor's default: `this._serverQueryVersion = serverQueryVersion ?? 0;`
     (line 102).

   Wire confirmation: `crates/shamir-client/src/wire_frames.rs:63,85`
   — `WireResumeOk`/the equivalent Rust-side wire struct declares
   `pub server_query_version: u8` as a genuine NAMED map field (NOT a
   positional-tuple field like `auth_ok`'s response — cross-check
   `crates/shamir-client-ts/src/core/protocol.ts:180-184` where
   `auth_ok`'s `serverQueryVersion` IS read, but from a positional
   index 7 in a different response shape; `resume_ok` is a plain
   msgpack MAP, so `resp.server_query_version` is the correct
   TS-side key to read — confirm this by checking the server's
   `ResumeOk` struct at `crates/shamir-connect/src/server/resume.rs:195`
   and how it's serialized onto the wire, to be certain of the exact
   field name and that it round-trips as a named map key, not a
   positional array slot).

   Consequence: after ANY `resume()` call, `_serverQueryVersion` is
   always `0`, regardless of what the server actually advertises —
   `this._serverQueryVersion >= 2` (used at `client.ts:679` to decide
   whether to use the id-encoding fast path) is now ALWAYS false post-resume,
   even though the underlying connection is fully v2-capable. This is
   a silent performance/feature downgrade on every reconnect, not a
   hard failure — which makes it easy to miss in testing (nothing
   throws, requests still work, they're just silently worse).

## Fix

1. **Export `CURRENT_QUERY_LANG_VERSION` from one place** in the TS
   client and use it at all 3 hardcoded-`1` call sites (`execute`,
   `txBegin`, `txExecute`). Check whether the TS package already
   mirrors any other Rust-side wire constant this way (grep for a
   similar "wire constant mirrored in TS" pattern in
   `crates/shamir-client-ts/src/core/`) to match the existing
   convention; if none exists, add a single exported `const
   CURRENT_QUERY_LANG_VERSION = 2;` (with a comment noting it must
   track `shamir-query-types/src/wire/db_message.rs`'s constant of the
   same name/value) in the most natural existing constants file, or
   `client.ts` itself if there's no dedicated constants module.
2. **Fix `resume()` to read and propagate `server_query_version`**:
   after decoding `resp`, extract `resp.server_query_version` the same
   way the other fields are extracted (check its wire type — likely a
   small integer, `u8` per the Rust struct — and coerce/validate
   accordingly, mirroring whatever pattern `protocol.ts`'s
   `runHandshake` uses for the analogous `auth_ok` field so the two
   paths handle a missing/malformed field the same way — e.g. default
   to `0` if absent/invalid, do NOT throw, since a pre-v2 server
   omitting this field entirely is a valid legacy case). Pass this
   value as the `serverQueryVersion` argument to the `new
   ShamirClient(...)` call inside `resume()`, matching `connect()`'s
   existing pattern exactly.

## TDD requirement

1. **Red**: write/extend tests in
   `crates/shamir-client-ts/src/core/__tests__/` (check the existing
   test file covering `client.ts`'s `connect`/`resume` — likely
   `client.test.ts` or similar; match its existing mocking convention
   for a fake WS socket / server response) that:
   - Assert the `execute`/`txBegin`/`txExecute` request envelopes sent
     over the wire carry `query_version: 2` (or whatever
     `CURRENT_QUERY_LANG_VERSION` resolves to — assert against the
     constant, not a magic number, so the test doesn't silently go
     stale if the constant changes), not the old hardcoded `1`.
   - Assert that after a mocked `resume()` call where the server
     response includes `server_query_version: 2`, the resulting
     client's `.serverQueryVersion()` returns `2` (not `0`). This test
     should FAIL against the current code (which always yields `0`
     post-resume) and PASS after the fix.
2. **Green**: implement the fix.
3. Confirm existing `client.ts`/`connect`/`resume`-related tests still
   pass.

## Test scope command

Check `crates/shamir-client-ts/package.json` for the exact test-runner
invocation (this project's other TS work in this session used `npx
vitest run <file>` / `npx tsc --noEmit` — follow the same pattern):

```
cd crates/shamir-client-ts && npx vitest run <relevant test file>
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
- Where `CURRENT_QUERY_LANG_VERSION` was exported from, and
  confirmation all 3 hardcoded-`1` sites now use it.
- The exact field-read added to `resume()` for `server_query_version`,
  and confirmation it round-trips into `.serverQueryVersion()`
  correctly (both the failing-then-passing test evidence).
- Whether you found and fixed any OTHER hardcoded `query_version: 1`
  or similar staleness in the TS package beyond the 3 cited sites
  (grep to confirm completeness).
- Full test suite run results (exact commands + pass/fail counts).
