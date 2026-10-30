# P34-10 - shamir-query-types + shamir-query-builder (+macros): conversion contract, receipt metadata, switch lowering, macro grammar, structure

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Reuse nested receipt metadata, add the CallOp constructor so q!(call ...) goes through the builder layer, normalise raw identifiers in select-function classification, take borrowed Batch encoding off deep clones, finish the DDL owner-file separation, fix the remaining documentation, and close the P2 carry-overs (pagination equality, allocation-free Big comparison, diagnostic fixtures).

This is one change set for shamir-query-types, shamir-query-builder, shamir-query-builder-macros: 13 active findings and 3 P2 carry-overs, effort units 39 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-query-types`, `shamir-query-builder`, `shamir-query-builder-macros`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-engine/src/query/batch/control/receipts/policy.rs` (0A0D9092)
- `crates/shamir-engine/src/query/batch/exec/batch_execute.rs` (0A0D9092)

## Acceptance

- Nested receipt-policy preflight no longer recollects descendant repositories on admin-write paths (0A0D9092).
- q!(call ...) builds its operation through CallOp::new / CallOp::with_repo; the expansion is unchanged for callers (2C459E8E).
- A raw `r#count` select function classifies like `count` (002C0148); borrowed Batch encoding does not deep-clone the request (9E172C69).
- The DDL list builders have their own owner files and the exports are preserved (7A0F3500); pagination equality and Big comparison carry-overs have tests that fail on the old code (3D922433, D687DBA3).

## Coordination with other tasks

Wave 3; starts after P34-06 are merged. 0A0D9092 edits engine receipt policy and batch_execute, which P34-06 also edits.
- 2C459E8E adds CallOp::new / with_repo in shamir-query-types (additive, source compatible).
- 0A0D9092 edits shamir-engine receipt policy and batch_execute: touch only those files in shamir-engine.

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-QUERY-BUILDER-2A648617, RV-QUERY-TYPES-182C703E, RV-QUERY-TYPES-4F767AA5, RV-QUERY-BUILDER-MACROS-2C459E8E, RV-QUERY-BUILDER-MACROS-9BCA14B1, RV-QUERY-TYPES-CC015A3C, RV-QUERY-BUILDER-21560B4A. The full decision paragraph sits in the finding below.

- L3-RV-QUERY-BUILDER-2A648617: **SKIP** (no change).
- L3-RV-QUERY-TYPES-182C703E: **SKIP** (no change).
- L3-RV-QUERY-TYPES-4F767AA5: **SKIP** (no change).
- L3-RV-QUERY-BUILDER-MACROS-2C459E8E: **IMPLEMENT** (change as decided).
- L3-RV-QUERY-BUILDER-MACROS-9BCA14B1: **SKIP** (no change).
- L4-RV-QUERY-TYPES-CC015A3C: **SKIP** (no change).
- L4-RV-QUERY-BUILDER-21560B4A: **SKIP** (no change).

## Findings (ordered by risk, then size)

### L3-RV-QUERY-BUILDER-2A648617 - `Batch::switch` guard construction is O(K^2) deep clones and O(K^2) emitted filter nodes

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted, documented cost. Batch::switch already documents O(K^2) lowering (crates/shamir-query-builder/src/batch/batch_refs.rs:273-275, as P2-14 required). A linear form needs a new wire primitive (a guard that references another entry, or a shared-condition node), which conflicts with ADR Decision 1 (switch is builder-only sugar). The AND(NOT(OR(prior)), c_i) form still emits O(K^2) nodes, and no harmful case count has been measured. A shared-condition representation is a separate wire-format design item.
- Evidence at `07bc804c`: crates/shamir-query-builder/src/batch/batch_refs.rs:273; crates/shamir-query-builder/src/batch/batch_refs.rs:298
- Recipe: Accept documented quadratic lowering or authorize a semantics-preserving shared-condition representation.
- Verifier note: Quadratic lowering remains reachable through switch, but is documented; accept cost or authorize a semantics-preserving shared representation.

