# P2-08 - shamir-storage: range termination, rename copy recovery, cache/test/doc cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Stop the default range filter at the upper bound, define the partial-copy contract of copy_store (rename retry recovery is deferred), remove the environmental thread-start panic, validate Fjall store names, and tidy storage docs and tests.

This is one change set for shamir-storage: 21 findings, effort units 43 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. 

## Scope

Crates: `shamir-storage`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- The default range filter stops polling after the first key above the end bound (oracle: no further polls).
- copy_store has an explicit documented partial-copy contract (incremental merge/upsert, no rollback) pinned by a contract test; rename retry contamination stays unremediated and is deferred to a separately scoped engine/database change; Fjall worker-start failure no longer panics.
- Documentation and test-organization cards are closed without behavior change.

## Coordination with other tasks

Suggested wave 1; no strict predecessor.
- record_id.rs (38F208C4, 9794FD53) is also edited by P2-12; collections lib.rs (8FEF1C5A) by P2-19. P2-12 and P2-19 start after P2-08.

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: 38F208C4, 9478B37E, 0A9E3A41, CC5C79BA, 1A5A9374, 88C4F2CC, FB7C0794.

- RV-STORAGE-38F208C4: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Approve documented probabilistic uniqueness, not collision-safe insertion. Replace uniform-128-bit and provably-pointless-probe claims with the actual relative-timestamp plus 64-bit Xoshiro-tail layout and explicitly accept collision/overwrite risk; qualify probability estimates by equal timestamp prefixes and the random-tail model. Preserve generation, encoding and insertion behavior; do not add a check-then-insert probe. Documentation-only: no behavioral regression test is required. Coordinate any record_id.rs documentation edit with P2-12.
- RV-STORAGE-9478B37E: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Choose explicit partial-copy semantics: copy_store incrementally merges/upserts into the destination, preserves unrelated keys, provides no snapshot or rollback guarantee, and may leave partial results on error or cancellation. Do not delete caller-owned destinations. Extend contract tests to prove completed batches, pre-existing unrelated keys and matching-key overwrites survive a later failure. Record rename retry contamination as unremediated and revise P2-08's recovery acceptance criterion; a rename-owned recovery protocol requires a separately scoped engine/database change.
- RV-STORAGE-0A9E3A41: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Fix worker startup only: make WriteWorker::spawn fallible and propagate startup failure through insert and nonempty transact using existing DbError::Io. Preserve lazy, single-worker initialization under concurrent first use; a private OnceLock<Result<WriteWorker, std::io::Error>> may cache startup failure, with propagated owned I/O errors preserving kind and source. Inject startup failure in CI tests and prove both methods, including subsequent attempts, return errors without panic or writes. Defer general source-chain redesign; preserve existing public error variants, conversions and wire codes.
- RV-STORAGE-CC5C79BA: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Add one Fjall-local store-name validator accepting nonempty names of at most 255 UTF-8 bytes, without character filtering or normalization. Validate ordinary get/delete, atomic staging and every commit name before keyspace resolution; return DbError::Validation. Preserve valid existing names and other backends' contracts. CI tests cover empty, 255/256-byte and multibyte names, accepted punctuation, and mixed valid/invalid batches with no keyspace creation or writes. Confirm this compatibility boundary against locked Fjall 3.1.10; locally inspected 3.1.6 source is not evidence of the locked implementation.
- RV-STORAGE-1A5A9374: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Clarify ordering documentation only: one worker executes jobs FIFO in actual channel-admission order, not concurrent invocation order. There is no universal submission order across handles, worker jobs, blocking routes or repository batches; callers needing sequencing must await preceding operations. Correct transact's every-point-write ordering claim. Preserve routes, visibility and durability behavior, and do not imply atomic reader visibility. No behavioral regression test is required.
- RV-STORAGE-88C4F2CC: **SKIP** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Defer interface-polish debt: the private stream alias is externally expressible, and changing Bytes bounds, copy signatures or public error payloads adds compatibility cost without a demonstrated runtime benefit. Preserve those APIs. Apply copy semantics and worker-start decisions under their dedicated cards; retain separately scoped alias-reuse and documentation cleanup without duplicating them here.
- RV-STORAGE-FB7C0794: **SKIP** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Accept current allocation exposure pending a demonstrated ingress path and compatibility-backed size policy. KeyBytes constructors and serialization accept arbitrary lengths; a Deserialize-only cap would reject previously round-trippable keys and cannot universally prevent allocation inside the underlying decoder. No remote exploit path is established by this review. Preserve encoding and accepted lengths; bounded diagnostics remain an independent change, not justification for a universal key-size cap.

