<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder-macros — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

All 28 claims and eight plans were independently checked. Scoped-token truncation remains refuted. Compiler-backend prefix complexity is now source-confirmed; standalone DTO-coupling severity is unsupported. A raw-identifier mislowering contradicts the historical no-silent-miscompile assurance.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 28 | 24 | 0 | 0 | 0 | 1 | 3 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — Missing sub-stream exhaustion checks: claimed silent token-drop in doc maps / call args / select-fn args does not occur under syn 2.0.114 — residual issue is diagnostic quality + implicit reliance on syn internals

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Generic leftover diagnostics and missing rejection pins remain; the original corruption mechanism is refuted. Syn full-consumption checking is an existing parser guarantee, not evidence of a future dependency failure.

Evidence: [Cargo.lock:4023](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4023); [crates/shamir-query-builder-macros/src/query_parse.rs:574](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L574); [crates/shamir-query-builder-macros/src/query_parse.rs:1014](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L1014).

<a id="review-1-2"></a>

### Claim 1.2 — `q!(call ...)` hardcodes `repo: "main"` with no grammar to override it

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The override/documentation limitation survives, but main matches existing defaults and authorization persists. A default-context-sensitive authorized procedure is necessary for wrong-repository effects.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:917](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L917); [crates/shamir-query-types/src/call/mod.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/call/mod.rs#L13); [crates/shamir-query-builder/src/batch/batch.rs:701](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L701); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:711](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L711).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — Trailing comma in `group_by`/`select`/`order_by` lists rejected with a confusing clause-order error; `peek_clause_keyword_after_comma` contains dead branches

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Terminal commas remain at the outer check. Asc/desc lookahead is reachable, including keyword-named list continuations; it is not dead code.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:252](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L252); [crates/shamir-query-builder-macros/src/query_parse.rs:300](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L300); [crates/shamir-query-builder-macros/src/query_parse.rs:566](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L566).

Grouping/duplicate: [correctness-tdd.md#6](correctness-tdd.md#review-6). This is not an additional independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — Doc drift: "all 19 predicate calls" (there are 17); `vector_similarity_ef`/`_opts` unreachable from the DSL

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The stale count survives. Vector-option functions are available through the builder; adding them to the explicitly limited whitelist is optional feature parity.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L129); [crates/shamir-query-builder-macros/src/filter_lower.rs:299](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L299); [crates/shamir-query-builder/src/filter/leaf.rs:295](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/filter/leaf.rs#L295).

Grouping/duplicate: [correctness-tdd.md#7](correctness-tdd.md#review-7). This is not an additional independent defect.

<a id="review-2-1"></a>

### Claim 2.1 — Quadratic token-stream accumulation in codegen loops

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `unverified`.

The exact dependency/backend proof establishes repeated growing-prefix work for the declared toolchain. This is the same compile-time mechanism, not a concurrency defect or measured hang.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:775](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L775); [crates/shamir-query-builder-macros/src/query_parse.rs:809](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L809); [Cargo.lock:2537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2537); [Cargo.lock:2660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2660); [rust-toolchain.toml:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/rust-toolchain.toml#L15).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — `q!(call ...)` silently pins `repo: "main"` — the security framing

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Wrong default context is possible for an authorized call; the macro does not bypass Execute authorization or change the effective actor's grants.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:917](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L917); [crates/shamir-db/src/shamir_db/execute/function_invoker.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/function_invoker.rs#L47); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:711](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L711).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — `q!(update ...)` without `where` generates an unguarded bulk update (`delete` is guarded — asymmetry)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Explicit grammar and registered tests support filterless UPDATE. Changing that behavior requires a product decision, not mandatory defect remediation.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L87); [crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs:522](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs#L522); [crates/shamir-query-builder/src/write/update.rs:105](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/update.rs#L105).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-3-3"></a>

### Claim 3.3 — The predicate-name whitelist invariant is unpinned by any test

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Only the rejection/arity facet is unpinned. Existing accepted-name and dotted-output tests provide positive counter-evidence to the title's broader assertion.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:299](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L299); [crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs#L194); [crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs:301](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs#L301).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — Quadratic token re-interpolation when accumulating builder chains in loops

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `unverified`.

Published exact dependencies and tagged compiler source establish quadratic shallow prefix work. The report's missing-backend premise no longer holds; quantitative latency and deep-copy assertions still do not follow.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:779](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L779); [crates/shamir-query-builder-macros/src/query_parse.rs:810](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L810); [crates/shamir-query-builder-macros/src/query_parse.rs:849](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L849); [Cargo.lock:2537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2537); [Cargo.lock:2660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2660); [rust-toolchain.toml:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/rust-toolchain.toml#L15).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — Where-clause tokens captured and re-parsed — 2x token traffic plus a full copy of every group

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Extra capture/reparse work is visible. Reconstruction concerns encountered outer groups; exact traffic or recursive copy multipliers remain unsupported.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:493](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L493); [crates/shamir-query-builder-macros/src/query_parse.rs:508](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L508); [crates/shamir-query-builder-macros/src/query_parse.rs:543](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L543).

Grouping/duplicate: [performance-hotpath.md#2](performance-hotpath.md#review-2). This is not an additional independent defect.

<a id="review-4-3"></a>

### Claim 4.3 — Per-predicate-call String/Vec micro-allocations in filter lowering

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The constructions remain. Input-dependent argument/path sizes and unmeasured cost prevent calling them universally constant-bounded or proven bottlenecks.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L120); [crates/shamir-query-builder-macros/src/filter_lower.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L132); [crates/shamir-query-builder-macros/src/filter_lower.rs:330](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L330).

Grouping/duplicate: [performance-hotpath.md#3](performance-hotpath.md#review-3). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — `q!(call ...)` violates the crate's own emitted-path contract: expansion requires a direct `shamir-query-types` dependency

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The direct generated crate references contradict the builder-only promise. Existing consumer tests have that dependency and therefore cannot catch the documented downstream setup failing.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L3); [crates/shamir-query-builder-macros/src/query_parse.rs:912](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L912); [crates/shamir-query-builder-macros/src/query_parse.rs:915](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L915); [crates/shamir-query-builder/Cargo.toml:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/Cargo.toml#L19).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — `q!(call ...)` bypasses the builder layer and hand-assembles the `CallOp` wire DTO, coupling every expansion site to its exact field set

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The literal matches current CallOp and its default. Future field additions require coordinated source changes regardless; unpublished alpha metadata promises no stable field-set evolution. Constructor ownership is optional design consolidation, not a separate present medium defect.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:911](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L911); [crates/shamir-query-types/src/call/mod.rs:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/call/mod.rs#L32); [crates/shamir-query-builder-macros/Cargo.toml:4](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/Cargo.toml#L4); [README.md:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/README.md#L17); [CLAUDE.md:554](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L554).

<a id="review-5-3"></a>

### Claim 5.3 — Clause keywords are silently reserved inside `q!` where/having, contradicting the documented "full `filter!` expression grammar"

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Top-level capture misidentifies field segments and RHS variables as clause boundaries. Grouping avoids it; standalone filter! does not reserve these names.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:499](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L499); [crates/shamir-query-builder-macros/src/query_parse.rs:547](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L547); [crates/shamir-query-builder-macros/src/lib.rs:127](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L127).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Two unaliased `count(*)` items silently produce duplicate `"count"` output keys

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Identical counts collapse redundantly. Mixed projection aliases can overwrite distinct values through the actual aggregate map insertion path. No per-item WHERE semantics exist.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:958](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L958); [crates/shamir-engine/src/query/read/aggregate.rs:826](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/aggregate.rs#L826); [crates/shamir-engine/src/query/read/aggregate.rs:950](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/aggregate.rs#L950); [Cargo.lock:1782](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1782).

Grouping/duplicate: [api-wire-protocol.md#5](api-wire-protocol.md#review-5). This is not an additional independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — `group_by` / `order_by` accept only bare idents — no dotted paths, no string-literal field names

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both restrictions remain. This is loud DSL feature asymmetry. Existing string constructors do not split dots; a nested-ordering extension must preserve segment vectors.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L223); [crates/shamir-query-builder-macros/src/query_parse.rs:265](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L265); [crates/shamir-query-builder/src/val/filter_value.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/val/filter_value.rs#L18); [crates/shamir-query-types/src/read/order_by.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/order_by.rs#L48).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-5-6"></a>

### Claim 5.6 — Doc: "All five forms" — there are six

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Six statement variants exist; three write terminals return Result rather than a bare DTO. Both parts of the overview need correction.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L59); [crates/shamir-query-builder-macros/src/lib.rs:156](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L156); [crates/shamir-query-builder-macros/src/query_parse.rs:171](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L171).

Grouping/duplicate: [api-wire-protocol.md#7](api-wire-protocol.md#review-7). This is not an additional independent defect.

<a id="review-5-7"></a>

### Claim 5.7 — Emitted `::shamir_query_builder` absolute paths break if a downstream renames the dependency

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Canonical-name emission assumes a matching consumer crate name. DTO re-exports cannot resolve an alias-only dependency declaration.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:94](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L94); [crates/shamir-query-builder-macros/src/query_parse.rs:725](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L725).

Grouping/duplicate: [api-wire-protocol.md#8](api-wire-protocol.md#review-8). This is not an additional independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — No error-path test coverage for any diagnostic branch of `filter!` / `q!`; no `trybuild` anywhere in the workspace

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

There is no reachable negative macro harness. Existing accepted-input tests cannot detect rejection-only mutations; neither local absence nor historical green labels establishes past TDD behavior.

Evidence: [crates/shamir-query-builder-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/Cargo.toml#L13); [crates/shamir-query-builder/src/macros/mod.rs:191](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L191); [crates/shamir-query-builder/src/macros/tests/mod.rs:2](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/mod.rs#L2); [scripts/test.sh:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/scripts/test.sh#L177).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — Unknown function-like select item produces a misleading clause-order error

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Unknown call-like select syntax falls through to field parsing and leaves its parentheses for the generic outer-query error.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:433](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L433); [crates/shamir-query-builder-macros/src/query_parse.rs:300](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L300).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-6-3"></a>

### Claim 6.3 — Field-path and alias spans are discarded; downstream errors point at the macro call site

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

String conversion erases identifier spans before literal generation. RHS spans remain. Exact hypothetical downstream error rendering has not been executed.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L333); [crates/shamir-query-builder-macros/src/query_parse.rs:947](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L947); [crates/shamir-query-builder-macros/src/query_parse.rs:1003](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L1003).

Grouping/duplicate: [error-handling-lifecycle.md#3](error-handling-lifecycle.md#review-3). This is not an additional independent defect.

<a id="review-6-4"></a>

### Claim 6.4 — Unbounded recursion in expression lowering; pathological nesting aborts rustc instead of erroring

Status: `unverified`. Current risk: `low`.

Prior-cycle decision: `unverified`.

Local recursion is uncapped, but parsing acceptance, stack threshold and abort manifestation are unproved. The suggested recursive new_spanned diagnostic does not establish safe depth rejection.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L26); [crates/shamir-query-builder-macros/src/filter_lower.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L337); [Cargo.lock:4023](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4023).

Grouping/duplicate: [error-handling-lifecycle.md#4](error-handling-lifecycle.md#review-4). This is not an additional independent defect.

<a id="review-6-5"></a>

### Claim 6.5 — Reused diagnostics carry the wrong context at some call sites

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

HAVING uses WHERE-specific text, and predicate field rejection uses comparison-LHS text. Ordinary rejection remains correct.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L235); [crates/shamir-query-builder-macros/src/query_parse.rs:540](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L540); [crates/shamir-query-builder-macros/src/filter_lower.rs:351](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L351).

Grouping/duplicate: [error-handling-lifecycle.md#5](error-handling-lifecycle.md#review-5). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — Mid-function `use syn::BinOp;` violates "Imports at the top"

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The local import persists without a permitted exception; its impact is convention-only.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L56); [CLAUDE.md:615](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L615).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — "19 predicates" doc count

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Same unchanged 19-versus-17 documentation mismatch.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L129); [crates/shamir-query-builder-macros/src/filter_lower.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L134).

