# P2-16 - shamir-wasm-host: runtime-compile SDK path, bounded compiler output, allocation and metadata cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Make runtime Rust-source compilation independent of the build machine checkout, bound the compiler output, validate pooling limits against custom function limits, and clean up allocation and metadata handling.

This is one change set for shamir-wasm-host: 14 findings, effort units 22 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. Bounding compiler output must keep the shamir-proc-tree lifecycle guarantees.

## Scope

Crates: `shamir-wasm-host`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- compile_rust_source resolves the SDK from a supported, configurable source and fails with an actionable error when it is missing (owner answer).
- Compiler output retained in memory is bounded with a truncation marker, keeping the proc-tree lifecycle guarantees; pooling ceiling versus custom function limits is validated.

## Coordination with other tasks

Suggested wave 2; no strict predecessor.
- wasm_function.rs is also edited by P2-17 (723F5FD2): P2-17 starts after P2-16. core.rs of shamir-db (4669FDA6) is also edited by P2-01.

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: C17D12C8, 4669FDA6, 98551A44, EB1654FA, FE02BB1D, FF7C2D6E, EBD32CA5.

- RV-WASM-HOST-C17D12C8: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Add an explicit SDK-directory compile option and SHAMIR_WASM_SDK_DIR fallback, with explicit configuration taking precedence and invalid explicit settings producing actionable errors. Preserve existing entry points and the checkout-relative development fallback. Support a trusted version-matched SDK source bundle including sibling path dependencies; do not introduce registry downloads. Safely escape manifest paths and coordinate native command arguments with 796ECAE4. CI must prove precedence, invalid-bundle errors, and compilation without the original checkout. Evidence: crates/shamir-wasm-host/src/compile.rs:640–667; crates/shamir-sdk/Cargo.toml:3–9.
- RV-WASM-HOST-4669FDA6: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Add checked FunctionMeta::try_from_record, retaining from_record as a legacy compatibility API but migrating both production catalogue/security callers. Write separate metadata_version=1; absent version selects legacy decoding with defaults only for absent fields. Reject malformed present fields, non-string grants, non-map records, and unsupported versions. Decode before recovery registration and return contextual initialization errors; malformed metadata in effective_fn_actor retains the caller without setuid escalation. Preserve artifact revision version. Coordinate DB scope with P2-01. CI must cover legacy defaults, versioned round trips, rejection, no partial registration, and no malformed-metadata escalation. Evidence: crates/shamir-wasm-host/src/builtin/meta.rs:113–179; crates/shamir-db/src/shamir_db/shamir_db/core.rs:476–485; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:251–265.
- RV-WASM-HOST-98551A44: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Add engine options for an explicit pooling memory ceiling, defaulting to the current 64 MiB, and preserve existing constructors as wrappers. Track actual allocator mode and effective ceiling; reject above-ceiling limits in both WAT and binary constructors with requested/available bytes and remediation. Do not apply a pool-derived rejection to on-demand engines or pooling fallback. CI must prove early rejection, growth above 64 MiB with a larger configured pool, and unchanged on-demand behavior. Land before P2-17. Evidence: crates/shamir-wasm-host/src/wasm/wasm_engine.rs:101–121,204; crates/shamir-wasm-host/src/wasm/wasm_function.rs:266–277,303–316,526–528.
- RV-WASM-HOST-EB1654FA: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Add a bounded-capture supervisor companion in shamir-proc-tree and use it for guest Cargo, preserving the existing supervise API and other callers' retention semantics. Retain at most 64 KiB per pipe plus a fixed truncation marker, continuously draining excess output to EOF. Preserve process-tree termination, bounded drain/reap, and teardown diagnostics. Expand the brief scope to shamir-proc-tree. CI must cover oversized simultaneous pipes, truncation, normal exit, timeout, and inherited-pipe descendants. Evidence: crates/shamir-proc-tree/src/supervise.rs:68–77,137–190; crates/shamir-wasm-host/src/compile.rs:715–726.
- RV-WASM-HOST-FE02BB1D: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Map actual task cancellation to existing Cancelled and task panic to existing Compute, preserving Argon2id context and string panic payloads with a stable non-string fallback. Do not add an error variant or resume unwinding. CI must inject string/non-string panics and cancellation and verify distinct classification while retaining ordinary KDF behavior. Evidence: crates/shamir-wasm-host/src/builtin/argon2id.rs:78–80; crates/shamir-wasm-host/src/runtime/error.rs:32–34,54–56.
- RV-WASM-HOST-FF7C2D6E: **SKIP** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Record as superseded by component cards, not accepted unresolved correctness debt. Weak ticker ownership and the shared gateway matcher already exist. Complete native path arguments under 796ECAE4 and checked metadata under 4669FDA6; authorize no duplicate implementation or regression suite. Evidence: crates/shamir-wasm-host/src/wasm/wasm_engine.rs:164–172; crates/shamir-wasm-host/src/gateway/net_gateway.rs:20; crates/shamir-wasm-host/src/compile.rs:701–703; crates/shamir-wasm-host/src/builtin/meta.rs:124–141.
- RV-WASM-HOST-EBD32CA5: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Remove only the unused direct serde dependency. Crate-wide Rust search finds prose mentions, not consumers. Preserve SDK-generated manifest behavior and transitive dependencies; make no measured build-savings claim. Existing isolated all-target CI checks suffice without a new behavioral test. Evidence: crates/shamir-wasm-host/Cargo.toml:15; crates/shamir-wasm-host/src/compile.rs:61,574.

## Findings (ordered by risk, then size)

