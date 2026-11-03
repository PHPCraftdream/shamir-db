# TOKEN-AUTH3 - end-to-end scenarios and documentation of login by password over HTTP (server + JS client)

Base: AUTH1 (server login endpoints and their tails), AUTH2 (JS `login(...)`), JS4 (e2e suites in `crates/shamir-client-ts/src/__tests__/connection/token/`, harness in `support/harness.ts`, nightly lane `.github/workflows/ts-e2e-nightly.yml`) and JS5 (`crates/shamir-client-ts/docs/TOKENS.md`) are all on master. Read first: `docs/guide-docs/client-server-protocol-spec/TOKEN_LOGIN_HTTP.md`, `TOKEN_API.md`, the existing `connection/token/e2e-login.test.ts` (it covers only a successful login plus a wrong password and a backoff case), `e2e-revocation.test.ts`, `e2e-browser.test.ts`, the server e2e `crates/shamir-server/tests/e2e/http_login/` (what the server side already proves), and `crates/shamir-client-ts/docs/TOKENS.md`. CLAUDE.md applies (layout: at most 7 entries per folder, 1000 lines per file; `connection/token/` already has 7 entries, so new test files go into a NEW subfolder or extend `e2e-login.test.ts` while it stays under 1000 lines; queries only through the typed builder of `shamir-client-ts`, never raw JSON).

## Deliverable 1 - audit table, then scenarios

Write `docs/dev-artifacts/audits/2026-11-03-token-auth3/DESIGN.md` with one row per scenario below, with the covering test (`file::test name`) or GAP, then implement only the GAP rows with the TS client against a real `shamir-server` (self-skipping like the other e2e suites):

1. Node.js login over loopback http AND over https (generated test certificate, CA bundle option), success -> the token client does a read, a write and an interactive transaction over http and over ws (`channel`) and a subscription over ws.
2. Browser-style login (the browser entry `src/browser.ts` under Node with `fetch`-compatible transport as the existing browser suite does) and then the browser first-frame WebSocket with the issued token.
3. Wrong password: typed error, no token issued; repeated wrong passwords: the documented lockout and `Retry-After`; a locked account refuses the correct password until the lock expires or an admin unlock; the failure limiter must not be tripped by earlier cases (put lockout cases last and on their own user).
4. `login_ttl`: a token from login expires after the configured TTL (use a short configured value in the harness config and a bounded poll on observable state, never `setTimeout` sleeping to wait for server state): http gets 401 and an open ws closes with the documented code.
5. Revocation: admin revokes the login token (`client.apiTokens.revoke`) and the session dies as documented.
6. Password change of the user revokes the user's login tokens (change through the SCRAM client).
7. Role-ineligible users: superuser and replicator-owned logins are refused with the documented code; read-only mode and channel restrictions of a login token as documented.
8. The server does not accept the login endpoint when `login.enabled` is false (404), and a wrong `Host` authority is refused as documented.

If an assertion contradicts the server behaviour, record it in the audit with evidence instead of editing the test to match a bug. Validation is by the nightly lane in the fork (`gh workflow run ts-e2e-nightly.yml --ref <branch>`), which the orchestrator runs; Windows and Linux are both covered by the fork CI.

## Deliverable 2 - documentation

- `crates/shamir-client-ts/docs/TOKENS.md`: extend the login section only where the scenarios above show something undocumented (browser limits, where to keep the token, lock/backoff handling for the application, TTL expiry handling); do not duplicate existing text.
- Operator guide: the operator-facing keys for login (`login.enabled`, `login.authority_allowlist`, `login.listener_id`, `tokens.login_ttl` or its real name in `crates/shamir-server/src/boot/config/`, limits, TLS and HSTS, shutdown budget 7 s) in `docs/guide-docs/guide/07-operations.md` or the token operations file introduced by JS5; verify each key against the config parser, never from memory. The final T8 task writes the full operator manual later; keep this to the login keys.
- Remove the stale status banner of `TOKEN_LOGIN_HTTP.md` ("not implemented yet", the "This feature is not implemented; no build or tests are claimed" paragraph in section 9) only if the code on master really implements the described behaviour; otherwise list the divergences in the audit.

## Rules

- NO LOCAL COMPILATION OR TESTS: the only commands allowed are `python -I scripts/ci/check_layout.py` (and `cargo fmt -p <crate> -- --check` if you touch Rust). No `npm`, `tsc`, `vitest`, no `cargo check/clippy/build/test`, no server start. A child that runs them violates the policy (three children already did and left 2 GB `target/` directories): say so in every child prompt. No dependency, `package.json`, lockfile or workflow changes without stopping and reporting.
- TypeScript strict; vitest 1.6 rules: hook callbacks must return void, no `Mock<any[], any>`, do not count microtasks, `JSON.stringify` throws on bigint, `toEqual` Buffer vs Uint8Array fails, `Buffer.slice()` aliases memory. Every network wait has an explicit timeout.
- Nothing is run: finish with a compiler-style read of every file you wrote (imports, types under strict, unused variables, vitest API usage).
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. No machine-local absolute paths in any file you write.
- Children only edit files, are awaited, and every touched file is re-read as the compiler would. Never finish with a child outstanding.
- Time: run `date` first; stop at the time in the run file and report per scenario DONE / PARTIAL / BLOCKED with `file:line` in the audit file.
