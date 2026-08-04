# Brief — e2e gap cluster G (low priority): misc OQL gaps

Task: #980 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md`, "Cluster G — Misc low-priority OQL gaps". A grab-bag of 7 small, independent gaps — bundle into one session, one file per logical group is fine.

## ⚠️ Correction to the source matrix — read before starting item 4

The matrix says `distinct:true` on an `Aggregate` SelectItem is untested,
phrased like `SUM(DISTINCT x)` is a working feature waiting to be tested.
**It is NOT** — verified against
`crates/shamir-engine/src/query/read/aggregate.rs::validate_aggregate_select`
(~line 826-869): `Aggregate { distinct: true, func }` is **REJECTED** with
`distinct_not_supported_for_fast_path_agg` for `Sum`/`Avg`/`Count`. Only
`Min`/`Max` with `distinct: true` are accepted (silently a correct no-op,
since distinct doesn't change a min/max). There is no `SUM(DISTINCT x)`
today. Item 4 below tests the REAL behavior: the rejection AND the allowed
no-op, not a working feature that doesn't exist.

## The 7 gaps

1. **`FieldEq` (`op:"field"`)** — `crates/shamir-client-ts/src/core/builders/filter.ts`
   ~line 117, `filter.fieldEq(field, value)`. Wire-identical to `Eq` but a
   distinct enum variant (`Filter::FieldEq`, `filter_enum.rs` ~line 131).
   Test: use `filter.fieldEq(...)` in a live query, assert it matches
   exactly like the equivalent `filter.eq(...)` would.

2. **`$expr`/`$fn` as a filter VALUE** (not as a top-level op — as the
   right-hand side of a comparison). `crates/shamir-query-types/src/filter/filter_value.rs`
   ~line 32-41 (`FilterValue::FnCall`/`FilterValue::Expr`).
   `crates/shamir-client-ts/src/core/builders/filter.ts` exports `fn(name, args?)`
   (~line 405) and `expr(op, args)` (~line 417) — these build `FilterValue`s,
   pass them as the `value` arg to `filter.eq(field, fn(...))` /
   `filter.eq(field, expr(...))`. Test: e.g. `filter.eq('computed_field',
   expr('add', [literal1, literal2]))` or similar — pick a real `ExprOp`/
   funclib scalar name and prove the comparison evaluates correctly against
   seeded rows (some matching, some not). This is NOT the same as
   `SelectItem::Function` (projection-side) — this is specifically `$fn`/
   `$expr` used as a comparison VALUE inside WHERE.

3. **Binary literal round-trip.** `FilterValue::Binary`/msgpack `bin` values
   in insert + filter. `crates/shamir-client-ts/src/core/framing.ts` ~line 41
   confirms the msgpack layer passes `Uint8Array` straight through as
   binary — the `WireValue` TS type just doesn't declare it (a type
   completeness gap, not a functional one; use `as unknown as WireValue` or
   similar cast if TypeScript complains, don't "fix" the type as part of
   this task). Test: `write.insert(table, { blob: someUint8Array })`, then
   `filter.eq('blob', anotherUint8Array)` — round-trip a small binary value
   through insert and match it back via filter equality.

4. **`distinct` on `Aggregate`/`AggregateFn` SelectItems** — see correction
   above. `crates/shamir-client-ts/src/core/types/query.ts` ~line 46-64 is
   the exact typed `SelectItem` shape the query builder's `.select(items)`
   accepts (`{ type: 'aggregate', func, field, distinct }` /
   `{ type: 'aggregate_fn', name, field, args, distinct }`) — this typed
   shape passed to `.select([...])` is the builder-sanctioned way to build
   an aggregate select item (not hand-assembled raw JSON). Tests:
   (a) `{ type: 'aggregate', func: 'min', field: [...], distinct: true }`
   is ACCEPTED and returns the correct min (no-op proof); (b)
   `{ type: 'aggregate', func: 'sum', field: [...], distinct: true }` is
   REJECTED with an error containing `distinct_not_supported_for_fast_path_agg`.

