<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-builder — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The allocation/codec mechanisms remain present and no builder benchmarks exist. The claimed latency multipliers and exact allocation counts are unsupported. The proposed raw Vec<QueryRecord> batching rewrite is not semantics-preserving for all row variants.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 6 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `Doc::set` does a full msgpack round-trip per field -- the crate's per-row hot loop

Status: `confirmed-open`. Current risk: `medium`.

Every set still encodes FilterValue into a byte buffer and decodes QueryValue. The exported literal converter remains unused here. This proves avoidable per-field codec work, not CPU dominance, single-digit-x slowdown, or two allocations for every scalar.

Evidence: [crates/shamir-query-builder/src/write/doc.rs:43](../../../../../crates/shamir-query-builder/src/write/doc.rs#L43); [crates/shamir-query-builder/src/macros/mod.rs:25](../../../../../crates/shamir-query-builder/src/macros/mod.rs#L25); [crates/shamir-query-types/src/filter/filter_value.rs:322](../../../../../crates/shamir-query-types/src/filter/filter_value.rs#L322); [crates/shamir-query-types/src/filter/tests/mod.rs:2](../../../../../crates/shamir-query-types/src/filter/tests/mod.rs#L2); [crates/shamir-query-types/src/filter/tests/filter_value_conv_tests.rs:152](../../../../../crates/shamir-query-types/src/filter/tests/filter_value_conv_tests.rs#L152).

<a id="review-2"></a>

### Claim 2 — `rows_as` deserializes via a per-record encode+decode round-trip instead of one batched pass

Status: `confirmed-open`. Current risk: `medium`.

rows_as still invokes deserialize_record once per row, allocating intermediate bytes repeatedly. Batching can amortize setup, but serializing qr.records directly changes Inserted and IdBytes semantics because the current helper serializes as_value(), not QueryRecord itself.

Evidence: [crates/shamir-query-builder/src/response/batch_response_ext.rs:90](../../../../../crates/shamir-query-builder/src/response/batch_response_ext.rs#L90); [crates/shamir-query-builder/src/response/batch_response_ext.rs:186](../../../../../crates/shamir-query-builder/src/response/batch_response_ext.rs#L186); [crates/shamir-query-types/src/read/query_record.rs:58](../../../../../crates/shamir-query-types/src/read/query_record.rs#L58); [crates/shamir-query-types/src/read/query_record.rs:189](../../../../../crates/shamir-query-types/src/read/query_record.rs#L189); [crates/shamir-query-types/src/write/inserted_record.rs:30](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L30); [crates/shamir-query-builder/src/response/tests/response_tests.rs:110](../../../../../crates/shamir-query-builder/src/response/tests/response_tests.rs#L110).

<a id="review-3"></a>

### Claim 3 — `try_build`'s conservative fallback re-serializes `Call`/`Subscribe` ops that are already typed

Status: `confirmed-open`. Current risk: `low`.

Only Read/Insert/Update/Set/Delete receive typed arms. Call and Subscribe still pay encode/decode before walking. Any Subscribe extension must respect delivery-time scope and inspect all relevant delivery forms rather than blindly traversing nested bodies.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1313](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1313); [crates/shamir-query-builder/src/batch/batch.rs:1333](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1333); [crates/shamir-query-builder/src/batch/batch.rs:1228](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1228); [crates/shamir-query-builder/src/batch/subscribe.rs](../../../../../crates/shamir-query-builder/src/batch/subscribe.rs).

<a id="review-4"></a>

### Claim 4 — `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path

Status: `confirmed-open`. Current risk: `low`.

build still clones accumulated fields and to_msgpack serializes that clone. SDK execution and interner-cache callers still use build. No consuming request/encoding alternative exists. Extra copying is proven; an exact peak-memory doubling is not.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:868](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L868); [crates/shamir-query-builder/src/batch/batch.rs:886](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L886); [crates/shamir-sdk/src/db.rs:139](../../../../../crates/shamir-sdk/src/db.rs#L139); [crates/shamir-client/src/interner_cache_ops.rs:201](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L201); [crates/shamir-client/src/interner_cache_ops.rs:233](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L233); [crates/shamir-client/src/interner_cache_ops.rs:272](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L272).

<a id="review-5"></a>

### Claim 5 — `Batch::switch` guard construction is O(K^2) deep clones and O(K^2) emitted filter nodes

Status: `confirmed-open`. Current risk: `low`.

Every case clones prior condition trees into its guard, yielding quadratic aggregate output/construction work for fixed-size conditions. Documentation still omits this scaling. It is a property of the chosen wire encoding, not evidence of measured harmful latency.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1033](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1033); [crates/shamir-query-builder/src/batch/batch.rs:1065](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L1065).

<a id="review-6"></a>

### Claim 6 — `to_request_via_msgpack` is a build+encode+decode triple on a public API

Status: `confirmed-open`. Current risk: `nit`.

The helper still clones via build, encodes, and decodes. Its documentation already explicitly describes round-tripping and test use. Relative to to_msgpack, the additional stage is decode; the asserted approximately-three-times cost is unmeasured.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:868](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L868); [crates/shamir-query-builder/src/batch/batch.rs:872](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L872); [crates/shamir-query-builder/src/batch/batch.rs:878](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L878).

## Corrections and qualified non-findings

- Finding 1's original High is not supported as a runtime severity without workload measurements; the structural optimization opportunity is confirmed.
- Scalar decoding need not allocate a new heap tree, so the universal 2*N*F transient-allocation lower bound is false. The borrowed converter clones strings/binaries and allocates arrays; it is not universally a zero-allocation move.
- Both current and proposed rows_as implementations traverse all row data. Batching reduces buffers/setup, not O(N) data processing or an established twofold decode cost.
- QueryRecord::Inserted serialization injects record identity whereas as_value returns cloned fields; IdBytes serializes binary whereas as_value returns Null. Preserve the current projection when batching and add variant-specific regression tests.
- Existing response tests are reachable through response/mod.rs and response/tests/mod.rs and cover ordinary Direct rows and malformed fields, not the proposed variant-preservation requirement.
- No benches or benchmark target exist in this crate. Claims of bottlenecks, CPU dominance, WASM amplification, and numerical speedups remain unverified.
- The fallback's 'unconditionally-correct' label must be removed; correctness findings 1 and 3 positively contradict it.

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
