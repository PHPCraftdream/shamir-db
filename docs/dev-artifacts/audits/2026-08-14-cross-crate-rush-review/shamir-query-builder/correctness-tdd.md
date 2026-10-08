<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-builder — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Nested-scope and marker validation defects remain open, as do silent alias-state loss and several validation gaps. The zero-case switch concern is a coverage gap, not an ambiguous current truth value. Public codec-panic reachability is unverified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 8 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Parent dependency inspection supplies the previously missing deep-owned-value / decode-budget counterexample; no runtime test was executed.

<a id="review-1"></a>

### Claim 1 — `Batch::try_build` falsely rejects `sub_batch`/`for_each` entries whose inner batch has internal `$query` dependencies

Status: `confirmed-open`. Current risk: `high`.

Batch and ForEach still enter the whole-op msgpack fallback, so inner references are checked against outer aliases. The planner collects only bind/over dependencies and the executor plans inner bodies separately. Registered nested-builder tests do not call try_build.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:912](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L912); [crates/shamir-query-builder/src/batch/batch.rs:1313](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1313); [crates/shamir-query-types/src/batch/planner.rs:308](../../../../../crates/shamir-query-types/src/batch/planner.rs#L308); [crates/shamir-engine/src/query/batch/query_runner.rs:680](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L680); [crates/shamir-engine/src/query/batch/query_runner.rs:880](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L880); [crates/shamir-query-builder/src/batch/tests/mod.rs:5](../../../../../crates/shamir-query-builder/src/batch/tests/mod.rs#L5); [crates/shamir-query-builder/src/batch/tests/sub_batch_tests.rs:17](../../../../../crates/shamir-query-builder/src/batch/tests/sub_batch_tests.rs#L17); [crates/shamir-query-builder/src/batch/tests/for_each_tests.rs:18](../../../../../crates/shamir-query-builder/src/batch/tests/for_each_tests.rs#L18).

<a id="review-2"></a>

### Claim 2 — `Batch::after` and `Batch::when` silently no-op when the handle's alias is not registered in this batch

Status: `confirmed-open`. Current risk: `medium`.

Both missing-target lookups still return success without recording anything; try_build cannot recover discarded intent. Alias replacement also resets guards/edges. A foreign handle with an existing local alias instead mutates that local entry, because handles have no ownership identity. Existing tests exercise recorded invalid references, not lookup misses or replacement.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1003](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1003); [crates/shamir-query-builder/src/batch/batch.rs:1025](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1025); [crates/shamir-query-builder/src/batch/batch.rs:1096](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1096); [crates/shamir-query-builder/src/batch/handle.rs:11](../../../../../crates/shamir-query-builder/src/batch/handle.rs#L11); [crates/shamir-query-builder/src/batch/tests/after_tests.rs:64](../../../../../crates/shamir-query-builder/src/batch/tests/after_tests.rs#L64); [crates/shamir-query-builder/src/batch/tests/when_tests.rs:81](../../../../../crates/shamir-query-builder/src/batch/tests/when_tests.rs#L81).

<a id="review-3"></a>

### Claim 3 — `collect_query_refs` uses the loose pre-#641 marker rule, diverging from the planner's exact marker-map convention

Status: `confirmed-open`. Current risk: `medium`.

The builder still collects every string-valued $query key regardless of sibling keys. The planner recognizes exact marker shapes before typed decoding, so a literal map containing $query plus another ordinary key is not an alias dependency there.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1122](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1122); [crates/shamir-query-types/src/batch/planner.rs:385](../../../../../crates/shamir-query-types/src/batch/planner.rs#L385).

<a id="review-4"></a>

### Claim 4 — `try_build` does not validate `return_only` aliases

Status: `confirmed-open`. Current risk: `low`.

Validation still ends without examining return_only. The executor retains only requested result keys, so an unknown name silently removes results rather than producing the local alias error.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:124](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L124); [crates/shamir-query-builder/src/batch/batch.rs:912](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L912); [crates/shamir-engine/src/query/batch/batch_execute.rs:868](../../../../../crates/shamir-engine/src/query/batch/batch_execute.rs#L868).

<a id="review-5"></a>

### Claim 5 — `switch` with zero cases emits `when: Not{Or{[]}}`; empty-`Or` truth value is unpinned and untested

Status: `confirmed-open`. Current risk: `low`.

The emitted shape and missing zero-case regression remain. Current execution is source-determined: Or uses iterator any, so empty Or is false and the default's Not is true. Existing switch tests cover AST construction, not this boundary's execution.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1077](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1077); [crates/shamir-engine/src/query/filter/filter_node.rs:664](../../../../../crates/shamir-engine/src/query/filter/filter_node.rs#L664); [crates/shamir-query-builder/src/batch/tests/when_tests.rs:154](../../../../../crates/shamir-query-builder/src/batch/tests/when_tests.rs#L154); [crates/shamir-query-builder/src/batch/tests/when_tests.rs:254](../../../../../crates/shamir-query-builder/src/batch/tests/when_tests.rs#L254).

<a id="review-6"></a>

### Claim 6 — DDL builders document server constraints they do not enforce

Status: `confirmed-open`. Current risk: `low`.

BindValidator accepts any u16 priority, although the execution-side management API rejects values outside 1000..=9999. ReplScopeBuilder still accepts table without repo, contrary to both builder and DTO contracts. Neither builder offers the proposed validation.

Evidence: [crates/shamir-query-builder/src/ddl/validator.rs:162](../../../../../crates/shamir-query-builder/src/ddl/validator.rs#L162); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:477](../../../../../crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L477); [crates/shamir-query-builder/src/ddl/replication.rs:49](../../../../../crates/shamir-query-builder/src/ddl/replication.rs#L49); [crates/shamir-query-builder/src/ddl/replication.rs:56](../../../../../crates/shamir-query-builder/src/ddl/replication.rs#L56); [crates/shamir-query-types/src/admin/types/repl_ops.rs:64](../../../../../crates/shamir-query-types/src/admin/types/repl_ops.rs#L64).

<a id="review-7"></a>

### Claim 7 — Public panicking helper `to_request_via_msgpack`

Status: `confirmed-open`. Current risk: `medium`.

The parent can now identify a constructible codec failure without executing it: owned Array/Map values can be nested beyond rmp-serde 1.3.1's default 1024-container decode budget. Serialization has no active depth counter; decoding those bytes returns DepthLimitExceeded, and the public helper's expect converts that Result to panic. This is a programmatic deep-input API defect, not proof of a normal network request or a measured target stack threshold; existing network decoder bounds do not cap locally built values.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:878](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L878); [crates/shamir-query-builder/src/write/doc.rs:47](../../../../../crates/shamir-query-builder/src/write/doc.rs#L47); [Cargo.lock:2949](../../../../../Cargo.lock#L2949).

Pinned dependency evidence: [rmp-serde 1.3.1, src/decode.rs:294](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs); [rmp-serde 1.3.1, src/decode.rs:566](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs); [rmp-serde 1.3.1, src/encode.rs:147](https://docs.rs/crate/rmp-serde/1.3.1/source/src/encode.rs).

Grouping/duplicate: `error-handling-lifecycle.md#1`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Stale doc references

Status: `confirmed-open`. Current risk: `nit`.

The leaf comment still names nonexistent val::query_ref, and the fallible-write comment still describes panicking IntoBatchOp implementations that were removed from Update/Upsert/Delete builders.

Evidence: [crates/shamir-query-builder/src/filter/leaf.rs:83](../../../../../crates/shamir-query-builder/src/filter/leaf.rs#L83); [crates/shamir-query-builder/src/val/filter_value.rs:166](../../../../../crates/shamir-query-builder/src/val/filter_value.rs#L166); [crates/shamir-query-builder/src/batch/batch.rs:248](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L248); [crates/shamir-query-builder/src/batch/into_batch_op.rs:41](../../../../../crates/shamir-query-builder/src/batch/into_batch_op.rs#L41); [crates/shamir-query-builder/src/batch/try_into_batch_op.rs:29](../../../../../crates/shamir-query-builder/src/batch/try_into_batch_op.rs#L29).

## Corrections and qualified non-findings

- The wired lit_u64 boundary, alias-error, SELECT-function-argument, HAVING, and switch-shape assertions support the stated structural coverage. They do not establish successful runtime execution or measured performance.
- Cursor tests are registered through cursor/tests.rs, not a tests/ manifest directory; the blanket layout guarantee is inaccurate.
- The switch tests use field-based guards and assert constructed trees; they do not demonstrate valid record-less when execution.
- SerializationFailed has only a directly constructed Display test. That does not demonstrate codec failure propagation, and its comment claiming failure is impossible today is not independently proven.
- Correctness finding 2 combines two mechanisms: lookup-miss no-ops and duplicate-alias overwrites. Track both separately when assessing closure.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder -- Correctness & TDD-coverage

## Summary

The crate is in strong shape against CLAUDE.md's discipline: per-module `tests/` directories with manifest-only `mod.rs`, topic-split test files, wire-level (msgpack-decoded) assertions, a cross-language msgpack fixture suite, and a matrix-driven CREATE INDEX accept/reject table shared with the TS client. The real defects cluster in `Batch::try_build`'s `$query`-ref validation, whose msgpack-fallback walk (a) contradicts the planner's documented nested-scope semantics by descending into `sub_batch`/`for_each` inner batches, falsely rejecting legitimate batches, and (b) still uses the loose pre-#641 marker rule the planner explicitly abandoned. Two post-hoc mutators (`Batch::after`, `Batch::when`) fail silently on an unregistered handle, and the silent-drop paths have zero negative tests -- the main TDD gap in an otherwise well-covered suite.

## Findings

### 1. `Batch::try_build` falsely rejects `sub_batch`/`for_each` entries whose inner batch has internal `$query` dependencies
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:1333-1345` (fallback arm of `collect_op_query_refs`) + `collect_query_refs` at `batch.rs:1122-1140`
- **Severity:** high
- **Issue:** `BatchOp::Batch` and `BatchOp::ForEach` are not on the typed fast path, so they fall to the "unconditionally-correct" msgpack round-trip, which serializes the ENTIRE inner `BatchRequest` and collects every `"$query"` key in it. The planner (`shamir-query-types/src/batch/planner.rs:308-322`) states the opposite contract verbatim: "outer deps come exclusively from `bind` values / `over` — Do NOT descend into the inner batch's queries — those are planned recursively at execution time." Inner aliases are a separate scope (that is exactly why `bind`/`param` exists), yet `try_build` checks every inner ref against the OUTER batch's alias set.
- **Failure scenario:** build an inner batch where entry `b` reads entry `a` via `a.first().field("id")`, embed it via `outer.sub_batch("proc", inner.build(), bind)` (or `outer.for_each(...)`), then call `outer.try_build()` → `Err(BuildError::UnknownAlias { alias: "a", referenced_by: "proc" })` for a batch the engine plans and executes correctly. Callers hit by this will drop to the unvalidated `build()`, losing all validation.
- **TDD gap:** `sub_batch_tests.rs` and `for_each_tests.rs` never call `try_build` at all, let alone with inner-to-inner refs; only `bind`/`over`/`CallOp` ref paths are covered (`call_tests.rs:158`, `batch_tests.rs`).
- **Suggested fix:** classify `BatchOp::Batch(sub)` and `BatchOp::ForEach(fe)` explicitly (per the crate's own exhaustive-match convention): validate only `sub.bind` values and `fe.over` against outer aliases and skip the inner `batch` body (its own `try_build`, run by whoever constructs the inner batch, is its validator). Add a red/green test: nested batch with internal ref must pass `try_build`; a `bind` value referencing an unknown outer alias must still fail.

### 2. `Batch::after` and `Batch::when` silently no-op when the handle's alias is not registered in this batch
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:1003-1008` (`after`), `batch.rs:1025-1030` (`when`); same silent-loss class via alias overwrite in `add_entry_after` (`batch.rs:1089-1110`, `TMap::insert` drops any previously attached `after`/`when`)
- **Severity:** medium
- **Issue:** both mutators are `if let Some(entry) = self.queries.get_mut(...) { ... }` with no else. A `Handle` from a *different* `Batch` instance (trivially possible -- both take `&Handle`, the compiler cannot tell), or re-registering an alias after attaching a guard/edge, silently discards the ordering edge or the guard. `try_build` cannot catch this because nothing was ever recorded.
- **Failure scenario:** `b.when(&h, guard)` with `h` from another batch → the op ships with `when: None` and executes **unconditionally** -- a conditional-execution safety primitive silently disabled. `b.after(&rows, &mk)` dropped → DDL→DML ordering edge gone → insert can run before `create_table` and the batch fails at execution time (or silently mis-orders for update/delete).
- **TDD gap:** `after_tests.rs` / `when_tests.rs` cover only the happy path plus manually-injected bad `after` strings; no test exercises an unknown/unregistered handle, and none pins the overwrite-wipes-guard behavior.
- **Suggested fix:** make the mutators fallible (`Result`/`bool` "was the alias found") or at minimum `debug_assert!(self.queries.contains_key(...))` plus a doc line naming the silent no-op; add negative tests for both.

### 3. `collect_query_refs` uses the loose pre-#641 marker rule, diverging from the planner's exact marker-map convention
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:1122-1140` vs `shamir-query-types/src/batch/planner.rs:385-391`
- **Severity:** medium
- **Issue:** the builder treats ANY map containing a `"$query"` string key as a ref; the planner (since #641) only treats len-1 maps with reserved keys (`$query`/`$fn`/`$cond`/`$expr`) or the exact 2-key `{"$query","path"}` shape as markers, everything else being literal data. The builder's header says it "mirrors planner.rs logic" -- it mirrors the pre-#641 logic the planner explicitly fixed.
- **Failure scenario:** user data stored via `Doc::set_value` / `mpack!` that happens to contain `{"$query": "...", "other": ...}` (a field literally named `$query` with extra keys) is data to the server but a ref to `try_build` → spurious `UnknownAlias` rejection of a batch the engine accepts.
- **Suggested fix:** port the marker-map rule (`map.len()` 1-or-`{"$query","path"}`-2) into `collect_query_refs`; add a test with a non-marker map containing a `"$query"` key that must be ignored.

### 4. `try_build` does not validate `return_only` aliases
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:912-987` (validates `$query`, `after`, `when` refs; `return_only` untouched)
- **Severity:** low
- **Issue:** `return_only(["typo_alias"])` passes validation and the server returns a silently reduced/empty result set -- the same "typo'd alias" class `try_build` exists to catch, with the alias set already in hand.
- **Suggested fix:** in `try_build`, check every `return_only` entry against `self.queries` (new `BuildError::UnknownReturnAlias` variant); test both directions.

### 5. `switch` with zero cases emits `when: Not{Or{[]}}`; empty-`Or` truth value is unpinned and untested
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:1077` (`let default_guard = not(or(seen_conditions));`)
- **Severity:** low
- **Issue:** `switch(vec![], default)` is accepted and produces a guard whose evaluation is an engine convention (vacuous OR). Depending on that convention the default branch runs always or never -- and `when_tests.rs` covers only 1, 2, and 4-case shapes, so nothing pins the degenerate input the builder permits.
- **Suggested fix:** either reject empty `cases` (a switch with only a default needs no guard at all -- or emit `when: None`), or add a test documenting the intended empty-`Or` semantics.

### 6. DDL builders document server constraints they do not enforce
- **File:line:** `crates/shamir-query-builder/src/ddl/validator.rs:162-166` (`BindValidator::priority` -- "must be in `[1000, 9999]`", accepts any `u16`); `crates/shamir-query-builder/src/ddl/replication.rs:49-53` (`ReplScopeBuilder::table` -- "requires `repo`", accepts `table` without `repo`)
- **Severity:** low
- **Issue:** both constraints are doc-only. The crate's own pattern (`CreateIndex::try_build`, `Query::try_build`, `BuilderError` guards) is to mirror server checks client-side; these two ship violating ops silently and have no `try_` variant.
- **Suggested fix:** add a fallible `build()`/range check (or `debug_assert`) plus tests, consistent with the `MissingAction` precedent in `AlterSubscriptionBuilder`.

### 7. Public panicking helper `to_request_via_msgpack`
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:878-881` (also `Doc::set`'s two `expect`s at `write/doc.rs:47-50`)
- **Severity:** nit
- **Issue:** panics on codec error in a public API. It is documented ("the builder always produces a serialisable request") and the #1083 precedent moved the analogous `try_build` panic into `BuildError::SerializationFailed`; this helper kept the panic. `Doc::set`'s expects are genuine invariants and fine.
- **Suggested fix:** return `Result<BatchRequest, rmp_serde::encode::Error>` (or gate the helper behind `#[cfg(test)]`-style usage guidance); not urgent.

### 8. Stale doc references
- **File:line:** `crates/shamir-query-builder/src/filter/leaf.rs:83-84` (references `val::query_ref(...)`, which does not exist -- the constructors are `val::qref`/`qref_all`); `crates/shamir-query-builder/src/batch/batch.rs:248-255` (fallible mirrors described as avoiding panics "inside `IntoBatchOp`", but `crate::write::Update`/`Upsert`/`Delete` no longer implement `IntoBatchOp` at all -- only `TryIntoBatchOp`)
- **Severity:** nit
- **Issue:** documentation drift that misdirects the next maintainer.
- **Suggested fix:** comment-only fix, `qref` for the first, reword the `IntoBatchOp`/`TryIntoBatchOp` split for the second.

## TDD-coverage notes (what is well covered)

For balance: `lit_u64`'s i64-boundary contract (FG-1), `Conds` AND/OR/group nesting incl. the CodeIgniter pattern, `switch` guard algebra up to 4 cases, `when`/`after` wire omission, `try_build` UnknownAlias/SelfReference/AfterPathIgnored (typed walkers incl. SELECT function args and HAVING per #1093), the create-index 12-check matrix via the shared JSON fixture, and cross-language msgpack hex fixtures for repl-DDL/vector-filter are all genuinely covered -- none of the above findings are invention; they are the residual gaps.

</details>
