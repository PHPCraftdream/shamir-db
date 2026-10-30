# P2-17 - shamir-sdk (+ sdk-macros): ABI contracts, macro consumer contract, conventions cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Document and enforce the SDK ABI contracts, support renamed-dependency consumers of the macros, and clean up conventions.

This is one change set for shamir-sdk, shamir-sdk-macros: 11 findings, effort units 24 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. 

## Scope

Crates: `shamir-sdk`, `shamir-sdk-macros`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- SDK ABI contracts are documented and enforced where the card asks; macros work for renamed-dependency consumers (owner answers).
- Convention cleanup closes without ABI changes beyond the cards.

## Coordination with other tasks

Suggested wave 3; starts after P2-16 are merged.
- Starts after P2-16 (wasm_function.rs); P2-14 starts after P2-17 (sdk api/db.rs).

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: 77261152, 44A9CC7A, E3875787, C8189C06, 723F5FD2.

- RV-SDK-77261152: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Choose signature-only scalar documentation: scalar receives one Params argument and no injected Ctx, but is not a purity or security boundary. Preserve Ctx construction and current host authorization/capabilities; add no scalar-specific import denial. Add a consumer compile fixture constructing Ctx inside a valid scalar without executing imports, retain invalid-signature rejection, and coordinate documentation/tests with RV-SDK-MACROS-44A9CC7A. Do not characterize construction as unauthorized host access.
- RV-SDK-MACROS-44A9CC7A: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Retain type_contains_ctx only as a best-effort signature diagnostic and remove semantic-purity claims. Document that generated concrete Params forwarding and Rust type checking determine signature compatibility, not lexical token matching. CI consumer fixtures must accept a Params alias and body-local Ctx construction, reject an SDK Ctx argument alias through type checking, and preserve the direct-Ctx diagnostic. Use the signature-only policy from RV-SDK-77261152; add neither semantic alias resolution nor capability enforcement.
- RV-SDK-E3875787: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Restrict packed pointer-result production and generated raw-pointer guest entrypoints to wasm32, preserving WASM export names, signatures and packed layout. Preserve native SDK/macro compilation and use typed encode/decode helpers and native-width ownership interfaces for native tests; never pack native allocations into guest pointer fields. CI must prove native consumers compile without executable guest pointer exports and WASM consumers retain the exact ABI. Coordinate target gating with response-range validation and retain native-width ownership tests.
- RV-SDK-MACROS-C8189C06: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Support optional crate = path on all four SDK attributes, defaulting to ::shamir_sdk, and substitute the parsed path throughout generated SDK references. Reject unknown options, duplicate crate options and malformed arguments while preserving no-argument behavior. Use existing syn/quote dependencies, not a new resolver dependency or Cargo-manifest discovery. CI must compile canonical and renamed-only consumers for every attribute and reject unsupported arguments; coordinate expansion edits with native ABI gating.
- RV-SDK-723F5FD2: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Document the existing interoperable ABI as nonnegative signed-i32 pointer/length fields with SDK exchange buffers in the lower 2 GiB and bounds-checked ranges. Preserve signatures, negative-address rejection and existing memory configuration limits. Explicitly distinguish the final packed-result reader's unsigned extraction rather than claiming all paths already reject high-bit pointers. Add CI boundary fixtures for negative/high-bit signed fields, valid ranges and out-of-bounds ranges without multi-gigabyte allocation. Coordinate host documentation/fixtures after P2-16 and align SDK response-range validation with this contract.

## Findings (ordered by risk, then size)

### RV-SDK-77261152 - Documented scalar "purity guarantee" is a token-match lint, trivially bypassed by a type alias

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-sdk/src/api/context.rs:14-16,64-65; crates/shamir-sdk-macros/src/lib.rs:583-591
- Recipe: Choose signature-only purity documentation or host-enforced scalar capability isolation.
- Note: A valid scalar body can construct Ctx directly.

### RV-SDK-MACROS-44A9CC7A - `type_contains_ctx` purity check is lexical, not semantic

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-sdk-macros/src/lib.rs:523-532,552-556,604-610; crates/shamir-sdk/src/api/context.rs:64-75
- Recipe: Define host-enforced scalar capability policy and replace the lexical purity guarantee with accurate documentation and tests.
- Note: Body-local Ctx construction remains possible; host-policy bypass not asserted.

