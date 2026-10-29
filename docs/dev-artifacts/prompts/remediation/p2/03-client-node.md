# P2-03 - shamir-client-node: integer policy, builder contract, credential lifetime, binding hygiene

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Fix the 9 findings below in one coherent change set for shamir-client-node (effort units 19: S=1, M=2, L=4). They are adjacent: same crate and overlapping files, so they are solved together and share tests and review. JS/TS plus napi crate. RV-CLIENT-8997B366 (Rust client resume ticket) belongs to P2-09; keep the zeroization approach consistent.

## Scope

Crates: `shamir-client-node`. Dependents are type-checked with `cargo check` when a public signature changes.

## Decisions

Filled by the orchestrator before launch (owner answers). Empty means: skip the cards marked OWNER DECISION: C42AA40A, 144A5920, 31D3BBE5.

- (none yet)

## Findings (ordered by risk, then size)

### RV-CLIENT-NODE-C42AA40A - Default object-level MessagePack conversion loses integer fidelity and rejects BigInt inputs

- Verdict: `STILL_OPEN`; risk `high`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-client-node/js/wrapper.js:82,105; crates/shamir-client-node/package.json:33
- Recipe: Define a lossless integer policy shared with TS DTOs and test independent int64 fixtures, bigint inputs and integer markers.
- Note: Default codec calls remain; rounding and bigint rejection are inferred, not locally reproduced.
- Second pass: Default codec calls remain; rounding and bigint rejection are inferred from the documented dependency behavior, not reproduced locally.

### RV-CLIENT-NODE-144A5920 - `execute(object)` teaches hand-assembled wire shapes; no builder, no exported BatchRequest/BatchResponse types

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-client-node/src/lib.rs:19-22; crates/shamir-client-node/js/wrapper.d.ts:67
- Recipe: Integrate supported builder and batch DTO exports plus an execution adapter and replace handwritten wire examples.
- Note: Coordinate with the lossless integer policy.
- Second pass: Handwritten wire example and untyped execution remain; TS Batch.execute requires the absent executeWithTouch adapter.

### RV-CLIENT-NODE-684A48EB - "Mirrors the Rust SDK 1:1" is false; resumption ticket getters dead-end with no resume path

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-client-node/src/lib.rs:3; crates/shamir-client-node/js/wrapper.d.ts:58-77
- Recipe: Document the supported Node subset and ticket-getter limitation instead of claiming Rust SDK parity.
- Note: Ticket getters still have no resume input path.
- Second pass: Parity claim remains false and ticket getters have no resume input; a focused supported-subset documentation correction suffices.

### RV-CLIENT-NODE-050EF626 - `create_scram_user` wrapper msgpack-decodes 16 raw user_id bytes and discards the result

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client-node/src/lib.rs:310; crates/shamir-client-node/js/wrapper.js:83,129-135
- Recipe: Distinguish raw successful user IDs from error payloads and test the valid 16-byte error-marker collision fixture.
- Note: Raw ID bytes still enter the kind-only MessagePack error discriminator.

### RV-CLIENT-NODE-4E3D7A44 - Secret copies in the napi struct are not zeroized (`resumption_ticket`, `session_id`, `pin`)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client-node/src/lib.rs:101-105,163,205,370-373
- Recipe: Zeroize owned session and ticket snapshots on destruction and clear them on close without claiming JS-copy erasure.
- Note: Ticket and session snapshots remain ordinary storage; the public-key pin is not secret.

### RV-CLIENT-NODE-F64125FA - Redundant decode + copy on every `repl`/`createScramUser` success, and a Uint8Array copy per `decodeOrThrow`

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client-node/js/wrapper.js:82,118,132
- Recipe: Decode Buffers directly and avoid decoding discarded raw success payloads through unambiguous error discrimination.
- Note: Extra copies and discarded decodes remain; performance impact is unmeasured.

### RV-CLIENT-NODE-C35E4678 - "Zeroised in the native side" overstates what the binding controls

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client-node/src/lib.rs:63-64,121-146,300-305
- Recipe: Wrap native passwords immediately on entry and qualify the guarantee to owned native buffers, excluding JS copies.
- Note: Validation and dispatched-request waits can precede Zeroizing ownership.

### RV-CLIENT-NODE-31D3BBE5 - Version drift: Cargo `0.1.0-alpha.1` vs npm/generated-loader `0.1.0`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-client-node/Cargo.toml:3; crates/shamir-client-node/package.json:3; crates/shamir-client-node/native/index.js:197
- Recipe: Document the owner-approved Rust and npm version relationship rather than silently changing release versions.
- Note: Metadata differs, but this does not itself reject local native loading.

### RV-CLIENT-NODE-4B69BB13 - Sanctioned-exception comment present at `repl` but missing at the identical `execute` rmp-serde boundary

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-client-node/src/lib.rs:243-244,271-272
- Recipe: Add an explicit sanctioned-FFI explanation covering both incoming MessagePack deserialization boundaries.
- Note: Execute still lacks the explanation; the former repl explanation is also gone.
- Second pass: Both incoming MessagePack boundaries lack the explicit sanctioned-FFI explanation; ownership comments do not supply it.

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

`cargo fmt -p shamir-client-node -- --check`; `cargo clippy -p shamir-client-node --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
