# Brief: CR-B6 — TS CAS types: `number | bigint` + safe-integer guard (#772)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run all commands (typecheck, test) in the
FOREGROUND. Do not background them.

This is a **TS-only** task — no Rust files, no server changes, no
blockers on other Wave A/B work (disjoint file set).

## Problem — declared type lies about what the wire can actually produce

`crates/shamir-client-ts/src/core/framing.ts`'s `decode()` (~line 66-71)
enables `useBigInt64: true`, so any wire integer in the u64/int64 range
decodes to a genuine JS `bigint`, not a `number` — this is deliberate
(the doc comment explains: without it, values `> 2^53` silently lose
precision). `encode()` additionally promotes any plain `number` outside
the i32/u32 range to `bigint` before handing it to the msgpack encoder
(`promoteWideInts`, ~line 34-48) — a `number` in that range would
otherwise encode as a msgpack float64 and the server would reject it.

Three public CAS-related surfaces currently declare a plain `number`,
which is provably NOT what the decoder can hand back once an MVCC version
counter exceeds `Number.MAX_SAFE_INTEGER` (or generally lands outside the
32-bit range the encoder leaves untouched):

1. `crates/shamir-client-ts/src/core/types/batch.ts:228` —
   `QueryResult.versions?: number[]`.
2. `crates/shamir-client-ts/src/core/types/write.ts:179` and `:206` —
   `UpdateOp.expected_version?: number` / `DeleteOp.expected_version?: number`.
3. `crates/shamir-client-ts/src/core/builders/write.ts:119` —
   `UpdateBuilder.expectedVersion(version: number): this` (and the same
   `number`-typed field on `UpdateBuilder.expectedVersionValue`, ~line 72,
   plus the functional `del()` builder's `opts.expectedVersion?: number`,
   ~line 187).

Once a version value decoded as `bigint` needs to flow back INTO one of
these (the whole point of a CAS: read a version, later pass it back as
`expected_version`), a caller is forced to narrow it via `Number(...)`
first to satisfy the declared type — which is exactly the silent
precision-loss bug this task exists to prevent. Compare with
`CursorId = number | bigint` (`types/cursor.ts:26`, FG-5a precedent) —
the SAME opaque-wire-integer problem, already solved correctly there.

## Fix

### 1. Widen the three type surfaces to `number | bigint`

- `types/batch.ts:228`: `versions?: (number | bigint)[]`. Update the
  surrounding doc comment (~lines 220-227) to note the same rationale
  `CursorId`'s doc comment already states — mirror its wording style
  rather than inventing new phrasing.
- `types/write.ts:179` and `:206`: `expected_version?: number | bigint`
  on both `UpdateOp` and `DeleteOp`.
- `builders/write.ts`:
  - `UpdateBuilder.expectedVersionValue: number | bigint | null` (~line 72).
  - `expectedVersion(version: number | bigint): this` (~line 119) — update
    its doc comment to state both forms are accepted and a `bigint`
    passes straight through unmodified (the encoder's `promoteWideInts`
    already emits a `bigint` as a genuine msgpack uint64/int64 — verify
    this yourself by re-reading `framing.ts` before writing the doc
    comment, don't just assert it).
  - The functional `del()`'s `opts.expectedVersion?: number` (~line 187) →
    `number | bigint`.

**Do NOT narrow a `bigint` value anywhere in this chain** — contrast with
`client.ts`'s epoch handling (~lines 595, 790, 796,
`wireEpochs[repo] = Number(e)`), which deliberately narrows because an
epoch is a small monotonic counter safe to represent as `number` in that
context. A CAS version is exactly the kind of value that must round-trip
EXACT — do not add a similar `Number(...)` narrowing call for versions
anywhere.

### 2. Guard the `number` path with `Number.isSafeInteger`

