# CI-1 (#916) — diagnose and fix "connection closed" cascade in e2e-permissions.test.ts

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

`ts-e2e-nightly`'s "ts client e2e (shamir-client-ts)" job has failed on
EVERY scheduled run for 10+ consecutive days (confirmed via `gh run list
--workflow=ts-e2e-nightly.yml`, runs back to at least 2026-07-23), unrelated
to any recent code change in this repo. The failure:
`crates/shamir-client-ts/src/__tests__/e2e-permissions.test.ts`'s FIRST
test, `"B-setup: create two databases and two users"`, fails with `Error:
connection closed` (thrown from `crates/shamir-client-ts/src/core/
framing.ts:103-105`'s `socket.onClose((err?: Error) => { this.fail(err ??
new Error('connection closed')); })`). Every subsequent test in the same
`describe` block then cascades into `TypeError: Cannot read properties of
null (reading 'executeWithTouch')` or assertion mismatches, because the
setup step's client handle never completed.

## What's already been ruled out (don't re-investigate this)

An earlier fix (commit `05c2028f`, "F-68 cluster C — panic=abort was
defeating all panic isolation", already on `master`) changed `Cargo.toml`'s
`[profile.release]` from `panic = "abort"` to `panic = "unwind"`, because
`panic = "abort"` was confirmed to defeat `shamir-server`'s per-connection
panic isolation (`connection/request_loop.rs`'s per-request `JoinSet` only
protects other connections if the panic actually unwinds). This looked
like a strong candidate explanation for "connection closed" (a server
crash would look exactly like this to a client). **It is NOT the explanation
for the CURRENT failure**: verified that the failing CI run
(`30727972168`, dispatched 2026-08-02) ran against commit `ed8bba00`, which
already includes `05c2028f` (`panic = "unwind"` confirmed present in that
commit's `Cargo.toml`) — yet the exact same "connection closed" cascade
still occurred. Whatever is closing the connection today is a DIFFERENT,
still-live issue (or the unwind fix has some gap this task needs to find).

## The missing diagnostic — fix this FIRST

`e2e-harness.ts`'s `startServer()` (lines 278-350) spawns the release
`shamir-server` binary and accumulates its stdout+stderr into `logBuf`,
exposed via the returned `ServerHandle.logs()` accessor (line 348). But
`e2e-permissions.test.ts` only calls `server.logs()` in ONE place — the
`beforeAll`'s `catch` block (lines 68-78), which only fires if the VERY
FIRST `connectAdmin` call fails. If a LATER test (like `B-setup`) is what
triggers the connection closure, `server.logs()` is never printed anywhere
— so today, CI's own log gives ZERO visibility into whether the server
panicked (and where), was killed by something external, hit a resource
limit, or something else entirely.

**Before attempting any fix, add this diagnostic**, following the same
"instrument, commit, observe on real CI, THEN fix" workflow this repo used
successfully for the F-68 cluster D hangs
(`docs/dev-artifacts/prompts/post-alpha/124-f68-cluster-d-hang-instrumentation.md`
is a good style reference). Concretely: wrap the test suite (or use
Vitest's `onTestFailed`/a global `afterEach` inside this `describe` block)
so that ANY test failure inside `e2e-permissions.test.ts`'s top-level
`describe` prints `server.logs()` to make the server's full stdout/stderr
visible in the CI log at the point of failure — not just the initial
connection attempt. Keep this instrumentation minimal and test-only (no
production code changes for this step).

Commit this diagnostic alone first if it's a clean, self-contained change,
then trigger `ts-e2e-nightly` on real CI (`gh workflow run
ts-e2e-nightly.yml --ref master`) and read the resulting log — the
orchestrator will do this triggering step and hand you the log output to
continue from, OR you may trigger it yourself if you have `gh` CLI access
in your environment (check first; if not, stop after landing the
diagnostic and report back what you added so the orchestrator can trigger
and relay the log).

## Once you have the server log from a real failing run

Determine the actual cause from the now-visible server-side log around the
`B-setup` test's operations (per `e2e-permissions.test.ts` lines 373-413:
two `setupDb()` calls — each running a `createDb` + `createRepo` +
multiple `createTable` batch — two `seed()` data-insert passes, two
`admin.chmod()` calls, then `createUserAndConnect(USER_B, USER_B_PW)`).
Look for:
- A panic message (even under `panic = "unwind"`, an UNCAUGHT panic in a
  context outside any `JoinSet`/`catch_unwind` boundary — e.g. the main
  accept loop, a background scheduler tick not wrapped in `catch_unwind` —
  would still kill the whole process; check
  `crates/shamir-server/src/scheduler.rs` and the connection-accept loop
  for any gaps in unwind-safety coverage).
- An explicit `std::process::exit`/`abort()` call somewhere in the DDL/user
  -creation path that might fire on a specific input this sequence
  produces.
- A resource-limit rejection that the server responds to by closing the
  socket instead of returning a structured error (check
  `security.connection.max_active_connections`,
  `security.query_limits.*`, and the two-databases-two-users sequence
  against any relevant default configured by `e2e-harness.ts`'s
  `writeKtavConfig`).
- Anything OS/environment-specific (the CI runner is `ubuntu-latest` per
  the workflow; check if this reproduces on a locally-built release binary
  too, or only under CI's specific resource constraints — this matters for
  deciding if it's a genuine bug vs. a CI-runner resource ceiling).

Fix the ROOT CAUSE once identified — do not paper over the symptom (e.g.,
do not just add a retry/reconnect to the test without first understanding
WHY the server closed the connection).

## Definition of done

- Diagnostic instrumentation added and (ideally) confirmed to surface
  useful log output on a real CI run.
- Root cause identified and fixed (server-side, if that's where the actual
  bug is) OR the test updated if investigation proves the test itself is
  wrong about expected behavior — but only after understanding why, never
  as a default fallback.
- `cargo fmt`/`clippy --workspace --all-targets -- -D warnings` clean for
  any Rust changes; TS lint/build clean for any `shamir-client-ts` changes.
- Re-run `ts-e2e-nightly` on real CI after the fix and confirm the
  `e2e-permissions (requires release binary)` suite passes end-to-end (not
  just `B-setup` — the whole cascade should resolve since it's all
  downstream of this one root cause).

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
