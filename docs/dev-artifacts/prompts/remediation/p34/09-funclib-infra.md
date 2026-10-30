# P34-09 - shamir-funclib + shamir-numa + shamir-bench-utils: error taxonomy, doc claims, test coverage

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Catalogue the resolver-generated scalar error codes, correct the remaining lock-free and canonical-exclusion documentation claims, repair the NUMA coverage claims and mirror-window prose, and fix the bench-utils documentation and the statistical-versus-exact test assertion.

This is one change set for shamir-funclib, shamir-numa, shamir-bench-utils: 8 active findings, effort units 9 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-funclib`, `shamir-numa`, `shamir-bench-utils`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-engine/benches/vector_search/vector_search.rs` (4E7A9919)

## Acceptance

- Catalogue assertions cover the resolver-generated scalar_task_failed and not_indexable codes with their spellings preserved (24C30E50).
- Documentation states the actual shared bucket read-lock contract (C8BC39BC) and the string-key-only exclusion (775ADABC).
- NUMA replica load_node/store_node out-of-range coverage uses a real seam instead of mock clamping (553CB81D).
- bench-utils: zero-noise per-row construction-identity coverage without weakening tolerances (F53F775F); GENERATOR_REVISION is bumped only if generator output changes.

## Coordination with other tasks

Wave 1; no strict predecessor.

## Decisions

No owner-decision card in this brief.

## Findings (ordered by risk, then size)

### L3-RV-FUNCLIB-24C30E50 - `ScalarError` string-code taxonomy is unpoliced: no code catalog, drifting synonyms, zero context

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-funclib/src/registry.rs:21-45; crates/shamir-funclib/src/registry.rs:61-67; crates/shamir-funclib/src/scalar_resolver.rs:155; crates/shamir-funclib/src/scalar_resolver.rs:176
- Recipe: Catalogue existing scalar_task_failed and not_indexable outputs, use builtin construction and extend catalogue assertions.
- Verifier note: Catalogue enforcement exists, but reachable resolver-generated scalar_task_failed and not_indexable bypass it; preserve spellings and extend catalogue coverage.

### L3-RV-FUNCLIB-775ADABC - Generic canonical APIs do not exclude an interned _prev_hash field despite their unconditional reserved-field documentation

- Verdict: `PARTIAL`; risk `low`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-funclib/src/functions/value/canonical/core.rs:19-29; crates/shamir-funclib/src/functions/value/canonical/core.rs:36-39; crates/shamir-funclib/src/functions/value/canonical/core.rs:190-195
- Recipe: Qualify the remaining determinism and reserved-constant promises as string-key-only exclusion.
- Verifier note: String-key-only exclusion is correctly specified in key places, but determinism and reserved-constant documentation retain unconditional exclusion promises.

### L3-RV-FUNCLIB-C8BC39BC - `UserScalarLayer::get` uses `get_sync` (exclusive bucket lock) while its docs claim "read-only hash probes with no locking"

- Verdict: `PARTIAL`; risk `low`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-funclib/src/scalar_resolver.rs:32-33; crates/shamir-funclib/src/scalar_resolver.rs:59-60; crates/shamir-funclib/src/scalar_resolver.rs:82-83
- Recipe: Replace ScalarResolver's blanket lock-free-map claim with the actual shared bucket read-lock contract.
- Verifier note: Exclusive lookup locking is removed, but ScalarResolver still labels the shared-lock map lock-free; correct that remaining description.

### L3-RV-NUMA-553CB81D - Test-coverage claims vs reality: `detect()` untested off-Linux, `LinuxTopology` error paths untested, README references a non-existent CI workflow

- Verdict: `PARTIAL`; risk `low`; fix size/kind `S/-`.
- Evidence at `07bc804c`: crates/shamir-numa/src/platform/tests/linux_probe_tests.rs:67; crates/shamir-numa/src/tests/node_replicated_tests.rs:82; crates/shamir-numa/src/core/node_replicated.rs:205
- Verifier note: First pass confused affinity OOB coverage with explicit replica load_node/store_node OOB coverage; the existing load_local test is intercepted by mock clamping.

### L4-RV-BENCH-UTILS-4E7A9919 - Reproducibility key described inconsistently: "(k, σ, seed) triple" vs. five values

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/-`.
- Evidence at `07bc804c`: crates/shamir-engine/benches/vector_search/vector_search.rs:19-20; crates/shamir-engine/benches/vector_search/vector_search.rs:115; crates/shamir-bench-utils/src/vector_data.rs:191-194
- Verifier note: First pass missed the workload comment still calling an unversioned five-field tuple the reproduction key while current contracts require six fields.

### L4-RV-BENCH-UTILS-67097ED5-FACET - Stale Criterion wording alongside the real Harness consumers

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-bench-utils/src/peak_mem.rs:12-24; crates/shamir-bench-utils/src/vector_data.rs:3
- Recipe: Replace the surviving criterion-bench module-header phrase with current Harness terminology.
- Verifier note: Harness integration is corrected but the vector-data module header still says criterion bench.

### L4-RV-BENCH-UTILS-F53F775F - `round_robin_balances_clusters` asserts a statistical property as an exact equality

- Verdict: `UNVERIFIABLE`; risk `nit`; fix size/kind `S/test`.
- Evidence at `07bc804c`: crates/shamir-bench-utils/src/vector_data/tests/generation_tests.rs:57-75; crates/shamir-bench-utils/src/vector_data.rs:248-252
- Recipe: Add zero-noise per-row construction-identity coverage and keep noisy geometric assertions separate without arbitrary tolerance weakening.
- Verifier note: Nearest-centroid counts are not construction labels, but no separation proof or affected-target failure witness establishes the deterministic fixture flake; add per-row zero-noise coverage without weakening tolerances.

### L4-RV-NUMA-6E1761A2-FACET - Unsupported numerical bound in mirror-window documentation

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-numa/src/core/node_replicated.rs:26; crates/shamir-numa/src/core/node_replicated.rs:134
- Recipe: Replace the numerical mirror-window claim with a scheduling- and contention-dependent interval without a time bound.
- Verifier note: Unsupported few-nanoseconds mirror-window prose survives; replace with scheduling- and contention-dependent wording.

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

`cargo fmt -p shamir-funclib -p shamir-numa -p shamir-bench-utils -- --check`; `cargo clippy -p shamir-funclib -p shamir-numa -p shamir-bench-utils --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
