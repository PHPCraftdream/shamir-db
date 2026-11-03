# TOKEN-JS4 implementation report

## Status
**PARTIAL — source implementation and compiler-style static review completed; runtime validation pending CI.** No TypeScript compiler/typecheck, builds, tests, or server runtime were run. No runtime-pass claim.

## Deliverables — static coverage and validation status
1. **Token administration — DONE (source); PARTIAL (runtime).** `crates/shamir-client-ts/src/__tests__/connection/token/e2e-admin.test.ts:23-106` covers create, usable token, public listing shape, individual revoke and revoke-user. The public API redacts `ApiToken`; list checks absence of token/secret/MAC/hash fields. This does **not** independently establish what the raw creation response returned: `crates/shamir-client-ts/src/core/client/client-internals.ts:38-45` validates the response and discards its local reference, while `:113-128` constructs list entries from selected fields. Wire-level secret-not-returned-by-list remains unverified.
2. **Node transports and scopes — DONE (source); PARTIAL (runtime).** `e2e-channels.test.ts:24-127` covers HTTP, HTTPS, WS, WSS, ping, typed write/read batches, WS transaction and cursor operations, plus read-only attempts on every transport. `:168-205` covers channel scope. Cursor config limits each session to one cursor (`support/harness.ts:101-104`); the test cancels/closes one then creates another to exercise cancellation with that cap. None of these assertions were run.
3. **Browser-style WS — DONE (source); PARTIAL (runtime).** `e2e-browser.test.ts:27-68` tests ws/wss first-frame shape and no Authorization; `:70-99` tests Origin rejection. `support/browser-socket.ts:46-116` observes the real WS handshake, first binary frame and close/status. TLS WSS trusts the generated certificate via CA input; the shared cert helper has SANs for localhost/127.0.0.1 (`src/__tests__/e2e-harness.ts:245-255`). Runtime handshake behavior remains unverified.
4. **Revocation/expiry — DONE (source); PARTIAL (runtime).** `e2e-revocation.test.ts:22-37` expects browser WS close 4011 and later REST 401; `:39-53` expects 4012 on short TTL and later REST rejection; `:55-64` expects browser channel denial 4013. No observed server result.
5. **Error mapping — DONE (source); PARTIAL (runtime).** `e2e-errors.test.ts:57-113` checks malformed/unknown/wrong canonical token, invalid channel, size cap, and REST transaction/cursor unsupported behavior. No runtime mapping verified.
6. **Password login/nightly lane — DONE (source); PARTIAL (runtime).** `e2e-login.test.ts:12-19` checks working login and wrong-password typed error; `:20-36` puts same-account failure/backoff case last. Server source shows begin checks pair lockout/backoff and returns 429 (`crates/shamir-server/src/security/perimeter/http_login/service/begin.rs:112-129`); finish registers failures (`.../service/finish.rs:116-125`). This supports the test’s intent, not a runtime pass. `.github/workflows/ts-e2e-nightly.yml:66-90` builds/verifies/exports `SHAMIR_SERVER_BIN` and runs TS typecheck/e2e.

## Harness source review
`crates/shamir-client-ts/src/__tests__/connection/token/support/harness.ts` uses three-level public-entry imports (`:6-7`), shared harness imports at two levels (`:8-17`), native local IPC SCRAM for admin/setup (`:222-264`), typed builders/batches and four multiline seed records (`:234-260`), distinct ephemeral plain/TLS HTTP listeners and listener log-bound port discovery (`:39-135`), and finite startup/shutdown waits (`:153-219`). Cursor cap is configured at `:101-104`. `generateSelfSignedCert` is reused from the existing harness; no duplicate helper was introduced. Windows pipe naming remains a unique temp-dir-based raw pipe endpoint (`:33-37`); server source validates `kind: unix` addresses through the platform-aware IPC endpoint validator (`crates/shamir-server/src/boot/config.rs:673-688`, `crates/shamir-transport-ipc/src/endpoint.rs:47-78`). The `kind=unix` listener log matcher is case-insensitive (`harness.ts:119-123`).

