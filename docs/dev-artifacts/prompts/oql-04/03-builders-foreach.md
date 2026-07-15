# Epic04/Phase C — Rust + TS builders for `for_each` (#654)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

Epic04 ("Loops / data-dependent for-each") landed its engine primitive in
commit `6ff521d5` (Phase B, #653): `BatchOp::ForEach(ForEachOp { over: FilterValue,
bind_row: String, batch: BatchRequest })`. The ADR is
`docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`. `over` is
value-producing — it may be a `$query` ref, a `$fn` call, or a literal array —
and is resolved to a list EXACTLY ONCE before the loop starts; the loop body
(`batch`) is planned once and executed once per element, with the current
element bound to the parameter named `bind_row` (reference it inside the body
via `{"$param": bind_row}` / `val::param(bind_row)`).

Wire shape (msgpack, flat, matches `ForEachOp`'s `#[serde(rename = "for_each")]`
on its `batch` field):

```json
{
  "over": { "$query": "@orders", "path": "[].id" },
  "bind_row": "row",
  "for_each": { "id": 1, "queries": { /* BatchRequest */ } }
}
```

This phase is PURELY ergonomics — a fluent Rust builder method and a fluent
TS builder method, mirroring the existing `sub_batch`/`subBatch` pattern for
`BatchOp::Batch(SubBatchOp)`. No engine changes.

## Reference implementations to copy the shape of

- Rust: `crates/shamir-query-builder/src/batch/batch.rs` — see `sub_batch`
  (line ~647) and `sub_batch_no_bind` (line ~665), and the `when` method
  (line ~885) for how `Filter`-typed params are threaded through.
- Rust: `crates/shamir-query-builder/src/batch/tests/sub_batch_tests.rs` —
  test-file layout convention.
- TS: `crates/shamir-client-ts/src/core/builders/batch.ts` — see `subBatch`
  (line ~163).
- TS: `crates/shamir-client-ts/src/core/types/batch.ts` — see `SubBatchOp`
  interface (line ~44) and the `BatchOpInput` union (line ~52).
- TS: `crates/shamir-client-ts/src/core/builders/__tests__/batch.test.ts` —
  existing `subBatch` tests to model new `forEach` tests on.

## Task 1 — Rust builder (`crates/shamir-query-builder/src/batch/batch.rs`)

Add a `for_each` method on `Batch`, alongside `sub_batch`/`sub_batch_no_bind`
(same "── nested sub-batch ──" section, or a new "── for-each loop ──"
section immediately after it):

```rust
/// Register a data-dependent for-each loop (`BatchOp::ForEach`, Epic04).
///
/// `over` resolves to a list EXACTLY ONCE before the loop starts (it may be
/// a `$query` reference, an `$fn` call, or a literal array — anything
/// convertible to `FilterValue`). The loop body (`inner`) is planned once
/// and executed once per element, with the current element bound to the
/// parameter named `bind_row` — reference it inside `inner` via
/// [`crate::val::param`].
///
/// See `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md` for the
/// primitive's full semantics (result shape, `max_iterations` limit, error
/// abort behavior).
pub fn for_each(
    &mut self,
    alias: impl Into<String>,
    over: impl Into<FilterValue>,
    bind_row: impl Into<String>,
    inner: impl Into<BatchRequest>,
) -> Handle {
    let op = ForEachOp {
        over: over.into(),
        bind_row: bind_row.into(),
        batch: inner.into(),
    };
    self.add_entry(alias, BatchOp::ForEach(op), true)
}
```

Import `ForEachOp` from `shamir_query_types::batch` (match however
`SubBatchOp` is currently imported at the top of the file).

Do NOT add a `for_each_no_bind`-style variant — `bind_row` is mandatory
(there is always a bound element), unlike `sub_batch`'s optional `bind` map.

## Task 2 — Rust builder tests

