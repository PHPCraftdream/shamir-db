# P2-14 - shamir-query-builder (+ macros): copying, reference collection, macro emission/parsing/docs, cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Fix the 20 findings below in one coherent change set for shamir-query-builder, shamir-query-builder-macros (effort units 38: S=1, M=2, L=4). They are adjacent: same crate and overlapping files, so they are solved together and share tests and review. Keep builder-only expansion paths (C5/H8) and clause-keyword disambiguation.

## Scope

Crates: `shamir-query-builder`, `shamir-query-builder-macros`. Dependents are type-checked with `cargo check` when a public signature changes.

## Decisions

Filled by the orchestrator before launch (owner answers). Empty means: skip the cards marked OWNER DECISION: FE2003D1, 9E172C69, 02993B77, DE20E96B.

- (none yet)

## Findings (ordered by risk, then size)

### RV-QUERY-BUILDER-FE2003D1 - `lit_u64`'s decimal-`String` encoding for `u64 > i64::MAX` is only specified for equality

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/read/val/filter_value.rs:76; crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:197,237
- Recipe: Specify and test large-integer ordering, range and membership across Big and decimal-string storage while preserving RU7.
- Note: Lexical String ordering and scalar-only extraction remain.

### RV-QUERY-BUILDER-8BEB53C0 - `try_build`'s conservative fallback re-serializes `Call`/`Subscribe` ops that are already typed

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/batch/batch_refs.rs:558-601
- Recipe: Add typed Call and scope-aware Subscribe reference collection with planner-parity tests.
- Note: Both variants still enter the codec fallback.

### RV-QUERY-BUILDER-9E172C69 - `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/batch/batch_refs.rs:54-67,161; crates/shamir-sdk/src/api/db.rs:216-220
- Recipe: Add a consuming validated build path and use it where callers own the batch, retaining borrowed reuse.
- Note: Validation was fixed; deep copying remains on the SDK path.

### RV-QUERY-BUILDER-MACROS-02993B77 - Nit: emitted `::shamir_query_builder` absolute paths break if a downstream renames the dependency

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/dep`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-builder-macros/src/filter_lower.rs:62; crates/shamir-query-builder-macros/src/query_parse_lower.rs:29,250-253
- Recipe: Resolve the consumer dependency alias for all builder expansion paths and add an alias-only consumer fixture.
- Note: Upgraded from nit because renamed-only consumers face a compilation limitation.

### RV-QUERY-BUILDER-MACROS-13A0D25A - Field-path and alias spans are discarded; downstream errors point at the macro call site

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder-macros/src/filter_lower.rs:327-342; crates/shamir-query-builder-macros/src/query_parse_lower.rs:274-290,303-305
- Recipe: Emit source-spanned string literals for field names and aliases and add CI diagnostic fixtures.
- Note: Span loss is visible; exact compiler underlines were not executed.

### RV-QUERY-BUILDER-2A648617 - `Batch::switch` guard construction is O(K^2) deep clones and O(K^2) emitted filter nodes

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/batch/batch_refs.rs:268-280
- Recipe: Document quadratic switch lowering and require a separate design decision before changing the wire representation.
- Note: Prior conditions are cloned into every successive guard.

### RV-QUERY-BUILDER-320AD085 - `switch` with zero cases emits `when: Not{Or{[]}}`; empty-`Or` truth value is unpinned and untested

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/test`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/batch/batch_refs.rs:280; crates/shamir-engine-query/src/query/filter/evaluation/filter_node.rs:360
- Recipe: Add execution-boundary coverage proving zero cases execute exactly the default operation.
- Note: Current empty-Or truth semantics are clear; coverage remains missing.

### RV-QUERY-BUILDER-0E30EB25 - Stale doc references

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/read/filter/leaf.rs:83; crates/shamir-query-builder/src/write/ops/insert.rs:44
- Recipe: Replace stale constructor and macro names and update fallible-conversion commentary.
- Note: query_ref and mpak references remain.
- Second pass: Stale constructor, macro and panicking-conversion references remain; documentation-only remediation spans several files.

### RV-QUERY-BUILDER-202E00CD - Duplicate re-exports: the same items re-exported in both a sibling file and its `mod.rs`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/read/select/select_item.rs:8; crates/shamir-query-builder/src/read/select/mod.rs:12; crates/shamir-query-builder/src/write/ops/update.rs:10; crates/shamir-query-builder/src/write/mod.rs:67
- Recipe: Keep public manifest exports and make redundant implementation sibling imports private.
- Note: Duplicate routes reference identical items.

### RV-QUERY-BUILDER-21560B4A - `val::func`/`val::expr` collide by name with `select::func`/`select::expr` for glob-import users

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/read/val/filter_value.rs:116; crates/shamir-query-builder/src/read/val/expr.rs:15; crates/shamir-query-builder/src/read/select/select_item.rs:42,76
- Recipe: Document qualified imports or explicit aliases for simultaneous val/select use.
- Note: Glob-import ambiguity is optional ergonomic debt.

