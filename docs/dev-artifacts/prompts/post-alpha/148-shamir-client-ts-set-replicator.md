# Task #926 -- add setReplicator to shamir-client-ts (TS/WS SDK)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Task #921 (earlier session) added `SetReplicator` client support to
`shamir-client` (Rust core) and `shamir-client-node` (napi binding) to
fix `tests/e2e`'s replication test failures. The THIRD client,
`crates/shamir-client-ts` (the TS/WS SDK real applications use, not just
the native test harness), still has zero `setReplicator`/`canonicalSetReplicator`
support anywhere -- a pure-TS-client application cannot grant or revoke
the replicator role at all.

`DbRequest::SetReplicator { user, on, hmac }` / `DbResponse::ReplicatorSet
{ user, on }` (`crates/shamir-query-types/src/wire/db_message.rs`, task
#621) mirror `SetSuperuser`/`SuperuserSet`'s shape and gate EXACTLY
(unconditional hmac required, no "last remaining" guard). The canonical
HMAC input function already exists:
`crates/shamir-query-types/src/hmac.rs::canonical_set_replicator(user: &str,
on: bool) -> Vec<u8>` -- read it for the exact byte layout before writing
the TS mirror.

## The EXACT precedent to mirror -- read all of this before writing code

`setSuperuser` is implemented across three files in this exact shape;
`setReplicator` needs the identical three-file addition:

1. **`crates/shamir-client-ts/src/core/hmac.ts`** -- `canonicalSetSuperuser(user:
   string, on: boolean): Uint8Array` (around line 142). Add
   `canonicalSetReplicator(user: string, on: boolean): Uint8Array`
   immediately following the same pattern -- it MUST byte-match
   `canonical_set_replicator` in the Rust source exactly (same null-byte-join
   convention, same field order, same string encoding). Do not guess the
   byte layout; read the Rust function.

2. **`crates/shamir-client-ts/src/core/types/admin.ts`** -- `SetSuperuserOp`
   interface (around line 258), re-exported from `core/types/index.ts`
   (around line 122). Add a `SetReplicatorOp` interface with the same
   shape (`{ op: 'set_replicator', user: string, on: boolean, hmac: string }`
   -- confirm the exact field name is `op` not something else, matching
   `SetSuperuserOp`'s actual shape) and re-export it from `types/index.ts`.

3. **`crates/shamir-client-ts/src/core/builders/admin.ts`** -- `setSuperuser(signer:
   HmacSigner, user: string, on: boolean): SetSuperuserOp` (around line
   297). Add `setReplicator(signer: HmacSigner, user: string, on: boolean):
   SetReplicatorOp` immediately following the same pattern: compute the
   canonical bytes via `canonicalSetReplicator`, return `{ op: 'set_replicator',
   user, on, hmac: signer.hmacTagHex(canonical) }`.

4. **`crates/shamir-client-ts/src/core/client.ts`** -- `ShamirClient.setSuperuser`
   (around line 966), plus its result-type interface `SuperuserSet`
   (around line 72) and its builder import (around line 39,
   `import { setSuperuser } from './builders/admin.js';`). Add:
   - `import { setReplicator } from './builders/admin.js';` alongside the
     existing `setSuperuser` import.
   - `export interface ReplicatorSet { user: string; on: boolean; }`
     mirroring `SuperuserSet`'s shape, placed near it.
   - `async setReplicator(user: string, on: boolean): Promise<ReplicatorSet>`
     mirroring `setSuperuser`'s method body exactly: call
     `this.sendDbRequest(setReplicator(this, user, on))`, check
     `r.kind === 'replicator_set'` (confirm this is the correct
     `DbResponse` serde tag value -- `DbResponse` is `#[serde(tag = "kind",
     rename_all = "snake_case")]` per the Rust side, so `ReplicatorSet` ->
     `"replicator_set"`), return `{ user: r.user as string, on: r.on as
     boolean }`, else throw `Error(unexpected DbResponse kind for
     set_replicator: ${r.kind})`.
   - Add a doc comment mirroring `setSuperuser`'s (top-level `DbRequest`,
     not a `BatchOp`; requires an already-superuser session; unconditional
     HMAC gate).

## Also check

- Does `src/index.ts` (the package's public export surface) re-export
  `setSuperuser`/`SuperuserSet` at the top level, or only expose them via
  the `ShamirClient` class method? If the former, add the equivalent
  `setReplicator`/`ReplicatorSet` exports too for consistency. If the
  builder functions (`ddl`/`admin` namespaces) are part of the public
  export surface (they are, per `src/index.ts`'s `export * from
  './core/builders/index.js'`), confirm `setReplicator` is reachable the
  same way `setSuperuser`/`chmod`/etc. already are (check
  `core/builders/admin.ts`'s aggregate `admin` export object near the
  bottom of that file, if one exists, and add `setReplicator` to it if
  so).

## Tests

Find `setSuperuser`'s existing test coverage (likely
`crates/shamir-client-ts/src/core/builders/__tests__/admin.test.ts` and/or
`crates/shamir-client-ts/src/core/__tests__/client.test.ts` or similar --
search for `setSuperuser` across `__tests__/` directories) and add
equivalent tests for `setReplicator`: the builder produces the correct
wire shape + hmac tag (using a fake `HmacSigner` the way existing tests
do), and the canonical-bytes function's output for a few
(user, on) combinations.

## Verification

- `cd crates/shamir-client-ts && npm run typecheck` (tsc --noEmit) clean.
- `cd crates/shamir-client-ts && npm test` (vitest run) -- all existing
  tests still pass, new `setReplicator` tests pass.
- `npm run build` (tsc -p tsconfig.build.json) succeeds, producing an
  updated `dist/` with `setReplicator` in the type declarations.

## Definition of done

- `setReplicator` reachable from `ShamirClient` instances (`client.setReplicator(user,
  on)`) with the same ergonomics as `setSuperuser`.
- `canonicalSetReplicator` byte-matches the Rust `canonical_set_replicator`
  function exactly (state this explicitly in your report -- show the
  Rust function's exact byte layout and confirm the TS mirror produces
  the same bytes for a test case).
- Test coverage mirroring `setSuperuser`'s existing tests.
- `npm run typecheck` / `npm test` / `npm run build` all clean.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