### L3-RV-QUERY-TYPES-0A0D9092 - Three separate full-tree recursive walks per request: `is_write`, `distinct_repos`, `collect_required_access`

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/code`.
- Evidence at `07bc804c`: crates/shamir-query-types/src/ops/batch/ops/query_entry.rs:94-120; crates/shamir-engine/src/query/batch/control/receipts/policy.rs:46-52; crates/shamir-engine/src/query/batch/exec/batch_execute.rs:169-171
- Recipe: Cache metadata per static body and reuse it throughout nested receipt validation and execution.
- Verifier note: Root metadata is fused, but nested receipt-policy preflight still recollects descendant repositories on reachable admin-write paths.

### L3-RV-QUERY-TYPES-182C703E - thiserror convention deviation: hand-rolled Display/Error impls and `Result<(), String>` public APIs

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted convention/API debt, same as the P2-13 SKIP. Manual Display/Error (batch_error.rs:300, :459) and the public String-returning validators (retention.rs:41, filter_enum.rs:235-252) stay. thiserror is not a dependency of shamir-query-types, which is also compiled for guest-core WASM; no incorrect classification has been shown; and callers only map the text (shamir-db admin_retention.rs:47). Same policy as shamir-tx FF6AB063.
- Evidence at `07bc804c`: crates/shamir-query-types/src/ops/batch/core/batch_error.rs:300; crates/shamir-query-types/src/ops/batch/core/batch_error.rs:459; crates/shamir-query-types/src/admin/types/db/retention.rs:41; crates/shamir-query-types/src/read/filter/filter_enum.rs:235
- Recipe: Adopt thiserror and typed validation errors under an approved public-API compatibility migration.
- Verifier note: Manual error implementations and callable public String-error validators remain; typed-error migration changes public compatibility.

### L3-RV-QUERY-TYPES-4F767AA5 - `From<QueryValue> for FilterValue` silently substitutes `Null` in release builds

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Documented limitation, consistent with P2-13, which explicitly kept From<QueryValue> for FilterValue as a documented legacy lossy adapter (filter_value.rs:198-224; checked path try_from_query_value at :60-73). It cannot be retired compatibly: #[deprecated] has no effect on trait impls, and Doc::set(impl Into<FilterValue>) (doc.rs:57) cannot divert QueryValue arguments without specialization. Removing the impl breaks the Rust API across implicit in-repo conversions. Release builds reach Null only for non-marker maps or sets passed to Doc::set or to q!(call) arguments; debug builds panic. The documented lossless path is Doc::set_value. Retire the impl in a dedicated API-break batch, together with the q!(call) argument path.
- Evidence at `07bc804c`: crates/shamir-query-types/src/read/filter/filter_value.rs:60-72; crates/shamir-query-types/src/read/filter/filter_value.rs:198-222; crates/shamir-query-builder/src/write/ops/doc.rs:57-62
- Recipe: Migrate callers to fallible conversion and retire the lossy From adapter through an approved API change.
- Verifier note: Fallible conversion exists, but Doc::set still reaches the legacy From adapter that release-substitutes Null for unsupported maps.

### L3-RV-QUERY-BUILDER-9E172C69 - `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-query-builder/src/batch/batch_refs.rs:24
- Recipe: Migrate ownership-available send sites to consuming terminals while retaining deliberate borrowed reuse.
- Verifier note: Consuming request and SDK terminals exist; reachable borrowed encoding still clones request payloads.

### L3-RV-QUERY-BUILDER-MACROS-002C0148 - Raw identifiers are silently lowered to database names containing the Rust r# escape prefix

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-query-builder-macros/src/query_parse.rs:463
- Recipe: Normalize raw select-function identifiers before whitelist classification and pin raw/non-raw equivalence.
- Verifier note: Select-function classification still uses escaped identifier spelling, rejecting raw count despite other normalization fixes.

