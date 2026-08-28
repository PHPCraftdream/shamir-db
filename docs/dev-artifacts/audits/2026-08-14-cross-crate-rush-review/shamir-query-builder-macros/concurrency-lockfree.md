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
