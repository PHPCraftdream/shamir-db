# shamir-query-builder-macros -- Performance & O(x->0)

## Summary

This crate is compile-time-only (two `#[proc_macro]` entry points; no runtime
state, no locks, no buffers), so the O(x->0) lens applies to **rustc
expansion cost**: every `filter!`/`q!` site pays this code on each build.
One genuine hidden O(N^2) exists -- the `chain = quote! { #chain.<m>(...) }`
accumulation pattern in three loops (order_by items, doc-map pairs, insert
rows) re-copies the entire accumulated token stream every iteration; the same
file already uses the linear Vec-splice pattern for `select`/`group_by`, so
the fix is in-repo precedent. Everything else is linear (the where-clause
token capture is 2x traffic but deliberate for span normalization), plus a
handful of compile-time micro-allocations that are negligible. Behavioral
macro coverage lives downstream in `shamir-query-builder/src/macros/tests/`
(~88 invocations across all statements/predicates); nothing measures
expansion cost, which is acceptable at realistic N but means the quadratic
loop would not be caught if a generated-code consumer scaled it up.

## Findings

### 1. Quadratic token re-interpolation when accumulating builder chains in loops

- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:775-785` (`order_by` loop), `:807-813` (`lower_doc_map`), `:847-850` (`InsertMacro::to_tokens` row loop)
- **Severity:** medium
- **Issue:** Inside these loops the emitted chain is rebuilt with
  `chain = quote! { #chain.order_by_asc(...) }` /
  `#chain.set(#key, #val)` / `#chain.row(#doc_ts)`. `quote!` deep-copies
  every token of the interpolated `#chain` into a freshly allocated
  `TokenStream2`, so for K iterations the macro performs
  sum(1..K) token copies -- O(K^2) -- plus K discarded intermediate
  allocations. The costs compound in `insert`: each `.row()` iteration
  re-copies the already-built doc-map chains of all previous rows. This is
  exactly the "hidden O(N^2), allocation in loop" class pillar 3 bans --
  relocated to compile time. Notably, `to_tokens` in the same file uses the
  linear pattern for `select` items (`:761-766`) and `group_by`
  (`:744-750`) (collect into `Vec<TokenStream2>`, splice once), so the
  inconsistency is visible in-file.
- **Failure scenario:** A generated bulk insert such as
  `q!(insert into t values {..}, {..}, ...)` with hundreds/thousands of docs
  (or machine-produced queries with long `order_by` lists) turns a linear
  expansion into hundreds of ms+ of repeated token copying per macro site,
  inflating workspace build time. Typical hand-written queries (K <= ~20)
  are unaffected.
- **Suggested fix:** Collect per-item parts and splice once, mirroring the
  existing `select`/`group_by` code:
  `let parts: Vec<_> = docs.iter().map(|d| { let ts = lower_doc_map(d); quote! { .row(#ts) } }).collect();`
  then `quote! { #chain #( #parts )* }` (same shape for doc-map `.set()`
  pairs and `order_by_asc`/`order_by_desc` items). Expansion becomes O(total tokens).

### 2. Where-clause tokens are captured and re-parsed -- 2x token traffic plus a full copy of every group

- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:493-544` (`parse_filter_expr`)
- **Severity:** nit
- **Issue:** The where/having expression is scanned token-tree by
  token-tree into a new `TokenStream2`; each paren/bracket/brace group is
  re-parsed (`content.parse::<TokenStream>()`) and re-copied into a newly
  constructed `Group` (solely to `set_span(span.join())`), then the rebuilt
  stream is parsed a second time as `syn::Expr`. Cost is linear (group
  interiors are captured wholesale, not rescanned), but every where-clause
  token is handled twice and each nested group is copied once more.
- **Failure scenario:** None functionally; a constant-factor compile-time
  tax on where-heavy `q!` sites.
- **Suggested fix:** If expansion cost ever shows up in profile, parse the
  raw group token-trees directly (or use speculative parsing with a
  clause-keyword terminator) instead of rebuilding groups; keep only if the
  joined-span normalization is load-bearing.

### 3. Per-predicate-call String/Vec micro-allocations in filter lowering

- **File:line:** `crates/shamir-query-builder-macros/src/filter_lower.rs:120` (callee `ident.to_string()`), `:145/160/191/207` (`syn::Ident::new(&name, ...)` re-allocated from that String), `:132` (`Vec<&Expr>` collect), `:317-327` (`field_path` `Vec<String>`)
- **Severity:** nit
- **Issue:** Every predicate call in a `filter!`/`q!` expansion allocates a
  `String` for the callee name, then re-allocates an equivalent `Ident`
  from it in each match arm; `field_path` allocates a `Vec<String>` per
  comparison. All compile-time-only and constant-bounded per call, so
  negligible at realistic filter sizes -- recorded only because the
  pattern can be avoided outright.
- **Failure scenario:** None.
- **Suggested fix:** Match the `Ident` directly against literals
  (`Ident: PartialEq<str>`) and emit `p.path.segments[0].ident` (cloned)
  instead of round-tripping through `String` -> `Ident::new`; build field
  paths from `Ident`s and convert during `quote!`.
