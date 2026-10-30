# P34-08 - shamir-types + shamir-collections: secret equality contract, decode caps, allocation, doc and import cleanup

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Add the Drop-dispatch regression for SecretString, keep the ratified trusted-root constraint of the generic bincode helper as a documented limitation, close the allocation and 32-bit gaps carried over from P2 with real oracles, and clean up documentation and imports.

This is one change set for shamir-types, shamir-collections: 6 active findings and 4 P2 carry-overs, effort units 16 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-types`, `shamir-collections`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs` (71AFA12F)

## Acceptance

- The SecretString regression exercises Drop dispatch, not only the wipe helper; its feature-gated test is compiled with its feature (FE40CB54).
- Direct username-bridge determinism and i64-range assertions exist (65DDE5B8).
- Allocation-count oracles that fail on the old code exist for 71AFA12F, E82C6BE1 and 313627EB; the 32-bit-only oracle of F070E432 is documented as such.
- Remaining test-body imports are hoisted (178ED8CB); documentation cards change comments only.

## Coordination with other tasks

Wave 1; no strict predecessor.
- RV-TYPES-A7B1FCEC is implemented in P34-06 (its edits are in shamir-db).

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-TYPES-5E4BFC76, RV-TYPES-FE40CB54. The full decision paragraph sits in the finding below.

- L3-RV-TYPES-5E4BFC76: **SKIP** (no change).
- L3-RV-TYPES-FE40CB54: **IMPLEMENT** (change as decided).

## Findings (ordered by risk, then size)

### L3-RV-TYPES-5E4BFC76 - Generic `bincode::from_bytes` helper lacks the depth caps its sibling decoders have

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Trusted-root constraint ratified; documented limitation (the P2-12 rustdoc has landed at codecs/basic/bincode.rs:30-33). The only production roots are non-recursive values read from the database's own storage: the interner chunk Vec<(InternerKey, UserKey)> (interner_manager.rs:221, :248) and the u64 record counter (record_counter.rs:304). Any future recursive untrusted root must use a type-specific decoder that caps depth while it decodes. The generic from_bytes signature and encoding stay unchanged.
- Evidence at `07bc804c`: crates/shamir-types/src/codecs/basic/bincode.rs:30-50
- Recipe: Ratify trusted-root constraints and require bounded type-specific decoding for any recursive untrusted input.
- Verifier note: The generic public helper still lacks depth enforcement; inspected storage roots are nonrecursive.

### L3-RV-TYPES-FE40CB54 - `SecretString`: constant-time-comparison footgun plus avoidable `unsafe`

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). The comparison contract stays as P2-12 chose it: ordinary PartialEq/Eq, documented as not constant-time, with no new comparison API or dependency. Add only a Drop-dispatch regression. In crates/shamir-types/src/entries/secret.rs, add a #[cfg(all(test, feature = "crypto"))] thread_local counter that wipe() increments (precedent: LIVE_REQUEST_BUFFERS in crates/shamir-sdk/src/runtime/__rt.rs). Extend crates/shamir-types/src/entries/tests/secret_wipe_tests.rs to assert that wipe runs exactly once when a SecretString is dropped, for empty and Unicode secrets, separately for a value and its clone. Also assert that into_inner returns the full cleartext and that Drop then wipes only the emptied buffer. The test must fail if Drop stops calling wipe. It must read no freed memory and use no unsafe. Feature gating, redacted Debug, serde pass-through, Clone and into_inner stay unchanged.
- Evidence at `07bc804c`: crates/shamir-types/src/entries/secret.rs:21-22,68-81; crates/shamir-types/src/entries/tests/secret_wipe_tests.rs:3-12
- Recipe: Owner-select the comparison contract and add a regression that exercises Drop dispatch rather than only the wipe helper.
- Verifier note: Safe wiping landed; ordinary equality remains warned against, and helper-only coverage does not detect disconnected Drop dispatch.

### L3-RV-TYPES-65DDE5B8 - Untested public surface with documented contracts: `Interner::generation`, `ResourceMeta::owner_field`, `principal64_from_username`, `SecretString::into_inner`, `ResourcePath::WasmCompiler`

