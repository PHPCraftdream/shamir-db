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
