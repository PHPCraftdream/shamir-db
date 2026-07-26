# Brief for F-26 (#819, P0) — `SelectItem::Expression` accepted but silently ignored

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context: a confirmed P0, silent-wrong-result — exactly the bug class Wave F exists to eliminate

Found by the deepest of three independent post-wave Wave F reviews
(`docs/dev-artifacts/research/2026-07-26-new-wave-release-review.md`, R5)
and personally confirmed by the orchestrator reading the code.

`SelectItem::Expression` (`crates/shamir-query-types/src/read/select.rs:113-118`,
doc comment literally says "Expression (future: computed fields)" — a
scaffolded-but-never-finished variant) is accepted at every layer of the
contract:

- Wire DTO: `crates/shamir-query-types/src/read/select_expr.rs:7` (`SelectExpr`).
- Parser: `crates/shamir-engine/src/query/read/parser.rs:190`.
- Public TS type: `crates/shamir-client-ts/src/core/types/query.ts:66`.

But the ONE production choke point every read plan shares —
`SelectProjection::new` (`crates/shamir-engine/src/query/read/select_projection.rs:75-108`)
— only builds output for `SelectItem::Field` and `SelectItem::Function`; its
`match item { ... }` has a bare `_ => {}` catch-all that `Expression` (and
anything else non-Field/non-Function) falls into, silently. The aggregate
path (`crates/shamir-engine/src/query/read/aggregate.rs:1001`) does the
same. A syntactically valid query with a computed field in its `SELECT`
returns a result set with that field simply **absent** — no error, no
warning, just missing data. Confirmed directly by reading
`select_projection.rs:87-108`'s match arms.

`SelectProjection::new` is confirmed (via
`grep -rn "SelectProjection::new" crates/shamir-engine/src/`) to be the
SINGLE production call site every read execution path funnels through —
`crates/shamir-engine/src/table/read_exec.rs` alone has 8 call sites
covering full scan, index2, temporal, and cursor paths, all calling
`exec::SelectProjection::new(&query.select, interner, ...)`. This makes it
the correct single choke point to reject at, rather than patching each
read-plan file individually or only the wire parser (which wouldn't catch
a `Select` constructed directly by Rust code that bypasses the wire).

## Design

**Minimal safe fix for this release: reject `SelectItem::Expression` with a
typed error at `SelectProjection` construction time. Do not implement a
computed-expression evaluator in this task** — that's a real feature
(needs an evaluator, builder support in both Rust and TS, and its own test
suite) explicitly out of scope here; this task's job is closing the silent
wrong-result hole, not shipping expression evaluation.

1. **Make `SelectProjection::new` fallible.** Change its signature from
   `pub fn new(select: &Select, interner: &Interner, scalars: ScalarResolver) -> Self`
   to return a `Result<Self, SelectProjectionError>` (or reuse/extend an
   existing error type if one already fits this crate's conventions — check
   `crates/shamir-engine/src/query/read/`'s existing error types before
   inventing a new one; if a new enum is warranted, follow this workspace's
   `thiserror` convention for library error enums). At construction time,
   if `select.items` contains ANY `SelectItem::Expression`, return an error
   (e.g. `SelectProjectionError::ExpressionNotSupported` or similar,
   surfaced to the wire/caller as a typed code like
   `select_expression_not_supported` — check how other typed rejections in
   this crate/`shamir-db` surface their error codes to callers, e.g.
   `err_code("...", ...)` in `admin_schema.rs`, and follow the SAME pattern
   at whatever layer ultimately turns this into a wire response).
