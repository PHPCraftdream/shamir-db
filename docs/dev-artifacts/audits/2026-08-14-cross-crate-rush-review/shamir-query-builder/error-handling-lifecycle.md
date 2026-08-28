# shamir-query-builder -- Error handling & resource lifecycle

## Summary

This crate is a strong performer on its own documented error-handling ideology: four typed error families (`BuildError`, `BuilderError`, `QueryBuildError`, `CreateIndexBuildError`) plus `ResponseError`, an infallible-legacy / fallible-`try_build` split with per-variant rationale, and near-exhaustive error-path test coverage (every `BuilderError`/`QueryBuildError`/`ResponseError`/`CreateIndexBuildError` variant is asserted in `src/*/tests/`). Resource lifecycle is trivially clean: the crate is a pure, synchronous, in-memory builder -- no locks, files, tasks, or `Drop` glue exist, so there is nothing to leak or clean up on an error path (`try_build`'s `?`-early-returns hold no resources). The residual findings are two panic sites on public APIs that contradict the crate's own #1083 rationale ("a malformed batch produces a `Result::Err`, not a panic" -- `build_error.rs:33-35`), one silent-swallow misuse path (`Batch::after`/`when`) that `try_build` structurally cannot see, and convention drift (thiserror rule in CLAUDE.md; incomplete roll-out of the fallible-build pattern). Note the crate targets WASM (`lib.rs:8-9`), where a residual panic traps the whole guest instance, which raises the cost of every remaining `expect`.

## Findings

### 1. `Batch::to_request_via_msgpack` panics on codec error -- public API, contradicts the crate's own stated ideology

- File:line: `crates/shamir-query-builder/src/batch/batch.rs:878-881` (doc at 872-877; `to_msgpack` at 868-870)
- Severity: medium
- Issue: `to_request_via_msgpack` is a public, non-test method that does `self.to_msgpack().expect("msgpack encode")` then `rmp_serde::from_slice(&bytes).expect("msgpack decode")`. Its doc asserts "the builder always produces a serialisable request" -- but the crate's own `BuildError::SerializationFailed` doc (`build_error.rs:29-42`) says an entry can hold "a value msgpack cannot represent", which is exactly why `try_build` was converted from `.expect` to a typed error in #1083. The two docs cannot both be right: if encoding is truly infallible, `to_msgpack` should not return `Result`; if it is not, this method converts a `Result` into a panic for no gain. `shamir-types` tests indicate rmp-serde currently encodes non-finite f64 as-is, so the panic is likely unreachable today -- but that is precisely the "looked safe to the original author" reasoning the #1083 test comment (`batch_tests.rs:417-428`) dismantles. On the WASM target a panic traps the guest, not just a task.
- Failure scenario: a future `QueryValue`/`BatchRequest` field (or a new `BatchOp` payload) makes `rmp_serde::to_vec_named` fail for some client-constructed value (`Batch::id(impl Into<QueryValue>)`, `Doc::set_value`, `Insert::row` all accept arbitrary client `QueryValue`s); the caller's `let req = b.to_request_via_msgpack()` panics at the validation call site instead of producing an `Err`.
- Suggested fix: add `try_to_request_via_msgpack(&self) -> Result<BatchRequest, rmp_serde::encode::Error>` (mapping the decode error into `Error::Syntax`, exactly as `wire/mod.rs:31-38` already does for `to_query_value`) and either move the panicking convenience behind `#[cfg(test)]`/`doc(hidden)` or keep it as a thin documented wrapper over the `try_` form. If the panicking variant stays public, reconcile its doc with `BuildError::SerializationFailed`'s.

### 2. `Batch::after` / `Batch::when` silently no-op on an unknown alias -- a misuse path `try_build` cannot catch

- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1003-1008` (`after`), `batch.rs:1025-1030` (`when`)
- Severity: medium
- Issue: both post-hoc methods look up the dependent entry with `if let Some(entry) = self.queries.get_mut(...)` and do nothing when the alias is absent. The crate's discipline everywhere else is to surface programmer misuse as a typed error (`ConflictingBuilderState`, `MissingWhereClause`, `AfterPathIgnored`); here a wrong `Handle` is silently swallowed. The doc for `after` (batch.rs:993-998) already concedes the two-`&Handle` transposition footgun, but transposition is not even the worst case: a `Handle` cloned from a *different* `Batch` (two batches in scope), or a `Handle` whose alias was never registered, drops the ordering edge silently. Crucially, `try_build` validates the `after` list *as recorded* -- since the edge was never recorded, its `UnknownAlias` check (`batch.rs:934-957`) has nothing to fire on. No test exercises the unknown-alias no-op either.
- Failure scenario: `b2.after(&handle_from_b1, &mk)` (or a transposed pair that happens to name an unknown alias) -- the DDL-before-DML ordering edge silently disappears, the server executes the insert before the table exists, and the failure surfaces as a remote batch error far from the bug site, with the local `try_build()` reporting `Ok`.
- Suggested fix: at minimum, make the misuse loud in debug builds: `debug_assert!(self.queries.contains_key(dependent.alias()), "Batch::after: unknown dependent alias")` (same for `when`), plus a doc line stating the no-op semantics for release builds. Better: return `Result<&mut Self, BuildError>` (or a small dedicated variant) so the fluent chain can `?` it; the internal `switch()` callers (`batch.rs:1071`, `1079`) always pass freshly-registered handles, so the churn is contained.

### 3. Fallible-build pattern not applied to builders with self-documented required fields (`CreateFunction`, `CreateValidator`, `BindValidator` priority)

- File:line: `crates/shamir-query-builder/src/ddl/function.rs:95-107` (build; contract doc at 86-92), `crates/shamir-query-builder/src/ddl/validator.rs:48-55` (CreateValidator::build), `crates/shamir-query-builder/src/ddl/validator.rs:162-166` (`priority`: "must be in `[1000, 9999]`")
- Severity: low
- Issue: the crate established the principle that a builder whose DTO requires a field must reject its absence at construction time ("so a caller finds out at *construction* time, not after a full round trip through the server" -- `create_index_build_error.rs:8-12`; `builder_error.rs:1-12`). Three builders violate it with contracts documented in prose but unenforced in code: `CreateFunction` (`hmac` "Required IFF `security == \"definer\"` or `secret_grants` is non-empty" -- unchecked; neither `source` nor `wasm` set builds fine), `CreateValidator` (same source/wasm gap), and `BindValidator::priority` (range documented, not checked; stored in `u16` so out-of-range values pass through silently). `FieldBuilder::build` (`schema.rs:377-386`) likewise accepts an empty `ty` type tag.
- Failure scenario: `create_function("f").security("definer").build()` (no `.hmac(...)`) compiles and flows to the server, which rejects it at DDL-execution time -- the exact late-failure round trip `CreateIndexBuildError` was created to eliminate.
- Suggested fix: extend `BuilderError` with the missing variants (`MissingImplementation`, `HmacRequired`, `PriorityOutOfRange`, `MissingFieldType`) and convert these `build()`s to `Result` like their `write/` siblings, adding `TryIntoBatchOp` impls + `Batch::try_op` coverage (the plumbing already exists).

