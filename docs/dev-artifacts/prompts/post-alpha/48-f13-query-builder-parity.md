# Brief for #803 (F-13) — query-builder parity (Rust/TS): having/group_by, page validation, fallible build()

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

Low priority, does NOT block the release (F-14/F-15 are not blocked by
this task) — but keep the fix proportionate to that priority. Three
independent discrepancies, each scoped below with a design decision
already made (do not re-litigate the decisions; implement them).

## 1. `having()` without `group_by()` — add a fallible `Query::try_build()`, do NOT break `Query::build()`

**Current state**: `crates/shamir-query-builder/src/query/query.rs:123-128`'s
`having()` unconditionally stores the filter with no check against
`group_by_fields`; `build()` (line 328) then silently emits
`GroupBy { fields: vec![], having: Some(f) }` — a valid-shaped but
semantically empty-group query. The TS builder
(`crates/shamir-client-ts/src/core/builders/query.ts:391-394`) rejects
this in `build()`:
```ts
build(): ReadQuery {
  if (this.havingFilter !== null && this.groupFields === null) {
    throw new Error('having() requires groupBy()');
  }
```

**Why NOT to change `Query::build()`'s signature**: `grep -rn "\.build()"`
across the workspace shows roughly **822** matches touching query-shaped
`.build()` calls (mostly test call sites) — changing `Query::build(self)
-> ReadQuery` to `-> Result<ReadQuery, _>` would force updating that
entire surface for a task explicitly scoped as low-priority/cheap. That
is disproportionate.

