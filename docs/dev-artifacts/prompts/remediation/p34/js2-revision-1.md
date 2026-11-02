# TOKEN-JS2 revision 1 - make the WebSocket transport compile-safe and complete

State: the first run of `js2-ws-transport.md` (read it in full first) produced a PARTIAL result that is committed in this branch (`git log -5`). Its own report lists unresolved defects: read `docs/dev-artifacts/audits/2026-11-01-token-js2-ws/DESIGN.md` ("Final report" and the notes) first. The JS1 review fixes were merged into this branch.

Orchestrator changes since that run (do not redo): `src/entry/token-lifecycle.ts` and `src/entry/token-ws-client.ts` were moved with `git mv` into the new folder `src/entry/ws/` (the `entry/` folder had 8 entries; it now has 7). THEIR RELATIVE IMPORTS AND ALL IMPORTS OF THEM (including the tests under `src/entry/__tests__/`) MUST BE UPDATED BY YOU FIRST; then check the whole `crates/shamir-client-ts/src` tree for any other stale path.

## Scope

1. Fix every defect named in the report: duplicate `Batch` type/value imports, missing `ShamirDbError` import, the socket-open callback 3-vs-4-argument typing, transaction union mismatches, `next()` on a subscription handle returning `IteratorResult` instead of `Event | undefined`, the cursor not being a terminal-aware subclass, missing response validation, lifecycle tests using the old connect signature and losing the early binding frame (hang), incomplete resource cleanup on terminal close (pending requests, cursors, transactions, subscriptions, timers, listeners).
2. Then do a full strict-TypeScript read-through of EVERY file this stage added or changed (`git diff --stat 6da5ac89b..HEAD -- crates/shamir-client-ts` lists them; also the files under `src/entry/ws/`, `src/platform/token/socket-*.ts`, `src/core/protocol/token/ws.ts`): read `tsconfig.json` and `configs/tsconfig.build.json` first (strict flags, `exactOptionalPropertyTypes`, `noUncheckedIndexedAccess`, `noUnusedLocals`), check every import path with its `.js` extension, every exported name, type-only imports, `Uint8Array`/`Buffer`/`ArrayBuffer` assignability across lib.dom, `@types/node` and `@types/ws`, discriminated-union narrowing, floating promises, unhandled rejections in fakes, and that every test's oracle matches the implementation.
3. Complete the test coverage listed in `js2-ws-transport.md` that is still missing (binding validation variants, rid correlation out of order, early-message buffer bound, every close-code mapping, terminal cleanup of cursors/transactions/subscriptions, the browser first frame exactness, refusal of non-loopback `ws://` in the browser adapter, no `node:` imports in the browser graph, token redaction, `executeWithTouch` makes no interner call).
4. Keep every folder at most 7 entries and every file under 1000 lines; one primary export per file; report folder counts before and after.

## Rules

- NO LOCAL BUILDS OR TESTS: no `npm`, `npx`, `tsc`, `vitest`, `node` runs, no installs, no cargo. The only command you may run besides reading is `python -I scripts/ci/check_layout.py` (it ignores untracked files: count new files by hand). CI typechecks and runs vitest after the orchestrator pushes.
- Touch only `crates/shamir-client-ts/` and the report. No dependency changes.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. You cannot move or delete files: list such needs (the layout is the orchestrator's job, but do not create new files in a full folder).
- DEADLINE RULE: when the time limit comes, STOP EDITING AND REPORT. Never revert, restore, delete or empty files to "clean up"; partial work stays and is reported as incomplete.
- No machine-local absolute paths in any file you write (repo-relative only). Do not create files outside `crates/shamir-client-ts/` and the audit note folder (a previous run created a stray `crates/shamir-db/worktrees/...` path: do not).
- Append your notes and a `## Revision 1 final report` (per item DONE / PARTIAL / DISPUTED with file:line, tests by name, files read end to end, what needs CI) to `docs/dev-artifacts/audits/2026-11-01-token-js2-ws/DESIGN.md`.
