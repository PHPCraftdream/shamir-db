# TOKEN-AUTH2 follow-ups - PRECIS vectors, error codes, buffer wiping, public API surface, channel check

Base: `auth2-login-ts.md` (original brief) and the audit `docs/dev-artifacts/audits/2026-11-02-token-auth2/DESIGN.md`. AUTH2 (login over HTTP in `shamir-client-ts`) and AUTH1 (the server side, `crates/shamir-server/src/security/perimeter/http_login/`, `crates/shamir-connect/src/server/auth/http_login/`) are both landed on master. Spec: `docs/guide-docs/client-server-protocol-spec/TOKEN_LOGIN_HTTP.md` and AUTH_PROTOCOL.md section 15.3 / 16 (cross-language username consistency is a release blocker).

## Scope (all items, in this order)

1. **PRECIS username vectors shared by Rust and TS.** Create a vector file (`username_precis.json`, location and loader pattern as `crates/shamir-query-types/test-vectors/admin_hmac_v2.json` and its Rust and TS consumers). Derive the cases by reading `crates/shamir-connect/tests/identity/integration_username_precis.rs` and the Rust `NormalizedUsername` implementation (about 20 cases: accepted inputs with the exact normalized string, rejected inputs with the reason class). You cannot run code, so the expected values are derived by reading the Rust implementation; a Rust test (new file in the existing test tree of `shamir-connect`, mounted) asserts every vector against `NormalizedUsername`, and a TS test asserts the same file against the login normalization in `src/login/protocol.ts`. Where the TS normalization disagrees with Rust, fix the TS side (or fail closed) and list it in the report. Multi-line, indented JSON.
2. **Distinct error code for a server-signature mismatch after HTTP 200** (not `authentication_failed`): add it to the login error codes (`src/login/errors.ts`, public type), use it in `core.ts`, test it in the fake-server tests.
3. **Fully wipe response chunks** in `src/login/node/node-http.ts` and `src/login/browser/browser-http.ts` (every chunk and the concatenated body, on success, error and abort paths); test via the injected fakes. Remember `Buffer.slice()` aliases memory (copy with `new Uint8Array(x)` before wiping what you keep).
4. **Do not expose the injected `LoginHttp` in the public API** (keep injection internal, for tests only; update exports in `src/index.ts` / `src/browser.ts` and the isolation-graph test).
5. **`connect({ channel })` checks `metadata.channels` before the request** and fails with a typed error without network I/O when the channel is not allowed.
6. **The overall `timeoutMs` must not be reported as `client_aborted`** (distinct timeout code), keep abort-by-caller as `client_aborted`.
7. **CA / development TLS option for Node login** (an explicit `ca` / `rejectUnauthorized`-free option: pass a CA bundle; never a switch that disables verification). Typed in `LoginOptions`; test with the injected transport.
8. **Fixed transcript and SCRAM vectors checked against the server implementation:** read the landed server transcript code (`crates/shamir-connect/src/server/auth/http_login/transcript.rs` and its tests/vectors) and add the TS test that reproduces the same transcript bytes from the same fixed inputs (and, if the server side already has published vectors, load that file instead of hand-copying).

## Constraints

- No dependency or `package.json` / `Cargo.toml` / lockfile changes (a new dependency means stop and report).
- TypeScript strict; vitest 1.6 rules: hook callbacks must return void, no `Mock<any[], any>`, do not count microtasks, `JSON.stringify` throws on bigint, `toEqual` Buffer vs Uint8Array fails, `Buffer.slice()` aliases.
- Layout: at most 7 direct entries per folder (`src/login/` is already over: do not add files there, put new code into the existing `node/`, `browser/`, `__tests__/` subfolders or split into a new subfolder and reduce the entry count if you touch it), at most 1000 lines per file, one primary export per file.
- Query construction rules and everything else in CLAUDE.md apply.

## Rules (restated)

- NO LOCAL COMPILATION OR TESTS: only `cargo fmt -p <crate>` (and `-- --check`) and `python -I scripts/ci/check_layout.py`. No cargo check/clippy/build/test, no rustc, no npm/tsc/vitest. A child that runs them violates the policy (three children already did and left 2 GB `target/` directories): say so in every child prompt.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index; only edit files; the orchestrator commits. No machine-local absolute paths in any file you write.
- Children only edit files, are awaited, and each touched file is re-read as the TypeScript compiler / vitest / rustc would. Never finish with a child outstanding.
- Time: run `date` first; stop editing at the time in the run file and report exactly what is done (per item DONE / PARTIAL, file:line). Append the report to `docs/dev-artifacts/audits/2026-11-02-token-auth2/DESIGN.md` under "Follow-ups".