## Behavior distinctions and remaining limits
- Native wrong-channel WS is an HTTP 403 before upgrade; Node client maps 403 to `permission_denied` (`crates/shamir-client-ts/src/platform/token/socket-node.ts:20-22`). Browser channel denial is post-upgrade close 4013, distinct from 4010-4012; browser pre-binding errors map to `authentication_failed` (`crates/shamir-client-ts/src/entry/ws/token-lifecycle.ts:40,50,128`). The tests preserve raw observer checks; no production client behavior was changed.
- Browser Origin rejection expects the real HTTP 403 and WS abnormal close; neither was observed locally.
- The TLS matrix explicitly permits insecure development TLS for Node (`e2e-channels.test.ts:34-44`); browser WSS instead supplies the generated certificate CA (`e2e-browser.test.ts:38-49`).
- All e2e, typecheck and server behavior remain unverified until CI.

## Crash-resilient progress journal
- DONE (source): harness uses local IPC SCRAM, typed fixture builders, finite waits, two ephemeral HTTP listeners, and bounded subprocess cleanup.
- DONE (source): token administration create/list/revoke/revoke-user coverage authored.
- DONE (source): HTTP/HTTPS/WS/WSS matrix, read/write, WS transaction/cursor, channel/read-only coverage authored.
- DONE (source): browser WS first-frame, no-Authorization and Origin observation authored.
- DONE (source): live revocation/expiry and channel-denial raw close-code assertions authored.
- DONE (source): malformed/wrong/unknown token, request cap and REST unsupported-operation assertions authored.
- DONE (source): HTTP login/password failure and final same-account backoff case authored.
- DONE (source): nightly lane binary export/verification and TS typecheck steps authored.

## Layout and validation
`python -I scripts/ci/check_layout.py` was observed to exit 0 with `layout check: OK (0 baselined, 0 hint(s))`; this checker result does not establish compliance for untracked/new files. Manual tree review: `connection/token/` has seven direct entries (six tests plus `support/`); `support/` has two. However, required `src/__tests__/connection/token/` makes `src/__tests__/` contain eight direct entries where the limit is seven. No unrelated moves or baseline/checker changes were permitted. **Parent layout remains unresolved and requires orchestrator decision.** No commands other than the previously observed layout check were run by this worker; no builds, typecheck, tests or server runs. No dependencies, manifests, Cargo files or lockfiles changed.

## Changed paths in this worktree
- `crates/shamir-client-ts/src/__tests__/connection/token/e2e-admin.test.ts`
- `crates/shamir-client-ts/src/__tests__/connection/token/e2e-browser.test.ts`
- `crates/shamir-client-ts/src/__tests__/connection/token/e2e-channels.test.ts`
- `crates/shamir-client-ts/src/__tests__/connection/token/e2e-errors.test.ts`
- `crates/shamir-client-ts/src/__tests__/connection/token/e2e-login.test.ts`
- `crates/shamir-client-ts/src/__tests__/connection/token/e2e-revocation.test.ts`
- `crates/shamir-client-ts/src/__tests__/connection/token/support/browser-socket.ts`
- `crates/shamir-client-ts/src/__tests__/connection/token/support/harness.ts`
- `.github/workflows/ts-e2e-nightly.yml`
- `docs/dev-artifacts/audits/2026-11-02-token-js4/DESIGN.md`

## Final summary
PARTIAL: added six token e2e suites and two support files under `crates/shamir-client-ts/src/__tests__/connection/token/`, and updated `.github/workflows/ts-e2e-nightly.yml`.
Source coverage includes all six brief items; compilation and runtime behavior remain unverified because local builds/tests were prohibited.
`python -I scripts/ci/check_layout.py` passed for tracked files, but the new `token/` makes its parent contain eight entries (limit seven): layout remains unresolved.
Full status and remaining limitations are in `docs/dev-artifacts/audits/2026-11-02-token-js4/DESIGN.md`; no manifests, lockfiles, commits, or pushes changed.