Grouping/duplicate: [correctness-tdd.md#7](correctness-tdd.md#review-7). This is not an additional independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — Zero tests in the crate; nothing points readers to the consumer-side coverage

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No pointer or local harness was added. Consumer registration is correct; the substantive remaining gap is rejection coverage, not where positive tests reside.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L1); [crates/shamir-query-builder/src/macros/tests/mod.rs:2](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/mod.rs#L2).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-7-4"></a>

### Claim 7.4 — `query_parse.rs` is a 1,019-line, five-role file — strains "one file = one primary export"

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The closely coupled private q! group is allowed by the stated rule. Optional splitting is not a missing production fix.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L64); [crates/shamir-query-builder-macros/src/query_parse.rs:1013](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L1013); [CLAUDE.md:498](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L498).

Grouping/duplicate: [style-claude-md.md#4](style-claude-md.md#review-4). This is not an additional independent defect.

<a id="review-7-5"></a>

### Claim 7.5 — Unjustified `pub`, redundant wrapper, duplicated field-path emitter

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The internal hygiene shapes remain without public API leakage or demonstrated divergence. Wrapper removal is optional; both emitters require the same raw-name correction.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L9); [crates/shamir-query-builder-macros/src/filter_lower.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L20); [crates/shamir-query-builder-macros/src/filter_lower.rs:317](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L317); [crates/shamir-query-builder-macros/src/query_parse.rs:1003](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L1003).

