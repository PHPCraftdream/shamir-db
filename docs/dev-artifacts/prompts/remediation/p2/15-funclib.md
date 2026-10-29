# P2-15 - shamir-funclib: numeric order contract, Argon2 oracle, documentation and test maintenance

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Fix the 15 findings below in one coherent change set for shamir-funclib (effort units 25: S=1, M=2, L=4). They are adjacent: same crate and overlapping files, so they are solved together and share tests and review. #330 knowingly kept the non-transitive comparator; a comparator fix must revisit the distinct-hashing contract.

## Scope

Crates: `shamir-funclib`. Dependents are type-checked with `cargo check` when a public signature changes.

## Decisions

Filled by the orchestrator before launch (owner answers). Empty means: skip the cards marked OWNER DECISION: 0FCB1771.

- (none yet)

## Findings (ordered by risk, then size)

### RV-FUNCLIB-0FCB1771 - `compare`'s documented "total (transitive)" order is not strictly guaranteed on the lossy fallback paths

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/value/compare/core.rs:107,124,143; crates/shamir-funclib/src/functions/numeric/agg/dedup.rs:1
- Recipe: Implement exact cross-numeric ordering, reconcile distinct hashing, and add boundary-triple and permutation oracles.
- Note: Lossy mixed equality remains non-transitive; #330 explicitly preserves it.
- Second pass: Lossy mixed numeric equality remains non-transitive and bucket aggregation explicitly preserves first-representative semantics.

### RV-FUNCLIB-22DF7C3C - `trusted_pure` gate: pub fields make the "explicit opt-in" convention-only; docs claim indexability the gate forbids

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/registry.rs:95,121,143; crates/shamir-funclib/src/functions/numeric/arrays/core.rs:29; crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_admitted.rs:230
- Recipe: Correct indexability claims and clarify explicit native embedder vouching while retaining the eligibility gate.
- Note: Documentation mismatch remains; public fields are not an untrusted bypass.
- Second pass: Indexability prose contradicts unvouched builtin metadata; engine eligibility enforcement remains intact and native metadata is trusted.

### RV-FUNCLIB-935392A1 - Argon2 concurrency-cap test still relies on scheduling-dependent saturation after the claimed deterministic-flake fixes

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/crypto/tests/crypto_tests.rs:246,279; crates/shamir-funclib/src/functions/crypto/core.rs:276
- Recipe: Establish saturation with a permit-held test rendezvous and controlled release rather than KDF duration.
- Note: Readiness barrier does not force overlap; no flake reproduced.

### RV-FUNCLIB-110F6E17 - Argon2id semaphore: poison-propagating `expect`s, and the in-flight counter leaks on panic

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/crypto/core.rs:158,160,170,233,280,285
- Recipe: Use RAII in-flight accounting and poison recovery for the unit notification mutex, with targeted unwind oracles.
- Note: Permit release is RAII, telemetry is not; production poison trigger unestablished.

### RV-FUNCLIB-37BE26F9 - Nits (test organization / dead code / minor drift)

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/text/validate/core.rs:364; crates/shamir-funclib/src/functions/value/canonical/core.rs:186; crates/shamir-funclib/src/functions/numeric/agg/core.rs:830,866; crates/shamir-funclib/src/tests/value_nav_tests.rs:143; df72cadc; b5129d39
- Recipe: Remove unused boolean state, complete type_of coverage, and consolidate remaining layout and header facets with related cards.
- Note: Regex caching and canonical serialization error propagation are fixed; other facets remain.
- Second pass: Regex caching and canonical error propagation are implemented; unused boolean state, incomplete type_of coverage and related organization facets remain.

### RV-FUNCLIB-3E2AD533 - `use` statements inside function bodies

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/lib.rs:77; crates/shamir-funclib/src/scalar_resolver.rs:103; crates/shamir-funclib/src/functions/crypto/tests/crypto_tests.rs:208,288
- Recipe: Hoist the cited function-local imports into their file headers with appropriate test-only boundaries.
- Note: The cited imports still have no documented applicable exception.

### RV-FUNCLIB-7539483C - `registry` contract tests nested under `math/tests/`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/numeric/math/tests/mod.rs:2; crates/shamir-funclib/src/functions/numeric/math/tests/registry_tests.rs:1
- Recipe: Move registry contract tests into the existing root registry test group and update manifests and imports.
- Note: Wrong ownership placement, not missing test discovery.

### RV-FUNCLIB-8447708B - Single-line JSON literals in tests

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/tests/encode_tests.rs:164; crates/shamir-funclib/src/functions/text/validate/tests/validate_tests.rs:186
- Recipe: Reformat the cited valid object JSON fixtures as multiline indented literals without changing test semantics.
- Note: Parser and codec fixtures violate formatting guidance, not builder-only query rules.

### RV-FUNCLIB-9F35E215 - Doc/code drift: `random()` documented as returning `F64`, actually `Dec`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/numeric/gen/core.rs:15,63; crates/shamir-funclib/src/registry.rs:350; crates/shamir-funclib/src/functions/numeric/gen/tests/gen_tests.rs:75
- Recipe: Document random as returning Dec and tighten its shape oracle to that existing return type.
- Note: F64 documentation contradicts the explicitly decimal-first constructor.

### RV-FUNCLIB-BDCBEE51 - Tests for five flat modules consolidated in `src/tests/` instead of per-module `tests/` dirs

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/tests/mod.rs:1; crates/shamir-funclib/src/lib.rs:89
- Recipe: Relocate the four category suites into owner-module tests directories while preserving root-owned registry tests.
- Note: Four category suites remain root-shared; discovery wiring exists.

### RV-FUNCLIB-06F1EB02 - `value_nav` int-step-into-map allocates a key `String` per step

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/value_nav.rs:121
- Recipe: Format integer map keys into a stack buffer and borrow the formatted signed decimal string.
- Note: Lookup is correct; per-step String allocation remains.
- Second pass: Registered navigation functions reach allocating integer-map lookup; stack formatting must preserve signed-i64 semantics.

### RV-FUNCLIB-50CD6B2E - `ScalarRegistry::register` collision policy undocumented

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/registry.rs:166,175
- Recipe: Document last-registration-wins for duplicate qualified scalar names.
- Note: Insertion overwrites; registration docs omit collision behavior.

### RV-FUNCLIB-7EDC7D1C - `strings/substring` doc comment contradicts implemented (and tested) behavior

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/text/strings.rs:133,140; crates/shamir-funclib/src/tests/strings_tests.rs:73
- Recipe: Document past-end substring starts as empty output and negative arguments as errors.
- Note: Existing oracle agrees with implementation, not the comment.

### RV-FUNCLIB-C1C6EEC7 - Doc drift: encode.rs header lists `str_escape_chars` as a registered function name

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/value/encode.rs:7,16,142
- Recipe: Replace str_escape_chars with json_escape in the registered function catalog.
- Note: Alias explanation exists but the catalog still advertises the helper name.

### RV-FUNCLIB-E6DE0D0F - `cast/to_bool` allocates a lowercased copy of the input per call

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-funclib/src/functions/value/cast/core.rs:178
- Recipe: Replace lowercase allocation with borrowed trimmed ASCII-case-insensitive comparisons.
- Note: Allocation remains; latency impact unmeasured.

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

`cargo fmt -p shamir-funclib -- --check`; `cargo clippy -p shamir-funclib --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
