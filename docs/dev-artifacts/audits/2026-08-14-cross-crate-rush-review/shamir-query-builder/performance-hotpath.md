<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Per-field and per-row codec work, request cloning, fallback serialization and quadratic switch output are directly visible. Their numerical costs and production bottleneck status are unmeasured. The explicit diagnostic roundtrip is intentional functionality.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 5 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `Doc::set` does a full msgpack round-trip per field -- the crate's per-row hot loop

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Every set encodes FilterValue and decodes QueryValue, including scalar literals supported by an existing converter. This proves avoidable work, not medium runtime harm, CPU dominance or two allocations per scalar. Converter equivalence must preserve binary and expression-marker shapes.

Evidence: [crates/shamir-query-builder/src/write/doc.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/doc.rs#L43); [crates/shamir-query-builder/src/macros/mod.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L25); [crates/shamir-query-types/src/filter/filter_value.rs:322](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_value.rs#L322).

<a id="review-2"></a>

### Claim 2 — `rows_as` deserializes via a per-record encode+decode round-trip instead of one batched pass

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The iterator calls deserialize_record for each row, producing repeated buffers/setup. No measured medium impact is available. Raw Vec&lt;QueryRecord&gt; batching is incompatible with current Inserted/IdBytes as_value projections; existing response tests cover Direct rows, not that preservation requirement.

Evidence: [crates/shamir-query-builder/src/response/batch_response_ext.rs:90](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/batch_response_ext.rs#L90); [crates/shamir-query-builder/src/response/batch_response_ext.rs:186](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/batch_response_ext.rs#L186); [crates/shamir-query-types/src/read/query_record.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L58); [crates/shamir-query-types/src/read/query_record.rs:189](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L189); [crates/shamir-query-builder/src/response/tests/response_tests.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/tests/response_tests.rs#L110).

<a id="review-3"></a>

### Claim 3 — `try_build`'s conservative fallback re-serializes `Call`/`Subscribe` ops that are already typed

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Neither operation has a typed collection arm. Call params can be walked directly; Subscribe requires separate source-filter and delivery-time scope analysis, including Records, Keys, Batch and Call. A mechanical all-field walk would preserve existing semantic mistakes.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1313](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1313); [crates/shamir-query-builder/src/batch/batch.rs:1333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1333); [crates/shamir-query-types/src/subscribe/deliver_mode.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/subscribe/deliver_mode.rs#L10).

<a id="review-4"></a>

### Claim 4 — `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

build clones accumulated queries and payloads; SDK execution and to_msgpack use it. No consuming alternative exists. Extra copying is proven, but exact peak-memory doubling is not; retain borrowed build for deliberate reuse.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:868](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L868); [crates/shamir-query-builder/src/batch/batch.rs:886](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L886); [crates/shamir-sdk/src/db.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L139).

<a id="review-5"></a>

### Claim 5 — `Batch::switch` guard construction is O(K^2) deep clones and O(K^2) emitted filter nodes

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each case clones all prior condition trees; summed guard size is quadratic for fixed-size conditions. Documentation omits this cost. No measured harmful case count exists, and a $cond value chain is not automatically equivalent to separate conditional BatchOps.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1033](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1033); [crates/shamir-query-builder/src/batch/batch.rs:1065](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1065); [crates/shamir-query-builder/src/batch/batch.rs:1077](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1077).

<a id="review-6"></a>