Grouping/duplicate: [style-claude-md.md#5](style-claude-md.md#review-5). This is not an additional independent defect.

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 7 | 0 | 0 | 0 | 0 | 1 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No harness was added. Register negative tests in a selected lib target or explicitly select full-mode integration tests. Add builder-only/renamed consumer fixtures and raw-name oracles; accepted-name coverage already exists. Compiler UI diagnostics must not be claimed stable across untested toolchains.

Evidence: [crates/shamir-query-builder-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/Cargo.toml#L13); [crates/shamir-query-builder/src/macros/tests/mod.rs:2](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/mod.rs#L2); [scripts/test.sh:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/scripts/test.sh#L177).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Call paths and override remain unchanged. A builder-owned constructor can fix dependency/name ownership while retaining main, ordered parameters and caller-controlled invocation authority. Re-exports alone do not change emission; omitted Rust fields cannot use serde defaults. Constructor consolidation itself is optional debt.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:911](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L911); [crates/shamir-query-types/src/call/mod.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/call/mod.rs#L41); [crates/shamir-query-builder/src/batch/batch.rs:701](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L701).

<a id="plan-p1-3"></a>

### Plan P1.3 — P1.3

Status: `confirmed-open`.

Prior-cycle decision: `unverified`.

Loops remain unchanged and the structural backend premise is now proven. One-pass method fragments remove repeated prefix reconstruction; retain source evaluation order, segments, spans and .build() return types. No numerical benefit is asserted.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:775](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L775); [crates/shamir-query-builder-macros/src/query_parse.rs:809](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L809); [crates/shamir-query-builder-macros/src/query_parse.rs:847](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L847); [Cargo.lock:2537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2537); [Cargo.lock:2660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2660); [rust-toolchain.toml:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/rust-toolchain.toml#L15).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `not-applicable`.

Prior-cycle decision: `not-applicable`.

Requiring WHERE/all changes explicit, tested bulk-update behavior. It needs a deliberate API decision and is not an outstanding defect fix.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L87); [crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs:522](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs#L522).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Grammar changes remain absent. Preserve path vectors; deliberately consume terminal commas rather than merely remove EOF lookahead. Asc/desc branches are reachable. Cover mixed duplicate aliases and preserve the single count default; silent auto-numbering changes output names. Clause probes must respect expression position.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L223); [crates/shamir-query-builder-macros/src/query_parse.rs:265](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L265); [crates/shamir-query-builder-macros/src/query_parse.rs:499](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L499); [crates/shamir-query-builder-macros/src/query_parse.rs:566](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L566); [crates/shamir-query-builder-macros/src/query_parse.rs:958](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L958); [crates/shamir-query-types/src/read/order_by.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/order_by.rs#L48).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

BinOp remains local. Hoisting is a safe style-only change; it is not runtime remediation.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L56); [CLAUDE.md:615](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L615).

<a id="plan-p2-7"></a>

### Plan P2.7 — P2.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Diagnostic changes remain absent. Explicit exhaustion checks are polish, not corruption repair. A depth guard returning syn 2.0.114 Error::new_spanned(expr) recursively tokenizes the rejected subtree and is not a bounded rejection proof; use a stored primitive span and address parsing/destruction separately.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:433](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L433); [crates/shamir-query-builder-macros/src/query_parse.rs:540](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L540); [crates/shamir-query-builder-macros/src/query_parse.rs:574](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L574); [crates/shamir-query-builder-macros/src/filter_lower.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L26); [Cargo.lock:4023](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4023).

<a id="plan-p2-8"></a>

### Plan P2.8 — P2.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Counts, test pointer and hygiene remain unchanged. Include Result-return documentation. Uniform canonical paths do not fix renaming; file splitting/wrapper removal/vector-option expansion remain optional. Shared literal emission should normalize raw identifiers without turning them into caller expressions.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L59); [crates/shamir-query-builder-macros/src/lib.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L129); [crates/shamir-query-builder-macros/src/filter_lower.rs:317](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L317); [crates/shamir-query-builder-macros/src/query_parse.rs:1003](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L1003).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — Raw identifiers are silently lowered to database names containing the Rust r# escape prefix

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

filter!(r#type == 1) passes the single-segment path guard, then emits field ["r#type"] instead of ["type"]. Likewise q!(from r#users) targets "r#users", and raw projection aliases/callee names retain the prefix. proc-macro2 1.0.106 Display delegates to the compiler or prints r# itself; quoted Strings preserve it literally. This is authored-input semantic mislowering, not remote code injection. Discriminating oracle: compare raw and ordinary spellings for a non-keyword identifier, and raw keyword fields against explicit builder string paths. [Raw identifier identity](https://doc.rust-lang.org/reference/identifiers.html#raw-identifiers).

Evidence: [Cargo.lock:2537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2537); [crates/shamir-query-builder-macros/src/lib.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L19); [crates/shamir-query-builder-macros/src/filter_lower.rs:332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L332); [crates/shamir-query-builder-macros/src/filter_lower.rs:339](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L339); [crates/shamir-query-builder-macros/src/query_parse.rs:719](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L719); [crates/shamir-query-builder-macros/src/query_parse.rs:903](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L903); [crates/shamir-query-builder-macros/src/query_parse.rs:947](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L947); [crates/shamir-query-builder-macros/src/query_parse.rs:1005](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L1005).

## Evidence and recipe corrections

- Replace the compiler-source-unavailable limitation: tagged Rust 1.94.0 primary source establishes the shallow prefix mechanism. It does not establish elapsed latency or process failure.
- Do not count CallOp literal coupling as an independent medium current defect. Its present DTO shape/default is correct; field evolution is prospective alpha compatibility debt.
- The historical 'no live silent-miscompile defect' assurance is false for raw identifiers, even though every cited malformed-group truncation is refuted.
- The current count is a claim-decision ledger, not a unique defect census. Recompute it after complexity-status and DTO-coupling changes; do not reuse historical severity totals.
- The historical syn-1 blanket comparison remains false: published syn 1.0.109 also tracks dropped-buffer leftovers and checks them in Parser::parse2.
- The depth-cap recipe requires bounded diagnostic construction, not Error::new_spanned on an arbitrarily deep AST.
- Stored-procedure positive integration coverage exists, but its native function replacements/System convenience execution do not prove authenticated WASM wrong-repository behavior.
- No source-fixed verdict applies: the relevant macro source and consumer tests are unchanged across the reviewed history.

## Module scope and limitations

Coverage: 8 assigned documents, 60 current claim rows, 8 plan rows, 0 pre-existing observation rows; 1 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [correctness-tdd.md](correctness-tdd.md); [api-wire-protocol.md](api-wire-protocol.md); [concurrency-lockfree.md](concurrency-lockfree.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [performance-hotpath.md](performance-hotpath.md); [security-crypto.md](security-crypto.md); [style-claude-md.md](style-claude-md.md); [SUMMARY.md](SUMMARY.md).

- Strictly read-only: no files written, commands reproduced, builds/tests/benchmarks executed, or agents launched.
- HEAD matched the required base. Relevant inspected implementation, tests, reports and runner files had no working-tree differences; externally changed dependency/toolchain metadata was read from the frozen commit.
- All assigned Markdown, including current ledgers, refinements, corrections and collapsed historical bodies, was inspected. No current Observation rows or assigned TASK_GROUPS.md exist.
- Cached published archives and extracted source were inspected for syn 2.0.114, quote 1.0.45, proc-macro2 1.0.106 and indexmap 2.14.0. Archive SHA-256 values matched the frozen lock checksums. Published syn 1.0.109 was additionally inspected for the historical comparison.
- Rust 1.94.0 was treated as declared metadata only. Its tagged primary source was inspected remotely; no installed compiler version or execution result is asserted.
- Compiler-backend copying complexity is now source-supported, but elapsed time, practical crossover sizes, interruptibility and crash thresholds remain unmeasured.
- Deep-input parsing, lowering, diagnostic construction and destruction are not proven safe; an actual compiler-abort manifestation remains unverified.
- Coverage is bounded claim revalidation, not an exhaustive cross-crate security or Rust Intel audit.

## Guarantee checks

- **Macros work for consumers depending only on shamir-query-builder.** — `diverges`. Call lowering emits absolute shamir_query_types references, unavailable through a merely transitive dependency. Other forms use the builder name, which additionally assumes it was not renamed. Reference: crates/shamir-query-builder-macros/src/lib.rs:3.
- **Malformed scoped doc maps, call arguments and select-function arguments cannot silently lose their trailing tokens.** — `supported`. Scoped buffers share unexpected-token state; their Drop records leftovers and the entry parser checks that state before code generation. Published source: https://docs.rs/crate/syn/2.0.114/source/src/parse.rs. Reference: Cargo.lock:4023; syn 2.0.114 published src/group.rs:79 and src/parse.rs:265,1293.
- **WHERE and HAVING support the full standalone filter expression grammar.** — `diverges`. The q! collector stops on six clause identifiers even in field, dotted-segment or RHS positions. Parenthesized expressions avoid that restriction. Reference: crates/shamir-query-builder-macros/src/lib.rs:127.
- **Identifier lowering preserves the database name denoted by an identifier.** — `diverges`. Raw identifiers retain r# when stringified, although it is not part of their identifier identity. Both proc-macro2 1.0.106 fallback and the declared compiler backend preserve that spelling. See [Rust identifiers](https://doc.rust-lang.org/reference/identifiers.html#raw-identifiers) and [Rust 1.94.0 Ident Display](https://github.com/rust-lang/rust/blob/1.94.0/library/proc_macro/src/lib.rs#L1005). Reference: crates/shamir-query-builder-macros/src/lib.rs:19; crates/shamir-query-builder-macros/src/filter_lower.rs:333.
- **All accepted predicate forms lower to matching typed builder constructors.** — `supported`. The 17 accepted names, arities and emitted argument positions match the actual constructors. String-pattern and vector arguments have their own types; not every argument is Into&lt;FilterValue&gt;. Reference: crates/shamir-query-builder-macros/src/filter_lower.rs:134; crates/shamir-query-builder/src/filter/leaf.rs:29.
- **UPDATE may omit WHERE, while DELETE requires it.** — `supported`. Parsing, builder terminals and registered positive tests implement that explicit asymmetry. Bulk UPDATE is intentional behavior, not an authorization bypass. Reference: crates/shamir-query-builder-macros/src/lib.rs:87; crates/shamir-query-builder-macros/src/query_parse.rs:637.
- **A call's repository is its invocation context, not a grant of repository access.** — `supported`. The emitted repository reaches FnCtx; invocation authorizes Execute on the function and installs a gateway with the effective actor. System/setuid behavior is separate existing authority, not conferred by the macro. Reference: crates/shamir-db/src/shamir_db/execute/function_invoker.rs:47; crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:711.
- **Growing-prefix code generation has quadratic copying on the declared compiler backend.** — `supported`. Source-derived, not measured: quote 1.0.45 clones streams; proc-macro2 1.0.106 flushes cloned deferred prefixes. Rust 1.94.0 uses shallow vector cloning/extension, establishing repeated prefix work for growing flat chains. See [compiler concatenation](https://github.com/rust-lang/rust/blob/1.94.0/compiler/rustc_expand/src/proc_macro_server.rs#L610) and [token-stream mutation](https://github.com/rust-lang/rust/blob/1.94.0/compiler/rustc_ast/src/tokenstream.rs#L612). Reference: Cargo.lock:2537; Cargo.lock:2660; rust-toolchain.toml:15.
- **Registered positive tests establish macro lowering behavior.** — `supported`. The builder lib registers real expansions, differential wire comparisons and independent snapshots. They catch changed operators, segments, argument order and DTO shape, but not rejection-only mutations or builder-only consumer dependency failures. Reference: crates/shamir-query-builder/src/lib.rs:47; crates/shamir-query-builder/src/macros/mod.rs:191; crates/shamir-query-builder/src/macros/tests/mod.rs:2.
- **The ordinary test entry point selects the macro's relevant consumer coverage.** — `supported`. Default lib mode includes registered builder macro tests when that package is selected. Selecting only the macro package does not select consumer tests. Integration harnesses require full mode. Reference: scripts/test.sh:125; scripts/test.sh:177.
- **Macro implementation owns no concurrency, crypto, unsafe or external resource lifecycle.** — `supported`. All three implementation files use local parser/code-generation state. This does not cover dependencies, arbitrary quoted RHS code or generated runtime builders. Reference: crates/shamir-query-builder-macros/Cargo.toml:13; crates/shamir-query-builder-macros/src/lib.rs:9.
- **Typed OQL makes every parser or semantic miscompilation bug impossible.** — `diverges`. No runtime SQL-text concatenation is involved, but q! has a compile-time grammar and name-lowering defects. Typed DTO construction alone does not prove semantic equivalence to authored input. Reference: docs/guide-docs/architecture/ARCHITECTURE.md:407.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- Material disagreement: concurrency-lockfree#1, performance-hotpath#1, SUMMARY#2.1 and SUMMARY#4.1 change unverified -&gt; confirmed-open, and Plan P1.3 changes unverified -&gt; confirmed-open. Exact cached dependencies plus declared Rust 1.94.0 tagged backend source close the prior missing implementation proof. Only shallow structural copying complexity is established.
- Material disagreement: SUMMARY#5.2 changes confirmed-open/medium -&gt; not-applicable/none as a standalone current defect. API#3 remains confirmed-open only for its repository/documentation facet. Current CallOp shape/default is correct; constructor ownership is prospective consolidation.
- Severity refinement: repository-default restrictions and absent optional vector forms do not justify medium correctness/security or low runtime-defect framing by themselves. The concrete dependency-contract violation remains medium.
- New counter-evidence: raw-identifier name mislowering invalidates the historical no-silent-miscompile overview; it is unrelated to syn's correctly rejected leftover scoped tokens.
- New recipe correction: syn 2.0.114 new_spanned recursively prints an Expr. The historical depth-cap rejection recipe can re-enter the same deep tree and does not establish stack-safe rejection.
- Independently upheld: malformed-group truncation is refuted, not fixed; bulk UPDATE and the closely coupled parser file are intentional/permitted; all 17 accepted predicates have real registered positive coverage; negative coverage remains absent.
- No tests were run and no report/source changes were made.

## Current follow-up order

1. Restore the builder-only call expansion contract and add a genuine builder-only consumer oracle; document the existing main default and make repository override a deliberate extension.
2. Normalize identifier-derived database names and pin raw/non-raw equivalence, including keyword fields, dotted segments, tables, aliases and callees.
3. Add registered negative parser/lowering coverage and the missing positive edge cases; select the builder consumer target rather than only the macro crate.
4. Resolve clause-boundary and list-terminator behavior; preserve segment vectors and define duplicate-output-name policy without silent alias renaming.
5. Remove repeated codegen prefix reconstruction while preserving expansion semantics; any practical speed claim requires later authorized measurement.
6. Improve targeted diagnostics and span fidelity. Treat depth safety as a full parse/render/drop obligation, not a lowering-only counter.
7. Correct counts and Result-return documentation; keep import/hygiene repairs small and optional structural refactors separate.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder-macros — Synthesized 7-lens review (consolidation of the 2026-08-14 cross-crate review)

Crate: `crates/shamir-query-builder-macros/` — the proc-macro crate providing the
`filter!` and `q!` DSLs, lowering developer-authored compile-time tokens into
builder calls on `shamir-query-builder` / wire DTOs of `shamir-query-types`
(three files: `lib.rs`, `filter_lower.rs`, `query_parse.rs`; deps: `syn 2`,
`quote`, `proc-macro2` only).

Review basis: the seven 2026-08-14 lens reports under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — read in full and consolidated. Structure/tone calibrated
against the two completed exemplar syntheses (`shamir-client-node/SUMMARY.md`,
`shamir-transport-ipc/SUMMARY.md`). Read-only synthesis; no build, no tests, no
source modifications. One cross-file contradiction (the "silent token-drop"
findings vs. the error-lifecycle non-finding) was resolved by spot-checking the
crate source and the resolved **syn 2.0.114** source (per `Cargo.lock:4024`,
read from the local cargo registry cache) — see finding 1.1, the synthesis's
main correction.

## Executive summary

The crate is architecturally clean — zero `unwrap`/`panic!` on input paths (all
rejections are spanned `syn::Error`s), no concurrency surface at all, verified
happy-path lowering against every real builder signature, and structural style
conformance — and it contains **no live silent-miscompile defect**: the two HIGH
findings asserting that malformed doc-maps/call-args "silently truncate" to a
lossy write op are **false for this repo's syn 2.0.114** (syn's scoped-buffer
drop machinery turns leftover sub-stream tokens into a hard "unexpected token,
expected `}`/`)`" compile error; the lens reviewer who checked syn's source was
right, the two who did not were not). What genuinely needs fixing first is:
(1) **the crate's entire diagnostic half is untested** — ~25 error branches,
zero compile-fail tests, no `trybuild` anywhere in the workspace, which is
exactly the vacuum that let two reviewers believe the silent-drop behavior —
and the same test gap leaves the predicate-whitelist injection invariant
unpinned; (2) the **`q!(call ...)` lowering is the one construct outside the
crate's own architecture contract** — it pins `repo: "main"` with no repo
grammar (a silent, security-relevant scoping decision), emits
`::shamir_query_types` paths that break the documented depends-only-on-
`shamir-query-builder` promise, and hand-assembles the `CallOp` wire DTO as a
struct literal coupled to its exact field set. Fix the test harness, then the
call-lowering cluster; everything else is diagnostics polish and hygiene.

---

## 1. correctness-tdd

### 1.1 [HIGH as filed → **LOW, corrected during synthesis**] Missing sub-stream exhaustion checks: claimed silent token-drop in doc maps / call args / select-fn args does not occur under syn 2.0.114 — residual issue is diagnostic quality + implicit reliance on syn internals
*(primary: `correctness-tdd.md` finding 1; the same root is the api-wire lens's
finding 1 — deduped here. Severity corrected high → low; see "Correction"
below.)*
- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:572-588`
  (`parse_doc_map`), `:685-700` (`CallMacro::parse` args loop), `:375-436`
  (`parse_select_item` count/sum/avg/min/max/agg_fn/func branches), `:481-488`
  (`parse_dotted_ident_from`); consumed by insert/update/upsert (`:590-671`).
- **Claim carried forward from both source files (original detail, not
  weakened):** every sub-parser consumes a delimited group element-by-element
  with a `if content.peek(Token![,]) { .. } else { break }` loop and never
  verifies `content` is empty; the reviewers asserted that leftover tokens are
  silently discarded, so e.g. `q!(insert into users values { "name" => "Alice"
  "age" => 30 })` (missing comma) would compile cleanly as a **one-field
  insert** (`"age" => 30` vanishing), `q!(call f(1 2))` would yield
  `params: [1]`, `count(age 5)` / `count(a, b)` / `agg_fn("m", a, b)` /
  `func("n", [x] junk)` would silently ignore trailing tokens — silent field
  loss on a database write, "the worst failure mode for a write path."
- **Correction (verified against source, this is the synthesis's call on the
  cross-file contradiction):** `error-handling-lifecycle.md`'s "Non-findings"
  section explicitly contradicts this, claiming syn 2's `check_unexpected`
  machinery surfaces leftover tokens. That reviewer is **right**. Verified
  against the resolved syn 2.0.114 (`Cargo.lock:4024`):
  1. Every cited site uses the scoped macros (`syn::braced!` at
     `query_parse.rs:574`; `syn::parenthesized!` at `:379`, `:400`, `:412`,
     `:426`, `:687`) — syn's `parse_delimited` gives the `content` buffer the
     **parent's shared `unexpected: Rc<Cell<Unexpected>>`** (syn
     `src/group.rs:87-88`).
  2. `impl Drop for ParseBuffer` (syn `src/parse.rs:265-275`): a scoped buffer
     dropped **with tokens remaining** records `Unexpected::Some(span,
     scope_delimiter)` into that shared chain.
  3. Both proc-macro entries route through `parse_macro_input!`
     (`query_parse.rs:1014` `q_macro`, `filter_lower.rs:11` `filter_macro`) →
     `Parser::parse2` (syn `src/parse.rs:1293-1305`), which runs
     `state.check_unexpected()?` after the parse fn returns `Ok` →
     `err_unexpected_token` → **"unexpected token, expected `}`" /
     "`)`"** (`src/parse.rs:1328-1336`), spanned at the leftover token.
  So every claimed truncation shape (`{ "a" => 1 "b" => 2 }`, `call f(1 2)`,
  `count(age, 5)`, `count(*, x)`, `agg_fn("m", a, b)`, `func("n", [x] junk)`,
  and missing-comma inserts/updates/upserts) **fails to compile**; no wire op
  is emitted, nothing is silently dropped. The workspace SUMMARY.md's
  "silent write miscompilation (dropped fields)" verdict text inherits this
  error.
- **Residual issue (why this survives as low, not deleted):** (a) the compile
  error is generic — "expected `}`" hints at a brace problem when the real
  defect is a missing comma — vs. the targeted message an explicit check would
  give; (b) the guarantee is *implicit*: it rests on syn's Drop-chain
  internals, not on any local check or comment (syn 1 silently ignored
  leftovers; a future hand-rolled group-slicing rewrite would too); (c) no
  test pins any of it — the compile-fail vacuum (finding 6.1) is precisely why
  two reviewers could believe the silent-drop story unchallenged.
- **Suggested fix (amended):** keep the source files' fix as
  defense-in-depth + DX — after each loop/single parse add
  `if !content.is_empty() { return Err(content.error("q!: expected `,` between doc-map pairs")) }`
  (or parse trailing `syn::parse::Nothing`), giving precise messages instead
  of syn's generic one. Add compile-fail fixtures for each site asserting the
  rejection (see 6.1) — these double as executable documentation of the
  syn-2 semantics this crate implicitly depends on.

### 1.2 [MEDIUM] `q!(call ...)` hardcodes `repo: "main"` with no grammar to override it
*(primary for the repo-pin defect; also flagged as `correctness-tdd.md`
finding 2, `security-crypto.md` finding 1, and the repo bullet of
`api-wire-protocol.md` finding 3 — deduped here.)*
- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:911-918`
  (emission; hardcode at `:917`), grammar `:673-708`; contrast
  `:328-347` (`parse_table_arg` repo-qualified tables → `with_repo`); builder
  precedent `crates/shamir-query-builder/src/batch/batch.rs:686-714`
  (`Batch::call` / `Batch::call_in_repo`); pinned by consumer tests
  `crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs:606-648`.
