# P34-03 - shamir-storage: rename copy recovery, decoder budgets, error sources, polish

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Pin the persisted buffer-config bytes with golden tests and state the schema-change policy, flatten the remaining test wrapper, add CI-measured allocation evidence before touching the batched-path clones, and, after the owner answer, make rename/create into a previously used table name safe against stale stores.

This is one change set for shamir-storage: 7 active findings, effort units 21 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-storage`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs` (9478B37E)
- `crates/shamir-engine/src/table/tests/table_core/config/buffer_config_tests.rs` (CCF2090E)
- `crates/shamir-index/src/model/meta/meta_envelope.rs` (CCF2090E)

## Acceptance

- The v1 MetaEnvelope bytes of the buffer config are pinned by golden tests and the schema-change policy is written next to them (CCF2090E).
- 9478B37E: a red CI test shows whether rename-swap, drop-then-create and retry after an injected copy failure merge stale rows. The fix is implemented only when the Decisions section contains the owner answer.
- Flattening the audit_2_3 wrapper (3D6F974D) keeps leaf test names or the report states the discovery-name change.
- B6B3CBB6 changes code only where a CI allocation measurement shows a benefit.

## Coordination with other tasks

Wave 1; no strict predecessor.
- CCF2090E edits crates/shamir-engine/src/table/tests/table_core/config/buffer_config_tests.rs and reads crates/shamir-index/src/model/meta/meta_envelope.rs; touch only those files outside shamir-storage.
- 9478B37E touches shamir-engine repo/instance/repo_instance/catalog.rs only after the owner answer.

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-STORAGE-9478B37E, RV-STORAGE-FB7C0794-SEMANTIC-FACET, RV-STORAGE-CCF2090E, RV-STORAGE-0A9E3A41-SEMANTIC-FACET, RV-STORAGE-88C4F2CC. The full decision paragraph sits in the finding below.

- L3-RV-STORAGE-9478B37E: **ESCALATE** (waits for the owner).
- L3-RV-STORAGE-FB7C0794-SEMANTIC-FACET: **SKIP** (no change).
- L3-RV-STORAGE-CCF2090E: **IMPLEMENT** (change as decided).
- L3-RV-STORAGE-0A9E3A41-SEMANTIC-FACET: **SKIP** (no change).
- L4-RV-STORAGE-88C4F2CC: **SKIP** (no change).

## Findings (ordered by risk, then size)

### L3-RV-STORAGE-9478B37E - Repo::copy_store default impl leaves a partially-populated destination on failure

- Verdict: `PARTIAL`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- **Decision: ESCALATE** (@oh design decider, 2026-10-30; compat `breaking`, risk `medium`). The owner must choose how name-keyed orphaned table stores are handled. Rename copies into the final __data__/__info__/__history__<to> stores (catalog.rs:329-342). DROP TABLE and the RENAME source leave stores in place (write_domain_membership.rs:26-33; table_management.rs:103-130), and the guard only checks has_table(to). So a rename-swap, DROP-then-RENAME/CREATE into the same name, or a retry after a failed copy appears to merge stale rows (inferred from code; confirm with a red CI test first). Options: (A) under the write permit, after confirming to is not live, store_delete the destination stores before copy, and do the same at CREATE. This deletes orphaned data, and MemBuffer/Cached/Hybrid store_delete coherence must be proven (repo_types.rs:266-274). (B) Refuse RENAME/CREATE with DbError::Validation when destination stores are non-empty. Nothing is deleted, but swap/recreate workflows break. (C) Incarnation-qualified store names in the catalogue, which is a persisted-format migration. Recommend (A), with CI tests: rename-swap does not merge, drop-then-create is empty, retry after an injected copy failure ends with exactly the source keys. Fall back to (B) for any backend whose purge cannot be shown coherent. Real risk is medium.
- This card waits for the owner: do not implement it; report it as SKIPPED(owner).
- Evidence at `07bc804c`: crates/shamir-storage/src/core/types.rs:625-641; crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:329-342
- Recipe: Introduce rename-owned staging or recovery that prevents partial-copy destination contamination.
- Verifier note: Generic partial merge semantics are documented, but rename copies into final destinations without contamination recovery; use rename-owned staging or recovery.

### L3-RV-STORAGE-FB7C0794-SEMANTIC-FACET - Nit: KeyBytes::Deserialize allocates an unbounded blob before any size check

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted allocation exposure, unchanged since P2. KeyBytes::deserialize still materializes a ByteBuf (key_bytes/core.rs:242-247), but no production remote ingress decodes KeyBytes: outside shamir-storage it appears in network-facing crates only in server test files, and WAL entries use Bytes. A Deserialize-only cap would reject keys that round-trip today and cannot bound allocation inside the outer decoder, which transport frame and decode caps already bound. Revisit only when an ingress path and a compatibility-backed key-size policy exist.
- Evidence at `07bc804c`: crates/shamir-storage/src/key_bytes/core.rs:242-245; crates/shamir-storage/src/key_bytes/tests/serde/serde_byte_identity_tests.rs:103-119
- Recipe: Identify decoder ingress and approve compatible size budgets before implementing bounded decoding.
- Verifier note: Public deserialization still materializes an uncapped ByteBuf; no production remote ingress established, so investigate compatible decoder budgets first.

