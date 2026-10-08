<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-builder-macros — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Own-crate concurrency guarantees remain clean: function-local expansion state, no locks, atomics, tasks, I/O or shared mutable registries. The codegen concern is compile-time and backend-dependent, not a runtime lock-free defect.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 0 | 0 | 0 | 0 | 1 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Quadratic token-stream accumulation in `chain = quote! { #chain … }` loops (pillar 3: O(x → 0) — allocation in loops)

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

All three growing-prefix re-quote loops remain. Pinned quote interpolates TokenStream using clone plus stream extension, not universal recursive deep copying. proc-macro2 fallback traverses accumulated tokens; its compiler backend delegates concatenation to rustc. Compiler-backend O(N²), build-hang behavior and interruptibility assertions are not proven.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:775](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L775); [crates/shamir-query-builder-macros/src/query_parse.rs:809](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L809); [crates/shamir-query-builder-macros/src/query_parse.rs:847](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L847); [Cargo.lock:2538](../../../../../Cargo.lock#L2538); [Cargo.lock:2661](../../../../../Cargo.lock#L2661); [rust-toolchain.toml:15](../../../../../rust-toolchain.toml#L15).

Grouping/duplicate: `SUMMARY.md#4.1`. This row is not another independent defect.

## Corrections and qualified non-findings

- No own-crate concurrency, await-held-lock, scc cardinality, hash-policy or resource-lifecycle surface was found; those guarantees remain not-applicable by construction.
- Retain growing-prefix concatenation as an optimization candidate, but do not describe compiler-backend quadratic deep copying, an uninterruptible server or a hung build as established facts.

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