- **Issue:** every other statement form accepts a repo-qualified target
  (`from main.users`, `insert into main.users`, …), but `call` has no repo
  syntax and unconditionally emits
  `repo: ::std::string::String::from("main")` into the generated `CallOp`.
  Repo is a security-relevant scoping domain in this workspace: transactions
  are scoped per-repo (`DbRequest::TxBegin { repo }`), admin ops
  HMAC-canonicalize per-repo (`shamir-query-types/src/hmac.rs`), and
  replication enforces `denied_repo`/`unknown_repo` — the macro makes a
  scoping decision the developer cannot express or see at the call site.
- **Failure scenario:** a developer with per-tenant repos writes
  `q!(call tenant_cleanup(arg))` intending tenant_a's context; the emitted
  `CallOp` executes the WASM proc with repo `main` (wrong data domain) —
  unintended side effects in `main` or a confusing server-side denial, with
  nothing at the call site signaling the pinned default. Non-security framing
  (correctness lens): a stored proc reading `vault.secrets` is unreachable
  from `q!` without hand-building `CallOp`, violating the builder-only rule
  the macros exist to enforce; the existing tests enshrine `"main"`.
- **Suggested fix:** extend the grammar (e.g. `call main.fn_name(...)` or
  `call fn_name in_repo r(...)`) mapping onto a `with_repo`-style construction
  (`call_in_repo`); when unqualified, omit the `repo:` field so the DTO's own
  `#[serde(default = "default_repo")]` supplies the default; at minimum
  document the pinned `"main"` in the `q!` doc's `call` section. Landing this
  together with 5.1/5.2 (same emission site) restores a single invariant for
  all six statement forms.

### 1.3 [LOW] Trailing comma in `group_by`/`select`/`order_by` lists rejected with a confusing clause-order error; `peek_clause_keyword_after_comma` contains dead branches
*(as filed: `correctness-tdd.md` finding 6.)*
- **File:line:** `query_parse.rs:561-567`, used at `:224`, `:252`, `:276`.
- **Issue:** when the fork finds end-of-input (or `asc`/`desc`) after a comma,
  the helper returns true and the loop breaks *without consuming the comma*,
  so the stray comma survives to the final `!input.is_empty()` check and
  yields "unexpected tokens after query; clauses must appear in order…". Doc
  maps *do* allow trailing commas (pinned by `q_insert_trailing_comma`,
  `q_macro_tests.rs:686-695`), so list syntax is inconsistent with map syntax.
  The `fork.peek(kw::asc)`/`kw::desc` arms are unreachable for any sensible
  input (the `order_by` loop errors on a missing direction first).
- **Failure scenario:** `q!(from users select a, b,)` errors with a message
  about clause order rather than about the comma.
- **Suggested fix:** consume the trailing comma before breaking (consistent
  with doc maps), or delete the `fork.is_empty()`/asc/desc arms so the loop
  stops only at real clause keywords.

