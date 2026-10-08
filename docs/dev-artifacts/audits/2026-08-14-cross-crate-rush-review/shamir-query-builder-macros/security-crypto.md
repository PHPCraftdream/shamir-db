<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-builder-macros — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

No own-crate crypto, unsafe, secrets or authentication implementation exists. Typed/whitelisted lowering avoids query-text injection. Remaining repository selection and bulk-update issues are developer-facing footguns, not demonstrated authorization bypasses.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 2 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Do not count deliberately supported bulk update as a broken authorization or validation guarantee.

<a id="review-1"></a>

### Claim 1 — `q!(call ...)` silently pins `repo: "main"` with no DSL override

Status: `confirmed-open`. Current risk: `low`.

The emitted repository is still main and reaches the invocation context. Execution still authorizes function access and carries an actor into the database gateway. Wrong default context can affect authorized developer-authored calls; the macro does not itself confer cross-repository permission.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:917](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L917); [crates/shamir-db/src/shamir_db/execute/function_invoker.rs:47](../../../../../crates/shamir-db/src/shamir_db/execute/function_invoker.rs#L47); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:711](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L711); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:732](../../../../../crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L732).

Grouping/duplicate: `SUMMARY.md#1.2`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `q!(update ...)` without `where` generates an unguarded bulk update (`delete` is guarded -- asymmetry)

Status: `not-applicable`. Current risk: —.

WHERE-optional bulk update is explicitly documented, accepted by a registered consumer test, and authorized at execution. An accidental omission is a developer footgun, but this is not an input-validation or authorization defect under the current contract. Requiring an all opt-in is a product/API change, not an outstanding mandatory fix.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:87](../../../../../crates/shamir-query-builder-macros/src/lib.rs#L87); [crates/shamir-query-builder-macros/src/query_parse.rs:614](../../../../../crates/shamir-query-builder-macros/src/query_parse.rs#L614); [crates/shamir-query-builder/src/write/update.rs:105](../../../../../crates/shamir-query-builder/src/write/update.rs#L105); [crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs:522](../../../../../crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs#L522); [crates/shamir-engine/src/table/write_exec.rs:603](../../../../../crates/shamir-engine/src/table/write_exec.rs#L603).

Grouping/duplicate: `SUMMARY.md#3.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — No tests in the crate; the predicate-name whitelist invariant is unpinned

Status: `confirmed-open`. Current risk: `low`.

Unknown-name and wrong-arity rejection remain untested. However, existing registered tests already cover all 17 accepted predicates and dotted field outputs against builder wire shapes; the report's assertion that neither half has any pin is false. No runtime hostile-token ingress is established.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:137](../../../../../crates/shamir-query-builder-macros/src/filter_lower.rs#L137); [crates/shamir-query-builder-macros/src/filter_lower.rs:299](../../../../../crates/shamir-query-builder-macros/src/filter_lower.rs#L299); [crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs:194](../../../../../crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs#L194); [crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs:301](../../../../../crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs#L301); [crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs:383](../../../../../crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs#L383); [crates/shamir-query-builder/src/macros/tests/mod.rs:2](../../../../../crates/shamir-query-builder/src/macros/tests/mod.rs#L2).

Grouping/duplicate: `SUMMARY.md#6.1`. This row is not another independent defect.

## Corrections and qualified non-findings

- The threat model is developer-authored compile-time Rust. Whitelisting is DSL confinement, not protection against arbitrary code execution by an already-untrusted Rust author; RHS expressions are deliberately emitted verbatim.
- The typed-AST/no-query-text-splicing guarantee holds. Not every argument uses Into<FilterValue>: patterns and FTS text use Into<String>, and vector arguments use their concrete builder types.
- Serde default_repo applies during deserialization. Omitting repo from the existing Rust CallOp struct literal would fail compilation; use a constructor that owns the default.
- Requiring WHERE or adding an explicit all keyword for update is a product/API decision and would change currently documented and tested behavior.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder-macros -- Security & crypto boundary

## Summary

This crate is a pure proc-macro DSL compiler (`filter!`, `q!`) that lowers developer-authored compile-time tokens into calls on `shamir-query-builder` / `shamir-query-types`; it contains **no** auth, HMAC/SCRAM/TLS, secret-handling, or `unsafe` code (verified by reading every file), so timing side-channels and crypto-boundary concerns do not apply here. Injection resistance is structural and verified: generated function identifiers are built only via `syn::Ident::new` from hardcoded predicate-name whitelists (unknown callees are rejected, not emitted), all generated paths are crate-absolute (`::shamir_query_builder::...`, `::std::...`, hygiene against local shadowing), every field name is lowered to a string literal (never an ident), and all values are type-checked `Into<FilterValue>` -- there is no string splicing into query text anywhere. The three findings below are boundary-hygiene issues (a silent authz-scope default, an asymmetric destructive-op guard, and an untested core invariant), all **low** severity; no critical/high/medium issues were found for this theme.

## Findings

### 1. `q!(call ...)` silently pins `repo: "main"` with no DSL override
- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:917` (`CallMacro::to_tokens`); grammar at `query_parse.rs:673-708`
- **Severity:** low
- **Issue:** Every other statement form in `q!` accepts a repo-qualified target (`main.users` -> `Query::with_repo`, `query_parse.rs:328-347`), but the `call` form has no repo syntax and hardcodes `repo: ::std::string::String::from("main")` into the generated `CallOp`. Repo is a security-relevant scoping domain in this workspace: transactions are scoped per-repo (`DbRequest::TxBegin { repo }`), admin ops HMAC-canonicalize per-repo (`shamir-query-types/src/hmac.rs`), and replication enforces `denied_repo`/`unknown_repo` -- so the macro silently makes a scoping decision the developer cannot express or see at the call site.
- **Failure scenario:** A developer with per-tenant repos writes `q!(call tenant_cleanup(arg))` intending it to run in the context of `tenant_a`; the emitted `CallOp` executes the WASM proc with `main` as its repository context (wrong data domain), producing either unintended side effects in `main` or a confusing server-side denial -- nothing at the call site signals the pinned default.
- **Suggested fix:** Support a repo-qualified callee (reuse the `parse_table_arg` dotted-pair approach, e.g. `call main.fn_name(...)`), and when unqualified, omit the `repo:` field from the generated struct so the DTO's own `#[serde(default = "default_repo")]` supplies the default; minimally, document the pinned `"main"` in the `q!` doc comment's `call` section.

### 2. `q!(update ...)` without `where` generates an unguarded bulk update (`delete` is guarded -- asymmetry)
- **File:line:** `crates/shamir-query-builder-macros/src/query_parse.rs:614-620` (`UpdateMacro::parse`) vs `query_parse.rs:637-641` (`DeleteMacro::parse`)
- **Severity:** low
- **Issue:** The DSL deliberately hard-requires `where` for `delete` (enforced both in the macro and downstream by `Delete::build()` -> `BuilderError`), but accepts `q!(update <table> set {...})` with no filter, and `Update::build()` permits it by design ("An update without where is valid (updates all records)" -- `shamir-query-builder/src/write/tests/write_tests.rs:290-299`). There is therefore no guard at any layer against a filterless mass update.
- **Failure scenario:** A refactor drops the `where` line from `q!(update users set { "tier" => "gold" } where total > 1000)`; the code still compiles and mass-updates every record in the table on first execution, with no compile-time or build-time signal.
- **Suggested fix:** Mirror the project's own delete precedent at the DSL layer: either require `where` for `q!(update ...)` too, or make an unbounded update an explicit opt-in keyword (e.g. `... set {...} all`) so it is a deliberate, greppable act rather than an omission.

### 3. No tests in the crate; the predicate-name whitelist invariant is unpinned
- **File:line:** `crates/shamir-query-builder-macros/src/` as a whole (no `tests/` directories exist under any module, contrary to the CLAUDE.md "Test organisation" layout of `src/<module>/tests/`)
- **Severity:** low
- **Issue:** The crate's injection resistance rests on one invariant: `lower_predicate_call` emits only fixed identifiers created via `syn::Ident::new` from hardcoded whitelists (`filter_lower.rs:145, 160, 191, 207`; `query_parse.rs:975`) and rejects unknown callee names (`filter_lower.rs:299-307`). No test pins either half of that invariant (whitelist coverage, unknown-name rejection, field-path string-literal emission), so a refactor that interpolates the callee path verbatim or drops the arity checks would compile cleanly and silently remove the confinement.
- **Failure scenario:** (regression) Someone "simplifies" `lower_predicate_call` to quote the user's callee path directly; rustc still catches nonexistent functions, but hygiene/collision behavior changes and unknown-predicate error messages vanish without any test failing.
- **Suggested fix:** Add `src/filter_lower/tests/` (and `src/query_parse/tests/`) per the CLAUDE.md layout, with: one acceptance test per whitelisted predicate asserting the exact emitted constructor, a rejection test for an unknown predicate, a wrong-arity rejection test, and a field-path test asserting dotted paths lower to string-literal arrays (`["address", "city"]`), never idents.

</details>