### 4. Five error enums hand-roll `Display` + `std::error::Error` despite the workspace thiserror rule

- File:line: `crates/shamir-query-builder/src/batch/build_error.rs:45-75`, `crates/shamir-query-builder/src/write/builder_error.rs:45-79`, `crates/shamir-query-builder/src/query/query_build_error.rs:40-68`, `crates/shamir-query-builder/src/ddl/create_index_build_error.rs:131-240`, `crates/shamir-query-builder/src/response/batch_response_ext.rs:39-70`
- Severity: low
- Issue: CLAUDE.md's error-handling section (lines 637-645) is normative: "`thiserror` for library error enums (with `#[from]` where natural)". Thirteen sibling crates depend on `thiserror = "2.0"`; this crate hand-implements `Display`/`Error` for all five enums (~250 lines of boilerplate, and new variants must remember both the match arm and the Display arm). `ResponseError::Deserialize { source, .. }` even hand-writes `source()` (`batch_response_ext.rs:63-70`) -- the textbook `#[source]` case. The impls are correct today (all variants covered; no drift found), so this is convention drift and maintenance cost, not a defect.
- Failure scenario: a future variant added to one enum without its `Display` arm is a compile error (exhaustive match) -- so the real cost is only boilerplate and review surface, which is why this is low.
- Suggested fix: a standalone `chore` task (per the "pre-existing lints / style sweeps get their own commit" rule) migrating the five enums to `thiserror::Error` with `#[error("...")]` attributes; no behavior change is expected and the `PartialEq`/`Clone` derives are unaffected.

