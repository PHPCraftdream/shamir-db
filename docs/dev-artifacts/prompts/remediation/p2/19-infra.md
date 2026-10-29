# P2-19 - shamir-numa + shamir-bench-utils + shamir-collections: detection, generator contract, oracles, docs

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Fix the Lcg generator endpoints (bump GENERATOR_REVISION), the NUMA detection, coverage and QEMU oracle, and the related docs.

This is one change set for shamir-numa, shamir-bench-utils, shamir-collections: 12 findings, effort units 23 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. Bump GENERATOR_REVISION when generator output changes.

## Scope

Crates: `shamir-numa`, `shamir-bench-utils`, `shamir-collections`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- Lcg::next_f32 stays in [0, 1) and next_range never reaches hi; GENERATOR_REVISION is bumped where generated data changes.
- NUMA detection/coverage and the QEMU oracle are corrected; the documentation cards are closed.

## Coordination with other tasks

Suggested wave 2; starts after P2-08 are merged.
- Starts after P2-08 (collections lib.rs).

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: 6FDBD35C, 92355828.

- RV-NUMA-6FDBD35C: (no answer yet - skip)
- RV-BENCH-UTILS-92355828: (no answer yet - skip)

## Findings (ordered by risk, then size)

### RV-NUMA-6FDBD35C - `detect()` re-probes `/sys` on every call — no memoized variant, and consumers already call it per-instance construction

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-numa/src/platform/detect.rs:24-30; crates/shamir-numa/src/platform/linux.rs:52-53; crates/shamir-index/src/base_index/meta/index_info.rs:146,370
- Recipe: Introduce shared cached topology for constructors while retaining an explicit fresh-probe path.
- Note: Repeated synchronous discovery remains; caching needs topology-change policy.

### RV-BENCH-UTILS-92355828 - next_range can reach hi even when next_f32 is strictly inside its promised interval

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-bench-utils/src/vector_data.rs:84-87
- Recipe: Enforce exclusive range endpoints, define invalid-bound handling and add adjacent-float tests with revision bump.
- Note: Adjacent-float interpolation can reach hi independently of next_f32 returning 1.

### RV-BENCH-UTILS-F3BD32CC - `Lcg::next_f32` violates its documented `[0, 1)` contract — can return exactly 1.0

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-bench-utils/src/vector_data.rs:50,73-81
- Recipe: Use an exact 24-bit half-open mapping, add endpoint predecessor-seed tests and bump generator revision.
- Note: Arithmetic implies high words can round upward to yield exactly 1.

### RV-NUMA-553CB81D - Test-coverage claims vs reality: `detect()` untested off-Linux, `LinuxTopology` error paths untested, README references a non-existent CI workflow

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/test`.
- Evidence at `7f04e096`: bcb21f4e; crates/shamir-numa/src/platform/tests/linux_probe_tests.rs:8-33; crates/shamir-numa/tests/linux_topology.rs:1-18; .github/workflows/numa.yml:40-57
- Recipe: Add off-Linux detect and direct Linux explicit-node out-of-range assertions.
- Note: Injected probe errors are covered now; the workflow allegation is refuted.
- Second pass: Injected probe errors are covered and workflow exists; off-Linux detect and direct Linux node-OOB assertions remain missing.

### RV-NUMA-79E4752D - `load_local` does not meet its documented "identical to a bare `ArcSwap` — zero overhead" contract

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-numa/src/core/node_replicated.rs:35-41,96-97; crates/shamir-numa/src/lib.rs:26-29; crates/shamir-numa/README.md:24-26
- Recipe: Replace absolute zero-overhead claims with the actual constant-time routing contract.
- Note: Single-replica loads still perform dynamic topology routing; latency was not measured.
- Second pass: Single-replica loading still routes through topology; absolute zero-overhead wording is unsupported, not a measured slowdown.

### RV-NUMA-B44795F2 - `detect()` degrades silently -- swallowed probe error has no observability

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-numa/src/platform/detect.rs:25-30; crates/shamir-numa/src/platform/linux.rs:174-180
- Recipe: Expose the fallback reason through a once-only operational diagnostic.
- Note: Intentional fallback still discards the classified source error.

### RV-NUMA-CA89A9AE - Stale "Фаза 1 scope" docs contradict shipped code

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-numa/src/lib.rs:34-40,48-50; crates/shamir-numa/README.md:22,79-81
- Recipe: Update shipped scope and roadmap while distinguishing smoke coverage from full guest integration.
- Note: Future-Linux and fallback-only prose still contradict implemented discovery.

### RV-NUMA-DB772C02 - QEMU smoke oracle counts announcements instead of distinct NUMA nodes

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/test`.
- Evidence at `7f04e096`: scripts/runners/ci-qemu-numa-test.sh:251-252,270-272
- Recipe: Require distinct configured node IDs and add duplicate-announcement rejection fixtures in CI.
- Note: Repeated announcements for one node still satisfy the smoke predicate.

### RV-NUMA-E3044ABE - Redundant guards in `detect()` / `probe()`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-numa/src/platform/detect.rs:25-27; crates/shamir-numa/src/platform/linux.rs:69-77; crates/shamir-numa/src/platform/cpulist.rs:66
- Recipe: Remove the redundant positive-node check and second sorting pass while preserving probe invariants.
- Note: Both redundant guards remain; no correctness impact is established.

### RV-BENCH-UTILS-88D1646B - Cargo.toml `description` advertises the removed BENCH_QUICK tier feature

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-bench-utils/Cargo.toml:6; crates/shamir-bench-utils/src/lib.rs:9-12
- Recipe: Describe the shipped fixture-generation and memory-measurement helpers instead of BENCH_QUICK.
- Note: Package metadata advertises removed functionality.
- Second pass: Metadata still advertises removed BENCH_QUICK functionality; correct the description only.

### RV-COLLECTIONS-4ED3931B - Redundant `use std::cmp::Eq` import

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-collections/src/lib.rs:17-19
- Recipe: Remove only the redundant std::cmp::Eq import.
- Note: Legal redundant prelude import remains; no warning regression claimed.
- Second pass: The explicit prelude Eq import remains legal but redundant; retain independent Hash imports.

### RV-NUMA-81E50CBF - README's "run tests" instruction bypasses the mandated central test entry point

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-numa/README.md:41-42
- Recipe: Document CI use of the central test wrapper and explicit permission for any local execution.
- Note: README still prescribes raw cargo test.

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

`cargo fmt -p shamir-numa -p shamir-bench-utils -p shamir-collections -- --check`; `cargo clippy -p shamir-numa -p shamir-bench-utils -p shamir-collections --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