### 1.4 [LOW] Doc drift: "all 19 predicate calls" (there are 17); `vector_similarity_ef`/`_opts` unreachable from the DSL
*(primary: `correctness-tdd.md` finding 7; also flagged as `style-claude-md.md`
finding 2 — deduped here, counted once at the higher filed severity.)*
- **File:line:** `src/lib.rs:129-130` vs. `src/filter_lower.rs:134-297` (17
  predicates; enumerated identically at `filter_lower.rs:124-127`, `:302-305`
  and `lib.rs:32-40`); builder-side gap at
  `crates/shamir-query-builder/src/filter/leaf.rs:295`, `:313`.
- **Issue:** the `q!` doc claims 19 predicate forms; the lowering implements
  17. Additionally `filter::vector_similarity_ef` and
  `filter::vector_similarity_opts` exist in the builder but have no
  `filter!`/`q!` predicate form, so they are only reachable by hand-building
  filters.
- **Failure scenario:** none at runtime; doc/feature-surface drift misleads
  DSL users about coverage.
- **Suggested fix:** correct the count (or drop it); either add the two vector
  predicates to the lowering or note their absence.

### Positive notes (kept for calibration parity)
All emitted paths were verified against real builder signatures and match
(`filter::*`, `query::Query::*`, `write::*`, `select::*`,
`shamir_query_types::call::CallOp`); the consumer-side differential tests
(macro output vs. builder output over msgpack/QueryValue round-trips, plus 3
wire snapshots, ~88+ invocations in `shamir-query-builder/src/macros/tests/`)
are genuine, non-vacuous green-path coverage. Correctness-tdd findings 4 and 5
(dotted `group_by`/`order_by`, reserved clause keywords) are carried under
5.5/5.3 below — same roots, fuller write-ups in the api-wire lens.

## 2. concurrency-lockfree

**General verdict: vacuously clean.** The crate has no concurrency primitives
at all: no `std::sync::Mutex`/`RwLock`, no `parking_lot`, no
`scc`/`dashmap`/`arc_swap`, no atomics, no `async`/`.await` (deps are `syn`,
`quote`, `proc-macro2` only). All state is function-local, so the pillar-1/5
checklist items are satisfied vacuously; no hash-keyed structures so pillar 4
does not apply; no global/static mutable state. With zero shared or locked
state there is no concurrency surface to test. The single in-theme finding is
shared with the perf lens:

### 2.1 [MEDIUM, primary: 4.1] Quadratic token-stream accumulation in codegen loops
*(primary write-up at 4.1; deduped — `concurrency-lockfree.md` finding 1 filed
the same three loops as low.)*

## 3. security-crypto

**General verdict: no crypto surface, injection resistance structurally sound.**
The crate contains no auth, HMAC/SCRAM/TLS, secret-handling, or `unsafe` code
(verified file-by-file by the lens reviewer). Injection resistance rests on one
invariant, verified in source: generated function identifiers are built only
via `syn::Ident::new` from hardcoded predicate-name whitelists
(`filter_lower.rs:145`, `:160`, `:191`, `:207`; `query_parse.rs:975`), unknown
callees are rejected not emitted (`filter_lower.rs:299-307`), all generated
paths are crate-absolute (`::shamir_query_builder::...`, `::std::...` —
hygiene against local shadowing), every field name is lowered to a string
literal (never an ident), and values are type-checked `Into<FilterValue>` —
no string splicing into query text anywhere. Findings:

### 3.1 [LOW, primary: 1.2] `q!(call ...)` silently pins `repo: "main"` — the security framing
*(deduped into 1.2: security lens flags the same hardcode as an unexpressible,
invisible authz-scope decision; per-tenant cross-repo execution scenario
there.)*