## Findings (ordered by risk, then size)

### RV-STORAGE-0786A4D6 - Trait-default range filter scans PAST the upper bound forever

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-storage/src/core/types.rs:578-604
- Recipe: Terminate the ascending range wrapper at the first above-end key and add a no-further-polls oracle.
- Note: Finite excess traversal, not an established infinite loop.
- Second pass: Cached ascending scans inherit continued polling above the upper bound; finite excess traversal, not a demonstrated infinite loop.

### RV-STORAGE-38F208C4 - Fresh random 128-bit id claim behind skipping the insert collision probe is false

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_fjall.rs:178-186; crates/shamir-types/src/types/ids/record_id.rs:44-61
- Recipe: Approve the actual probabilistic uniqueness contract or implement cross-handle collision-safe insertion.
- Note: Timestamp plus 64-bit random tail is not a uniform 128-bit random ID.

### RV-STORAGE-9478B37E - Repo::copy_store default impl leaves a partially-populated destination on failure

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-storage/src/core/types.rs:647-661; crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:332-338
- Recipe: Choose explicit partial-copy semantics or a rename-owned staging and recovery protocol.
- Note: Earlier destination batches survive later failure on the rename path.

### RV-STORAGE-0A9E3A41 - Error-source chains flattened; thread-spawn failure panics instead of DbResult

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-storage/src/core/error.rs:227-234; crates/shamir-storage/src/stores/storage_fjall.rs:121-124; f90cea3d (#361)
- Recipe: Return worker-start failures through DbResult and approve source-preserving error representations.
- Note: Typed interner conversion improved; other flattening and spawn panic remain.
- Second pass: Typed quota conversion improved; lazy worker startup still panics and other error chains remain flattened.

### RV-STORAGE-2F8C6A8C - moka cache built with default RandomState instead of workspace THasher

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_membuffer/core.rs:297,319-334
- Recipe: Parameterize Moka cache types with THasher and construct them with build_with_hasher.
- Note: Default RandomState remains; no measured speed regression.

### RV-STORAGE-B7839F16 - Public-API rustdoc drift: prefetch promise, phantom engines, stale KeyBytes status

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-storage/src/core/types.rs:441,494-500; crates/shamir-storage/src/key_bytes/mod.rs:3-9
- Recipe: Correct prefetch, supported-backend and completed KeyBytes migration documentation.
- Note: Serial Fjall batching contradicts the prefetch promise.
- Second pass: Prefetch, backend and migration narration remain stale; correct documentation rather than adding unnecessary behavior.

### RV-STORAGE-BE72410B - Raw key bytes — including attacker-influenced indexed values — embedded in error messages

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_fjall.rs:617; crates/shamir-storage/src/key_bytes/core.rs:167-171
- Recipe: Use a bounded key diagnostic formatter and test its maximum output length.
- Note: Full numeric byte diagnostics remain; cross-tenant leakage is unproved.
- Second pass: Missing-key diagnostics format full numeric key slices across backends; use bounded diagnostics without claiming proven cross-tenant leakage.

### RV-STORAGE-CC5C79BA - Store names passed to the durable engine unvalidated

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_fjall.rs:224-230,399-408
- Recipe: Apply one approved backend-compatible name validator before ordinary and atomic keyspace operations.
- Note: Ordinary store names remain unchecked; traversal is not established.

### RV-STORAGE-1A5A9374 - NIT: cross-path write ordering inside FjallStore rests only on fjall's internal journal mutex arrival order

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/doc`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_fjall.rs:410-414,522-537
- Recipe: Document per-worker ordering and explicitly decide whether cross-route submission ordering is required.
- Note: Independent routes remain; overlapping reordering alone is not a proved bug.

### RV-STORAGE-8FEF1C5A - User-influenced keys enter non-keyed FxHash maps despite the documented no untrusted hash inputs premise

- Verdict: `CHANGED`; risk `low`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-collections/src/lib.rs:21-48; crates/shamir-storage/src/stores/storage_membuffer/core.rs:141; b3cb621d (#334)
- Recipe: Align the older project-policy premise with binding RU8; do not seed individual storage maps.
- Note: Explicit accepted threat model replaces the unacknowledged premise; policy narration still drifts.
- Second pass: RU8 accepts deterministic hashing of client-controlled inputs; align older narration without seeding individual maps.

### RV-STORAGE-9794FD53 - Cross-crate wire-format literal duplicated privately: [0,0,0,0] system-record prefix re-encoded in storage_mirrored

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_mirrored.rs:44-48,173-179; crates/shamir-types/src/types/ids/record_id.rs:122-137
- Recipe: Use existing RecordId length validation and system classification while retaining the tag allowlist.
- Note: Private literals agree today but duplicate the wire classifier.

### RV-STORAGE-DCBCB721 - Stale module doc in key_bytes.rs claims the type is unused and that RecordKey = Bytes

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-storage/src/key_bytes/mod.rs:3-9,54-56; crates/shamir-storage/src/core/types.rs:11
- Recipe: Replace pre-migration narration with the live KeyBytes alias and representation obligations.
- Note: Unused-KeyBytes and RecordKey-equals-Bytes claims remain false.

### RV-STORAGE-88C4F2CC - Interface polish nits

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `L/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-storage/src/core/types.rs:13-14,488,506-510,647; crates/shamir-storage/src/core/error.rs:13-26
- Recipe: Approve a cohesive API-polish scope before changing aliases, bounds, copy semantics or public errors.
- Note: Interface debt remains; external implementations are not disproved.

### RV-STORAGE-3D6F974D - storage_membuffer_tests.rs packs three unrelated fixture topics into nested inline mods instead of topic files

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-storage/src/tests/stores/storage_membuffer_tests/clear_race_tests.rs:32,139; crates/shamir-storage/src/tests/stores/storage_membuffer_tests/mod.rs:5
- Recipe: Split clear-race fixture topics through a topical subdirectory without growing the seven-entry folder.
- Note: Other topics were extracted; two nested fixture modules remain reachable.
- Second pass: Reachable clear-race fixtures still contain nested topics; split through the existing folder entry.

### RV-STORAGE-6FEF6FDD - Duplicate private RecordStream alias re-declared instead of importing the canonical one

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:5; crates/shamir-storage/src/tests/types_tests.rs:12-13; crates/shamir-storage/src/tests/stores/storage_cached_tests/basic_tests.rs:618
- Recipe: Import the canonical RecordStream alias in remaining test consumers.
- Note: Production MemBuffer duplication is gone; test aliases remain.
- Second pass: Production alias reuse is implemented; two test consumers independently redeclare RecordStream.

### RV-STORAGE-B26793CA - Nit — stale narration of retired mechanisms in #535 test/hook docs and mid-body imports

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-storage/src/test_seams/membuffer_clear_race_hook.rs:53-70; crates/shamir-storage/src/tests/stores/storage_membuffer_tests/clear_race_tests.rs:120-137
- Recipe: Update retired-boolean narration to the current oracle and hoist ordinary body imports.
- Note: Main header improved but batch-hook and test narration remain stale.

### RV-STORAGE-B6B3CBB6 - Minor allocations/clones on batched paths

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:519-524; crates/shamir-storage/src/stores/storage_fjall.rs:834,900
- Recipe: Evaluate ownership-preserving miss handling and bounded batch-aware reservation using allocation evidence.
- Note: Vector clone and fixed initial capacities remain; benefit is unmeasured.

### RV-STORAGE-B7703B8B - Orphaned Tests banner comments left behind after tests moved out

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_in_memory.rs:368-372; crates/shamir-storage/src/stores/storage_fjall.rs:945
- Recipe: Remove surviving empty Tests banners without changing test registration.
- Note: Cosmetic remnants remain despite the CachedStore rewrite.

### RV-STORAGE-F26D060C - Function-local imports violate the mandatory Imports at the top rule

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-storage/src/core/types.rs:554,583,648; crates/shamir-storage/src/stores/storage_fjall.rs:827
- Recipe: Hoist ordinary function and closure imports to file headers and remove duplicates.
- Note: Do not confuse valid nested-module header imports with body imports.

### RV-STORAGE-FB7C0794 - Nit: KeyBytes::Deserialize allocates an unbounded blob before any size check

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-storage/src/key_bytes/core.rs:242-245
- Recipe: Approve a key-size limit and enforce it with a bounded deserialization visitor before growth.
- Note: ByteBuf materialization remains uncapped; direct remote reachability is unestablished.

### RV-STORAGE-458CFAD8 - Drifted line-number reference in a comment

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-storage/src/stores/storage_fjall.rs:812,872
- Recipe: Remove the stale numeric location and retain the iter_stream method reference.
- Note: The comment still points to approximately line 323.

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

`cargo fmt -p shamir-storage -- --check`; `cargo clippy -p shamir-storage --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