### L3-RV-QUERY-BUILDER-MACROS-2C459E8E - `q!(call ...)` bypasses the builder layer entirely and pins `repo: "main"` with no repo-qualified syntax

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `additive`, risk `low`). Add two inherent constructors to CallOp in its owner module crates/shamir-query-types/src/ops/call/call_op.rs, following the TableRef::new/with_repo pattern: CallOp::new(call: impl Into<String>, params: impl IntoIterator<Item = impl Into<FilterValue>>), whose repo comes from the existing private default_repo() so the "main" default lives in one place, and CallOp::in_repo(call, repo: impl Into<String>, params). Batch::call and Batch::call_in_repo (crates/shamir-query-builder/src/batch/batch.rs:771-799) delegate to them. The q!(call ...) lowering (crates/shamir-query-builder-macros/src/query_parse_lower.rs:228-257) emits #root::CallOp::new(..) when no repo is given and #root::CallOp::in_repo(..) when one is. Each argument is still converted with Into::<#root::FilterValue>::into(arg) so error spans stay on the user's argument, and the arguments are passed as an explicitly typed ::std::vec::Vec<#root::FilterValue> so that a zero-argument call still type-checks. Must stay compatible: the public CallOp fields and struct-literal construction, the serde wire bytes including the repo default, the DSL grammar, the #root trampoline from P2-14 02993B77, and owned String fields (no Cow; see CC015A3C). Regression tests: q!(call f(..)) with zero and several arguments, and q!(call repo.f(..)), equal CallOp::new/in_repo and the BatchOp produced by Batch::call/call_in_repo, with identical msgpack bytes; update the src/tests/lowering_tests.rs expectations. Out of scope: the lossy QueryValue-to-FilterValue argument conversion (4F767AA5), and any change to authorization or the default-repo semantics.
- Evidence at `07bc804c`: crates/shamir-query-builder-macros/src/query_parse_lower.rs:249
- Recipe: Approve a public call-construction builder entry point and route macro expansion through it.
- Verifier note: Repository override is repaired, but expansion still constructs CallOp directly rather than using a call builder.

### L3-RV-QUERY-BUILDER-MACROS-9BCA14B1 - Unbounded recursion in expression lowering; pathological nesting aborts rustc instead of erroring

- Verdict: `UNVERIFIABLE`; risk `low`; fix size/kind `M/test`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Documented limitation, accepted. The input is compile-time, author-controlled DSL code. syn::Expr parsing and Drop are themselves recursive with no depth cap (filter_lower.rs:13; query_parse.rs:615, :768), so a depth cap in lower() (filter_lower.rs:28) would not reliably turn deep nesting into a diagnostic. No compiler abort has been observed, and there is no runtime, wire or security exposure. Revisit only if a real abort is reported, and then cap parsing as well as lowering.
- Evidence at `07bc804c`: crates/shamir-query-builder-macros/src/filter_lower.rs:28
- Recipe: Establish isolated pathological-input CI evidence across parsing, lowering, diagnostics and destruction before choosing limits.
- Verifier note: Uncapped recursion is present, but accepted pathological depth and compiler abort remain unobserved; require isolated CI evidence before limits.

### L3-RV-QUERY-BUILDER-MACROS-30DA1AC8 - Clause keywords are silently reserved inside `q!` where/having, contradicting the documented "full `filter!` expression grammar"

- Verdict: `PARTIAL`; risk `low`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-query-builder-macros/src/query_parse.rs:612
- Recipe: Qualify the full-grammar claim with the accepted clause-keyword start-position exception.
- Verifier note: Initial keyword reservation remains accepted and documented; the proposed documentation qualification is already substantially present.

### L4-RV-QUERY-TYPES-CC015A3C - Per-construction `"main"` String allocations

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted allocation cost, same as the P2-13 SKIP. Keep the public owned String repository fields and the "main" defaults (table_ref.rs:21-26, the default_repo helpers). No measurement shows a bottleneck, and Cow or interning would change public field ownership. The new CallOp constructors from 2C459E8E keep the owned String.
- Evidence at `07bc804c`: crates/shamir-query-types/src/ops/table_ref.rs:21; crates/shamir-query-types/src/ops/table_ref.rs:63; docs/dev-artifacts/prompts/remediation/p2/13-query-types.md:33
- Recipe: Retain accepted owned String defaults; require CI measurements before reconsidering ownership.
- Verifier note: Construction and bare-string deserialization still allocate main; retain accepted ownership cost unless measurements justify an owner-approved API redesign.

