# shamir-query-types -- Error handling & resource lifecycle

## Summary

This is a pure-DTO crate, so there is no I/O resource lifecycle to manage (no files, sockets, locks, or spawns anywhere in `src/`), and error discipline is generally strong: no `anyhow`/`Box<dyn Error>` leakage, serde decode errors flow through `de::Error::custom`, and the only three non-test `unwrap`/`expect` sites sit on documented invariants. The real error-handling exposure is DoS-surface rather than cleanup: the planner contains unbounded recursion over client-shaped dependency graphs that the crate's own hardening standard (`NESTING_WALK_LIMIT`) says must be iterative, the `MAX_FILTER_DEPTH` guard does not actually cover the `$cond` nesting its doc claims, and two conversion/arithmetic paths silently degrade (`FilterValue::from(QueryValue)` → `Null`, non-saturating pagination math). In-crate error-path test coverage has specific gaps: the four headline `BatchError` variants documented on `BatchPlanner::plan`, `QueryReference::parse` and all seven `ReferenceParseError` variants, and `QueryRecord`'s non-finite-float rejection are tested only (or not at all) outside this crate.

## Findings

### 1. Unbounded recursion in `detect_cycle` / `calculate_max_depth` — stack overflow aborts the server
- **File:** `crates/shamir-query-types/src/batch/planner.rs:671-703` (dfs), `planner.rs:721-747` (`depth`), contrast `planner.rs:69` + `planner.rs:770-773` (`NESTING_WALK_LIMIT` iterative walk)
- **Severity:** medium
- **Issue:** `BatchPlanner::plan` deliberately made the sub-batch nesting walk iterative with a hard 64-deep cap, commented "so a malicious deeply-nested payload cannot blow the call stack" — but `detect_cycle`'s DFS (line 692) and `calculate_max_depth`'s `depth()` (lines 735-737) still recurse once per node in the dependency graph. Recursion depth equals the longest dependency chain, which is bounded only by `limits.max_queries` — a value the crate itself never caps (the server clamps it to the operator-configured `max_queries_per_batch`; config default 100, but explicitly settable arbitrarily high, and `QueryLimitsCap::UNLIMITED` is `usize::MAX`).
- **Failure scenario:** An operator configures `max_queries_per_batch = 500_000` (plausible for a bulk-load deployment). A client submits an acyclic linear chain `a1 → a2 → … → aN` of N entries. `queries.len() > max_queries` passes, `detect_cycle` recurses N frames deep, and the tokio worker thread (~2 MB stack) overflows. A stack overflow is an abort, not a panic: no unwind, no `Drop` guards run, the whole server process dies — remotely triggerable, restart-loopable.
- **Suggested fix:** Give both walks the same treatment as `max_nesting_depth_of_ops`: convert to an explicit worklist/stack, or add a chain-length cap mirroring `NESTING_WALK_LIMIT` (a chain longer than any sane `max_dependency_depth` can be rejected before recursing). `topological_sort` is already iterative (Kahn's) and can be used as the pattern.

### 2. `check_filter_depth` does not descend into `FilterValue` operands — doc claims `$cond` coverage it doesn't have
- **File:** `crates/shamir-query-types/src/filter/filter_enum.rs:216-238`
- **Severity:** low
- **Issue:** The constant's doc says the cap exists "to prevent stack overflow post-handshake" and the function's contract lists "`$cond`/`not`/`and`/`or`" as the capped shapes, but the implementation only pushes children for `And`/`Or`/`Not`. A `Filter::ValueCompare { left: FilterValue::Cond { … } }` (or an `$expr`/`$fn` arg, or a `Filter` carried inside a `$cond`'s `condition`) is treated as a leaf, so a deep `And`/`Not` chain hidden inside any `FilterValue` operand bypasses `MAX_FILTER_DEPTH = 64` entirely at the engine's `batch_validate` call site. Today the only backstop is rmp-serde's own ~1024-container decode depth limit — an accident of the codec, not this crate's 64-deep contract, and one that silently changes if the wire codec ever does.
- **Failure scenario:** A client sends a WHERE filter whose `$cond.then` embeds a ~900-deep `And` chain. `check_filter_depth` returns `Ok(())`; every downstream recursive walk over the tree (the planner's `extract_deps_from_filter`/`filter_value_contains_field_based_comparison`, engine compile/eval) runs ~900 frames deep — survivable per-request, but the crate's stated 64-deep guarantee is not actually enforced for this shape.
- **Suggested fix:** Descend into `FilterValue` operands (`Cond` condition/branches, `Expr`/`FnCall` args, `Array` items) in the same iterative stack walk — the planner already has the mutually-recursive pair (`contains_field_based_comparison` / `filter_value_contains_field_based_comparison`, planner.rs:535-606) to mirror. Alternatively, fix the doc to state that operand trees are bounded only by the msgpack depth limit.

### 3. `From<QueryValue> for FilterValue` silently substitutes `Null` in release builds
- **File:** `crates/shamir-query-types/src/filter/filter_value.rs:257-279` (tier 3, lines 270-278)
- **Severity:** low
- **Issue:** When both the direct conversion and the msgpack round-trip fail, the impl returns `FilterValue::Null` and guards the case with only a `debug_assert!(false, …)`. CLAUDE.md's rule is "Return `Result<T, E>` … avoid silent failure"; a `From` impl that can fail must not exist as infallible — production builds get a wrong value (`Null`) with no error, no log, and no trace. Current callers are mostly literal conversions (builders/tests), and the live DDL-default path deserializes `FilterValue` directly, so this is latent rather than active — but `QueryValue::Map` is exactly the tier-2 input a future caller can hand it, and a malformed expression default would silently become a NULL default stamped on writes.
- **Failure scenario:** Any future call site doing `let fv: FilterValue = client_map.into();` with a map that doesn't decode as a `FilterValue` gets `Null` in production — a silent data substitution — while debug builds panic at the same site, i.e. the failure mode differs by build profile.
- **Suggested fix:** Remove the tier-3 fallback: make the fallible conversion an explicit `try_from`-style `Result`/`Option` API (the crate already has the right shape in `query_value_to_filter_value -> Option`), and keep `From` only for the provably-infallible literal conversions.

### 4. Non-saturating arithmetic on client-controlled `u64` pagination fields
- **File:** `crates/shamir-query-types/src/read/limit.rs:180` (`page.saturating_sub(1) * page_size`), `limit.rs:294` (`skip + page_size < total`)
- **Severity:** low
- **Issue:** `Pagination::resolve` and `PaginationInfo::compute` perform unchecked `*` and `+` on wire-supplied `u64`s (`page`, `page_size`, `offset`). Everything around them is saturating (`saturating_sub` on the same line; the planner's ForEach gate uses `saturating_mul`), so these two sites are inconsistent with the crate's own arithmetic discipline.
- **Failure scenario:** `Pagination::Page { page: u64::MAX, page_size: u64::MAX }` from a hostile/buggy client: in a debug build line 180 panics ("attempt to multiply with overflow"); in release it wraps (here to `skip = 2`, so the client gets page-2 rows for `page = u64::MAX`), and line 294 wraps again into wrong `has_next`. Wrong metadata / wrong page, no corruption, but a panic in any debug-mode deployment and a silently wrong answer in release.
- **Suggested fix:** Use `saturating_mul` on line 180 and `skip.saturating_add(page_size) < total` on line 294 (and reject `page == 0` / absurd values at the decode boundary if a stricter contract is wanted).

### 5. Missing in-crate error-path tests: headline planner errors, `QueryReference::parse`, non-finite float rejection
- **File:** `crates/shamir-query-types/src/batch/tests/planner_tests.rs` (error variants covered), `src/batch/tests/mod.rs:1-8` (no `reference_tests`), `src/read/query_record.rs:117-122`
- **Severity:** low
- **Issue:** Three concrete gaps where error paths have no coverage under `./scripts/test.sh -p shamir-query-types` (the CLAUDE.md-mandated central entry point):
  - `BatchPlanner::plan`'s own `# Errors` doc lists `TooManyQueries`, `UnknownAlias`, `CircularDependency`, `TooDeep`, but this crate's planner tests exercise only `NestingTooDeep`, `AfterPathIgnored`, `InvalidWhenFilter`, `InvalidCondCondition` (and ForEach's `TooManyQueries` gate). The four headline variants — including the cycle detection and depth computation that finding 1 shows are the most fragile code in the planner — are tested only in `shamir-engine`'s suite, so a regression here ships green through this crate's own gate.
  - `QueryReference::parse` and all seven `ReferenceParseError` variants (`MissingAt`, `EmptyAlias`, `InvalidAlias`, `UnclosedBracket`, `InvalidIndex`, `TrailingDot`, `UnexpectedChar`) have zero tests in this crate (only `shamir-engine/src/query/batch/tests/reference_tests.rs` covers them), despite the module owning the parser per the crate's "tests live with the module" layout.
  - `QueryRecordVisitor::visit_f64`'s explicit rejection of non-finite floats (`de::Error::custom("non-finite float in QueryRecord")`) is untested anywhere in the crate.
- **Suggested fix:** Add `src/batch/tests/reference_tests.rs` (happy paths + each error variant), extend `planner_tests.rs` with the four documented error variants, and add a decode test that a NaN/inf payload fails `QueryRecord` deserialization.

### 6. thiserror convention deviation: hand-rolled Display/Error impls and `Result<(), String>` public APIs
- **File:** `crates/shamir-query-types/src/batch/batch_error.rs:245-368`, `src/batch/reference.rs:243-275`, `src/filter/filter_enum.rs:219` (`check_filter_depth -> Result<(), String>`), `src/admin/types/retention.rs:40-47` (`Retention::validate -> Result<(), String>`)
- **Severity:** low
- **Issue:** CLAUDE.md says "`thiserror` for library error enums"; this crate hand-writes `Display` + empty `std::error::Error` impls for both of its error enums (`BatchError` ~120 lines, `ReferenceParseError`), and thiserror is not even a dependency. Two public validators return untyped `Result<(), String>`, which callers cannot match on programmatically (the server has to stringly re-classify them). The crate's minimal-dependency stance may justify this, but nothing in CLAUDE.md carves out that exception and no comment documents the deviation.
- **Suggested fix:** Either adopt `thiserror` for the two enums (it's a proc-macro with no runtime footprint) or add a short doc comment on each enum stating the deliberate no-macro rationale; give `check_filter_depth`/`Retention::validate` typed error enums (or at minimum a `#[non_exhaustive]` error type) so callers can match rather than parse strings.

### 7. `Pagination`'s `PartialEq` can panic via `key_bytes`' `expect`
- **File:** `crates/shamir-query-types/src/read/limit.rs:130-133`
- **Severity:** nit
- **Issue:** `key_bytes` asserts "serializing Vec<QueryValue> is infallible", but rmp-serde's encoder has a `DepthLimitExceeded` failure mode (~1024 nested containers). Wire-decoded seek tuples can't exceed the decoder's symmetric limit, so this is practically unreachable — but any in-process construction of a >1024-deep `QueryValue` turns a comparison (`pagination == other`) into a panic in a trait impl.
- **Suggested fix:** Compare with a fallback (`unwrap_or_default()` on the encode result) or document the invariant next to the `expect` the way `hmac.rs` does.

### 8. `expect` in HMAC tag compute/verify (acceptable, but undocumented-as-invariant)
- **File:** `crates/shamir-query-types/src/hmac.rs:414-415`, `hmac.rs:428-429`
- **Severity:** nit
- **Issue:** `Mac::new_from_slice(key).expect("HMAC-SHA256 accepts any key length")` — genuinely infallible for the fixed `&[u8; 32]` key (HMAC only rejects keys larger than the block size), and thus within CLAUDE.md's "invariant" allowance, but these are the crate's only reachable-by-panic `pub fn` bodies on both client and server paths and carry no inline comment naming them as invariants.
- **Suggested fix:** Add the one-line "32 < SHA-256 block size (64) — infallible by construction" comment, or hoist a `Hmac<Sha256>`-per-key construction to make the invariant structural.

### 9. `TableRef` deserialization silently ignores trailing seq elements
- **File:** `crates/shamir-query-types/src/table_ref.rs:71-79`
- **Severity:** nit
- **Issue:** `visit_seq` validates the minimum length (2) but never checks that the sequence ended, so `["repo", "table", "garbage"]` deserializes successfully with the third element dropped — a lenient-accept of malformed input in a crate that is otherwise strict about rejecting ambiguous wire shapes (cf. `de_binary_strict`, `AfterPathIgnored`).
- **Suggested fix:** Read one more element and error (`invalid_length(2, &"exactly 2"`) if it is `Some`, mirroring the existing minimum-length errors.
