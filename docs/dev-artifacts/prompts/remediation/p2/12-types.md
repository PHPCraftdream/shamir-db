# P2-12 - shamir-types: interner hydration validation, decoder arithmetic, logging safety, cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Reject conflicting interner hydration state while keeping legitimate recaptures, close the 32-bit binary-length overflow, stop logging raw resource names unescaped, and fix docs and test hygiene.

This is one change set for shamir-types: 14 findings, effort units 28 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. Keep identical delta recaptures and the contiguous-prefix cursor from H13.

## Scope

Crates: `shamir-types`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- Conflicting hydrated ids/names are rejected while identical delta recaptures and the contiguous-prefix cursor keep working (H13).
- The 32-bit binary-length arithmetic is checked like the string case; raw resource names are not interpolated unescaped into logs.

## Coordination with other tasks

Suggested wave 2; starts after P2-01, P2-08, P2-09 are merged.
- Starts after P2-01, P2-08 and P2-09 (shared files). Touches shamir-db authorize.rs (71AFA12F, A7B1FCEC): rebase on P2-01. Shares types/value.rs with P2-04 in the same wave (small overlap, resolve in the integration merge).

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: 5E4BFC76, A7B1FCEC, EFA1B8F4, FE40CB54.

- RV-TYPES-5E4BFC76: (no answer yet - skip)
- RV-TYPES-A7B1FCEC: (no answer yet - skip)
- RV-TYPES-EFA1B8F4: (no answer yet - skip)
- RV-TYPES-FE40CB54: (no answer yet - skip)

## Findings (ordered by risk, then size)

### RV-TYPES-973FEEFD - `Interner::with_state` silently collapses duplicate ids/names in hydrated state

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `L/code`.
- Evidence at `7f04e096`: crates/shamir-types/src/core/interner/interner.rs:149-154; crates/shamir-engine/src/table/support/storage/interner_manager.rs:248-255,292
- Recipe: Validate hydrated ID/name bijection fallibly, allow identical recaptures, reject conflicts, and propagate corruption errors.
- Note: Quota deduplication and cursor repair do not validate conflicting pairs.
- Second pass: Hydration still accepts conflicting pairs; quota and persisted-cursor repairs do not validate the ID/name bijection.

### RV-TYPES-5E4BFC76 - Generic `bincode::from_bytes` helper lacks the depth caps its sibling decoders have

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-types/src/codecs/basic/bincode.rs:41-45; crates/shamir-engine/src/table/support/storage/interner_manager.rs:248
- Recipe: Define trusted-type/input constraints and require explicitly bounded decoding for recursive untrusted roots.
- Note: No wrapper depth policy; inspected production root is nonrecursive, not a proven QueryValue exploit.
- Second pass: Generic decoding has no depth policy; the inspected production root is nonrecursive, so no production QueryValue exploit is established.

### RV-TYPES-A7B1FCEC - Authorization helpers allocate fresh Strings + Vecs per check on the access-gate path

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-types/src/entries/access/resource_path.rs:155-190; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:56-67
- Recipe: Introduce borrowed ancestor traversal and compatible metadata lookup while preserving nearest-first awaited authorization.
- Note: Admin bypass remains; non-admin traversal still materializes cloned ancestor paths.

### RV-TYPES-71AFA12F - Log-forging surface: raw resource names rendered into trace/denial lines

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-types/src/entries/access/resource_path.rs:198-218; crates/shamir-types/src/entries/access/policy.rs:77-78; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:73-78,103-106
- Recipe: Escape controls at trace and diagnostic boundaries without changing resource identity or URI Display.
- Note: Raw-name logging surface remains; sink-specific forged-line exploitation is unverified.

### RV-TYPES-E82C6BE1 - `merge_storage_bytes` allocates intermediate Vecs per NEW set_map entry before copying into the output buffer

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-types/src/codecs/interned/messagepack.rs:657-679
- Recipe: Serialize replacement and new-entry payloads directly into the output buffer, preserving byte parity.
- Note: Temporary encoding buffers and copies remain; material impact is unmeasured.

