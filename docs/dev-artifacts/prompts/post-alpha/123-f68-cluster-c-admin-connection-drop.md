# F-68 (#895) cluster C follow-up — adminClient connection drops mid-file in `e2e-permissions.test.ts`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## Hard rule — root cause only, no workarounds (unchanged from the parent brief)

No retry wrappers, no reconnect-and-continue, no loosened assertions, no
`#[ignore]`/`.skip`. Find why an established, working connection closes
unexpectedly and fix that; do not paper over the symptom.

## What is now known (this session, live CI reproduction on two separate
diagnostic runs against `ci/f68-diagnostics`, commit `9e38351e`)

`crates/shamir-client-ts/src/__tests__/e2e-permissions.test.ts` is ONE flat
`describe.skipIf(...)` block (no nested `describe()`s) with a single
top-level `beforeAll` that opens `server` + `adminClient`, and several
`it()`-scoped setup steps that lazily create per-section users
(`userAClient`, `userBClient`, `gClient`) reusing that SAME `adminClient`
throughout the whole file for every `chgrp`/`chmod`/`createUser`/`seed`
admin operation.

Two live runs this session both failed with the client's generic
`Error: connection closed` (`framing.ts:104`, fired from the WebSocket's
`onClose` with no explicit error — i.e. the far end closed cleanly, no
network error) — but at DIFFERENT points in the file each time:

- Nightly nightly run (2026-07-29, run 30434464672) and a prior scheduled
  run: failed inside `it('A11/G4d-group: group membership + chgrp + group
  bits grant read; removal re-denies', ...)` — specifically on the LAST
  query in that test, right after `removeGroupMember`.
- This session's live diagnostic run (`ts-e2e-nightly` run 30523448058,
  job 90808875675): failed inside `it('B-setup: create two databases and
  two users', ...)` — much EARLIER in the file, on one of that test's own
  `adminClient!.execute(...)` calls (`setupDb`/`seed`/`chmod`, all via
  `adminClient`). Confirmed via the raw vitest log: `B-setup` itself is
  the FAILING test (`Error: connection closed` at `framing.ts:104`), and
  every subsequent test that reuses `userBClient` (B2, B4, B6 — all lines
  using `userBClient!`) cascades into
  `TypeError: Cannot read properties of null (reading 'executeWithTouch')`
  because `userBClient` was never assigned (B-setup died before reaching
  its `userBClient = await createUserAndConnect(...)` line at line 412).
  Tests using `userAClient!` (B1, B3, B5) passed fine in this same run —
  `userAClient` was set up successfully earlier and its connection was
  NOT the one that dropped this time.

**The failure point moving between runs (group-test vs. B-setup) is the
important clue**: it rules out a bug tied to one specific query's logic
(e.g. something specific to `removeGroupMember`) and points instead to a
resource- or timing-dependent server-side connection-lifecycle issue that
can fire at varying points depending on how many connections/operations
have accumulated by then, or on scheduling jitter. This file opens several
concurrent client connections over its lifetime (admin, `userAClient`,
a transient `rbacUser` admin op — NOT a new connection, `suClient`
(explicitly closed at line 313), `userBClient`, and later `gClient`) all
against ONE server process (`startServer()`, own ephemeral port, per this
file's own comment "no conflict with other e2e suites").

## What to investigate

1. Read the server's connection-acceptance and session-teardown path:
   `crates/shamir-server/src/connection/request_loop.rs`,
   `crates/shamir-server/src/connection/connection_context.rs`, the
   `server_launcher.rs`/`server_handle.rs` accept loop, and
   `crates/shamir-server/src/config.rs` /
   `crates/shamir-server/src/db_handler/config.rs` for ANY connection cap,
   idle-timeout, or per-session limit that could cause an existing,
   actively-used connection (`adminClient`'s) to be closed by the server
   while OTHER connections are being opened concurrently (`userBClient`
   connecting, or a transient user/connection from an earlier step still
   settling).
2. Check `crates/shamir-server/src/tx_registry.rs` and any session-reaper
   task for a sweep that could target the wrong session (e.g. keyed by
   something that collides between `adminClient` and a freshly-created
   user session, or a reaper whose "past deadline" check races a
   just-issued ticket).
3. Check whether `admin.createUser`/`createScramUser` or `admin.chgrp`/
   `admin.chmod`/group-membership mutations have ANY path that closes
   OTHER live connections as a side effect (e.g. a broadcast/notify
   mechanism intended to invalidate ONE user's cached tickets that
   over-broadly closes the admin's own connection, or a shared
   connection-table entry keyed incorrectly). The comment at
   `e2e-permissions.test.ts:272-276` already documents that role
   grant/revoke bumps a target user's `tickets_invalid_before_ns` and
   explicitly avoids doing this to `userAClient` to sidestep exactly this
   kind of cascade — that comment is evidence the test author already
   suspected a class of "connection got invalidated by an unrelated admin
   op" bug and worked around it in ONE place; this investigation is about
   finding whether the SAME class of bug reaches `adminClient` itself via
   a different path (`createUser`, `chgrp`, `chmod`, or `addGroupMember`/
   `removeGroupMember`) that was not accounted for.
4. Try to reproduce locally: this is pure Rust server + TS client, so
   (unlike cluster D's platform-only hangs) you likely CAN reproduce this
   on a Windows dev box. Build the release server
   (`cargo build --release -p shamir-server`) and run
   `crates/shamir-client-ts`'s own test command (check `package.json` for
   the e2e script — likely `npm test` or a vitest invocation scoped to
   `e2e-permissions.test.ts`) in a LOOP (10-20x) to try to catch the drop
   locally. If it never reproduces locally, that itself is data — note
   what's different about the CI environment (timing, connection setup
   latency) that this local environment doesn't have, rather than
   guessing.
5. Once you find the actual mechanism (a specific connection/session table
   bug, an over-broad ticket-invalidation broadcast, a reaper race, or a
   genuine connection-count limit being hit), fix the ROOT CAUSE in the
   Rust server code — not the test. If the test itself SHOULD be more
   resilient in some legitimate way that isn't a workaround (e.g. it was
   relying on undocumented behavior), say so explicitly and justify it,
   but the default expectation is a server-side fix.

## Definition of done for this follow-up

- `e2e-permissions.test.ts` passes on a live CI run (`ts-e2e-nightly`
  `workflow_dispatch` on a diagnostic branch, or the next real nightly)
  without any test-side retry/reconnect logic added.
- Commit message states the actual mechanism found (which connection
  closed, why, and what code path triggered it).
- If reproduced locally: a repeat-run demonstration (loop the test file
  several times) showing the fix holds under repetition.
- `cargo fmt`/`cargo clippy --workspace --all-targets -- -D warnings` clean
  for any touched Rust crates; `./scripts/test.sh -p shamir-server --full`
  (and any other touched crate) green.

If, after a genuine investigation, you cannot find the mechanism, say so
explicitly rather than guessing a fix — report exactly what was ruled out
and what remains unknown, the same way the parent F-68 investigation
honestly reported cluster D's two hangs as unresolved rather than
papering over them.