Wherever a caller-supplied `number` (not `bigint`) is accepted as a
version — `expectedVersion(version)`, the functional `del()`/`update()`
opts, and any other CAS-version entry point you find in the audit below —
validate `Number.isSafeInteger(version)` when `typeof version ===
'number'` (skip the check entirely for `bigint`, which has no such
precision concern) and `throw new TypeError(...)` with a clear message
explaining WHY (a version above `2^53` silently rounds under IEEE-754
double precision, which would corrupt CAS semantics — the caller almost
certainly meant to pass the `bigint` they got back from a decoded
response instead of narrowing it). Decide whether a small shared helper
(e.g. `assertSafeVersion(v: number | bigint, context: string): void` in
whatever this crate's existing shared-validation location is, or inline
at each of the ~2-3 call sites if a shared helper feels like
over-abstraction for this few call sites) — check for an existing
validation-helper module first; if none exists, a couple of inline guards
are fine, this codebase doesn't have a strong precedent either way here.

### 3. Audit for other version-touching TS surfaces

- `crates/shamir-client-ts/src/core/errors.ts`'s `isVersionConflict`
  (~line 46) — READ it: it only inspects `err.code === 'version_conflict'`,
  it does not compare or narrow any version VALUE. Confirm this yourself
  and, if so, no change is needed there — say so explicitly in your
  report rather than silently skipping it.
- Search the whole `crates/shamir-client-ts/src/` tree for any OTHER place
  that reads, compares, or forwards a version number (grep for
  `expected_version`, `expectedVersion`, `\.versions\b`, `version_conflict`
  outside the three sites already named and `errors.ts`). If you find a
  comparison like `a === b` or `a < b` between two version-typed values,
  note that `bigint !== number` even when numerically equal in JS (`5 ===
  5n` is `false`) — any such comparison needs explicit normalization
  (e.g. `BigInt(a) === BigInt(b)`, being careful that `BigInt()` on a
  non-safe-integer `number` is itself lossy-safe only for INTEGER values,
  which versions always are). If no such comparison exists anywhere in
  the SDK today, say so explicitly rather than inventing one to "fix."

## Tests (vitest — mirror existing builder/type test conventions)

Find the existing test file(s) for `UpdateBuilder`/`del`/CAS behavior
(likely under `crates/shamir-client-ts/src/core/builders/__tests__/` —
check `write.test.ts` or similar) and add:

- `expectedVersion` accepts a `bigint` and the built `UpdateOp.expected_version`
  is the EXACT same `bigint` value (no narrowing).
- `expectedVersion` accepts a safe `number` — unchanged behavior from
  before this task (regression guard).
- `expectedVersion` with an UNSAFE `number` (e.g.
  `Number.MAX_SAFE_INTEGER + 2`, or `2 ** 60`) throws a `TypeError` with a
  message mentioning safety/precision.
- Same three cases for the functional `del()`'s `opts.expectedVersion`
  (and `update()`'s functional form if one exists with the same option,
  check `write.ts`'s exports).
- A minimal decode-round-trip-style test (or, if a real decode fixture is
  impractical here, at minimum a type-level check) proving a
  `QueryResult.versions` array containing a `bigint` element compiles and
  is usable without a type error — the real gate for this one is `tsc
  --noEmit` passing on a small snippet, not necessarily a runtime
  assertion.

## Gate

```
npm --prefix crates/shamir-client-ts run typecheck
npm --prefix crates/shamir-client-ts run test
```

(Use whatever the actual package.json script names are if these exact
names don't exist — check `crates/shamir-client-ts/package.json`.)

Both must pass before returning. Stay inside `shamir-client-ts`'s
`src/core/types/batch.ts`, `src/core/types/write.ts`,
`src/core/builders/write.ts`, `src/core/errors.ts` (read-only audit, only
touch if you found a real issue), and their test files. Do NOT touch
`framing.ts`'s encode/decode logic itself — it already does the right
thing; this task only widens the TYPE surface and adds a validation
guard on top of it, it does not change wire behavior.
