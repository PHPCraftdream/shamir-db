# T9-1b / S0-S2 - TS client pre-refactor: explicit session layers (behaviour unchanged)

Base: master. Design: `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN.md` section 2.2 (read it first, then CLAUDE.md). Crate: `crates/shamir-client-ts`. S3 (dialers) and S4 (the `changePassword` binding fix) are NOT part of this brief.

## Goal

Separate the session path into a transport-neutral link description and a handshake step, without changing a byte on the wire or any exported symbol.

- **S0 (pure move):** move the tests `__tests__/connection/{connect,resume,e2e-connect-local,e2e-resume}` (the unit and e2e files with those names; list the folder first and move only files that exist) into a new subfolder `__tests__/connection/session/` so the folder has free slots. Fix their relative imports. Do not change test bodies.
- **S1 (types + binding):** add `core/protocol/session/` with `binding.ts` (`ChannelBinding {transportKind, bindingMode, exporter}` plus the constants it uses, re-using the existing constants in `core/protocol/scram.ts`), `endpoint.ts` (`SessionEndpoint`, `SessionLink`), `index.ts` (re-exports). `runHandshake` (`core/protocol/protocol.ts`) takes the binding as input while keeping its current defaults for existing callers; `ShamirClient` stores the binding it was opened with (new private/internal field, no new public member). The browser and Node-WS profile is `{transportKind: WS, bindingMode: TLS_NO_EXPORT, exporter: zeros}`; the IPC profile is `{transportKind: UNIX, bindingMode: NONE, exporter: zeros}`.
- **S2 (extract bodies):** move the bodies of `ShamirClient.connect`, `connectLocal` and `resume` out of `core/client/client.ts` into `core/protocol/session/open.ts` and `resume.ts`; the static methods stay as thin delegates with identical signatures. `openSession` returns plain data for the constructor, never a `ShamirClient`, so there is no import cycle. `client.ts` must end below 800 lines (it is 993 today).
- Add a unit test `core/protocol/session/__tests__/binding.test.ts` asserting the `auth_init`-relevant fields produced for the browser profile and for the ipc profile (use the existing test helpers and fakes; no network).

## Boundary (must hold)

- Exports of `src/index.ts` and `src/browser.ts` unchanged; signatures of `ShamirClient.connect/connectLocal/resume(platform, opts)` and the types `ConnectOptions`, `ConnectLocalOptions`, `ResumeOptions` unchanged.
- The token path (`entry/*`, `platform/token/*`, `core/protocol/token/*`, `login/*`) is not touched.
- Guard tests keep their bodies: `__tests__/connection/{client-demux,...}`, `core/client/__tests__/{client,connect-local}.test.ts`, `platform/__tests__/{node-ipc-socket,ws-subprotocol}.test.ts`, `platform/connect/__tests__/public-connect.test.ts`, `core/protocol/__tests__/*`, and the e2e files (`e2e-subscriptions`, `auth/e2e-change-password`, data and query e2e). Only the S0 moves change their location/imports.
- Layout: at most 7 entries per folder (`core/protocol` currently has 6 incl. `__tests__`, so `session/` makes 7; `__tests__/connection` and `core/client` are at 7: add nothing there), at most 1000 lines per file, one primary export per file. Run `python -I scripts/ci/check_layout.py`.
- Keep the wire identical: do not touch `core/protocol/scram.ts` framing, and do not change which profile `connect` uses (it stays on `wss://<host>:<port>/shamir/v1/browser` with `BINDING_MODE_TLS_NO_EXPORT`).

## Rules

- NO LOCAL BUILDS OR TESTS: no `npm`, `tsc`, `vitest`, `node` test runs, no cargo. The only allowed commands are read-only file listing, `python -I scripts/ci/check_layout.py`, and `cargo fmt` is irrelevant here. No `package.json`, lockfile or workflow edits; a new dependency means stop and report.
- Without a compiler, read every touched file as `tsc --strict` would: import paths with the `.js` extension used by this package, exported names, `import type`, circular imports (draw the import graph of `session/*` and `client.ts`), exhaustive unions, optional-property typing under `exactOptionalPropertyTypes` if the tsconfig enables it, and vitest config include patterns for the moved tests.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show/log/diff` is fine). Only edit files; the orchestrator commits. A moved file that must disappear from its old location is reported (the orchestrator removes it); do not leave the old copy edited. No machine-local absolute paths.
- Hard session limit 150 minutes: stop editing 40 minutes before the time in the run file and spend that time on the compiler-style read. Never leave a half-moved item: finish it or revert by editing back and record it BLOCKED. Append a "T9-1b S0-S2 report" section to the audit DESIGN.md as the last step.
- Verification is CI only: the orchestrator runs `ci.yml` (full tier) and `ts-e2e-nightly.yml` via dispatch on the branch.
