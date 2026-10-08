<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder-macros — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The boundary is authored compile-time Rust, not hostile runtime query text. Typed lowering and authorization counter-evidence support the qualified non-findings; optional bulk UPDATE is intentional.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 2 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `q!(call ...)` silently pins `repo: "main"` with no DSL override

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Main reaches invocation context, but function Execute authorization and effective-actor gateway access remain. Cross-repository effects depend on the authorized procedure's behavior and grants. Batch::call_in_repo avoids the limitation.

Evidence: [crates/shamir-query-builder-macros/src/query_parse.rs:917](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L917); [crates/shamir-db/src/shamir_db/execute/function_invoker.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/function_invoker.rs#L47); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:711](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L711); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:732](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L732).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — `q!(update ...)` without `where` generates an unguarded bulk update (`delete` is guarded -- asymmetry)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

WHERE-optional UPDATE is explicit, registered and supported by the builder. Server-facing execute_as authorizes the request before execution. Requiring WHERE/all would intentionally change the API, not fix a violated current guarantee.

Evidence: [crates/shamir-query-builder-macros/src/lib.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/lib.rs#L87); [crates/shamir-query-builder-macros/src/query_parse.rs:614](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/query_parse.rs#L614); [crates/shamir-query-builder/src/write/update.rs:105](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/update.rs#L105); [crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs:522](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/q_macro_tests.rs#L522); [crates/shamir-db/src/shamir_db/execute/db_execute.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_execute.rs#L40); [crates/shamir-engine/src/table/write_exec.rs:603](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/write_exec.rs#L603).

<a id="review-3"></a>

### Claim 3 — No tests in the crate; the predicate-name whitelist invariant is unpinned

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Unknown-name/wrong-arity rejection lacks pins. All 17 accepted names and dotted outputs already have registered differential tests. This is DSL-confinement coverage, not a security boundary against an author who can supply arbitrary Rust RHS code.

Evidence: [crates/shamir-query-builder-macros/src/filter_lower.rs:299](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder-macros/src/filter_lower.rs#L299); [crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs#L194); [crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs:301](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs#L301); [crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs:383](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/filter_macro_tests.rs#L383); [crates/shamir-query-builder/src/macros/tests/mod.rs:2](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/tests/mod.rs#L2).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

## Evidence and recipe corrections

- No query-text splicing is supported; no arbitrary-code-execution confinement is promised against a Rust source author.
- Not every predicate argument is Into&lt;FilterValue&gt;: patterns/text use Into&lt;String&gt;, vectors use Vec&lt;f32&gt; and k uses u32.
- Stored-procedure macro integration tests use native replacement functions and convenience execute with System. They exercise DTO-to-invoker behavior, not an authenticated WASM repository-isolation witness.
- Keep explicit bulk UPDATE out of mandatory security-remediation counts.

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