### 5. `Doc::set` `.expect()`s the `FilterValue` -> `QueryValue` msgpack round-trip in a public setter

- File:line: `crates/shamir-query-builder/src/write/doc.rs:47-50`
- Severity: low
- Issue: two `.expect("... is infallible")` calls convert a hypothetical codec failure into a panic in the crate's most-used value builder. The invariant genuinely holds today (msgpack encodes every `FilterValue` shape, including non-finite f64, and `QueryValue` decodes any valid msgpack), and the comment says so -- but this is the exact same "no way to construct a failing value today" assumption that #1083 (`batch_tests.rs:417-428`) shows does not age: one new `FilterValue` variant with a non-round-trippable payload turns every `doc().set(...)` into a WASM-trapping panic.
- Failure scenario: `FilterValue` gains a variant whose serde shape `QueryValue` cannot decode (or a codec regression on a float edge); `doc().set("k", v)` panics inside a client builder instead of surfacing an error.
- Suggested fix: keep the fast path but route through `ToWire::to_query_value`'s precedent: map the encode error into a `QueryValue`-decode `Error::Syntax` and either make `Doc::set` return `Result<Self, rmp_serde::encode::Error>` (matching the `Update`/`Upsert`/`Delete` fallible-build precedent, with `doc!` macro `.expect`-ing on top so ergonomics are unchanged) or leave it panicking with an explicit `debug_assert` + doc-note referencing the #1083 rationale.

### 6. Guarded `unwrap()`/`expect()` cluster in `TryFrom<&CreateIndex>` is sound but could be total

- File:line: `crates/shamir-query-builder/src/ddl/create_index.rs:772, 778, 815` (`itype.unwrap()`), `create_index.rs:838-839` (double `.expect("vector_dim checked Some & > 0")`), `create_index.rs:862` (`.expect("sorted index checked to have exactly one field")`)
- Severity: nit
- Issue: all five sites are guarded invariants (`non_btree` = `matches!(itype, Some(...))` for the unwraps; check 4 at line 782 for the dimension; check at line 826 for the single field), so per CLAUDE.md ("avoid `panic!` outside ... invariant violations that mean a programmer bug") they are sanctioned, and the comments name the checks that establish them. Two cosmetic observations: line 838-839 duplicates the same expect message around `NonZeroU32::new(...)` when a single `.expect` on the constructor result suffices, and the three `itype.unwrap()` sites could bind `let t = itype.unwrap_or_default()`-style locals (or `if let Some(t) = itype`) to remove the operator entirely. No behavior change implied.
- Failure scenario: none under current code; the guards and construction sites are additionally pinned by `index_spec_tests.rs` and the `create_index_matrix.json` fixture.
- Suggested fix: optional tidy-up only -- collapse the double expect, and consider a small `fn expect_itype(...) -> &str` helper (or `IndexSpec`-carrying type enum) so the invariant lives in one place rather than five comments.

## Test-coverage note (error paths)

Coverage of the error paths that exist is genuinely strong, judged against the suite in `src/*/tests/`:

- `BuilderError` -- all six variants asserted (`try_into_batch_op_tests.rs:35-150`, `write_tests.rs:403-430`, `schema_ddl_tests.rs:553`, `replication_ddl_tests.rs:270`), each with a happy-path sibling proving `try_*` and non-`try` produce identical wire shapes.
- `QueryBuildError` -- all three variants asserted (`query_tests.rs:1068-1102`), including the negative check that legacy `build()` deliberately does *not* surface `HavingWithoutGroupBy` (`query_tests.rs:1085-1096`).
- `ResponseError` -- all three variants asserted, including `Deserialize` via a genuinely malformed record (`response_tests.rs:143, 160, 199-208`) and `Error::source` propagation.
- `CreateIndexBuildError` -- matrix-tested against the committed `create_index_matrix.json` fixture plus per-variant asserts (`index_spec_tests.rs:174-324`).
- `BuildError` -- `UnknownAlias`/`SelfReference`/`AfterPathIgnored` triggered end-to-end; `SerializationFailed` is honestly documented as untriggerable through valid builder inputs, so only its `Display` is tested (`batch_tests.rs:414-440`) -- an acceptable, explicit gap.

The only error-path behavior with zero coverage is the silent no-op of finding 2, which is itself the finding. No resource-cleanup tests are needed: the crate holds no resources across any fallible boundary.