- Verdict: `PARTIAL`; risk `low`; fix size/kind `S/test`.
- Evidence at `07bc804c`: crates/shamir-types/src/entries/tests/access_tests.rs:8-35; crates/shamir-types/src/entries/access/identity.rs:33-39
- Recipe: Add direct username-bridge determinism and i64-range assertions without claiming collision resistance.
- Verifier note: Projection coverage is not direct username-bridge determinism/range coverage; other named surfaces have assertions.

### L4-RV-TYPES-178ED8CB - Mid-function imports scattered through test files

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-types/src/entries/tests/access_tests.rs:307-310; crates/shamir-types/src/core/interner/tests/interner_tests/user_and_touch.rs:28; crates/shamir-types/src/codecs/interned/tests/bytes/storage_bytes_tests.rs:438
- Recipe: Hoist remaining non-exempt test imports into their file headers.
- Verifier note: Ordinary non-exempt test-body imports remain across multiple files; scoped hoisting is sufficient.

### L4-RV-COLLECTIONS-201D84D3-FACET - Public rustdoc discoverability and local lint-comment presentation

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-collections/src/lib.rs:13; crates/shamir-collections/src/lib.rs:20-175
- Recipe: Explain the sanctioned collection-facade lint allowance immediately beside it.
- Verifier note: Public documentation is repaired; the sanctioned lint allowance still lacks adjacent rationale.

### L4-RV-TYPES-F8CF2706 - API-polish bundle (five items; two folded into other findings)

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `07bc804c`: crates/shamir-types/src/macros/mpack.rs:279-282; crates/shamir-types/src/types/value.rs:19-28
- Recipe: Correct the helper prose from two implementations to three without changing the API.
- Verifier note: Alias and unsealed-helper guidance are corrected; implementation-count prose still says two while listing three.

## Carried over from the P2 sprint

These P2 cards stayed open after the sprint (state from `P2-SPRINT-REPORT-2026-10-30.md`). Their evidence and recipe are the P2 revalidation at master `7f04e096`; re-locate the code by symbol.

### RV-TYPES-71AFA12F - Log-forging surface: raw resource names rendered into trace/denial lines

- Sprint state: PARTIAL: implemented; no allocation-count oracle.
- Risk `low`; fix size/kind `M/code`.
- Evidence: crates/shamir-types/src/entries/access/resource_path.rs:198-218; crates/shamir-types/src/entries/access/policy.rs:77-78; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:73-78,103-106
- Recipe: Escape controls at trace and diagnostic boundaries without changing resource identity or URI Display.
- Verifier note: Raw-name logging surface remains; sink-specific forged-line exploitation is unverified.

### RV-TYPES-E82C6BE1 - `merge_storage_bytes` allocates intermediate Vecs per NEW set_map entry before copying into the output buffer

- Sprint state: PARTIAL: implemented; no allocation-count oracle.
- Risk `low`; fix size/kind `M/code`.
- Evidence: crates/shamir-types/src/codecs/interned/messagepack.rs:657-679
- Recipe: Serialize replacement and new-entry payloads directly into the output buffer, preserving byte parity.
- Verifier note: Temporary encoding buffers and copies remain; material impact is unmeasured.

### RV-TYPES-F070E432 - `pos + len` unchecked additions in the tree decoder's `read_str`/`read_bin`

- Sprint state: PARTIAL: implemented; 32-bit-only oracle.
- Risk `low`; fix size/kind `M/code`.
- Evidence: crates/shamir-types/src/codecs/interned/messagepack.rs:133-152
- Recipe: Use checked range arithmetic in read_bin and add a malformed maximum-length Bin32 CI oracle on 32-bit.
- Verifier note: String arithmetic is checked; Bin32 still reaches unchecked cursor-plus-length arithmetic and requires a 32-bit oracle.

### RV-TYPES-313627EB - `QueryValue::set_path` builds the error-message path prefix during successful traversals too

- Sprint state: PARTIAL: implemented; no allocation-count oracle.
- Risk `nit`; fix size/kind `S/code`.
- Evidence: crates/shamir-types/src/types/value.rs:666,678-682,691,703
- Recipe: Track traversed byte extent and allocate the error prefix only when returning NotAMap.
- Verifier note: Successful traversals still construct diagnostic-only text.

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

`cargo fmt -p shamir-types -p shamir-collections -- --check`; `cargo clippy -p shamir-types -p shamir-collections --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