5. **Funclib `AggregateFn` breadth** beyond `count_distinct` (which is
   already covered elsewhere). `crates/shamir-funclib/src/agg.rs` ~line
   98-121 registers `median`, `stddev`, `percentile` (default p=0.5, or a
   caller-supplied `p` via `args: [0.9]` etc.), `string_agg` (default sep
   `","`, or caller-supplied via `args: [";"]`). Validation source:
   `crates/shamir-engine/src/query/read/aggregate.rs::validate_aggregate_select`
   ~line 826-860 (only `percentile` and `string_agg` accept `args`; `p` must
   be in `[0.0, 1.0]`). Tests: seed numeric rows, assert `median`/`stddev`
   (no args) produce correct values; `percentile` with an explicit `args:
   [0.9]` produces a DIFFERENT result than the p=0.5 default (proves args
   are actually honored, not ignored); `string_agg` with an explicit
   separator arg produces the expected joined string.

6. **`History` temporal `from`/`to` window bounds.** `order`/`limit` are
   already covered live (`crates/shamir-client-ts/src/__tests__/e2e-data.test.ts`
   ~line 968-1025, `.history({ order, limit })`) — extend that SAME setup
   (reuse `histDb`'s multi-version record, don't duplicate the setup), add
   tests for `.history({ from, to })` (check the `.history(opts)` builder
   signature at `crates/shamir-client-ts/src/core/builders/query.ts` ~line
   357-369, `At` is `{Version(u64)}` or `{Timestamp(u64)}` — check the TS
   `At`-building helpers, e.g. `atVersion`/`atTimestamp`, used elsewhere in
   that file). Prove `from`/`to` actually bound the returned version range
   (a version outside the window is excluded; one inside is included).

7. **Batch `durability` (`synced`/`async_index`) live effect.**
   `crates/shamir-client-ts/src/core/builders/batch.ts` ~line 278,
   `Batch.durability(level)`. `crates/shamir-query-types/src/batch/batch_request.rs`
   ~line 67-81 documents the 3 levels (`buffered` default, `synced`,
   `async_index` — the latter "only meaningful for `transactional: true`
   batches"). This is a durability/timing guarantee, not something easily
   observable via query results alone — the honest, achievable e2e proof is
   narrower than "prove crash durability": (a) a `.durability('synced')`
   transactional batch completes successfully and its writes are
   immediately visible on read (functional smoke test); (b) same for
   `.durability('async_index')`. Do NOT attempt to simulate an actual crash
   to prove durability semantics — that's out of scope for an e2e client
   test; just prove the option round-trips and doesn't break the write.

## Required work

Pick file homes matching existing conventions in
`crates/shamir-client-ts/src/__tests__/` — likely extending `e2e-data.test.ts`
(already has a lot of filter/select/history coverage, see items 1-6) for
items 1-6, and possibly `e2e.test.ts` or `e2e-batch-sequencing.test.ts` for
item 7 (durability) — check which file already covers batch options before
picking. Your call on exact placement; check file sizes/conventions first.

Use ONLY query/filter/batch builders (`filter.fieldEq`, `filter.fn`,
`filter.expr`, `.select([...])` with the typed `SelectItem` shape,
`.history({...})`, `Batch.durability(...)`) — no hand-assembled wire
objects (repo-wide CLAUDE.md rule). The typed `SelectItem` object literals
passed to `.select([...])` ARE the builder-sanctioned form (see item 4) —
not a violation of the rule.

## Verification

- Run the full vitest suite in `crates/shamir-client-ts` (`npx vitest run`)
  — baseline after #979 is 57 files / 1037 tests passed. Report exact
  counts before and after.
- `npx tsc --noEmit` in that package — must stay clean.
- If you touch the JS suite, also run `cd tests/e2e && node e2e.test.js`
  (baseline: 19 files / 147 passed) and report counts.

## Scope discipline

- Do NOT touch cluster H (#981) — separate task, DDL-focused.
- Do NOT modify production Rust or the query/filter/batch builders. If any
  of the 7 items behaves differently than documented above (especially
  item 4's rejection behavior, or item 7's durability semantics), STOP and
  report it as a real bug/doc-mismatch instead of silently adjusting the
  test.
- Item 3's `WireValue` type gap (no `Uint8Array` variant) is a KNOWN,
  accepted minor gap — work around it with a cast, do not "fix" the type
  as part of this task (that would be unrelated scope creep).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create test files and run read-only/test
commands.

## What to report back

List every test added (all 7 items should each have at least one test) and
what it proves. For item 4, explicitly confirm you tested the REJECTION
path (not a nonexistent working `SUM(DISTINCT x)`). For item 5, state the
exact numeric values proving `args` are honored. Give exact test-run output
with real pass/fail counts.
