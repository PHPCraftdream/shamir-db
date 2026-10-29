# P2-07 - shamir-index (managers, runtime registry, model): posting cache, DROP sweeps, registry scans, docs

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Give the posting cache a byte budget, page the DROP sweeps, replace the O(N) registry scans, and clean up docs and convention debts in the index managers and runtime model.

This is one change set for shamir-index: 13 findings, effort units 24 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. Files under src/base_index/** (except infra/index_manager_unique hash code owned by P2-06) and src/model/**.

## Scope

Crates: `shamir-index`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- The posting cache has a byte budget with an eviction test; DROP sweeps page their key list; registry lookups are not O(N) per call.
- No local use statements or inline cfg(test) modules remain in the touched files; docs match the shipped behavior.

## Coordination with other tasks

Suggested wave 3; starts after P2-06, P2-12 are merged.
- Starts after P2-06 (tokenizer.rs) and P2-12 (interner.rs, for 002DDDA6).

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: 95108B6D, 3854F3CC.

- RV-INDEX-95108B6D: (no answer yet - skip)
- RV-INDEX-3854F3CC: (no answer yet - skip)

## Findings (ordered by risk, then size)

### RV-INDEX-002DDDA6 - `IndexExpr::Scalar` evaluation constructs a fresh `Interner` per row

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/query/expr.rs:172-187; crates/shamir-types/src/core/interner/interner.rs:109-112
- Recipe: Use a lazy or evaluation-scoped conversion context while preserving map-key meaning and reentrancy.
- Note: Fresh allocated Interner still constructed for scalar-only evaluations.
- Second pass: Successful scalar operands still allocate fresh scratch interner state; lazy context must preserve map-key meaning and reentrancy.

### RV-INDEX-95108B6D - Posting cache is bounded by entry count, not bytes

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/base_index/index_manager/lookup.rs:488-510; crates/shamir-index/src/base_index/index_manager/inner.rs:34
- Recipe: Reserve retained bytes atomically and account for admission, replacement, removal and invalidation.
- Note: Entry-count eviction does not bound bytes or concurrent admission.
- Second pass: Entry-count eviction bounds neither retained posting bytes nor concurrent admission; budget policy must be specified.

### RV-INDEX-12B88835 - `.unwrap()` on `SystemTime::duration_since(UNIX_EPOCH)` on four DDL success paths

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/base_index/index_manager/drop_rename.rs:260-262; crates/shamir-index/src/base_index/index_manager_unique/create.rs:347-349; crates/shamir-index/src/base_index/sorted_index_manager/ddl.rs:363-365,858-860
- Recipe: Use consistent non-panicking timestamp conversion and cover pre-epoch input.
- Note: All four named DDL status paths still unwrap clock conversion.

### RV-INDEX-1443E02B - `plan_records_created_batch` bypasses the in-flight-online-build dirty-set capture that every other write path performs

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/base_index/index_manager/lookup.rs:55-65,119-133; crates/shamir-engine/src/table/table_manager/transactions/tx_insert.rs:354,571
- Recipe: Apply single-row in-flight dirty capture to batch planning and test online-build interleaving in CI.
- Note: Reachable routing mismatch remains; broader corruption is not demonstrated.

### RV-INDEX-3854F3CC - `lease_by_field_and_kind` is an O(N) full-registry scan on every index2 read dispatch

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/model/runtime/registry.rs:631-643,659-680
- Recipe: Choose multi-match selection semantics and maintain a lifecycle-synchronized reverse lookup.
- Note: Ready lookup still scans the registry and borrows descriptors.

### RV-INDEX-ECE5EBA5 - DROP-sweep paths materialize the entire index key list before removing (O(index-size) memory spike, one giant batch)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/base_index/index_manager/flags.rs:530-540; crates/shamir-index/src/base_index/sorted_index_manager/ddl.rs:160-168
- Recipe: Delete bounded resumable batches while preserving scan correctness, drain ordering and durable tombstones.
- Note: Both sweep implementations retain every key before one removal batch.
- Second pass: Both real DROP sweeps retain all keys before removal; bounded deletion must preserve cursor correctness, reader drain and durable recovery ordering.

### RV-INDEX-32C98E9F - `use` statements inside function/block bodies across six production files

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/query/expr.rs:87-88; crates/shamir-index/src/model/runtime/write_ops.rs:165-166; crates/shamir-index/src/fts/tokenizer.rs:448-449
- Recipe: Hoist ordinary local imports to headers and remove redundant imports without changing cfg scope.
- Note: Multiple live non-exception local imports preserve the finding.

### RV-INDEX-858BB5E1 - `ReaderDrainGate` doc invariant ("Never acquire any other lock while holding a `ReadGuard`") is contradicted in letter by `lookup_by_index`'s DashMap access inside the guard scope

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/runtime/reader_drain_gate.rs:85; crates/shamir-index/src/base_index/index_manager/lookup.rs:411,434-452
- Recipe: Document permitted leaf-cache locking and the actual drain-order invariant in both locations.
- Note: Absolute wording contradicts cache access; no deadlock is established.

### RV-INDEX-97E36480 - Bincode ordinal-stability contract is documented on some persisted enums but missing on others

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/kind.rs:10-40,48-52; crates/shamir-index/src/model/query/expr.rs:19-22
- Recipe: Document append-only bincode ordinal obligations on IndexKind, TokenizerKind and IndexExpr.
- Note: Preventive documentation debt; no current reorder established.
- Second pass: Three persisted enums still lack ordinal warnings; no current reorder or corruption established.

### RV-INDEX-AFA66311 - Feature-gated inline loom test module in an implementation file

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/runtime/reader_drain_gate.rs:306-310; crates/shamir-index/build.rs:13-14
- Recipe: Extract the loom model to a file-backed module while preserving feature gating and CI selection.
- Note: Feature-gated inline model remains; structural cleanup only.

### RV-INDEX-B683BA68 - Comment nits: typo and a stale cross-file doc reference

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/runtime/reader_drain_gate.rs:113; crates/shamir-index/src/base_index/meta/index_definition.rs:48
- Recipe: Correct suffient and replace the stale provenance source reference with the actual type path.
- Note: Both named comment defects remain.
- Second pass: Both comment defects remain; two-file correction is M under the supplied size definition.

### RV-INDEX-20415F00 - Stale lifecycle doc contradicts the shipped `IndexState` wire enum; minor `IndexRecordKey` API warts

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/runtime/lifecycle.rs:31-53; crates/shamir-index/src/model/runtime/state.rs:73; crates/shamir-index/src/base_index/infra/index_record_key.rs:62,104-111
- Recipe: Correct lifecycle documentation and retain public key APIs absent a separate breaking-change decision.
- Note: Failed is shipped; prefix decoding and String errors are not proven correctness bugs.

### RV-INDEX-DE92095C - `apply_index_ops_at_commit` does a linear backend find per provenance group

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/runtime/write_ops.rs:184-190
- Recipe: Build one name-to-backend lookup per application call while retaining provenance semantics.
- Note: Groups-times-backends lookup remains; practical significance unmeasured.

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

`cargo fmt -p shamir-index -- --check`; `cargo clippy -p shamir-index --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