New file `crates/shamir-query-builder/src/batch/tests/for_each_tests.rs`
(wire it into `crates/shamir-query-builder/src/batch/tests/mod.rs` per the
repo's `tests/mod.rs`-is-a-manifest convention). Cover, at minimum:

- `for_each` with a literal-array `over` (e.g. `vec![lit(1), lit(2)]`) and a
  simple inner batch — assert the built `BatchRequest`'s `queries[alias]`
  deserializes to `BatchOp::ForEach` with the right `over`/`bind_row`/`batch`.
- `for_each` with a `$query`-ref `over` (e.g. referencing an earlier
  `Handle`'s column via `.field(...)` or however the existing builder
  exposes column refs — check `sub_batch_tests.rs`/`val.rs` for the
  idiomatic way to build a `$query` ref `FilterValue` from a `Handle`).
- `for_each` with an `$fn`-call `over` — build a `FilterValue::FnCall` (check
  `shamir_query_builder::val` or similar for a helper; if none exists, build
  it directly).
- Confirm the produced wire bytes round-trip through
  `shamir_query_types::batch::BatchOp` as `ForEach`, not `Batch` (regression
  guard for the wire-key-collision bug fixed in #653).

## Task 3 — TS types (`crates/shamir-client-ts/src/core/types/batch.ts`)

Add a `ForEachOp` interface mirroring `SubBatchOp`'s pattern, right after it:

```ts
/**
 * A data-dependent for-each loop (Epic04, `{ "over": ..., "bind_row": ...,
 * "for_each": <BatchRequest> }`). Mirrors the server's `ForEachOp`.
 * `over` resolves to a list EXACTLY ONCE before the loop starts (it may be a
 * `$query` ref, an `$fn` call, or a literal array); the body is executed
 * once per element with the element bound to the parameter named
 * `bind_row`. The inner `BatchRequest` field is wire-keyed `for_each` (not
 * `batch`) to avoid colliding with `SubBatchOp`'s wire key.
 */
export interface ForEachOp {
  over: FilterValue;
  bind_row: string;
  for_each: BatchRequest;
}
```

Add `ForEachOp` to the `BatchOpInput` union (next to `SubBatchOp`).

## Task 4 — TS builder (`crates/shamir-client-ts/src/core/builders/batch.ts`)

Add a `forEach` method alongside `subBatch`:

```ts
/**
 * Add a data-dependent for-each loop under `alias`.
 *
 * `over` resolves to a list EXACTLY ONCE before the loop starts — it may be
 * a `$query` reference, an `$fn` call (`FilterValue` shape), or a literal
 * array. `inner` may be a `Batch` instance (`.build()` called automatically)
 * or a raw `BatchRequest`. The current element is bound to the parameter
 * named `bindRow` — reference it inside `inner` via `{ "$param": bindRow }`.
 *
 * `opts.returnResult` and `opts.after` behave identically to `.add()`.
 */
forEach(
  alias: string,
  over: FilterValue,
  bindRow: string,
  inner: Batch | BatchRequest,
  opts?: {
    returnResult?: boolean;
    after?: string[];
  },
): this {
  const resolved: BatchRequest =
    typeof (inner as Partial<Batch>).build === 'function'
      ? (inner as Batch).build()
      : (inner as BatchRequest);

  const entry: QueryEntry = {
    over,
    bind_row: bindRow,
    for_each: resolved,
  } as QueryEntry;

  if (opts?.returnResult === false) {
    entry.return_result = false;
  }

  if (opts?.after && opts.after.length > 0) {
    entry.after = opts.after;
  }

  this.queriesMap[alias] = entry;
  return this;
}
```

Adjust to match whatever helper/import style `subBatch` actually uses once
you read the real file (e.g. if there's a shared `buildEntry` helper, prefer
it over hand-rolling — but only if it doesn't lose the `for_each`-specific
wire shape).

## Task 5 — TS builder tests

Add `forEach`-focused tests to
`crates/shamir-client-ts/src/core/builders/__tests__/batch.test.ts` (or a new
`__tests__/for-each.test.ts` if the file convention there favors splitting —
check how `when`/`switchCase` tests are organized first). Cover:

- literal-array `over`
- `$query`-ref `over`
- `$fn`-call `over`
- wire round-trip: build → serialize → assert the JSON shape matches
  `{ over, bind_row, for_each }` at the top level of the entry (not nested
  under an extra `batch` key)

## Verification (MANDATORY before you report done)

- Rust: `./scripts/test.sh -p shamir-query-builder -p shamir-query-types -- for_each`
  must be green. Then run `cargo fmt -p shamir-query-builder -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` (full workspace —
  every prior phase this session broke some OTHER crate by growing a
  struct/enum without updating all call sites; check ALL of them, not just
  the crates you touched).
- TS: run the TS test suite for the touched files (check
  `crates/shamir-client-ts/package.json` for the test script — likely
  `npm test` or `npm run test` scoped to the builders directory; do NOT run
  the full e2e suite, that's Phase E's job).
- Report exactly what you ran and its output — do not claim success without
  showing the command and its result.

## Out of scope (do not touch)

- Anything under `crates/shamir-engine/` or `crates/shamir-query-types/src/batch/`
  besides re-exporting/importing `ForEachOp` if needed — the engine is done.
- E2E tests (Phase E, #656), benchmarks (Phase F, #657), docs (Phase G, #658).
- The deferred while-loop design (#659) — out of scope entirely for this repo
  right now.
