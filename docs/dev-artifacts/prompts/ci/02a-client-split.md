# CI-02a — split `crates/shamir-client-ts/src/core/client.ts` (1402 lines)

Parent brief: `docs/dev-artifacts/prompts/ci/02-layout-sweep-client-ts.md`. Pure
structural refactor of one file; behavior, public API and types stay identical.

## Rules (binding)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `git rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits.
- NO `cargo` / `npm` / `tsc` / `vitest` / `node` or any compile, test or
  install command. Allowed: read-only git (`git show`, `git ls-files`), `rg`,
  `wc -l`, `python -I scripts/ci/check_layout.py` (instant, no build), file
  tools, `mkdir`, `rm` of source files.
- Never silence a diagnostic with `@ts-ignore` / `@ts-expect-error`.
- Imports at the top of each file. Preserve existing comments verbatim when
  moving code; do not author new comments; do not reformat untouched code.
- No machine-specific absolute paths. Preserve LF endings and trailing newline.

## Task
Read `crates/shamir-client-ts/src/core/client.ts` in full (1402 lines). Known
layout: header/imports ~1–68; exported interfaces `TxOpened` (69), `ScramUserCreated`
(79), `SuperuserSet` (87), `ReplicatorSet` (95); private `interface PendingRequest`
(103 — STAYS in client.ts); `class ShamirClient` 114–~1315; module-level helpers
`withConnectTimeout` (1317), `tableRefToRepo` (1345), `extractRepo` (1357),
`batchHasRefs` (1384), `hasQueryRef` (1395).

Create `crates/shamir-client-ts/src/core/client-internals.ts` and move into it:
1. The four exported interfaces (with their doc comments), exported from the new
   file under the same names. In client.ts replace them with a type re-export
   keeping the public path stable:
   `export type { TxOpened, ScramUserCreated, SuperuserSet, ReplicatorSet } from './client-internals.js';`
   (src/index.ts and src/browser.ts do `export type { ... } from './core/client.js'`
   — do NOT edit those two files; the re-export keeps them valid.)
2. The five module-level helper functions (with doc comments), exported; client.ts
   imports the ones it uses.
3. The user-admin instance methods `createScramUser` (1026), `setSuperuser` (1055),
   `setReplicator` (1075), `changePassword` (1097), `resolvePrincipal` (1198):
   reimplement each body as an exported async free function in client-internals.ts
   with the same name and identical logic; the class method remains as a one-line
   delegate with IDENTICAL public signature and JSDoc, calling the free function.
   Free functions must NOT take the client instance nor touch private members:
   thread exactly the values/bound functions they use (e.g. the bound
   `sendDbRequest`, `hmacTagHex`, `_normalizedUser`) as parameters — read the
   bodies and choose the minimal set. No `private` → public widening anywhere.
4. If client.ts is still >1000 lines after 1–3 (it will be close), also move
   `mergeInternerDelta` (726), `touchFields` (748), `executeWithTouch` (832) the
   same way (free functions + one-line delegates, deps threaded explicitly).
   Target: BOTH files ≤1000 physical lines.

Constraints: identical exported-name surface for client.ts; no behavior change
(delegates add exactly one call); internal-only naming collisions avoided by
import aliasing in client.ts if needed. Do not touch any other file.

## Verify (no compile)
- `wc -l` both files ≤1000.
- `python -I scripts/ci/check_layout.py` from repo root exits 0.
- `rg "TxOpened|SuperuserSet" crates/shamir-client-ts/src` shows importers still
  on './core/client.js'.
Report: final line counts, exactly what moved, delegate list.
