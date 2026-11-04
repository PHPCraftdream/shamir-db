# T9-1b / S4 - TS client: `changePassword` must use the session's real channel binding

Base: master (S0-S2 landed: `core/protocol/session/` with `ChannelBinding`, `ShamirClient._channelBinding` stored but unused). Design: `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN.md` section 2.2 (read it, then CLAUDE.md). Crate: `crates/shamir-client-ts`.

## Problem (verify it first, by reading code)

`changePassword` in `src/core/client/client-internals.ts` (around line 392-440) builds `auth_message_cp` with a hard-coded `transportKind: TRANSPORT_KIND_WS` and `bindingMode: BINDING_MODE_TLS_NO_EXPORT`. The server builds `auth_message_cp` from the real parameters of the session (`crates/shamir-connect/src/server/auth/changepw.rs`, around lines 140-150, and whatever it reads from the session). For a session opened with `connectLocal` (IPC: `TRANSPORT_KIND_UNIX`, `BINDING_MODE_NONE`) the two sides then sign different messages and the change fails with an authentication error. The existing e2e (`src/__tests__/auth/e2e-change-password.test.ts`) only covers the browser-WS session, where the hard-coded values happen to match.

Step 0 (mandatory, write the result into the report): read the server side and confirm which fields it uses (transport kind, binding mode, exporter bytes) and whether the client's `buildAuthMessageCp` (`src/core/protocol/scram.ts`) can express them. If the server does NOT depend on the session transport, stop making the code change, keep the new e2e test as a regression guard and say so in the report.

## Change

- `changePassword` takes the session's `ChannelBinding` (from `ShamirClient._channelBinding`, passed by the `ShamirClient.changePassword` method) and uses its `transportKind` and `bindingMode` (and the exporter if `buildAuthMessageCp` needs it; for both existing profiles it is zeros). For a resumed client the binding is `undefined`: keep the existing "requires a full connect() session" refusal; do not invent a binding.
- Remove the now unused hard-coded constants/imports if they become unused (TS `noUnusedLocals` is off but keep imports clean).
- New e2e test `src/__tests__/auth/e2e-change-password-ipc.test.ts` (the folder `__tests__/auth` has 3 files, 4 after this): start the server through `e2e-harness.ts` with the IPC listener the way `src/__tests__/connection/session/e2e-connect-local.test.ts` does (read it first and reuse its harness helpers; if the harness lacks a helper, add the smallest one inside `e2e-harness.ts` without changing its existing exports), create a SCRAM user through an admin session, `connectLocal` as that user, `changePassword(old, new)`, then assert the old password no longer works and the new one does (reconnect with `connectLocal`), mirroring the structure of `e2e-change-password.test.ts`. Add one more case: wrong old password over IPC is rejected with the same error as over WS.
- Also extend a unit test next to the code (find the existing unit tests for `changePassword`, e.g. under `src/core/client/__tests__/`): assert the `auth_message_cp` bytes differ between a WS-profile and an IPC-profile client, i.e. the binding is really used. Use fakes only; no network.
- Layout: `core/client` is at 7 entries (add nothing there), at most 1000 lines per file (`client.ts` is 777), `python -I scripts/ci/check_layout.py`.

## Boundary

Public API unchanged (`ShamirClient.changePassword(old, new)` signature). Nothing else in `client-internals.ts` changes. No wire change for the browser-WS session (its binding is exactly the old constants). Do not touch the token path or `login/`.

## Rules

- NO LOCAL BUILDS OR TESTS: no `npm`, `tsc`, `vitest`, `node` test runs, no cargo. Only allowed commands: read-only file listing and `python -I scripts/ci/check_layout.py`. No `package.json`, lockfile or workflow edits; a new dependency means stop and report. Children (if any) only edit files and must not run commands.
- Without a compiler, read every touched file as `tsc --strict` would: import paths with `.js`, exported names, `import type`, circular imports, the signature change and every caller of `changePassword` (grep the package, tests included).
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show/log/diff` is fine). Only edit files; the orchestrator commits. No machine-local absolute paths.
- Hard session limit 150 minutes: stop editing 40 minutes before the time in the run file and spend that time on the compiler-style read. Never leave a half-edited item. Append a "T9-1b S4 report" section to the audit DESIGN.md as the last step (Step 0 finding, files changed).
- Verification is CI only: the orchestrator runs `ci.yml` (full tier) and dispatches `ts-e2e-nightly.yml` on the branch; this e2e is the red/green proof.