### 3.2 [LOW] `q!(update ...)` without `where` generates an unguarded bulk update (`delete` is guarded — asymmetry)
*(as filed: `security-crypto.md` finding 2.)*
- **File:line:** `query_parse.rs:614-620` (`UpdateMacro::parse`) vs
  `:637-641` (`DeleteMacro::parse`); downstream confirmation:
  `shamir-query-builder/src/write/delete.rs:90-93`
  (`where_clause: ...ok_or(BuilderError::MissingWhereClause)?`) vs
  `shamir-query-builder/src/write/tests/write_tests.rs:290-299` ("An update
  without where is valid (updates all records)").
- **Issue:** the DSL hard-requires `where` for `delete` (macro + builder), but
  accepts `q!(update <table> set {...})` filterless, and `Update::build()`
  permits it by design — no layer guards a filterless mass update.
- **Failure scenario:** a refactor drops the `where` line from
  `q!(update users set { "tier" => "gold" } where total > 1000)`; the code
  still compiles and mass-updates every record on first execution, with no
  compile-time or build-time signal.
- **Suggested fix:** mirror the delete precedent at the DSL layer: require
  `where` for `q!(update ...)` too, or make unbounded update an explicit
  opt-in keyword (e.g. `... set {...} all`) so it is a deliberate, greppable
  act.

### 3.3 [LOW, primary: 6.1] The predicate-name whitelist invariant is unpinned by any test
*(deduped into 6.1 — the security lens's specific stake in the test gap: no
test pins whitelist coverage, unknown-name rejection, or field-path
string-literal emission, so a refactor interpolating the callee path verbatim
or dropping arity checks would compile cleanly and silently remove the
confinement.)*

## 4. performance-hotpath

Compile-time-only crate (two `#[proc_macro]` entry points; no runtime state),
so the O(x→0) lens applies to **rustc expansion cost**: every `filter!`/`q!`
site pays this code on each build. Behavioral coverage lives downstream and
nothing measures expansion cost — acceptable at realistic N, but the quadratic
loop would not be caught if a generated-code consumer scaled it up.

### 4.1 [MEDIUM] Quadratic token re-interpolation when accumulating builder chains in loops
*(primary: `performance-hotpath.md` finding 1; also flagged as
`concurrency-lockfree.md` finding 1 — deduped here.)*
- **File:line:** `query_parse.rs:775-785` (`order_by` loop), `:807-813`
  (`lower_doc_map`), `:841-853` (`InsertMacro::to_tokens` row loop).
- **Issue:** inside these loops the emitted chain is rebuilt with
  `chain = quote! { #chain.order_by_asc(...) }` / `#chain.set(#key, #val)` /
  `#chain.row(#doc_ts)`. `quote!` deep-copies every token of the interpolated
  `#chain` into a fresh `TokenStream2`, so for K iterations the macro performs
  sum(1..K) token copies — O(K²) — plus K discarded intermediate allocations.
  Costs compound in `insert`: each `.row()` iteration re-copies the
  already-built doc-map chains of all previous rows. Exactly the "hidden
  O(N²), allocation in loop" class pillar 3 bans — relocated to compile time.
  The inconsistency is visible in-file: `select` (`:760-766`) and `group_by`
  (`:744-750`) use the linear `Vec<TokenStream2>`-splice pattern; so do
  `UpdateMacro`/`UpsertMacro`/`DeleteMacro` (bounded appends).
- **Failure scenario:** a bulk `q!(insert into t values {..}, {..}, ...)` with
  hundreds/thousands of docs (or machine-produced queries with long
  `order_by` lists) turns linear expansion into hundreds of ms+ of repeated
  token copying per macro site, inflating workspace build time. Expansion runs
  single-threaded inside rustc's proc-macro server and cannot be interrupted
  mid-expansion — a few-thousand-row bulk-seed script manifests as a build
  that looks *hung* rather than slow. Typical hand-written queries (K ≤ ~20)
  are unaffected — hence medium, not high.
- **Suggested fix:** collect per-item fragments into a `Vec<TokenStream2>` and
  interpolate once (mirror the in-file `select`/`group_by` pattern):
  `let parts: Vec<_> = docs.iter().map(|d| { let ts = lower_doc_map(d); quote! { .row(#ts) } }).collect();`
  then `quote! { #chain #( #parts )* .build() }`. Same shape for doc-map
  `.set()` pairs and `order_by_asc`/`order_by_desc` items. Expansion becomes
  O(total tokens).

### 4.2 [NIT] Where-clause tokens captured and re-parsed — 2x token traffic plus a full copy of every group
- **File:line:** `query_parse.rs:493-544` (`parse_filter_expr`).
- **Issue:** the where/having expression is scanned token-tree by token-tree
  into a new `TokenStream2`; each paren/bracket/brace group is re-parsed
  (`content.parse::<TokenStream>()`) and re-copied into a newly constructed
  `Group` (solely to `set_span(span.join())` — verified valid against syn 2's
  `extra::DelimSpan`), then the rebuilt stream is parsed a second time as
  `syn::Expr`. Linear (group interiors are captured wholesale), but every
  where-clause token is handled twice and each nested group copied once more.
- **Failure scenario:** none functionally; constant-factor compile-time tax on
  where-heavy `q!` sites.
- **Suggested fix:** if expansion cost ever shows in profile, parse the raw
  group token-trees directly (or use speculative parsing with a
  clause-keyword terminator); keep only if the joined-span normalization is
  load-bearing.

### 4.3 [NIT] Per-predicate-call String/Vec micro-allocations in filter lowering
- **File:line:** `filter_lower.rs:120` (callee `ident.to_string()`),
  `:145/160/191/207` (`syn::Ident::new(&name, ...)` re-allocated from that
  String), `:132` (`Vec<&Expr>` collect), `:317-327` (`field_path`
  `Vec<String>`).
- **Issue:** every predicate call allocates a `String` for the callee name,
  then re-allocates an equivalent `Ident` per match arm; `field_path`
  allocates a `Vec<String>` per comparison. Compile-time-only,
  constant-bounded — negligible; recorded because the pattern is avoidable.
  (Distinct from 6.3: that finding is about span fidelity, this about cost.)
- **Failure scenario:** none.
- **Suggested fix:** match the `Ident` directly against literals
  (`Ident: PartialEq<str>`) and emit `p.path.segments[0].ident` (cloned)
  instead of round-tripping `String` → `Ident::new`; build field paths from
  `Ident`s and convert during `quote!`.

## 5. api-wire-protocol

The macro surface is well-aligned with the runtime builder: every emitted
path was cross-checked against `shamir-query-builder` and matches an existing
public constructor with compatible arity, and `q!`/`filter!` output is
wire-shape-tested against hand-built builder equivalents (all 17 predicates,
all 6 statement forms, msgpack snapshots) downstream. The real gaps are the
`call`-lowering family (5.1/5.2 here, 1.2 in correctness) and grammar
asymmetries.

### 5.1 [MEDIUM] `q!(call ...)` violates the crate's own emitted-path contract: expansion requires a direct `shamir-query-types` dependency
*(as filed: `api-wire-protocol.md` finding 2.)*
- **File:line:** `src/lib.rs:1-5` (contract) vs `query_parse.rs:911-918`
  (violation).
- **Issue:** the crate doc states: "These macros emit **fully-qualified paths**
  (`::shamir_query_builder::...`) so they work from any crate that depends on
  `shamir-query-builder`." The read/insert/update/delete/upsert lowerings
  honor this; `CallMacro::to_tokens` alone emits
  `::shamir_query_types::call::CallOp { ... }` and
  `::std::convert::Into::<::shamir_query_types::filter::FilterValue>::into(...)`.
  `shamir-query-builder` deliberately re-exports wire DTOs so guests don't
  need `shamir-query-types` (`shamir-query-builder/src/lib.rs:66-68`) and
  re-exports the macros themselves (`:79`), but does **not** re-export
  `CallOp` or `FilterValue` (verified: only `FnCall`, via
  `val/filter_value.rs:7`).
- **Failure scenario:** an external consumer (WASM guest, SDK user) whose
  `Cargo.toml` lists only `shamir-query-builder` — exactly the setup the doc
  promises to support — writes `q!(call my_proc(1))` and gets an
  unresolved-crate compile error pointing at macro-generated tokens, the least
  actionable error site. Inside this workspace every `q!` user happens to also
  depend on `shamir-query-types`, so the trap is invisible locally.
- **Suggested fix:** (a) add `pub use shamir_query_types::call::CallOp;` and
  `pub use shamir_query_types::filter::FilterValue;` to `shamir-query-builder`'s
  root and emit `::shamir_query_builder::...` paths in `CallMacro`
  (preferred — restores a single invariant for all six forms), or (b) amend
  `src/lib.rs:1-5` to document the exception and the extra dependency.

### 5.2 [MEDIUM] `q!(call ...)` bypasses the builder layer and hand-assembles the `CallOp` wire DTO, coupling every expansion site to its exact field set
*(as filed: `api-wire-protocol.md` finding 3, minus the repo-pin bullet deduped
into 1.2; same emission site, complementary failure mode.)*
- **File:line:** `query_parse.rs:899-921` (struct-literal construction);
  grammar doc `src/lib.rs:102-111`; DTO
  `shamir-query-types/src/call/mod.rs:31-43` (not `#[non_exhaustive]`), wire
  default `:13-15`.
- **Issue:** all five other statement forms lower into `shamir-query-builder`
  constructors; `call` alone hand-assembles the raw wire DTO via struct
  literal. Consequences beyond the repo pin: (a) the struct literal
  hard-couples every expansion site to `CallOp`'s exact field set — adding a
  field breaks all downstream `q!(call ...)` uses with raw struct-literal
  errors, and the macro would keep emitting a stale literal even if the DTO's
  serde `default_repo()` ever changed (silently overriding the wire default
  instead of inheriting it); (b) this is the one place the builder-only
  construction rule (CLAUDE.md "Query construction — builder only", incl. its
  "state why in a comment" requirement) is bypassed without a justification
  comment.
- **Failure scenario:** a future `CallOp` field addition becomes a
  workspace-wide compile break at every expansion site; meanwhile users
  needing anything beyond the hardcoded literal must hand-build `CallOp` —
  re-introducing exactly the hand-assembled wire op the DSL exists to prevent.
- **Suggested fix:** emit the DTO through a single constructor (e.g.
  `CallOp::new(name, params)` / a builder-level `call`/`call_in_repo` free
  function) that the macro calls, so field-set evolution has one owner; if
  the struct literal is kept deliberately, add the one-line "why" comment the
  convention requires. Natural to land together with 1.2 and 5.1.

### 5.3 [LOW] Clause keywords are silently reserved inside `q!` where/having, contradicting the documented "full `filter!` expression grammar"
*(primary: `api-wire-protocol.md` finding 4; also flagged as
`correctness-tdd.md` finding 5 — deduped here.)*
- **File:line:** `query_parse.rs:497-501` (terminator check) and `:547-554`
  (`is_clause_keyword`); doc claim `src/lib.rs:125-130`.
- **Issue:** `parse_filter_expr` stops at `select`/`group_by`/`having`/
  `order_by`/`limit`/`offset` anywhere at token depth 0 — including when those
  tokens are *field names*. `q!(from users where limit == 5)` breaks
  immediately ("expected a filter expression after `where`"),
  `q!(... where status == 1 && order_by == 2)` produces a misleading raw-syn
  "expected expression", and even a dotted segment `where a.select == 1`
  breaks. The same names work in standalone `filter!` (whose `field_path` has
  no reserved words), so the doc's "Both use the full `filter!` expression
  grammar" overpromises. Field names are arbitrary strings in a document DB,
  so `limit`/`offset` as field names are plausible.
- **Failure scenario:** always a loud compile error (no silent corruption —
  checked), but the user filtering a field named `limit` gets an error pointing
  at the wrong thing; parenthesized uses (`where (limit > 5)`) work, making
  the failure extra confusing.
- **Suggested fix:** minimum — document the reserved-word set in the `q!`
  grammar section and note the `filter!` escape hatch; emit a targeted
  "field name collides with a clause keyword; parenthesize it" error when the
  scan breaks on a lopsided buffer. Better — only treat a keyword as a
  terminator when not immediately preceded by `.` and when it begins a
  syntactically plausible clause (fork-probe "keyword + rest parses as
  clause").

### 5.4 [LOW] Two unaliased `count(*)` items silently produce duplicate `"count"` output keys
*(as filed: `api-wire-protocol.md` finding 5.)*
- **File:line:** `query_parse.rs:958-966` (implicit `"count"` alias), grammar
  `src/lib.rs:136`; underlying constructor
  `shamir-query-builder/src/select/select_item.rs:83-87` accepts any alias
  without uniqueness validation.
- **Issue:** `count(*)` is the only select item with an optional alias,
  defaulting to `"count"`. `q!(from users select count(*), count(*))` lowers
  to two `SelectItem::CountAll { alias: Some("count") }` entries — an
  ambiguous projection whose result-map keys collide at execution time. Every
  other aggregate requires an explicit `as alias`; the default exists only for
  the single-`count(*)` idiom, but nothing enforces that.
- **Failure scenario:** user writes two `count(*)` items; the wire op carries
  two identical output keys and the result silently collapses to one.
- **Suggested fix:** error (or auto-number `count`, `count_2`, …) when a
  second alias-less `count(*)` appears in one projection; a duplicate-alias
  check across all select items would close the whole class.

### 5.5 [LOW] `group_by` / `order_by` accept only bare idents — no dotted paths, no string-literal field names
*(primary: `api-wire-protocol.md` finding 6; also flagged as
`correctness-tdd.md` finding 4 — deduped here.)*
- **File:line:** `query_parse.rs:219-230` (`group_by`:
  `input.parse::<Ident>()`), `:261-282` (`order_by`), `:441-452` (`select`
  fields, which *do* support `a.b`).
- **Issue:** `select` items and where-LHS support dotted field paths, and
  tables accept string literals (`from "user-events"`), but `group_by a.b` and
  `order_by address.city desc` are parse errors (group_by stops at `.` then
  trips the trailing-tokens check; order_by fails with "expected `asc` or
  `desc`", pointing at the dot), and no field position accepts a string
  literal — a hyphenated or non-ident field name cannot be projected, grouped,
  or ordered at all. The builder accepts dotted paths
  (`Query::group_by_many` takes `IntoFieldPath`; `order_by_asc/desc` take
  `Into<String>`), and the `q!` doc defines `<field>` via the select section
  where `field or a.b` is explicit.
- **Failure scenario:** loud compile errors, no wire risk — but users must
  abandon the DSL for the raw builder for a dotted sort/group key, and the
  order_by error misdirects toward the direction keyword.
- **Suggested fix:** reuse `parse_dotted_ident_from` for `group_by` and
  per-item field parsing in `order_by` (lowering to `["a","b"]` paths the
  builders already accept), and consider accepting string literals for field
  names in all field positions, mirroring `<table>`.

### 5.6 [NIT] Doc: "All five forms" — there are six
- **File:line:** `src/lib.rs:59-60`; grammar documents six statement types
  (from/insert/update/delete/upsert/call) and the AST has six variants.
- **Suggested fix:** "All six forms".

### 5.7 [NIT] Emitted `::shamir_query_builder` absolute paths break if a downstream renames the dependency
- **File:line:** `query_parse.rs:64, 70-72, 725, 732, 808, 843-894, 940-997`
  (all emissions); `filter_lower.rs` throughout.
- **Issue:** expansions reference the builder by canonical crate name, so a
  downstream `shamir-query-builder = { package = "...", rename }` breaks every
  expansion. Standard proc-macro limitation; crate is `publish = false`,
  workspace-internal, and the re-export at
  `shamir-query-builder/src/lib.rs:79` mitigates discovery — but since
  `CallMacro` (5.1) must touch this decision anyway, it is the natural moment
  to standardize on re-exported-path emission for all lowerings.
- **Suggested fix:** no action needed now; if touching 5.1, emit all DTO paths
  through `::shamir_query_builder` re-exports uniformly.

## 6. error-handling-lifecycle

**General verdict: exemplary on the Result/panic axis; the test vacuum is the
crate's one high.** Every fallible path returns `syn::Result`; both entries use
`parse_macro_input!` + `to_compile_error()` (spanned compile errors, never
panic-on-input); zero `unwrap()`/`expect()`/`panic!`/`todo!`/`unimplemented!`
in real code (the only `.unwrap()`s are inside ```ignore```-fenced doc
examples; `doctest = false`). Resource lifecycle is trivially satisfied: a
proc-macro crate owns no runtime resources (no files, locks, tasks, channels).
`thiserror`/`anyhow` rules are N/A by construction (`syn::Error` is the error
type). Verified-clean non-finding carried forward: leftover tokens in nested
`parenthesized!`/`braced!` buffers are flagged by syn 2's `check_unexpected`
machinery — "not silent, not a panic; no finding" (the basis for the 1.1
correction, now source-verified).

### 6.1 [HIGH] No error-path test coverage for any diagnostic branch of `filter!` / `q!`; no `trybuild` anywhere in the workspace
*(primary: `error-handling-lifecycle.md` finding 1; the same root is
`correctness-tdd.md` finding 3 (TDD framing), `security-crypto.md` finding 3
(whitelist-invariant framing), and `style-claude-md.md` finding 3 (placement/
discoverability framing) — deduped here, counted once.)*
- **File:line:** `Cargo.toml:1-19` (no `[dev-dependencies]`, no `[[test]]`);
  `src/` has no `tests/` directory at all; all coverage is consumer-side
  (`crates/shamir-query-builder/src/macros/tests/{filter_macro_tests.rs,
  q_macro_tests.rs}` — 100% happy-path differential + snapshot testing; layout
  there conforms, `tests/mod.rs` is manifest-only). ~25 distinct error paths
  untested: unknown statement keyword (`query_parse.rs:197-199`), `order_by`
  missing `asc`/`desc` (`:273`), clause-order violation (`:300-305`), empty
  `where` (`:539-541`), insert/update/delete/upsert/call trailing-token errors
  (`:601-603`, `:621-623`, `:644-646`, `:662-664`, `:702-704`), `delete`
  without `where` (`:637-641`), required-alias errors (`:469-478`), unknown
  predicate (`filter_lower.rs:299-307`), per-predicate arity errors
  (`:137-141`, `:153-157`, `:168-172`, `:183-187`, `:199-203`, `:215-218`,
  `:231-234`, `:246-250`, `:263-266`, `:280-285`), unsupported binary/unary
  operators (`:83-86`, `:107-111`), unsupported-expression catch-all
  (`:43-49`), non-path predicate callee (`:121-129`), invalid/tuple-index
  field path (`:343-346`, `:349-353`). Also uncovered green branches:
  `count(*)` without `as` (`:958-966` — undocumented default),
  `write_table_tokens`' string-literal-table arm (`:822-831`), the
  doc-promised bare-variable RHS in `filter!` (`lib.rs:23-24`). The repo's own
  release audit already acknowledges the gap
  (`docs/dev-artifacts/research/2026-07-17-release-audit/08-test-coverage-ci-robustness.md:260`:
  "no trybuild/UI tests pinning" macro diagnostics).
- **Issue:** the crate's entire product is compile-time diagnostics, yet not
  one error branch is pinned — violating CLAUDE.md's Red/Green/Refactor
  protocol for exactly the half of the DSL that is error behavior. Any
  refactor can silently change a diagnostic's text, trigger site, or turn a
  rejection into an acceptance with the gate green. This vacuum is also
  load-bearing for the review itself: it is why the false silent-token-drop
  HIGHs (1.1) went unchallenged across two lens reports, and the
  predicate-whitelist injection invariant (3.3) has no pin either — a refactor
  interpolating the callee path verbatim or dropping arity checks would
  compile cleanly and silently remove the confinement.
- **Failure scenario:** a refactor changes `is_clause_keyword`/
  `peek_clause_keyword_after_comma` or one arity check; a malformed `q!(...)`
  that today produces a clean compile error starts being accepted (or vice
  versa) and CI stays green.
- **Suggested fix:** add a `trybuild` dev-dependency (to this crate or the
  consumer; cargo permits cyclic dev-deps, so pointing at
  `shamir-query-builder` re-exports avoids a workspace cycle concern) plus a
  `tests/compile_fail/*.rs` fixture per branch listed above, each asserting
  the expected diagnostic string; include the token-drop shapes from 1.1 as
  fixtures asserting today's "unexpected token" rejection (executable
  documentation of the syn-2 reliance), and the 3.3 whitelist tests (one
  acceptance test per whitelisted predicate asserting the exact emitted
  constructor, unknown-predicate rejection, wrong-arity rejection, field-path
  → string-literal array). Follow the CLAUDE.md `tests/` layout
  (manifest-only `mod.rs`, `filter_error_tests.rs`/`q_error_tests.rs`) and run
  through `./scripts/test.sh`. Add one line to the crate docs pointing readers
  to `shamir-query-builder/src/macros/tests/` (style lens's framing).

### 6.2 [LOW] Unknown function-like select item produces a misleading clause-order error
- **File:line:** `query_parse.rs:433-435` (fallthrough), error surfaced at
  `:300-305`.
- **Issue:** in `parse_select_item`, an `ident(...)` whose name is not one of
  `count|sum|avg|min|max|agg_fn|func` falls through to plain-field parsing
  (`_ => {}`), which consumes only the ident and leaves the call parens
  unconsumed; the select-items loop then breaks (next token is `(`, not `,`),
  and the enclosing `!input.is_empty()` check fires with "q!: unexpected
  tokens after query; clauses must appear in order…". Input is correctly
  rejected (no silent acceptance — syn's group-buffer machinery also flags the
  leftovers), but the message blames clause order when the real problem is an
  unrecognized function name.
- **Failure scenario:** `q!(from t select myfunc(x))` — user hunts for a
  misplaced clause instead of checking the supported select-function list.
- **Suggested fix:** in the `_` arm, when the fork confirmed `Ident` followed
  by `Paren`, return a targeted error immediately:
  "q!: unknown select function `{id}`; use count, sum, avg, min, max, agg_fn,
  or func".

### 6.3 [LOW] Field-path and alias spans are discarded; downstream errors point at the macro call site
- **File:line:** `filter_lower.rs:317-327` (`field_path`), `:330-355`
  (`collect_field_segments` collects `String`s); `query_parse.rs:1003-1011`
  (`segments_to_field_path`), `:947`, `:974`, `:987`, `:994` (alias
  `to_string()`).
- **Issue:** LHS field segments and select aliases are flattened to `String`
  and re-emitted via `quote!`, so generated string literals carry
  `Span::call_site()` instead of the user's ident spans. The RHS is quoted
  verbatim (spans preserved), but a downstream type error landing on the
  generated field-path/alias token — e.g. a `filter::*`/`select::*` signature
  mismatch after a builder API change — is attributed to the whole macro
  invocation rather than the specific field token.
- **Failure scenario:** `filter!(status == "active")` fails because `filter::eq`'s
  field parameter changed; rustc underlines the entire `filter!(...)` call
  with no pointer to `status`, unlike the RHS which underlines correctly.
- **Suggested fix:** preserve identity spans when materializing the literal:
  `syn::LitStr::new(&s, ident.span())` instead of `id.to_string()` +
  `quote!{ #s }` in `field_path`, `segments_to_field_path`, and the alias
  sites. Mechanical, no behavior change.

### 6.4 [LOW] Unbounded recursion in expression lowering; pathological nesting aborts rustc instead of erroring
- **File:line:** `filter_lower.rs:26-51` (`lower` recursion), `:330-348`
  (`collect_field_segments` recursion).
- **Issue:** both recurse once per nesting level of user input with no depth
  cap. A pathologically deep filter (hundreds of thousands of nested
  parens/negations) can overflow the proc-macro thread stack — an abrupt rustc
  abort (stack overflow), not a spanned compile error; the classic proc-macro
  crash class that motivated syn's own recursion guard for types. Input is
  first-party code, so this is hardening, not an exploit vector — but note it
  is the compile-time miniature of the workspace's worst finding (funclib
  `is_json` stack-overflow abort).
- **Failure scenario:** a generated or copy-pasted filter with extreme nesting
  wedges the developer's build with an OS-level stack-overflow message and no
  file/line attribution.
- **Suggested fix:** thread a `depth: usize` through `lower`/
  `collect_field_segments`, returning
  `Err(syn::Error::new_spanned(expr, "filter!: expression is too deeply
  nested"))` past a cap (e.g. 128). Cheap; converts the worst case into an
  ordinary diagnostic.

### 6.5 [NIT] Reused diagnostics carry the wrong context at some call sites
- **File:line:** `query_parse.rs:539-541` ("after `where`" fires from the
  `having` path at `:233-239`); `filter_lower.rs:349-353` ("LHS of comparison"
  fires for predicate-call field arguments, e.g. `like("status", ...)` via
  `:143`).
- **Issue:** `parse_filter_expr`'s empty-token error hardcodes "after
  `where`" but also serves `having`; `collect_field_segments`' catch-all
  hardcodes "LHS of comparison must be a field name" but also validates the
  field argument of every predicate call. Rejection behavior is correct; only
  the message text misleads in secondary contexts.
- **Suggested fix:** parameterize the context word
  (`parse_filter_expr(input, clause: &str)`) and give the predicate-argument
  path its own message ("predicate field must be an ident or dotted field
  path"), or split into two thin wrappers.

## 7. style-claude-md

**Largely conformant.** `lib.rs` holds only thin `#[proc_macro]` delegates
plus rustdoc; each macro's logic lives in its own sibling file; no `mod.rs`
files to police; the `// ── section ──` banner/comment discipline is clean; no
stray debug files; no inline `#[cfg(test)] mod tests` in implementation files.

### 7.1 [MEDIUM] Mid-function `use syn::BinOp;` violates "Imports at the top"
*(as filed: `style-claude-md.md` finding 1.)*
- **File:line:** `src/filter_lower.rs:56` (inside `fn lower_binary`).
- **Issue:** CLAUDE.md ("📦 Imports at the top") requires all `use` statements
  in the file header, "never inside a function or block body", with three
  documented exceptions (test `use super::*`, trait-for-one-method with a
  collision comment, `cfg`-gated bodies). None applies: `BinOp` is an enum
  used for pattern matching, hoisting collides with nothing (no other `BinOp`
  in the file), and the body is not macro-generated/`cfg`-gated.
- **Failure scenario:** none functional; it normalizes the exception and
  erodes the auditability of the convention (`grep '^use '` header checks
  silently miss it).
- **Suggested fix:** merge into the header import
  (`use syn::{parse_macro_input, BinOp, Expr};`) and delete line 56. One line.

### 7.2 [LOW, primary: 1.4] "19 predicates" doc count
*(deduped into 1.4 — style lens's framing: reader stops trusting the doc; the
error strings in `filter_lower.rs` are the authoritative enumeration.)*

### 7.3 [LOW, primary: 6.1] Zero tests in the crate; nothing points readers to the consumer-side coverage
*(deduped into 6.1 — style lens adds the discoverability fix: one line in the
crate docs naming `shamir-query-builder/src/macros/tests/` as where the
behavioral coverage lives, and noting that placement outside the proc-macro
crate is legitimate because emitted tokens reference `::shamir_query_builder`,
unresolvable from inside this crate.)*

### 7.4 [LOW] `query_parse.rs` is a 1,019-line, five-role file — strains "one file = one primary export"
*(as filed: `style-claude-md.md` finding 4.)*
- **File:line:** `src/query_parse.rs:1-1019`.
- **Issue:** the file holds the `kw` keyword module (`:38-60`), ~10 private
  AST types (`:64-178`), six `Parse` impls (`:182-708`), token-buffering
  helpers (`:493-567`), and all codegen (`:710-1019`). Within the letter of
  CLAUDE.md this is one closely-coupled group serving a single macro with all
  types private, so it is not a violation — but the rule's stated motivation
  ("atomic diffs and meaningful `git blame`") is strained: Read-grammar edits
  and Insert codegen land in one file.
- **Failure scenario:** blame noise and merge contention as the DSL grows.
- **Suggested fix (optional):** split into `q_ast.rs`/`q_parse.rs`/`q_gen.rs`
  siblings with `lib.rs` wiring them — only as a dedicated task, honouring "no
  new files unless the task genuinely needs them".

### 7.5 [NIT] Unjustified `pub`, redundant wrapper, duplicated field-path emitter
*(as filed: `style-claude-md.md` finding 5.)*
- **File:line:** `filter_lower.rs:20-22, 317`; `query_parse.rs:1003-1011`.
- **Issue:** (a) `pub fn field_path` (`filter_lower.rs:317`) has no caller
  outside its own file — `query_parse.rs` uses its own
  `segments_to_field_path` instead — so it should be private (since
  `mod filter_lower;` is private it never leaks, but minimal visibility
  expresses intent). (b) `lower_expr` (`:20-22`) is a zero-value wrapper
  around `lower`. (c) The output-emission half is duplicated between
  `field_path`/`collect_field_segments` (single segment → bare string, multi →
  `[a, b]`) and `segments_to_field_path`; the tuple-index rejection exists
  only in the former, and the two can drift.
- **Failure scenario:** the two emitters diverge silently (one gains
  validation/quoting, the other does not).
- **Suggested fix:** make `field_path` private (or have `query_parse.rs`
  reuse it), inline the `lower_expr` wrapper, and consolidate the
  segment-array emission into one helper.

---

## Finding counts

| Severity | Lens-tagged findings | Distinct defects after dedup (finding numbers) |
|---|---|---|
| critical | 0 | — |
| high | 3 | 6.1 (zero error-path tests; filed high once, cross-tagged in 3 more lenses) |
| medium | 6 | 5 — 1.2 (`call` repo pin: correctness + security + api-wire), 4.1 (quadratic codegen: perf + concurrency), 5.1 (path-contract leak), 5.2 (DTO coupling), 7.1 (mid-function `use`) |
| low | 16 | 11 — 1.1 (token-drop pair, **corrected high→low**), 1.3 (trailing comma), 1.4 (doc 19 vs 17: correctness + style), 3.2 (unguarded bulk update), 5.3 (reserved clause keywords: api-wire + correctness), 5.4 (duplicate `count(*)`), 5.5 (dotted group/order paths: api-wire + correctness), 6.2 (misleading unknown-fn error), 6.3 (span loss), 6.4 (unbounded recursion), 7.4 (five-role file) |
| nit | 7 | 6 — 4.2 (2x where-capture), 4.3 (micro-allocs), 5.6 ("five forms"), 5.7 (renamed dep), 6.5 (wrong-context messages), 7.5 (pub/wrapper/dup emitter) |
| **total** | **32** | **23 distinct defects** |

- Lens-tagged row matches the workspace `SUMMARY.md` per-crate breakdown for
  this crate (0c / 3h / 6m / 16l / 7n = 32, pre-dedup, severities as filed).
- **Synthesis correction:** the deduped census moves the 1.1 token-drop pair
  (filed HIGH in two lenses; the workspace scorecard's "silent write
  miscompilation (dropped fields)" verdict rests on them) to **low**, based on
  source-verified syn 2.0.114 behavior (see 1.1). Corrected deduped census:
  **0 critical, 1 high, 5 medium, 11 low, 6 nit = 23 distinct defects**.
- Dedup groups counted once: correctness#1+api-wire#1 (→1.1); correctness#2 +
  security#1 + api-wire#3-repo (→1.2); correctness#3 + security#3 +
  error-lifecycle#1 + style#3 (→6.1); concurrency#1+perf#1 (→4.1);
  correctness#5+api-wire#4 (→5.3); correctness#4+api-wire#6 (→5.5);
  correctness#7+style#2 (→1.4). api-wire#3 was split rather than merged: its
  repo bullet joins 1.2, its DTO-coupling body stands as 5.2 (same emission
  site, distinct failure mode and fix).

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Build the error-path test harness (Red for the whole diagnostic half).**
   Add `trybuild` (in this crate or, to avoid cycle concerns, the consumer)
   with a compile-fail fixture per ~25 diagnostic branch, asserting expected
   messages; include the token-drop shapes from 1.1 (asserting today's
   "unexpected token" rejection — this both pins the syn-2 reliance and would
   have falsified the silent-drop claim immediately), plus the 3.3 whitelist
   acceptance/rejection/field-path-literal pins and the uncovered green
   branches (`count(*)` default alias, string-literal tables, bare-variable
   RHS). Closes **6.1** (the crate's only high) and materially de-risks
   **1.1, 3.2, 5.3, 5.4, 6.2, 6.5** regressions.
2. **Make `q!(call ...)` a first-class lowering.** Repo-qualified callee
   grammar (`call main.name(...)`) mapping onto `call_in_repo`-style
   construction (unqualified → omit `repo:` so the wire default applies);
   emit through a single constructor so `CallOp` field evolution has one
   owner; re-export `CallOp`/`FilterValue` from `shamir-query-builder` and
   emit `::shamir_query_builder` paths (builder-crate half of the edit);
   add the "why" comment if the struct literal is kept. Closes **1.2, 5.1,
   5.2** — the wrong-repo execution scenario is the one live
   silent-misbehavior risk found in this crate.

**P1 — soon**
3. **Linearize codegen:** Vec-splice the three accumulating loops
   (`order_by`, `lower_doc_map`, insert rows), mirroring the in-file
   `select`/`group_by` pattern. Closes **4.1** (and its concurrency-lens
   duplicate).
4. **Guard the bulk-update footgun:** require `where` for `q!(update ...)` or
   add an explicit `all` opt-in keyword, mirroring the delete precedent.
   Closes **3.2** (needs a small product decision on which form).
5. **Grammar-consistency cluster:** dotted paths for `group_by`/`order_by`
   (closes **5.5**); consume trailing commas in lists + delete the dead
   `peek_clause_keyword_after_comma` arms (closes **1.3**); duplicate
   alias-less `count(*)` guard (closes **5.4**); document the reserved-word
   set and/or fork-probe clause terminators (closes **5.3**).
6. **Hoist the mid-function `use syn::BinOp;`** into the file header. One
   line; closes **7.1**.

**P2 — backlog**
7. **Diagnostic polish:** explicit `!content.is_empty()` checks with
   missing-comma messages at the 1.1 sites (defense-in-depth + better errors);
   targeted unknown-select-function error (**6.2**); parameterized context
   words in reused diagnostics (**6.5**); `LitStr::new(&s, ident.span())` for
   field/alias literals (**6.3**); depth cap in `lower`/
   `collect_field_segments` (**6.4**).
8. **Doc/hygiene:** "19"→"17" + note on `vector_similarity_ef`/`_opts`
   (**1.4**); "five forms"→"six" (**5.6**); pointer to consumer-side tests in
   crate docs (**7.3**, if not already covered by item 1); renamed-dependency
   limitation note or uniform re-exported-path emission (**5.7**);
   visibility/dup-emitter cleanup (**7.5**); optional `query_parse.rs` split
   into ast/parse/gen (**7.4**); profile-driven only: where-clause
   re-capture (**4.2**) and predicate-lowering micro-allocations (**4.3**).

</details>