### RV-TYPES-EFA1B8F4 - `mpack!` rejects negative (or any multi-token) literals as OBJECT values — asymmetric with arrays/lists, uncovered by tests

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-types/src/macros/mpack.rs:239-260; crates/shamir-types/src/macros/tests/mpack_tests.rs:33,56
- Recipe: Add negative-literal object arms and CI cases with and without trailing commas while retaining explicit expression escapes.
- Note: Scalar negative coverage does not exercise object-value grammar.

### RV-TYPES-F070E432 - `pos + len` unchecked additions in the tree decoder's `read_str`/`read_bin`

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-types/src/codecs/interned/messagepack.rs:133-152
- Recipe: Use checked range arithmetic in read_bin and add a malformed maximum-length Bin32 CI oracle on 32-bit.
- Note: read_str is checked now; read_bin retains unchecked cursor-plus-length arithmetic.
- Second pass: String arithmetic is checked; Bin32 still reaches unchecked cursor-plus-length arithmetic and requires a 32-bit oracle.

### RV-TYPES-FE40CB54 - `SecretString`: constant-time-comparison footgun plus avoidable `unsafe`

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-types/src/entries/secret.rs:21,67-73
- Recipe: Use safe String zeroization, settle public comparison semantics, and add lifecycle and feature CI coverage.
- Note: Ordinary equality and avoidable unsafe remain; no live timing-sensitive comparison established.

### RV-TYPES-FEB0B5E4 - Predictable RecordId random tail and embedded timestamps — invariant undocumented

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-types/src/types/ids/record_id.rs:41-60,64-72,94-102
- Recipe: Document timestamp disclosure and non-secret identifier semantics, explicitly prohibiting capability-by-possession use.
- Note: Collision-oriented PRNG rationale exists but the public security-use boundary remains undocumented.

### RV-TYPES-178ED8CB - Mid-function imports scattered through test files

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/test`.
- Evidence at `7f04e096`: crates/shamir-types/src/codecs/interned/tests/bytes/merge_storage_bytes_tests.rs:296,318
- Recipe: Hoist the shared RecordView import to the file header and remove function-local duplicates.
- Note: Inspected imports do not fit the documented local-import exceptions.

### RV-TYPES-2A360D16 - README describes an obsolete locking model for `Interner`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-types/src/core/README.md:64-67,76,84; crates/shamir-types/src/core/interner/interner.rs:84-93
- Recipe: Update interner layout, return types, and locking descriptions to match ArcSwap reverse reads and serialized publication.
- Note: README still describes reverse DashMap and Mutex counter.

### RV-TYPES-313627EB - `QueryValue::set_path` builds the error-message path prefix during successful traversals too

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-types/src/types/value.rs:666,678-682,691,703
- Recipe: Track traversed byte extent and allocate the error prefix only when returning NotAMap.
- Note: Successful traversals still construct diagnostic-only text.

### RV-TYPES-8AC2CFC5 - Nit: vacuous self-comparison assertion in `from_ts_produces_unique_ids_with_same_timestamp`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/test`.
- Evidence at `7f04e096`: crates/shamir-types/src/types/tests/record_id_tests.rs:159-163,185-194
- Recipe: Compare timestamp prefixes against independently calculated expected bytes in the uniqueness test.
- Note: Same-constructor comparison remains; separate layout coverage limits the gap.

### RV-TYPES-A31DDF76 - Dead "Tests" section banner left behind after inline-test extraction

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-types/src/core/sort_codec.rs:152-154
- Recipe: Remove the empty trailing Tests banner.
- Note: Implementation file still ends with a banner and no test body.
- Second pass: The implementation still ends with an empty Tests banner; removal is sufficient.

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

`cargo fmt -p shamir-types -- --check`; `cargo clippy -p shamir-types --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