### RV-SDK-17BC120C - Guest ABI builds slices from host-returned `(ptr, len)` with no sanity check (undocumented unsafe trust assumption)

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-sdk/src/runtime/__rt.rs:118-159,190-196
- Recipe: Validate packed response ranges centrally before ownership reconstruction and test malformed ranges.
- Note: Allocation trust is documented; range validation remains absent.
- Second pass: Ownership trust is documented, but packed response ranges remain unchecked before slice construction.

### RV-SDK-E3875787 - `leak_result` truncates pointers on 64-bit host targets (latent UB in the host-testing ABI path)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-sdk/src/runtime/__rt.rs:92-96
- Recipe: Restrict packed result ABI to wasm32 and use native-width interfaces for native testing.
- Note: Pointer truncation persists; native dereference harm is conditional.

### RV-SDK-MACROS-C8189C06 - Generated code hardcodes `shamir_sdk::` paths; `_attr` tokens silently ignored

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-sdk-macros/src/lib.rs:107,243,375,565,605-606
- Recipe: Resolve dependency aliases and reject nonempty unsupported attribute tokens with consumer CI fixtures.
- Note: Canonical generated paths and ignored attributes both remain.
- Second pass: Generated SDK paths remain canonical and attribute arguments ignored; approve any new resolver dependency and add consumer compile fixtures.

### RV-SDK-9ABDB893 - `Value` edge cases: `visit_u64` wrap-around and NaN/Infinity untestable under `PartialEq`

- Verdict: `PARTIAL`; risk `low`; fix size/kind `S/test`.
- Evidence at `7f04e096`: crates/shamir-sdk/src/api/value.rs:125-134; eea842a6 (#313)
- Recipe: Add NaN bit/classification and infinity MessagePack fixtures.
- Note: Unsigned wrap-around fixed; float-edge coverage remains.
- Second pass: Unsigned wrap is removed; independent NaN and infinity conformance fixtures remain absent.

### RV-SDK-MACROS-5D1F61B0 - `#[procedure]` doc example does not compile

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-sdk-macros/src/lib.rs:355-362; crates/shamir-sdk/src/api/db.rs:112
- Recipe: Propagate query errors with ? and return Value::List(rows); compile the corrected example in CI.
- Note: Ignored example returns incompatible Result<Vec<Value>> as Value.

### RV-SDK-723F5FD2 - Guest ABI passes pointers as signed `i32`; host rejects addresses ≥ 2 GiB

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `L/doc`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-sdk/src/runtime/__rt.rs:52-58; crates/shamir-wasm-host/src/wasm/wasm_function.rs:366-367,407-409
- Recipe: Explicitly define the below-2-GiB ABI range and align memory documentation and boundary fixtures.
- Note: Signed-address rejection remains outside ordinary small-memory use.
- Second pass: High-bit pointers remain rejected; default 64 MiB memory limits reachability, but the supported ABI range still needs an explicit cross-crate contract.

### RV-SDK-9D3236E4 - Function-body `use` imports in `src/tests/` violate the "Imports at the top" rule

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-sdk/src/tests/value/value_tests.rs:146,279-280; crates/shamir-sdk/src/tests/validation_tests.rs:239,305
- Recipe: Hoist non-exempt test imports to file headers.
- Note: Convention-only debt survives the test-file move.

### RV-SDK-8BC846C6 - Convention nits

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-sdk/src/api/db.rs:104-111; crates/shamir-sdk/src/api/value.rs:9; b2e6383e (#363)
- Recipe: Correct the stale Value test path together with RV-SDK-DDF49297.
- Note: Builder exception is documented; stale path remains.
- Second pass: Bridge exception is documented; stale Value test path remains.

### RV-SDK-DDF49297 - `value.rs` module doc points at a test path that does not exist

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-sdk/src/api/value.rs:9; crates/shamir-sdk/src/tests/value/value_tests.rs:1
- Recipe: Replace the stale test path with src/tests/value/value_tests.rs.
- Note: The documented test location remains wrong.

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

`cargo fmt -p shamir-sdk -p shamir-sdk-macros -- --check`; `cargo clippy -p shamir-sdk -p shamir-sdk-macros --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