2. **Propagate `Result` through every call site.** `SelectProjection::new`'s
   callers already return fallible types in every production path (the
   surrounding functions in `read_exec.rs`/`query/read/exec.rs` are already
   `Result`-returning or easily made so where they aren't — check each of
   the ~8 call sites in `crates/shamir-engine/src/table/read_exec.rs` plus
   `crates/shamir-engine/src/query/read/exec.rs:38`'s `apply_select_value`
   and confirm/adjust each caller's own signature). This is a mechanical,
   bounded propagation similar in shape to F-13's `Query::try_build`
   rollout — follow the same "thread `?` upward, update test call sites"
   pattern. Test-only call sites (`crates/shamir-engine/src/query/read/tests/select_projection_tests.rs`,
   `crates/shamir-engine/src/query/filter/tests/field_path_cache_tests.rs`,
   `query_ref_cache_tests.rs`, `crates/shamir-engine/src/table/tests/recordview_cutover_parity_tests.rs`,
   `s3_bytes_path_parity_tests.rs`) will need `.unwrap()`/`.expect(...)`
   added since none of them exercise `SelectItem::Expression` today.
3. **Aggregate path.** `aggregate.rs:1001`'s expression-ignoring branch
   needs the SAME rejection — investigate whether it's reachable via a
   DIFFERENT path than `SelectProjection::new` (aggregate queries may
   validate their `SELECT` items separately from the plain projection path)
   and add the equivalent reject there too if it's a genuinely separate
   code path, rather than assuming the `SelectProjection::new` fix alone
   covers it.
4. **TS public type.** Decide, while writing this brief's implementation,
   whether to keep `SelectItem`'s `Expression`/`expr` union arm in
   `crates/shamir-client-ts/src/core/types/query.ts:66` as-is (since the
   SERVER now rejects it with a clear typed error, a TS caller who
   constructs one will get an explicit rejection, not silent data loss —
   arguably acceptable for a "not yet implemented" arm) OR mark it
   `@deprecated`/add a doc comment noting it is accepted by the type system
   but currently always rejected by the server. Do NOT remove the TS type
   entirely (that would be a larger, unrelated breaking change to the
   public type surface) — a doc/comment update is sufficient for this task.

## Tests

1. **Full scan, index2, temporal, and cursor read plans each reject a
   query containing `SelectItem::Expression`** with the SAME typed error
   code — one test per plan type (find the existing per-plan-type test
   files in `crates/shamir-engine/src/table/tests/` and
   `crates/shamir-server/src/db_handler/tests/` and add one focused test
   to each, rather than only testing the shared `SelectProjection`
   constructor in isolation — the whole point is confirming EVERY
   production entry point actually rejects, not just the shared helper).
2. **Aggregate path also rejects** an `Expression` item mixed into an
   aggregate query's `SELECT` (covers the `aggregate.rs:1001` site).
3. **Regression: `Field`/`Function`/`Aggregate`/`AggregateFn`/`CountAll`
   items are unaffected** — run the existing `select_projection_tests.rs`
   suite (with `.unwrap()` added per the signature change) and confirm
   every existing case still passes unchanged in behavior.
4. Add a doc-comment update to the rejected variant in `select.rs` (no
   longer "future: computed fields" with no caveat — note it is currently
   REJECTED at execution time, not silently accepted) and update
   `docs/guide-docs/KNOWN_LIMITATIONS.md` with a new bullet describing this
   as a closed silent-wrong-result gap (cite file:line for the reject
   site).

## Constraints

- Do NOT implement expression evaluation — reject-with-typed-error only.
- Do NOT remove `SelectItem::Expression`/`SelectExpr`/the TS type entirely
  — keep the wire shape intact for a future real implementation, only add
  the rejection at execution time.
- Keep the diff scoped to the reject path + its call-site propagation +
  tests + the two doc updates named above — no incidental refactors of
  `select_projection.rs`/`read_exec.rs` beyond what threading `Result`
  requires.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`. If the TS type comment is touched, no TS test changes
  should be needed (a doc-comment-only edit), but confirm
  `npx vitest run`/typecheck still passes if you touch `query.ts` at all.
- `cargo fmt -p shamir-engine -p shamir-server -p shamir-query-types --
  --check` and `cargo clippy` on the same crates with `--all-targets -- -D
  warnings` must be clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-server -p shamir-query-types -- --check
cargo clippy -p shamir-engine -p shamir-server -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- select_projection
./scripts/test.sh -p shamir-engine -- aggregate
./scripts/test.sh @engine
./scripts/test.sh -p shamir-server --full
```