### RV-WASM-HOST-C17D12C8 - `compile_rust_source` hardwires the SDK path to the build machine's `CARGO_MANIFEST_DIR`

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/compile.rs:641-645,667
- Recipe: Define a deployment-supported SDK source and configurable runtime resolution with actionable errors.
- Note: Optional source compilation still depends on the build-machine checkout.
- Second pass: Optional source compilation still requires the build-checkout SDK path; deployment-supported SDK resolution needs an explicit contract.

### RV-WASM-HOST-4669FDA6 - Catalogue record decoding has silent fallbacks and no format versioning

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/builtin/meta.rs:113-141; crates/shamir-db/src/shamir_db/shamir_db/core.rs:484
- Recipe: Introduce checked version-aware metadata decoding with explicit malformed-field errors and legacy defaults.
- Note: Invalid present fields still default or disappear silently.

### RV-WASM-HOST-203F2147 - `host_call` rebuilds the secret-grants set on every nested call instead of cloning the `Arc`

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/wasm/bindings/call.rs:96,161; crates/shamir-wasm-host/src/runtime/context.rs:660-661
- Recipe: Add a crate-private shared-grants setter and reuse the existing immutable Arc.
- Note: Nested calls still clone grant strings into a fresh set.

### RV-WASM-HOST-98551A44 - Pooling allocator hardcodes `max_memory_size` to `WasmLimits::default()`, silently capping configured limits

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/wasm/wasm_engine.rs:204; crates/shamir-wasm-host/src/wasm/wasm_function.rs:303-316,526-528
- Recipe: Define an engine memory ceiling and reconcile accepted function limits with pooling capacity.
- Note: Custom store limits remain independent of the default pool ceiling.

### RV-WASM-HOST-EB1654FA - Unbounded stdout/stderr buffering of the guest `cargo build`

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/compile.rs:718,723-726; crates/shamir-proc-tree/src/supervise.rs:68-77
- Recipe: Drain both pipes continuously while retaining capped diagnostics with explicit truncation markers.
- Note: Unbounded retention moved into shared process supervision.

### RV-WASM-HOST-796ECAE4 - `compile.rs` degrades invalid-UTF-8 temp paths into `unwrap_or("")` arguments for cargo instead of a typed error

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/compile.rs:701-703
- Recipe: Pass Cargo paths as native Path or OsStr arguments instead of Unicode strings.
- Note: Failed Unicode conversion still substitutes empty arguments.

### RV-WASM-HOST-C5833F4E - Per-invocation input prep: deep params clone + msgpack encode + redundant `.to_vec()` copy

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/wasm/wasm_function.rs:471-474
- Recipe: Retain encoded bytes directly and encode a borrowed parameter map without cloning its tree.
- Note: Deep map clone and encoded-buffer copy remain.
- Second pass: Invocation retains deep map cloning and encoded-buffer copying; borrowed encoding needs byte-equivalence coverage.

### RV-WASM-HOST-FE02BB1D - Panic inside the Argon2id blocking task is collapsed into `FunctionError::Cancelled`, discarding the panic payload

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/builtin/argon2id.rs:78-80
- Recipe: Distinguish blocking-task panic from cancellation and preserve a diagnostic panic cause.
- Note: Every JoinError still becomes Cancelled.

### RV-WASM-HOST-FF7C2D6E - Nits bundle (correctness-tdd finding 12, five items, distributed to their primary lenses)

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/wasm/wasm_engine.rs:164-171; crates/shamir-wasm-host/src/gateway/net_gateway.rs:20,107; crates/shamir-wasm-host/src/compile.rs:701-703; crates/shamir-wasm-host/src/builtin/meta.rs:124-141
- Recipe: Complete the remaining path and metadata components through cards 796ECAE4 and 4669FDA6.
- Note: Ticker ownership, shared glob and duplicate prose are resolved; two components remain.
- Second pass: Weak ticker ownership is implemented; empty-path fallbacks and silent malformed-grant filtering remain, with metadata policy requiring ratification.

### RV-WASM-HOST-741D48B5 - `wasm/mod.rs` doc list omits the sanitizer re-exports

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/wasm/mod.rs:3-5,16
- Recipe: Include verify_wasm_module and SANCTIONED_HOST_IMPORTS in the public-surface documentation list.
- Note: The sanitizer re-exports remain omitted from the stated list.
- Second pass: The public-surface list still omits both sanitizer exports.

### RV-WASM-HOST-8FA4CADD - Redundant artifact copies in `maybe_wasm_opt`

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/compile.rs:744,754,761,794,798
- Recipe: Pass the owned artifact Vec into maybe_wasm_opt and return it directly on fallback.
- Note: Unavailable-tool and failed-optimization paths still copy the artifact.

### RV-WASM-HOST-92EA3A88 - Mid-body `use` statements violate the "Imports at the top" rule (7 sites)

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/compile.rs:715-719; crates/shamir-wasm-host/src/tests/runtime/compile_tests.rs:112,129,141,154,189,553
- Recipe: Hoist the remaining test-body imports into the file header.
- Note: Two old reader imports disappeared, but six test-body imports remain.
- Second pass: Old reader imports disappeared; six ordinary test-body imports remain.

### RV-WASM-HOST-C43E9092 - Internal audit-tracking references leaked into public API docs

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-wasm-host/src/gateway/net_gateway.rs:148,161,174,267
- Recipe: Remove internal finding labels while retaining the substantive DNS-pinning explanation.
- Note: Public API docs still reference finding 2c.

### RV-WASM-HOST-EBD32CA5 - unused `serde` dependency

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/dep`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wasm-host/Cargo.toml:15; crates/shamir-wasm-host/src/compile.rs:61,574; crate-wide Rust reference search
- Recipe: Remove the unused direct serde dependency.
- Note: Only explanatory source mentions remain; transitive serde use is unrelated.

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

`cargo fmt -p shamir-wasm-host -- --check`; `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