### RV-QUERY-BUILDER-7A0F3500 - `ddl/` applies one-file-one-export inconsistently — family files bundle many unrelated public builders

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/ddl/schema/schema.rs:36,408,469,517,563; crates/shamir-query-builder/src/ddl/policy/access_control.rs:78
- Recipe: Split independent builders into owner files while preserving public re-exports and layout limits.
- Note: Downgraded: bundled independent exports remain, without runtime impact.

### RV-QUERY-BUILDER-A548E006 - All four `macro_rules!` definitions live inline in `macros/mod.rs`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/macros/mod.rs:30,56,72,98
- Recipe: Move macro definitions into sibling implementation files while preserving exports and lexical wiring.
- Note: Downgraded: structural placement only.

### RV-QUERY-BUILDER-C861C539 - `cursor` module's tests use a bare `tests.rs` instead of the documented `tests/` directory + manifest

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/read/cursor.rs:81-82; crates/shamir-query-builder/src/read/cursor/tests.rs:6
- Recipe: Move cursor tests into a tests directory with a manifest and topical test file.
- Note: Downgraded: tests are registered, not orphaned.

### RV-QUERY-BUILDER-DE20E96B - Five error enums hand-roll `Display` + `std::error::Error` despite the workspace thiserror rule

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/dep`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/batch/op/build_error.rs:64; crates/shamir-query-builder/src/ddl/index/create_index_build_error.rs:131; crates/shamir-query-builder/Cargo.toml:15
- Recipe: Add thiserror and migrate the five error implementations while preserving messages and sources.
- Note: Downgraded: convention debt, not demonstrated handling failure.

### RV-QUERY-BUILDER-E2602921 - Imports not at top: one production-code site + a pervasive function-local `use` pattern in tests

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder/src/batch/batch_refs.rs:330; crates/shamir-query-builder/src/batch/tests/flow/after_tests.rs:24
- Recipe: Hoist production and test imports into file headers except genuine documented exceptions.
- Note: Downgraded: imports-at-top policy debt.
- Second pass: Production and test local imports remain without documented exceptions; imports-at-top policy debt only.

### RV-QUERY-BUILDER-MACROS-1AF72B7B - Unjustified `pub`, redundant wrapper, duplicated field-path emitter

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder-macros/src/filter_lower.rs:10,20,315; crates/shamir-query-builder-macros/src/query_parse_lower.rs:274,359
- Recipe: Narrow helper visibility and consolidate equivalent field-path emitters while retaining the useful lowering seam.
- Note: Private modules prevent an external API leak; wrapper removal is not independently justified.

### RV-QUERY-BUILDER-MACROS-A690BEA8 - Per-predicate-call String/Vec micro-allocations in filter lowering

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder-macros/src/filter_lower.rs:118,130-135,327-342
- Recipe: Borrow callee identifiers and use existing argument storage instead of collecting a temporary reference vector.
- Note: Structural allocation candidates remain without measured bottleneck evidence.

### RV-QUERY-BUILDER-MACROS-EC1B86B3 - Where-clause tokens are captured and re-parsed -- 2x token traffic plus a full copy of every group

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-query-builder-macros/src/query_parse.rs:614-669
- Recipe: Avoid redundant group reconstruction and prefix reparsing while preserving clause-keyword disambiguation.
- Note: Repeated processing exists; exact 2x traffic and full recursive group copying are not established.
- Second pass: Prefix reparsing and outer-group reconstruction remain; preserve keyword disambiguation and do not claim an exact traffic multiplier.

### RV-QUERY-BUILDER-MACROS-83FE5F55 - Doc: "All five forms" — there are six

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-query-builder-macros/src/lib.rs:63-64,166-175; crates/shamir-query-builder-macros/src/query_parse_lower.rs:263-269
- Recipe: Document six forms and distinguish bare DTO results from fallible write terminals.
- Note: Overview contradicts dispatch and its own Result-returning write examples.

### RV-QUERY-BUILDER-MACROS-E78E1666 - Doc drift: "all 19 predicate calls" (there are 17); `vector_similarity_ef`/`_opts` unreachable from the DSL

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-query-builder-macros/src/lib.rs:135; crates/shamir-query-builder-macros/src/filter_lower.rs:132-305; crates/shamir-query-builder/src/read/filter/leaf.rs:296,314
- Recipe: Document 17 supported DSL predicates and direct vector-option users to builder APIs.
- Note: Vector option DSL parity is optional; the predicate-count claim is wrong.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- Cards marked `OWNER DECISION` are implemented only if the "Decisions" section below contains the answer; otherwise skip them and say so in the final report.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-query-builder -p shamir-query-builder-macros -- --check`; `cargo clippy -p shamir-query-builder -p shamir-query-builder-macros --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