**Design decision (already made — implement, don't re-derive)**: add a
NEW method `Query::try_build(self) -> Result<ReadQuery, QueryBuildError>`
that performs the having/group_by check (and the pagination check from
§2 below) and otherwise builds identically to `build()`. Leave the
existing `Query::build()` completely UNCHANGED (still infallible, still
the lenient/legacy path all 822 existing call sites keep using
unmodified). Document on both methods, in the doc comments, that
`try_build()` is the validating sibling and `build()` remains
permissive for backward compatibility — mirror the exact wording style
`crates/shamir-query-builder/src/batch/build_error.rs` / `Batch::try_build()`
already uses for this same infallible-vs-fallible split (that's the
established precedent in this crate — read it before writing the new
type).

**New error type**: `crates/shamir-query-builder/src/batch/build_error.rs::BuildError`
is batch-DAG-shaped (`UnknownAlias`/`SelfReference`/`AfterPathIgnored`)
— a different semantic family from query-shaped validation. Per this
workspace's "one file = one primary export" convention, add a NEW
sibling type instead of overloading `BuildError`: e.g.
`crates/shamir-query-builder/src/query/query_build_error.rs` with
`pub enum QueryBuildError { HavingWithoutGroupBy, InvalidPage { page: u64 }, InvalidPageSize { page_size: u64 } }`
(exact variant shape is yours to finalize; match this crate's existing
`Display`/`std::error::Error` manual-impl style from `build_error.rs`,
no `thiserror` — check whether `thiserror` is even a dependency of
`shamir-query-builder` before reaching for it; if it already is, using
it here is fine too, just be consistent with the existing sibling type's
style unless `thiserror` is clearly better and already used elsewhere in
this crate).

**TS side**: already correct (throws) — no TS change needed for this
sub-item specifically (its `build()` already rejects), EXCEPT the page
validation addition from §2 below, which extends the SAME existing
`throw` block.

## 2. `page=0` / `page_size=0` — reject in both builders' validation paths, do NOT touch the wire type

**Current state, confirmed at every layer**:
- Rust builder: `query.rs:181-187`, `page(page: u64, size: u64)` —
  assigns straight into `Pagination::Page { page, page_size: size }`, no
  validation.
- Wire type: `crates/shamir-query-types/src/read/limit.rs:26-32`,
  `Pagination::Page { page: u64, page_size: u64 }` — no constraint.
  `resolve()` (~line 176-186) does `page.saturating_sub(1) * page_size`,
  so `page=0` already behaves IDENTICALLY to `page=1` (silently
  "corrected", not an error) — `page_size=0` passes through literally as
  "0 rows per page", also unvalidated.
- TS builder: `crates/shamir-client-ts/src/core/builders/query.ts:293-298`,
  `page(page, pageSize)` — no checks; `buildPagination()` (~line 459-464)
  passes both straight to the wire object.

**Design decision**: validate ONLY at the builder layer (both languages),
NOT the wire type (`Pagination` in `shamir-query-types`) — the wire type
is deliberately permissive/defensive (its `resolve()` already tolerates
`page=0` via `saturating_sub`, which may be relied on by other direct
wire producers outside these two builders; changing its semantics is out
of proportion for a builder-parity task and risks affecting server-side
assumptions this task hasn't audited).

- **Rust**: add the `page==0 || page_size==0` check into the SAME
  `Query::try_build()` from §1 (bundle both checks into one fallible
  method — both are "reject before shipping an ill-formed query" checks
  belonging to the same validation pass), using the two `QueryBuildError`
  variants sketched above. Do NOT add the check to `Query::page()` itself
  (that would need `page()` to return `Result` too, forcing a builder-chain
  API change — keep the check centralized in `try_build()`, matching how
  `having`'s check is deferred to build-time in both languages already).
- **TS**: add the SAME check into the EXISTING `build()` throw block in
  `query.ts` (it already throws for having/groupBy right at the top of
  `build()` — add `if (this.paginationMode === 'page' && (this.pageNumber === 0 || this.pageSize === 0)) { throw new Error(...) }` alongside it, matching
  the existing error-message style in that file).

## 3. `expect()`/`Null`-placeholder builders → `Result`-returning `build()`

Five affected builders, confirmed exact sites — for these, UNLIKE
`Query` above, the blast radius is small enough that changing `build()`'s
signature directly (not adding a `try_build()` sibling) is proportionate.
Confirmed caller counts (files referencing each, rough upper bound):
Delete ~25, Update ~3, Upsert ~4, AddSchemaRuleBuilder ~13,
AlterSubscriptionBuilder ~12 — all manageable for a single task.

1. **`Delete::build()`** (`crates/shamir-query-builder/src/write/delete.rs:87-96`):
   currently `pub fn build(self) -> DeleteOp` with
   `self.where_clause.expect("Delete::build() requires a where clause...")`
   (documented `# Panics` at lines 83-86). Change to
   `pub fn build(self) -> Result<DeleteOp, BuilderError>`, replacing the
   `expect()` with a proper `Err` return.
2. **`Update::build()`** (`crates/shamir-query-builder/src/write/update.rs:97-105`):
   `set_value` defaults to `QueryValue::Null` (line 30/41) and is never
   validated — an `Update` with no `.set(...)` call silently ships
   `set: Null`. Change `build()` to
   `-> Result<UpdateOp, BuilderError>`, erroring when `set_value` is still
   the initial `QueryValue::Null` sentinel AND `.set()` was never called
   (track this with an explicit `bool`/`Option<QueryValue>` field instead
   of relying on `QueryValue::Null` as BOTH "unset" and "a legitimately
   set null value" — the current representation cannot distinguish
   "caller never called `.set()`" from "caller called `.set(QueryValue::Null)`
   on purpose"; fix the representation, not just the check, so a
   deliberate `.set(QueryValue::Null)` still builds successfully).
3. **`Upsert::build()`** (`crates/shamir-query-builder/src/write/upsert.rs:54-60`):
   same `QueryValue::Null`-sentinel ambiguity for BOTH `key` and `value`
   (lines 24-25/33-34) — same fix as Update: track "was `.key()`/`.value()`
   ever called" explicitly (e.g. `Option<QueryValue>` internally, or a
   pair of bools), change `build()` to
   `-> Result<SetOp, BuilderError>`, erroring if either was never set.
4. **`AddSchemaRuleBuilder::build()`** (`crates/shamir-query-builder/src/ddl/schema.rs:488-493`):
   `self.rule.expect("AddSchemaRuleBuilder: rule is required")` → change
   `build()` to `-> Result<BatchOp, BuilderError>`.
5. **`AlterSubscriptionBuilder::build()`** (`crates/shamir-query-builder/src/ddl/replication.rs:269-272`):
   `self.action.expect("alter_subscription().build() requires a terminal action...")`
   → change `build()` to `-> Result<BatchOp, BuilderError>`.

**New shared error type**: add `BuilderError` (name deliberately distinct
from `QueryBuildError` in §1 and the existing `batch::BuildError` — three
separate error families for three separate builder domains, matching
this crate's existing precedent of NOT overloading one enum across
unrelated builder kinds) in a new file, e.g.
`crates/shamir-query-builder/src/write/builder_error.rs`, with variants
covering all 5 sites: `MissingWhereClause` (Delete), `MissingSetValue`
(Update), `MissingKey`/`MissingValue` (Upsert — two variants, since
either can independently be missing), `MissingRule` (AddSchemaRuleBuilder),
`MissingAction` (AlterSubscriptionBuilder). Manual `Display` +
`std::error::Error` impl matching `build_error.rs`'s style (or
`thiserror` if already a dependency and used elsewhere in this crate —
check first, be consistent).

**Update every call site** across the workspace (production code AND
tests) that calls `.build()` on any of these 5 builders to handle the
new `Result` — `.build().unwrap()` is fine in tests, production code
paths should propagate via `?` or match explicitly. This is expected,
mechanical work given the confirmed caller counts above — do not treat
finding+fixing all of them as optional.

## Constraints

- Do NOT change `Query::build()`'s signature — only add the new
  `try_build()` sibling (§1/§2).
- Do NOT change `Pagination`'s wire-type semantics in
  `shamir-query-types` — validation lives in the builders only (§2).
- Do NOT touch `Insert::build()` — confirmed NOT to have this bug
  (empty `Vec` defaults are a legitimately-shaped, if pointless,
  `InsertOp`).
- Do NOT touch `crates/shamir-query-builder/src/batch/build_error.rs`
  itself (read it for style precedent, don't modify it) — new error
  types are new sibling files, not additions to that enum.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`, for the Rust side. For the TS side, use whatever test
  runner `crates/shamir-client-ts/`'s own `package.json` already wires up
  (check `crates/shamir-client-ts/src/core/builders/__tests__/` for the
  existing convention — likely vitest/jest; do not introduce a new test
  runner).
- `cargo fmt -p shamir-query-builder` and
  `cargo clippy -p shamir-query-builder --all-targets -- -D warnings`
  must be clean. Also run the SAME fmt/clippy against every OTHER crate
  in the workspace that calls the 5 changed `build()` methods (the
  compiler will tell you which via `cargo check --workspace` — fix every
  resulting call site, do not leave the workspace non-compiling).
- Follow workspace conventions: `use` at file top, one primary export
  per new file, surgical diff — do not refactor unrelated parts of any
  touched builder.

## Tests

For EACH of the three items, add or extend tests in this crate's
existing `tests/` convention (check
`crates/shamir-query-builder/src/query/tests/` and
`crates/shamir-query-builder/src/write/tests/` — or wherever this
crate's own test layout already lives; match it, don't invent a new
layout):

1. `Query::try_build()` returns `Err(QueryBuildError::HavingWithoutGroupBy)`
   for `.having(f)` with no `.group_by(...)`, and `Ok(_)` for the same
   query WITH `.group_by(...)` first. `Query::build()` (the OLD method)
   still returns the SAME lenient `ReadQuery` as before for the
   without-group_by case — a regression guard that `build()`'s existing
   behavior is untouched.
2. `Query::try_build()` returns `Err(QueryBuildError::InvalidPage{..})` /
   `InvalidPageSize{..}` for `page=0` / `page_size=0` respectively, and
   `Ok(_)` for `page=1, page_size>0`.
3. Each of the 5 converted builders: a regression test that omitting the
   required field returns the expected `Err(BuilderError::...)` variant
   (replacing whatever `#[should_panic]` test, if any, previously
   covered the `expect()` panic — check for and update/remove any such
   existing test), and a happy-path test that setting the field still
   builds `Ok(_)` correctly.
4. TS: extend `crates/shamir-client-ts/src/core/builders/__tests__/`'s
   existing query-builder test file with a case asserting `.build()`
   throws for `page(0, ...)` / `page(..., 0)`, matching the existing
   having/groupBy throw test's style.

## Verification the orchestrator will run

```
cargo fmt -p shamir-query-builder -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder
```
(the workspace-wide clippy check is deliberate here — this task's Rust
changes ripple call sites across other crates; the orchestrator will
confirm nothing else broke before accepting the diff. For the TS side,
whatever the existing `crates/shamir-client-ts` test command is, e.g.
`npm test` / `pnpm test` from that directory — check `package.json`.)
