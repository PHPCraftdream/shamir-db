<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder-macros — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The previously missing compiler-backend source closes the complexity evidence gap. Three loops perform quadratic shallow prefix work on declared Rust 1.94.0; practical latency remains unknown.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 3 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Quadratic token re-interpolation when accumulating builder chains in loops

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `unverified`.

quote 1.0.45 ToTokens clones the prefix; proc-macro2 1.0.106 unwraps/flushed cloned DeferredTokenStreams. Tagged Rust 1.94.0 concatenation calls push_tree/push_stream; Arc::make_mut and iter().cloned()/extend copy growing top-level prefixes. Hence fixed-size method fragments incur quadratic aggregate prefix work. Nested groups are shared, not universally deep-copied. [Exact backend](https://github.com/rust-lang/rust/blob/1.94.0/compiler/rustc_ast/src/tokenstream.rs#L612).

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:779](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L779); [crates/shamir-query-builder-macros/src/query_parse.rs:810](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L810); [crates/shamir-query-builder-macros/src/query_parse.rs:849](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L849); [Cargo.lock:2537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2537); [Cargo.lock:2660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2660); [rust-toolchain.toml:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/rust-toolchain.toml#L15).

<a id="review-2"></a>

### Claim 2 — Where-clause tokens are captured and re-parsed -- 2x token traffic plus a full copy of every group

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Capture, reconstruction of encountered outer groups and Expr reparsing remain. Group interiors are captured wholesale, not individually reconstructed by this loop. Exact traffic ratios and latency are not established.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:493](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L493); [crates/shamir-query-builder-macros/src/query_parse.rs:508](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L508); [crates/shamir-query-builder-macros/src/query_parse.rs:543](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L543).

<a id="review-3"></a>

### Claim 3 — Per-predicate-call String/Vec micro-allocations in filter lowering

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Callee String, argument-reference Vec, field Strings and selected identifier reconstruction are visible. They are optimization candidates without measured significance; arguments are collected before arity rejection and path length is input-dependent.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L120); [crates/shamir-query-builder-macros/src/filter_lower.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L132); [crates/shamir-query-builder-macros/src/filter_lower.rs:145](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L145); [crates/shamir-query-builder-macros/src/filter_lower.rs:330](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L330).

## Evidence and recipe corrections

- Upgrade structural compiler-backend complexity from unverified to confirmed-open, while removing hundreds-of-milliseconds, small-N immunity, hang and universal deep-copy claims.
- Exact dependency sources checked: quote 1.0.45 src/to_tokens.rs:273; proc-macro2 1.0.106 src/wrapper.rs:75,245 and src/fallback.rs:309. Published labels: https://docs.rs/crate/quote/1.0.45/source/src/to_tokens.rs and https://docs.rs/crate/proc-macro2/1.0.106/source/src/wrapper.rs.
- One-pass fragments must preserve method order, spans, values, evaluation order and terminal return types. This is a source-level complexity improvement, not a measured speedup.
- If field intermediates become Ident values, explicitly materialize string literals before emission; quoting Idents directly would change field names into caller expressions.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