### Claim 6 — `to_request_via_msgpack` is a build+encode+decode triple on a public API

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The helper explicitly promises codec roundtripping, notably for tests. Its extra decode is the requested behavior, not redundant implementation of to_msgpack. Public availability alone establishes no misuse or performance defect; panic handling remains separately actionable.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:872](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L872); [crates/shamir-query-builder/src/batch/batch.rs:878](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L878); [crates/shamir-query-builder/src/batch/tests/to_request_via_msgpack_tests.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/tests/to_request_via_msgpack_tests.rs#L12).

## Evidence and recipe corrections

- Findings 1 and 2 are confirmed low-severity optimization opportunities, not measured medium-impact bottlenecks.
- The literal converter borrows and clones strings/binary payloads; it is not a universal zero-allocation move. Arrays containing expressions can require fallback.
- Batching rows reduces per-row buffer/setup frequency, not the amount of row data traversed. Preserve as_value projections, first-failure behavior and decoder-depth boundaries; an extra enclosing sequence can change a boundary case.
- Replacing switch operations with a nested value expression must preserve operation aliases, result shape, dependencies, skip propagation and effects; it is not an established drop-in O(K) recipe.
- No approximately-three-times ratio follows from counting stages. to_msgpack already includes build plus encode.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder -- Performance & O(x->0)

## Summary

The crate is a client-side builder (once-per-request construction, no server hot loop, no locks/concurrency state at all), so the theme's exposure is concentrated in per-field/per-row allocation loops rather than asymptotic blowups on shared structures. The one genuine per-row hot loop is `Doc::set`, which pays a full msgpack encode+decode round-trip for *every field of every record* even though a typed `filter_value_to_query_value` fast path already exists exported from `shamir-query-types` (round-trip-tested there); `BatchResponseExt::rows_as` repeats the same double-codec-per-row pattern on extraction. `try_build`'s documented conservative msgpack fallback also re-serializes `Call`/`Subscribe` ops whose payloads are already typed. No benches exist for this crate (no `benches/` dir), so none of these codec-heavy paths have perf regression coverage; behavioral test coverage (tests/ + per-module tests/) is otherwise solid for the paths reviewed.

## Findings

### 1. `Doc::set` does a full msgpack round-trip per field -- the crate's per-row hot loop

- File:line: `crates/shamir-query-builder/src/write/doc.rs:43-53` (amplified by the `doc!` macro, `src/macros/mod.rs:25-32`, which calls `.set` once per key)
- Severity: high (for this theme)
- Issue: Every `.set(key, value)` call executes `rmp_serde::to_vec_named(&fv)` (heap `Vec` allocation + full serialization of the value tree) followed by `rmp_serde::from_slice` (full decode into a fresh `QueryValue` tree) -- two codec passes and at least two allocations **per field**, discarded immediately. Bulk insert construction (`insert(t).row(doc().set(...).set(...))` for N rows x F fields) therefore pays N*F double codec passes plus 2*N*F transient allocations, dominating the CPU cost of building the request (far exceeding the single `to_msgpack` encode that actually ships it). This is precisely the "allocation in loops / per-row instead of batched+amortized" pattern pillar 3 (O(x->0)) bans, on the crate's central write-path primitive -- and it hits WASM guests hardest, where `shamir-sdk`'s `db.execute` path funnels all builder traffic.
- Failure scenario: No functional failure -- pure waste. A client streaming many small inserts (or a browser WASM guest building large batches) burns single-digit-x CPU and allocator traffic it never needed; `Doc::set` shows up as the hottest builder frame instead of the wire encode.
- Suggested fix: Use the already-exported typed converter `shamir_query_types::filter::filter_value_to_query_value(&fv)` (`shamir-query-types/src/filter/filter_value.rs:322`, mirrored/symmetry-tested in `filter_value_conv_tests.rs`): scalars and nested `Array`s convert directly with zero codec involvement; only when it returns `None` (the expression variants `FieldRef`/`QueryRef`/`FnCall`/`Expr`/`Cond`/`Param`) fall back to the existing msgpack round-trip. The common literal-field case becomes a single-pass move; the round-trip remains the conservative tail for expression defaults. Alternatively add a total, exhaustive `FilterValue -> QueryValue` mapping in `shamir-types` under the same "new variant must compile-error here" convention `batch.rs` already uses for `collect_filter_refs`.

### 2. `rows_as` deserializes via a per-record encode+decode round-trip instead of one batched pass

- File:line: `crates/shamir-query-builder/src/response/batch_response_ext.rs:90-107` (helper `deserialize_record`), applied per row at `:186-189`
- Severity: medium
- Issue: `rows_as<T>` maps `deserialize_record` over every record: each record is individually serialized to msgpack (fresh `Vec` allocation) and decoded into `T`. For an alias with R records that is 2R codec passes, R intermediate byte-buffer allocations, and R decoder setups, where the whole job is two passes over the same bytes. Pillar 3 explicitly prefers "batched + amortized over per-row"; this is a textbook per-row loop with hidden O(N) allocation churn that scales with result-set size (large `SELECT` pages, cursor pages via `fetch_next`).
- Failure scenario: A read returning thousands of rows makes typed extraction (`get_as`/`rows_as`) a measurable client-side bottleneck -- 2x the decode work plus per-row allocator pressure, again amplified inside WASM guests.
- Suggested fix: Encode once, decode once: `let bytes = rmp_serde::to_vec_named(&qr.records)?;` then `rmp_serde::from_slice::<Vec<T>>(&bytes)` -- `Vec<QueryRecord>` is `Serialize`, the wire encoding is identical, error semantics (first failing row surfaces the same `Deserialize` error) are preserved, and cost drops to 2 passes + 1 allocation total. Keep single-record `row_as` as-is.

### 3. `try_build`'s conservative fallback re-serializes `Call`/`Subscribe` ops that are already typed

- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1308-1345` (fallback arm at `:1333-1345`)
- Severity: low
- Issue: The #1093 typed fast path covers `Read`/`Insert`/`Update`/`Set`/`Delete`; every other variant -- deliberately, per the well-documented module rationale -- falls back to `to_vec_named` + `from_slice` into a `QueryValue` tree just to walk for `"$query"` keys. But `BatchOp::Call(CallOp { params: Vec<FilterValue>, .. })` carries exactly the shape `collect_filter_value_refs` (`batch.rs:1228`) already walks exhaustively, and `Subscribe`'s `SubscriptionSource.filter: Option<Filter>` / `DeliverMode::Batch(SubBatchOp { bind: TMap<String, FilterValue>, .. })` are likewise closed typed shapes. The fallback is pure conservatism for these, and `Call` params routinely embed large literals (vector embeddings: dim*4 bytes each), so each `try_build` re-encodes + re-decodes potentially kilobytes of params that the typed walker could inspect directly.
- Failure scenario: None (documented deferral, tracked in #1093); it is per-request validation overhead, not a correctness or unbounded-growth issue -- hence low.
- Suggested fix: Extend the fast path to `BatchOp::Call` (walk `params` via `collect_filter_value_refs`) and `BatchOp::Subscribe` (source filters + `DeliverMode::Batch` bind values), keeping the unconditionally-correct msgpack fallback for the genuinely un-audited admin/DDL variants. Update the #1093 audit note accordingly.

### 4. `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path

- File:line: `crates/shamir-query-builder/src/batch/batch.rs:886-900` (`queries: self.queries.clone()` etc.); reached per request from `to_msgpack` (`:868-870`), `shamir-sdk/src/db.rs:139-141`, `shamir-client/src/interner_cache_ops.rs:201,233,272`, `shamir-client/src/client.rs:402`
- Severity: low
- Issue: `build(&self)` deep-clones the whole accumulated state -- the `queries` `TMap` (every op, doc, filter tree, all `$query` payload values), `return_only`, `interner_epochs` -- so every send pays one full tree copy of its own payload *in addition to* the wire encode. A consuming variant would move the fields with zero copies.
- Failure scenario: None functional; doubles transient peak memory per request and adds a full deep copy per send -- noticeable for large batches and for WASM guests with tight heaps.
- Suggested fix: Add `pub fn into_request(mut self) -> BatchRequest` that moves each field (no clones) and route the encode path through it (`into_msgpack(self)` or have callers do `let req = b.into_request()`); keep `build(&self)` for callers that legitimately reuse the `Batch` (tests, retry loops).

### 5. `Batch::switch` guard construction is O(K^2) deep clones and O(K^2) emitted filter nodes

- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1062-1083` (fold at `:1066-1069`)
- Severity: low
- Issue: For case *i*, the guard folds `.iter().cloned()` over **all** prior conditions, so K cases produce sum(i-1) = O(K^2) full filter-tree deep copies at build time, and the complementary `when` filters emitted on the wire themselves total O(K^2) nodes (guard i embeds i-1 negated prior conditions). With the K=2..5 the sugar targets this is negligible, but nothing documents a ceiling: a 100-case switch silently clones ~5,000 condition trees and bloats the request proportionally.
- Failure scenario: Only pathological use (many cases) -- super-linear build cost and request size growth; no correctness risk.
- Suggested fix: Document a practical case ceiling on `switch` (matching the "builder-only sugar" ADR framing), or for large K emit an O(K) first-match shape (nested `$cond` chain via `val::switch_case`, or an ordered `when`-evaluation convention) instead of per-case negation conjunctions. If K stays small by convention, at minimum note the O(K^2) in the doc comment.

### 6. `to_request_via_msgpack` is a build+encode+decode triple on a public API

- File:line: `crates/shamir-query-builder/src/batch/batch.rs:878-881`
- Severity: nit
- Issue: `build()` deep clone (finding 4) + full msgpack encode + full decode, ~3x the work of `to_msgpack`. The doc scopes it to tests ("notably tests"), but it is `pub` and indistinguishable from a production entry point at the call site; nothing prevents per-request adoption.
- Failure scenario: A caller using it as their send path triples per-request builder/codec cost.
- Suggested fix: Either `#[doc(hidden)]`/move behind `#[cfg(test)]`-adjacent placement with a "test-only, do not ship" doc banner, or rename to make intent unmistakable (e.g. `round_trip_via_msgpack_for_tests`).


</details>