### L3-RV-QUERY-BUILDER-7A0F3500 - `ddl/` applies one-file-one-export inconsistently — family files bundle many unrelated public builders

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-query-builder/src/ddl/schema/schema.rs:1; crates/shamir-query-builder/src/ddl/policy/access_control.rs:1; crates/shamir-query-builder/src/ddl/schema/list.rs:24; crates/shamir-query-builder/src/ddl/schema/list.rs:62; crates/shamir-query-builder/src/ddl/schema/list.rs:106; crates/shamir-query-builder/src/ddl/schema/list.rs:151
- Recipe: Finish independent DDL owner-file separation and resolve temporary config placement while preserving exports.
- Verifier note: Schema/access owners are separated, but independent list builders remain bundled; finish structural separation while preserving exports.

### L4-RV-QUERY-BUILDER-0E30EB25 - Stale doc references

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-query-builder/src/batch/batch.rs:290
- Recipe: Replace obsolete panicking IntoBatchOp commentary with current fallible registration and conversion behavior.
- Verifier note: Constructor and macro names are repaired; obsolete panicking IntoBatchOp commentary remains.

### L4-RV-QUERY-BUILDER-21560B4A - `val::func`/`val::expr` collide by name with `select::func`/`select::expr` for glob-import users

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted policy: users who import both val and select write qualified val::func/select::func (and val::expr/select::expr) or use explicit import aliases, as documented in crates/shamir-query-builder/src/read/val/mod.rs:8-10 (P2-14). No prelude merges the two modules (shamir_sdk::prelude re-exports neither). Additive aliases would put two names for each function into the API for a nit.
- Evidence at `07bc804c`: crates/shamir-query-builder/src/read/val/mod.rs:8
- Recipe: Explicitly accept qualified use as policy or authorize additive disambiguated aliases without renaming existing exports.
- Verifier note: Qualified-use guidance mitigates but does not remove simultaneous-glob ambiguity; prefer explicit policy acceptance.

## Carried over from the P2 sprint

These P2 cards stayed open after the sprint (state from `P2-SPRINT-REPORT-2026-10-30.md`). Their evidence and recipe are the P2 revalidation at master `7f04e096`; re-locate the code by symbol.

### RV-QUERY-BUILDER-MACROS-13A0D25A - Field-path and alias spans are discarded; downstream errors point at the macro call site

- Sprint state: PARTIAL: span assertions, no compiler diagnostic-underlining fixtures.
- Risk `low`; fix size/kind `M/code`.
- Evidence: crates/shamir-query-builder-macros/src/filter_lower.rs:327-342; crates/shamir-query-builder-macros/src/query_parse_lower.rs:274-290,303-305
- Recipe: Emit source-spanned string literals for field names and aliases and add CI diagnostic fixtures.
- Verifier note: Span loss is visible; exact compiler underlines were not executed.

### RV-QUERY-TYPES-3D922433 - `Pagination::eq` (`After`) — two msgpack encodes per equality comparison

- Sprint state: PARTIAL: allocation-free Big comparison / pagination equality carry-over.
- Risk `low`; fix size/kind `M/code`.
- Evidence: crates/shamir-query-types/src/read/clause/limit.rs:84-88,123,130-132
- Recipe: Implement allocation-free encoded-value equivalence with numeric, float and map-order regression oracles.
- Verifier note: Two encodes remain after matching limit and ID; hot use is unestablished.

### RV-QUERY-TYPES-D687DBA3 - `FieldPath` type alias defined in `filter/mod.rs`

- Sprint state: PARTIAL: FieldPath file placement.
- Risk `nit`; fix size/kind `M/code`.
- Evidence: crates/shamir-query-types/src/read/filter/mod.rs:24
- Recipe: Move FieldPath into a sibling definition file and preserve its existing public re-export.
- Verifier note: Downgraded: manifest-only rule remains violated.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-query-types -p shamir-query-builder -p shamir-query-builder-macros -- --check`; `cargo clippy -p shamir-query-types -p shamir-query-builder -p shamir-query-builder-macros --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
