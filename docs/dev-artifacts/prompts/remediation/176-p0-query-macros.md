# P0 stage C5 — p0-query-macros

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Expansion uses only the documented paths; a `trybuild` compile-fail suite covers every diagnostic; the listed silent-drop / duplicate-key / misleading-error cases fail with precise messages.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-QUERY-BUILDER-MACROS-36328515 — `q!(call ...)` violates the crate's own emitted-path contract: expansion requires a direct `shamir-query-types` dependency

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder-macros/src/lib.rs:3; crates/shamir-query-builder-macros/src/query_parse_lower.rs:216
- Revalidation note: AGREE — OBSERVED: the layout split preserves direct shamir_query_types references in call expansion despite the documented builder-only dependency contract.

### RV-QUERY-BUILDER-MACROS-A63844FD — Zero negative/error-path tests; the entire macro diagnostic surface is unverified (TDD "Red" missing for every error path)

- Status at HEAD: `confirmed-open`, risk `medium`; evidence: crates/shamir-query-builder-macros/Cargo.toml:9; crates/shamir-query-builder/src/macros/tests/mod.rs:1
- Revalidation note: AGREE — OBSERVED: source and harness searches find accepted-input suites but no registered parser-rejection or compile-fail coverage; historical TDD activity cannot be established from this absence.

### RV-QUERY-BUILDER-MACROS-2C459E8E — `q!(call ...)` bypasses the builder layer entirely and pins `repo: "main"` with no repo-qualified syntax

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder-macros/src/query_parse.rs:671; crates/shamir-query-builder-macros/src/query_parse_lower.rs:221
- Revalidation note: OBSERVED: call parsing still has no repository qualifier and expansion constructs CallOp directly with main, retaining the scoped repository-override limitation rather than establishing an authorization bypass.

### RV-QUERY-BUILDER-MACROS-30DA1AC8 — Clause keywords are silently reserved inside `q!` where/having, contradicting the documented "full `filter!` expression grammar"

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder-macros/src/query_parse.rs:495; crates/shamir-query-builder-macros/src/lib.rs:128
- Revalidation note: OBSERVED: where/having token collection still terminates on any top-level clause keyword, including a dotted field segment or RHS identifier, despite promising the full filter grammar.

### RV-QUERY-BUILDER-MACROS-E24D5F62 — Missing sub-stream exhaustion checks: claimed silent token-drop in doc maps / call args / select-fn args does not occur under syn 2.0.114 — residual issue is diagnostic quality + implicit reliance on syn internals

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder-macros/src/query_parse.rs:570; crates/shamir-query-builder-macros/src/query_parse_lower.rs:318
- Revalidation note: OBSERVED: local substream exhaustion diagnostics and rejection pins remain absent, but pinned syn 2.0.114 still rejects unconsumed nested tokens, preserving the prior refutation of silent token loss.

### RV-QUERY-BUILDER-MACROS-E50875AB — Unknown function-like select item produces a misleading clause-order error

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder-macros/src/query_parse.rs:431; crates/shamir-query-builder-macros/src/query_parse.rs:298
- Revalidation note: OBSERVED: an unknown select call still falls through to field parsing and leaves its parentheses for the generic clause-order error instead of identifying the unsupported function.

### RV-QUERY-BUILDER-MACROS-FB738525 — Two unaliased `count(*)` items silently produce duplicate `"count"` output keys

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-query-builder-macros/src/query_parse_lower.rs:262; crates/shamir-engine/src/query/read/aggregation/group_projection.rs:233
- Revalidation note: OBSERVED: unaliased counts still use count, validation does not reject duplicate output keys, and later aggregate insertion can overwrite that count with a distinct aggregate sharing its alias.

### RV-QUERY-BUILDER-MACROS-7E797992 — Reused diagnostics carry the wrong context at some call sites

- Status at HEAD: `confirmed-open`, risk `nit`; evidence: crates/shamir-query-builder-macros/src/query_parse.rs:538; crates/shamir-query-builder-macros/src/filter_lower.rs:351
- Revalidation note: OBSERVED: empty HAVING still reports after where, and invalid predicate field arguments still report a comparison-LHS error; both remain correctly rejected.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.

## ORCHESTRATOR NOTES (binding; override the acceptance wording where they differ)
* NO `trybuild`. A compile-fail suite needs committed `.stderr` snapshots generated by running rustc, and test execution is banned locally (CI only, CI cannot commit). Instead make every diagnostic testable WITHOUT rustc: keep proc-macro parsing/lowering in plain functions over `proc_macro2::TokenStream`/`syn` (the proc-macro entry points stay thin wrappers) and cover every rejection path with unit tests that parse a token stream (`quote!`/`str::parse`) and assert the exact `syn::Error` message text (and, where cheap, the span start line/col). Adding `proc-macro2`/`quote` as dev-dependencies is allowed ONLY if they are already in Cargo.lock; no other new dependency; do not change versions.
* Tests live in `tests/` directories per the repo layout rules (a `src/tests/` dir with a `mod.rs` manifest for the proc-macro crate); all imports at the top; ≤7 entries per folder, ≤1000 lines per file.
* Expansion paths (card 36328515): the macros may reference only paths re-exported by `shamir-query-builder` (add `#[doc(hidden)]` re-exports of the needed `shamir-query-types` items there if missing); a crate using only `shamir-query-builder` must compile `q!(call ...)`. Add a unit test that scans the emitted token stream of every macro form and asserts no `shamir_query_types` path segment appears.
* C4 is merged: `Doc::set` now returns `Result`; `doc!`/`q!` already unwrap and document that over-deep literals panic. Do not undo that.
* Dedicated worker note: do not run any cargo test. Allowed gates: `cargo clippy -p shamir-query-builder-macros -p shamir-query-builder --all-targets -- -D warnings`, `cargo fmt -p ... -- --check`, `python -I scripts/ci/check_layout.py`.
