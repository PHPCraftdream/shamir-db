<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder-macros — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Own-crate concurrency checks are vacuous. The expansion-cost concern is source-confirmed for the declared compiler backend, without establishing latency or runtime contention.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Quadratic token-stream accumulation in `chain = quote! { #chain … }` loops (pillar 3: O(x → 0) — allocation in loops)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `unverified`.

Exact quote/proc-macro2 sources plus tagged Rust 1.94.0 concatenation show growing flat prefixes repeatedly cloned/extended. This establishes structural quadratic work, not recursive deep copying, build hangs or an interruptibility property.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:775](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L775); [crates/shamir-query-builder-macros/src/query_parse.rs:809](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L809); [crates/shamir-query-builder-macros/src/query_parse.rs:847](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L847); [Cargo.lock:2537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2537); [Cargo.lock:2660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2660); [rust-toolchain.toml:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/rust-toolchain.toml#L15).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

## Evidence and recipe corrections

- The compiler implementation is available as primary tagged source; backend absence no longer justifies an unverified complexity verdict.
- No-lock/no-async findings apply to these three macro implementation files, not arbitrary generated RHS expressions or runtime constructors.
- Do not describe all remaining operations as universally O(tokens-in): recursive lowering, dependency parsing and diagnostic rendering require separate complexity bounds.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder-macros — Concurrency & lock-free invariants

## Summary

This proc-macro crate contains no concurrency primitives at all: no `std::sync::Mutex`/`RwLock`, no `parking_lot`, no `scc`/`dashmap`/`arc_swap`, no atomics, and no `async`/`.await` (`Cargo.toml` declares only `syn`, `quote`, `proc-macro2`). All state across `lib.rs`, `filter_lower.rs`, and `query_parse.rs` is function-local, so the pillar-1/pillar-5 checklist items (lock-free, no locks across `.await`, no O(N) `scc::*::len()` without ack) are satisfied vacuously; there is no `tests/` directory, but with zero shared or locked state there is no concurrency surface to test. The single in-theme issue is a pillar-3 (O(x → 0)) shape violation: codegen accumulates builder chains by re-quoting the entire accumulated token stream inside loops, i.e. quadratic token-copying per macro expansion (compile-time only).

## Findings

### 1. Quadratic token-stream accumulation in `chain = quote! { #chain … }` loops (pillar 3: O(x → 0) — allocation in loops)

- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:809-812` (`lower_doc_map`), `query_parse.rs:847-850` (`InsertMacro::to_tokens`), `query_parse.rs:775-785` (`QueryMacro::to_tokens`, the `order_by` loop)
- **Severity:** low
- **Issue:** Each loop iteration executes `chain = quote! { #chain.set(#key, #val) };` (respectively `.row(...)` per insert doc, `order_by_asc`/`order_by_desc` per order item). `quote!` deep-copies the whole accumulated `TokenStream2` before appending one fragment, so for N accumulated items the expansion performs 1+2+…+N ≈ N²/2 token-tree copies — the classic quote-in-loop O(N²) pattern, exactly the "hidden O(N)/O(N²) in helpers / allocation in loops" shape pillar 3 tells us to avoid. Note the contrast within the same file: `group_by` (line 750) and `select` (line 766) interpolate a `Vec` in a single `quote!` (linear), and `UpdateMacro`/`UpsertMacro`/`DeleteMacro` make a bounded number of appends (fine) — only the three loops above accumulate.
- **Failure scenario:** Compile-time only; no runtime hot path is affected, which is why this is low rather than medium. A wide doc map (hundreds of `"key" => value` pairs) or a bulk `q!(insert into t values {…}, {…}, … × N)` multi-row insert makes expansion cost grow quadratically with authored input. Macro expansion runs single-threaded inside rustc's proc-macro server and cannot be interrupted mid-expansion, so a few-thousand-row bulk-seed script manifests as a build that looks hung rather than merely slow.
- **Suggested fix:** Collect the repeated fragments into a `Vec<TokenStream2>` and interpolate once, e.g. for inserts:
  ```rust
  let rows: Vec<TokenStream2> = self.docs.iter()
      .map(|d| { let ts = lower_doc_map(d); quote! { .row(#ts) } })
      .collect();
  Ok(quote! { #prefix #( #rows )* .build() })
  ```
  Single linear pass; apply the same treatment to the `.set(...)` pairs in `lower_doc_map` and the `order_by` items.

No other findings for this theme: no lock of any kind on any path (pillars 1/5 trivially clean), no `.await` anywhere, no `scc::*::len()` (the crate has no `scc` dependency at all), no hash-keyed structures so pillar 4 (`THasher`/Fx) does not apply, no global/static mutable state, and every operation is O(tokens-in) except the loops flagged above.

</details>