### L3-RV-STORAGE-CCF2090E - Persisted MemBufferConfig wire format has no versioning guardrails despite a stable wire-format claim

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/test`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Pin the v1 persisted bytes of the buffer config and state the schema-change policy. Add golden tests to crates/shamir-engine/src/table/tests/table_core/config/buffer_config_tests.rs. Build MetaEnvelope{magic: ENVELOPE_MAGIC, version: 1, written_at_nanos: <fixed constant>, payload: MemBufferConfig with distinct non-default values}, once with ttl_ms Some and once with None. Assert encode() equals a hand-written byte literal derived from the bincode 1.3 default layout (fixint little-endian, usize as u64, Option as a 1-byte tag plus value). Never paste the current encoder output, which would make the test tautological. Also assert MetaEnvelope::open and buffer_config::load decode those literals into the expected struct. Replace the 'Stable wire-format' doc on MemBufferConfig (storage_membuffer/core.rs:17-18) with the real contract: positional bincode inside MetaEnvelope v1; adding, removing, reordering or retyping a field is a format break that must ship with either a versioned decode path or explicit fail-closed rejection, and the golden literal changes only together with that decision. No behaviour change, no new dependency. Out of scope: migration code, envelope version bump, legacy pre-envelope support.
- Evidence at `07bc804c`: crates/shamir-index/src/model/meta/meta_envelope.rs:62-78; crates/shamir-engine/src/table/tests/table_core/config/buffer_config_tests.rs:313-350
- Recipe: Pin buffer-config v1 bytes and require explicit rejection or migration policy for schema changes.
- Verifier note: Header-first rejection exists; current-type-generated tests do not pin historical buffer-schema bytes or future compatibility policy.

### L3-RV-STORAGE-0A9E3A41-SEMANTIC-FACET - Error-source chains flattened; thread-spawn failure panics instead of DbResult

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted, deliberately deferred diagnostic debt. P2 already made Fjall worker startup fallible and kind/source-preserving (storage_fjall.rs:148-156,541-557), which was the only Result-discipline violation. Codec/backend flattening into the public DbError::Codec(String)/Storage(String) variants (error.rs:30,52,227-248) stays: about 500 construction sites and tests matching Codec(_) mean a source-bearing redesign would be a workspace-wide public enum change with no runtime benefit. Preserve public variants, conversions and wire codes, as decided in P2.
- Evidence at `07bc804c`: crates/shamir-storage/src/stores/storage_fjall.rs:148-156,411-414,544-557; crates/shamir-storage/src/core/error.rs:227-246
- Recipe: Approve source-bearing codec/backend representations or retain explicitly deferred diagnostic debt.
- Verifier note: Worker startup preserves I/O kind/source without panic; general codec/backend source flattening remains.

### L4-RV-STORAGE-88C4F2CC - Storage alias/bounds/copy/error-wording polish; external implementations already work

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `L/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Deferred interface polish. The alias, Bytes bounds, copy signature, KeyExists wording and engine-domain errors (types.rs:13-14,476,490-495,631; storage_in_memory.rs:200) do not block external implementations, and changing them is public-API churn without runtime benefit. Keep the copy_store signature stable while 9478B37E is with the owner. Same as the P2 decision.
- Evidence at `07bc804c`: crates/shamir-storage/src/core/types.rs:13-14,476,490-495,631; crates/shamir-storage/src/stores/storage_in_memory.rs:200
- Recipe: Approve a narrowly scoped compatibility-aware interface polish plan or retain current APIs.
- Verifier note: Deferred interface and wording polish remains; external implementation is not blocked.

### L4-RV-STORAGE-B6B3CBB6 - Minor allocations/clones on batched paths

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:519-524; crates/shamir-storage/src/stores/storage_fjall.rs:869,934
- Recipe: Measure allocations in CI, then optimize miss-key ownership and bounded reservation only where beneficial.
- Verifier note: Miss-key cloning and fixed scan reservations remain; optimize only with CI allocation evidence and bounded reservation.

### L4-RV-STORAGE-3D6F974D - storage_membuffer_tests.rs packs three unrelated fixture topics into nested inline mods instead of topic files

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/test`.
- Evidence at `07bc804c`: crates/shamir-storage/src/tests/stores/storage_membuffer_tests/audit_2_3_tests.rs:19-24; crates/shamir-storage/src/tests/stores/storage_membuffer_tests/clear_race_tests/mod.rs:3-4
- Recipe: Flatten the remaining audit_2_3 wrapper while preserving test names and registration.
- Verifier note: Race topics are split; audit wrapper remains. Flattening preserves leaf names but changes fully qualified discovery names.

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

`cargo fmt -p shamir-storage -- --check`; `cargo clippy -p shamir-storage --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
