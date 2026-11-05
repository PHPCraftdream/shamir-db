# Brief: failure-only diagnostics, TypeScript flaky tests (FLAKELOG-2)

Read first: `docs/dev-artifacts/design/flake-failure-logs.md`, `CLAUDE.md`,
`crates/shamir-client-ts/src/__tests__/connection/token/support/subscription-diagnostics.ts` and
`crates/shamir-client-ts/src/__tests__/connection/token/e2e-login.test.ts` (the existing pattern: timeline +
server log rendered into the error, only on failure).

## Deliverable

1. Generalise the pattern into a small shared helper next to the existing support code (no new dependency): a
   per-test recorder with `mark(label)`, `note(key, value)`, a bounded buffer of client-side events (frames
   sent/received, close codes, errors) and a `render()` appended to the failure only. Provide
   `withFailureDiagnostics(recorder, serverLogs, fn)` (or the equivalent for the repo's vitest style) that runs
   the body and, if it throws, rethrows an error whose message carries the original message plus the rendered
   timeline, notes, client events and the server log tail; when the body passes it prints and records nothing.
2. Adopt it in:
   * the token WS e2e tests under `crates/shamir-client-ts/src/__tests__/connection/token/` that expect a close
     code or a reset (the TypeScript counterpart of the 4012 case);
   * `crates/shamir-client-ts/src/login/__tests__/browser-login.test.ts`: the test "settles promptly on abort
     while fetch is pending and cancels a late response" (marks: fetch started, abort called, promise settled
     with what, late response delivered, timings);
   * any other e2e under `__tests__/connection/` that uses `fixture.server.logs()` for diagnostics today (read
     them and use the helper instead of ad-hoc code, keeping behaviour).
3. Unit tests for the helper itself in the existing vitest layout (a passing body leaves no output; a throwing
   body carries the rendered sections; the buffer bound holds).

## Acceptance

No `console.log` in passing tests; the failure message contains timeline, notes and server log tail; no test
timeout or assertion changed; TypeScript compiles under the repo's config by reading (you cannot run tsc).

Final message: files changed, what each adopted test now records, anything you could not confirm.

## Policy (verbatim, mandatory)

**Compilation, builds and tests run on GitHub CI, not on this machine.** Banned: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest builds and test runs,
e2e/integration harnesses, any release or full-workspace build. You may not run npm, tsc or vitest: only edit
files. The only command you may run is `python -I scripts/ci/check_layout.py` (it reads tracked files only: the
orchestrator stages before the final check). A child that believes it needs a build or test run says so in its
final message.

> ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
> working tree or index. Only edit files; the orchestrator commits.

Layout: at most 7 entries per folder, at most 1000 lines per file. No machine-local absolute paths in any file you
write. Do not mention how or where CI is run. No version bumps; no dependency changes.
