<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-builder — api-wire-protocol revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Nested scoping, alias-state loss, macro dependency leakage, and local validation/documentation gaps remain. Numeric ordering is not simply absent: scalar comparisons have exact Big/String support, while range/set and decimal-string paths require narrower guarantees. Public codec-panic reachability remains unverified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 12 | 12 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Parent dependency inspection supplies the previously missing deep-owned-value / decode-budget counterexample; no runtime test was executed.

<a id="review-1"></a>

### Claim 1 — `Batch::try_build` false-rejects valid nested `sub_batch` / `for_each` batches with inner `$query` refs

Status: `confirmed-open`. Current risk: `high`.

Whole-op fallback traverses inner aliases and validates them in the outer namespace, unlike planner and recursive executor scoping.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1313](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1313); [crates/shamir-query-types/src/batch/planner.rs:308](../../../../../crates/shamir-query-types/src/batch/planner.rs#L308); [crates/shamir-engine/src/query/batch/query_runner.rs:680](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L680); [crates/shamir-engine/src/query/batch/query_runner.rs:880](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L880).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `Batch::after` / `Batch::when` silently no-op when the handle's alias is not registered

Status: `confirmed-open`. Current risk: `medium`.

Missing target aliases still silently discard mutations. Foreign handles with matching local aliases instead modify the local entry; replaced handles do not automatically become lookup misses.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1003](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1003); [crates/shamir-query-builder/src/batch/batch.rs:1025](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1025); [crates/shamir-query-builder/src/batch/handle.rs:11](../../../../../crates/shamir-query-builder/src/batch/handle.rs#L11).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Re-registering an alias silently replaces the earlier op — a database operation vanishes

Status: `confirmed-open`. Current risk: `medium`.

add_entry_after still ignores the replaced value returned by map insertion and constructs fresh after/when fields. No collision state survives for try_build to diagnose.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1089](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1089); [crates/shamir-query-builder/src/batch/batch.rs:1096](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1096); [crates/shamir-query-builder/src/batch/batch.rs:1110](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1110).

<a id="review-4"></a>

### Claim 4 — `subscribe!` / `bind!` macros hardcode foreign crate paths, breaking the crate's dependency-hiding contract

Status: `confirmed-open`. Current risk: `medium`.

Exported expansions still contain call-site shamir_collections and shamir_query_types paths. The builder-only re-export contract and existing $crate-based macros provide positive contrast. Internal macro tests already have those dependencies, so they cannot detect a builder-only downstream failure.

Evidence: [crates/shamir-query-builder/src/macros/mod.rs:63](../../../../../crates/shamir-query-builder/src/macros/mod.rs#L63); [crates/shamir-query-builder/src/macros/mod.rs:87](../../../../../crates/shamir-query-builder/src/macros/mod.rs#L87); [crates/shamir-query-builder/src/macros/mod.rs:100](../../../../../crates/shamir-query-builder/src/macros/mod.rs#L100); [crates/shamir-query-builder/src/macros/mod.rs:162](../../../../../crates/shamir-query-builder/src/macros/mod.rs#L162); [crates/shamir-query-builder/src/lib.rs:66](../../../../../crates/shamir-query-builder/src/lib.rs#L66); [crates/shamir-query-builder/src/macros/tests/mod.rs:4](../../../../../crates/shamir-query-builder/src/macros/tests/mod.rs#L4); [crates/shamir-query-builder/Cargo.toml:17](../../../../../crates/shamir-query-builder/Cargo.toml#L17).

<a id="review-5"></a>

### Claim 5 — `try_build` does not validate `return_only` aliases

Status: `confirmed-open`. Current risk: `low`.

No return_only membership check exists; execution filters away results for unknown names. Explicit return_only overrides return_result flags, so rejecting silent aliases is not justified by the current executor.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:912](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L912); [crates/shamir-engine/src/query/batch/batch_execute.rs:868](../../../../../crates/shamir-engine/src/query/batch/batch_execute.rs#L868).

Grouping/duplicate: `correctness-tdd.md#4`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — `Query` pagination setters silently clobber each other

Status: `confirmed-open`. Current risk: `low`.

Setters still replace pagination families; limit/offset reset incompatible state. try_build validates only final page values and HAVING, and setter docs do not explain cross-family replacement.

Evidence: [crates/shamir-query-builder/src/query/query.rs:154](../../../../../crates/shamir-query-builder/src/query/query.rs#L154); [crates/shamir-query-builder/src/query/query.rs:182](../../../../../crates/shamir-query-builder/src/query/query.rs#L182); [crates/shamir-query-builder/src/query/query.rs:207](../../../../../crates/shamir-query-builder/src/query/query.rs#L207); [crates/shamir-query-builder/src/query/query.rs:355](../../../../../crates/shamir-query-builder/src/query/query.rs#L355).

<a id="review-7"></a>

### Claim 7 — Public API leaks `rmp_serde` error types; decode errors are re-labeled as encode errors

Status: `confirmed-open`. Current risk: `low`.

Public encoding methods still return codec enums, ResponseError carries decode::Error, and to_query_value maps decoding failures into encode::Error::Syntax. This is API coupling/phase attribution debt, not proof that a compatible dependency release breaks callers.

Evidence: [crates/shamir-query-builder/src/wire/mod.rs:31](../../../../../crates/shamir-query-builder/src/wire/mod.rs#L31); [crates/shamir-query-builder/src/wire/mod.rs:37](../../../../../crates/shamir-query-builder/src/wire/mod.rs#L37); [crates/shamir-query-builder/src/batch/batch.rs:868](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L868); [crates/shamir-query-builder/src/response/batch_response_ext.rs:30](../../../../../crates/shamir-query-builder/src/response/batch_response_ext.rs#L30); [Cargo.lock:2949](../../../../../Cargo.lock#L2949).

<a id="review-8"></a>

### Claim 8 — `Batch::to_request_via_msgpack` panics in a library API

Status: `confirmed-open`. Current risk: `medium`.

The parent can now identify a constructible codec failure without executing it: owned Array/Map values can be nested beyond rmp-serde 1.3.1's default 1024-container decode budget. Serialization has no active depth counter; decoding those bytes returns DepthLimitExceeded, and the public helper's expect converts that Result to panic. This is a programmatic deep-input API defect, not proof of a normal network request or a measured target stack threshold; existing network decoder bounds do not cap locally built values.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:878](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L878); [crates/shamir-query-builder/src/batch/tests/to_request_via_msgpack_tests.rs:12](../../../../../crates/shamir-query-builder/src/batch/tests/to_request_via_msgpack_tests.rs#L12); [Cargo.lock:2949](../../../../../Cargo.lock#L2949).

Pinned dependency evidence: [rmp-serde 1.3.1, src/decode.rs:294](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs); [rmp-serde 1.3.1, src/decode.rs:566](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs); [rmp-serde 1.3.1, src/encode.rs:147](https://docs.rs/crate/rmp-serde/1.3.1/source/src/encode.rs).

Grouping/duplicate: `error-handling-lifecycle.md#1`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — `lit_u64`'s decimal-`String` encoding for `u64 > i64::MAX` is only specified for equality

Status: `confirmed-open`. Current risk: `low`.

The builder rustdoc remains equality-focused, but exact Big/Str ordering is implemented and documented for Compare, refuting the proposed missing-gt mechanism. Remaining caveats are concrete: Str/Str ordering is lexical, and Between/InSet require scalar_at without Compare's Big/raw-u64 materialization fallback.

Evidence: [crates/shamir-query-builder/src/val/filter_value.rs:62](../../../../../crates/shamir-query-builder/src/val/filter_value.rs#L62); [crates/shamir-engine/src/query/filter/resolve.rs:161](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L161); [crates/shamir-engine/src/query/filter/resolve.rs:193](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L193); [crates/shamir-engine/src/query/filter/filter_node.rs:670](../../../../../crates/shamir-engine/src/query/filter/filter_node.rs#L670); [crates/shamir-engine/src/query/filter/filter_node.rs:952](../../../../../crates/shamir-engine/src/query/filter/filter_node.rs#L952); [docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md:69](../../../../../docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md#L69); [crates/shamir-engine/src/query/filter/tests/eval_tests/u64_big_filter_match_tests.rs:133](../../../../../crates/shamir-engine/src/query/filter/tests/eval_tests/u64_big_filter_match_tests.rs#L133).

<a id="review-10"></a>

### Claim 10 — Doc drift: nonexistent `val::query_ref` referenced; `mpak!` typo

Status: `confirmed-open`. Current risk: `nit`.

Both stale references remain. The query_ref example is a plain implementation comment, not an executable rustdoc example; constructors are qref/qref_all.

Evidence: [crates/shamir-query-builder/src/filter/leaf.rs:83](../../../../../crates/shamir-query-builder/src/filter/leaf.rs#L83); [crates/shamir-query-builder/src/val/filter_value.rs:166](../../../../../crates/shamir-query-builder/src/val/filter_value.rs#L166); [crates/shamir-query-builder/src/write/insert.rs:44](../../../../../crates/shamir-query-builder/src/write/insert.rs#L44).

Grouping/duplicate: `correctness-tdd.md#8`. This row is not another independent defect.

<a id="review-11"></a>

### Claim 11 — `val::func`/`val::expr` collide by name with `select::func`/`select::expr` for glob-import users

Status: `confirmed-open`. Current risk: `nit`.

Both modules still export both names. Using both globs and calling an unqualified conflicting name is ambiguous; module-qualified calls remain valid. This is an ergonomics issue, not a wire defect.

Evidence: [crates/shamir-query-builder/src/val/filter_value.rs:116](../../../../../crates/shamir-query-builder/src/val/filter_value.rs#L116); [crates/shamir-query-builder/src/val/expr.rs:15](../../../../../crates/shamir-query-builder/src/val/expr.rs#L15); [crates/shamir-query-builder/src/select/select_item.rs:42](../../../../../crates/shamir-query-builder/src/select/select_item.rs#L42); [crates/shamir-query-builder/src/select/select_item.rs:76](../../../../../crates/shamir-query-builder/src/select/select_item.rs#L76).

<a id="review-12"></a>

### Claim 12 — `Batch`'s ~40 named DDL/DML methods do not constrain the op family

Status: `confirmed-open`. Current risk: `nit`.

Named methods still accept broad IntoBatchOp inputs and forward the actual variant. Their family names provide no enforcement. A mismatched name alone does not cause a server failure: the serialized variant, not the convenience-method name, determines execution.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:180](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L180); [crates/shamir-query-builder/src/batch/batch.rs:355](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L355); [crates/shamir-query-builder/src/batch/batch.rs:372](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L372); [crates/shamir-query-builder/src/batch/into_batch_op.rs:13](../../../../../crates/shamir-query-builder/src/batch/into_batch_op.rs#L13).

## Corrections and qualified non-findings

- Wire and matrix assertions are registered and compare actual encoded output. Integration tests require the integration/full scope; doctests are disabled by Cargo.toml:13.
- The universal tests/ directory claim excludes cursor, which uses cursor/tests.rs but is wired.
- Finding 9's gt(u64::MAX) example is a poor oracle: no valid u64 value exceeds that threshold. Use distinguishing values and explicit storage representations when adding coverage.
- Do not infer absence of ordering support from equality-only constructor rustdocs. Existing Compare support predates this review; range/set support and decimal-string semantics must be assessed independently.
- Duplicate-alias replacement preserves alias lookup identity but changes the referenced operation and resets attached state. It is not inherently a later no-op.
- Changing convenience method names or matching the correct op family is independent of server authorization. The examples establish API permissiveness, not security bypass.
- Codec wrapping is a design option; a future compatible release causing a breaking error-type change was not demonstrated.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder -- API & wire-protocol design

## Summary

The crate is a disciplined, thin builder layer over `shamir-query-types`: every terminal produces exactly a wire DTO, the msgpack (named-fields) codec is exercised end-to-end in tests (`wire_tests`, `to_request_via_msgpack_tests`, byte-identical `create_index_matrix.json` cross-language fixtures), and the infallible `build()` / validating `try_build()` split is applied consistently across `Batch`, `Query`, and `CreateIndex` with three separate, well-documented error families. The theme's real problems concentrate in the client-side validation pass — `Batch::try_build` over-validates nested `sub_batch`/`for_each` bodies against the *outer* alias namespace, directly contradicting the planner's documented scoping rule — and in several silent-noop / silent-overwrite paths in the `Batch` alias/`Handle` model that no `try_build` check can observe. The exported `subscribe!`/`bind!` macros also bypass the crate's own "downstream guests need no direct shamir-query-types dependency" re-export contract.

## Findings

### 1. `Batch::try_build` false-rejects valid nested `sub_batch` / `for_each` batches with inner `$query` refs
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:1333-1345` (fallback arm of `collect_op_query_refs`), driven by `batch.rs:913-931`; contrast `crates/shamir-query-types/src/batch/planner.rs:308-322`
- **Severity:** high
- **Issue:** For every `BatchOp` outside the 5 typed fast-path variants, `try_build` serializes the whole op and walks the resulting tree for `"$query"` strings — including the *inner* `BatchRequest` of `BatchOp::Batch(SubBatchOp)` and `BatchOp::ForEach(ForEachOp)`. Every collected ref is then checked against the **outer** batch's `queries` map (`UnknownAlias` / `SelfReference`). The planner explicitly does the opposite: "outer deps come exclusively from `bind` values. Do NOT descend into the inner batch's queries — those are planned recursively at execution time" (`planner.rs:308-315`, same for `ForEach` at 316-322). Inner aliases live in the inner namespace, so the validator and the executor disagree on scoping.
- **Failure scenario:** A client builds an outer batch containing `b.sub_batch("proc", inner, bind)` where `inner` legitimately chains two of its own entries via `Handle::column` (the crate's headline dependency feature). `inner.build()` is fine and the server would accept it, but `outer.try_build()` unconditionally returns `BuildError::UnknownAlias { alias: "inner_q", .. }`. Callers must then either drop `try_build` (losing *all* validation, including the correct checks) or contort the model. There is no test covering `try_build` × inner-batch refs (`sub_batch_tests.rs` only tests `build()`), so the gap is invisible to the suite.
- **Suggested fix:** Give `Batch` and `ForEach` typed arms in `collect_op_query_refs` mirroring the planner: collect refs only from `SubBatchOp.bind` values / `ForEachOp.over`, and (optionally) recursively validate the inner `BatchRequest` against the inner request's own alias set. Add a regression test: nested batch whose inner entries reference inner aliases must pass `try_build`.

### 2. `Batch::after` / `Batch::when` silently no-op when the handle's alias is not registered
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:1003-1008` (`after`), `1025-1030` (`when`)
- **Severity:** medium
- **Issue:** Both post-hoc methods are `if let Some(entry) = self.queries.get_mut(handle.alias()) { ... }` with no else. `Handle` is just a `String` alias with no batch identity, so a handle from a *different* `Batch`, or one whose entry was later replaced under the same alias (see finding 3), makes the call a silent no-op: no ordering edge is written and no guard is attached.
- **Failure scenario:** The documented primary use of `after` is DDL→DML ordering ("`create_table` then `insert`"). If the dependent handle is stale/mis-batched, `after()` silently drops the edge; the planner then sees no dependency and may run the insert before the table exists — a runtime server error (or, for `when`, an unconditional execution of an op meant to be guarded). `try_build` cannot catch it because the `after` list was never populated.
- **Suggested fix:** Return `Result<(), BuildError>` (or at minimum `debug_assert!` + document) when `dependent.alias()` is absent from `queries`; the `try_op`-style precedent shows this crate already prefers typed errors over silent drops.

### 3. Re-registering an alias silently replaces the earlier op — a database operation vanishes
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:1096-1109` (`add_entry_after` uses `TMap::insert`, which overwrites)
- **Severity:** medium
- **Issue:** Aliases are the result-key namespace, and `add_entry_after` `insert`s unconditionally. Registering the same alias twice silently discards the first `BatchOp` — including a write or destructive DDL — and also wipes any `after`/`when` state attached to it (the replacement entry starts with `after: []`, `when: None`), silently invalidating handles previously returned for that alias. Not even `try_build` detects the collision.
- **Failure scenario:** `b.insert("row", ins_a); b.insert("row", ins_b);` ships a batch containing only `ins_b`; `ins_a` is never executed and no error, warning, or result entry ever mentions it. In loops that reuse a literal alias, this silently drops all but the last iteration's op.
- **Suggested fix:** Reject duplicate aliases in `add_entry_after` (typed `BuildError::DuplicateAlias`, surfaced via `try_build` at minimum; a debug_assert in the infallible path), or rename-with-suffix and return the surviving `Handle`.

### 4. `subscribe!` / `bind!` macros hardcode foreign crate paths, breaking the crate's dependency-hiding contract
- **File:line:** `crates/shamir-query-builder/src/macros/mod.rs:63-69` (`bind!` emits `shamir_collections::new_map()`), `85-189` (`subscribe!` emits `shamir_query_types::subscribe::EventMask`, `shamir_query_types::TableRef::with_repo`, `shamir_query_types::batch::SubBatchOp`)
- **Severity:** medium
- **Issue:** `lib.rs:66-79` re-exports the DTOs explicitly "so a downstream guest (the SDK) can name them without depending on shamir-query-types directly" (WASM-lean footprint is a stated design goal, `lib.rs:21-23`). The `#[macro_export]` macros contradict this: `macro_rules!` hygiene resolves the hardcoded `shamir_query_types::…` / `shamir_collections::…` paths **in the downstream crate**, so any consumer of `subscribe!`/`bind!` needs direct dependencies on both crates at compatible versions. Other macros in the same file correctly route through `$crate::` (`doc!`, `vals!`), so the fix pattern already exists in-file.
- **Failure scenario:** The WASM/SDK guest that depends only on `shamir-query-builder` cannot compile `subscribe!` or `bind!` (unresolved crate paths); a workspace-internal user is masked from the problem until the guest build breaks.
- **Suggested fix:** Route every emitted path through `$crate::` re-exports (`$crate::val::...`, plus new `pub use` re-exports for `TableRef` and `SubBatchOp` — or move bind-map construction behind a `$crate::batch::bind_map(...)` helper).

### 5. `try_build` does not validate `return_only` aliases
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:912-987` (validates `$query`, `after`, `when` refs only; `return_only` set at `124-131` is unchecked)
- **Severity:** low
- **Issue:** `try_build`'s stated purpose is to catch client-side mistakes ("the base alias exists as a key in `queries`") before a server round trip, but `return_only(["typo"])` passes validation even though the alias does not exist in the batch — the same class of mistake `UnknownAlias` exists for.
- **Failure scenario:** A typo'd `return_only` entry silently narrows the response (or is rejected only server-side after a full round trip), defeating the "find out at construction time" guarantee the crate documents for its other validation passes.
- **Suggested fix:** In `try_build`, check each `return_only` alias against `self.queries` (and, if the planner requires it, that the entry is non-silent).

### 6. `Query` pagination setters silently clobber each other
- **File:line:** `crates/shamir-query-builder/src/query/query.rs:154-188` (`limit`/`offset`/`page`), `207-231` (`after`/`after_with_id`)
- **Severity:** low
- **Issue:** `limit`/`offset` convert any non-`LimitOffset` pagination to `LimitOffset` (silently discarding a prior `page(n, size)`), and `page`/`after`/`after_with_id` replace the variant wholesale. The crate already solved exactly this bug class for `CreateIndex` (`check_conflicting_state` + `try_build`, `create_index.rs:81-160`), so the asymmetric leniency here is an inconsistency, not a principled choice.
- **Failure scenario:** `.page(3, 20).limit(5)` silently becomes `LimitOffset { limit: 5, offset: 0 }` — page 1, 5 rows — with no error; a keyset `.after(..)` call silently erases an earlier `.page(..)`.
- **Suggested fix:** Either track "pagination already set" and panic-free-reject/deny the second family via a `try_build`-style check (mirroring `CreateIndexBuildError`), or document the last-write-wins rule on each method.

### 7. Public API leaks `rmp_serde` error types; decode errors are re-labeled as encode errors
- **File:line:** `crates/shamir-query-builder/src/wire/mod.rs:31-44` (`to_query_value` / `to_msgpack` return `rmp_serde::encode::Error`), `wire/mod.rs:37` (decode failure mapped to `encode::Error::Syntax`), `src/batch/batch.rs:868` (`Batch::to_msgpack`), `src/response/batch_response_ext.rs:15-37` (`ResponseError::Deserialize` carries `rmp_serde::decode::Error`)
- **Severity:** low
- **Issue:** The crate's public error surface directly embeds a third-party crate's error enums, coupling the semver of this alpha crate to rmp-serde's (a future codec major bump becomes an API break). Additionally, `to_query_value` reports a *decode* failure as an encode error variant — documented in-line, but callers matching on the error cannot distinguish the phases.
- **Failure scenario:** Upgrading `rmp-serde` (even to a compatible-looking minor with error-type changes) breaks downstream `match` arms; a decode failure in `to_query_value` is misdiagnosed as an encoding bug by naive handlers.
- **Suggested fix:** Wrap codec errors in a crate-owned error enum (`WireError::{Encode(String), Decode(String)}`), following the crate's own `SerializationFailed { reason: String }` precedent in `BuildError`.

### 8. `Batch::to_request_via_msgpack` panics in a library API
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:878-881` (`.expect("msgpack encode")` / `.expect("msgpack decode")`)
- **Severity:** low
- **Issue:** A public, non-test method panics on codec failure. The crate's own `#1083` note (`batch_tests.rs:414-429`) establishes the repo position that "panicking a public non-test API instead of returning a `Result`" is a defect worth fixing (that's why `try_build` returns `SerializationFailed`). The doc comment argues "the builder always produces a serialisable request", but the same argument was explicitly rejected for `try_build`.
- **Failure scenario:** A future `QueryValue` extension (e.g. a value the codec rejects) turns a validation problem into an abort inside library code, exactly the class `#1083` removed elsewhere.
- **Suggested fix:** Return `Result<BatchRequest, rmp_serde::encode::Error>` (or the wrapper from finding 7); keep a panicking alias only if test ergonomics demand it, marked `#[doc(hidden)]`.

### 9. `lit_u64`'s decimal-`String` encoding for `u64 > i64::MAX` is only specified for equality
- **File:line:** `crates/shamir-query-builder/src/val/filter_value.rs:62-82`
- **Severity:** low
- **Issue:** The unified-u64 contract documents that the `Str(decimal)` representation matches stored `Big` values via "the engine's cross-type comparison layer (`Big`↔`Str` **equality**)". For range/set operators (`gt`/`lt`/`between`/`in_`) built with the same value, no cross-type ordering guarantee is stated, and the builder cannot enforce one.
- **Failure scenario:** `Query::from("t").where_gt("id", lit_u64(u64::MAX))` compiles and ships `{"field": ["id"], "value": "<decimal string>"}`; if the engine's ordering layer only special-cases equality, the filter silently matches nothing (or errors per-op), and the wire encoding offers the server no way to know the string was intended numerically.
- **Suggested fix:** Either verify/extend the engine contract for ordering comparisons and broaden the doc, or add a typed `FilterValue::Big`-style constructor once the wire DTO grows one, so the numeric intent survives serialization.

### 10. Doc drift: nonexistent `val::query_ref` referenced; `mpak!` typo
- **File:line:** `crates/shamir-query-builder/src/filter/leaf.rs:83-84`; `crates/shamir-query-builder/src/write/insert.rs:44`
- **Severity:** nit
- **Issue:** The `value_*` doc comment's example (`value_gte(val::query_ref("balance"), ...)`) names a function that does not exist — the constructors are `val::qref` / `val::qref_all` (`val/filter_value.rs:166-182`). `Insert::row`'s doc says "e.g. from `mpak!({...})`" (elsewhere consistently `mpack!`).
- **Failure scenario:** Users copy the doc snippet and hit a compile error; search for `val::query_ref` finds nothing.
- **Suggested fix:** Replace with `val::qref("balance", "[0].balance")`-style snippets; fix the `mpak!` → `mpack!` typo.

### 11. `val::func`/`val::expr` collide by name with `select::func`/`select::expr` for glob-import users
- **File:line:** `crates/shamir-query-builder/src/val/filter_value.rs:116` vs `src/select/select_item.rs:42`; `src/val/expr.rs:15` vs `src/select/select_item.rs:76`
- **Severity:** nit
- **Issue:** Both modules are designed for `use …::*` consumption (their own module docs say so), yet they export same-named functions with different signatures. The crate has an established rename precedent for exactly this (`and_expr`/`or_expr`/`not_expr` in `val/expr.rs:80-100`, `negate` in `FilterExt`), so these two pairs are inconsistent with it.
- **Failure scenario:** `use shamir_query_builder::{val::*, select::*};` makes bare `func(...)`/`expr(...)` calls an ambiguity error; users discover it only after writing call sites.
- **Suggested fix:** Rename the select-side constructors (e.g. `select_fn`/`select_expr` aliases, or keep `func` only behind the module path), matching the `*_expr` precedent.

### 12. `Batch`'s ~40 named DDL/DML methods do not constrain the op family
- **File:line:** `crates/shamir-query-builder/src/batch/batch.rs:318-655` (all delegate to `op: impl IntoBatchOp`)
- **Severity:** nit
- **Issue:** Every specialized method (`create_table`, `insert`, `delete`, …) accepts any `IntoBatchOp`, so `b.create_table("t", insert("users").row(...))` compiles and ships a `BatchOp::Insert` under a create-table alias. The names provide documentation value only; no type- or validation-time check ties the method name to the op variant (`op()` is the *documented* escape hatch, which implies the named methods promise more than they enforce).
- **Failure scenario:** Swapped/mis-paired arguments compile cleanly and fail only at server execution, after a round trip — the exact failure mode the crate's `try_build` passes exist to prevent.
- **Suggested fix:** If enforcement is desired, wrap each family in a marker newtype implementing `IntoBatchOp` and make the named methods generic over the marker; otherwise document the methods as aliases of `op()` so the lack of enforcement is explicit.

---
*Scope note: reviewed against CLAUDE.md's documented standards (builder-only query construction, `Result`-based error handling, three-family error split, test organization, surgical-style module layout). Test-coverage claim verified: `wire`, `batch` (11 test files incl. msgpack round-trip, `after`, `when`/`switch`, sub-batch, call, for-each, try_into_batch_op), `query`, `select`, `filter`, `val`, `write`, `ddl`, `macros`, `cursor`, `response` all have `tests/` directories per convention, plus cross-language fixtures (`tests/repl_ddl_msgpack.rs`, `tests/create_index_matrix.rs`). No builder-rule violations (raw `json!`/`Value` query assembly) found inside the crate itself; the msgpack round-trips present are the wire-format-under-test exception documented in CLAUDE.md.*

</details>
