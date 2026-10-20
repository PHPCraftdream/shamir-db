# S5c-X PR6b-3 / W4 — port the engine test suites onto the receipt-only contract

Task 121 brief: `docs/dev-artifacts/prompts/remediation/121-s5cx-retire-legacy-follower.md`
(read it, especially §4 "Test disposition"). W1-W3 landed: strict v3 loaders + typed
refusals (`DbError::FollowerStateRefused`, `FollowerRefusalReason`), v3-only
`save_fence_record_v3_aware`, classification at cold open, admission
(`resume::activation::admit_follower` → `FollowerAdmission::{Native, Fresh}`) with fresh
adoption (v3 baseline) in the pipeline, seed writer v3 baseline, and the DELETION of the
legacy executor (`apply_replicated` free fn + `RepoInstance` wrapper, `apply_prepared`,
`apply_batch_with_info`, `apply_replicated_batch_legacy`, `BatchDurabilityGuard`,
`reserve_floor*`, `batch.rs`, `barrier.rs`, legacy seam phases, `advance_replication_bookmark`,
`disable_receipt_lookup`). The ONLY follower apply entry is
`crate::tx::apply_replicated::apply_replicated_batch(&RepoInstance, &[ReplicatedEvent],
bookmark, ApplyPolicy) -> DbResult<ApplyOutcome>` (also re-exported as
`crate::tx::apply_replicated_batch`). The only bookmark writer is
`crate::meta::repl::bookmark_v3::save_bookmark_v3` (+ atomic batches inside the pipeline);
legacy fixtures are encoded BY TESTS via raw `MetaEnvelope` tuples.

## Hard rules
- ⛔ NEVER run any git command that mutates the working tree or index (no reset/checkout/
  clean/stash/restore/rm). Only edit files; the orchestrator commits. Remove deleted test
  FILES with your file tool, never git.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other compile or test
  command. Only `cargo fmt -p shamir-engine` as your LAST step.
- Never silence a lint with `#[allow]`. Gate test-only imports with `#[cfg(test)]`.
- Tests in `tests/` folders, one topic per file, wired into the enclosing `tests/mod.rs`;
  remove deleted files from manifests. Imports at top. `mod.rs` re-exports only.
- ≤7 entries per folder; ≤1000 lines per file; over-limit files/folders must not GROW
  (splitting a test file along topics is allowed and often required).
- In tests, JSON literals are always multi-line and indented. No machine-specific absolute
  paths.

## Scope: crates/shamir-engine tests only
Other crates' tests are W5's scope — do NOT touch `shamir-server` / `shamir-db`.

### 1. `src/tx/tests/apply_replicated_tests.rs`, `apply_replicated_preflight_tests.rs`,
###    `apply_replicated_counter_tests.rs`, `apply_replicated_journal_order_tests.rs`
The design (§4): "Preflight, translation, counters and journal ordering: port the
single-event test families onto the public receipt entry." Port EVERY still-meaningful
guarantee:
- Call `apply_replicated_batch(&follower, &[ReplicatedEvent{..}], watermark, policy)` — a
  single-event batch; repos built on InMemory backends self-adopt (v3 baseline) on the first
  batch. `ApplyOutcome::Applied { local_version }` is the gate high-water mark after the
  chunk; local versions are still contiguous allocations — adapt exact-version assertions to
  read the outcome / gate instead of hard-coding where the old per-event executor's numbers
  no longer apply.
- Idempotency (`Skipped` for `commit_version <= watermark`), rejection reasons
  (`UnknownTable`, `NoMvccStore`, `IndexedTable`, `MissingValue`, `UndecodablePayload`,
  `MissingFieldBinding`, `ConflictingFieldBinding`), interner re-translation, counter
  deltas, downstream re-projection local-version keying: all preserved by the receipt path —
  keep the assertions.
- Journal ordering tests that hooked the LEGACY executor
  (`fire_post_publish_pre_broadcast_test_hook`, prepare-before-publish) must be re-anchored
  on the receipt pipeline's actual ordering: journal verify/confirm → publish → checkpoint
  → broadcast (`receipt/apply.rs::durable_tail`, `receipt/publish.rs`). If a legacy-only
  hook no longer fires anywhere, rewrite the test to assert the receipt phases via
  `test_seam::Phase::{ReceiptPrepared, ReceiptMaterialized, ReceiptCheckpointed}` park/notify
  instead. Do NOT keep tests whose subject is a deleted executor.
- Where a test seeded legacy bookmark bytes to steer the old router, seed v3 via
  `save_bookmark_v3` instead (or rely on fresh adoption).

### 2. `src/tx/follower_durability/receipt/tests/`
- `activation_routing_tests.rs` — REWRITE per design acceptance tests 1/2/3/5: v1/v2/u64
  records (encoded by the test via raw `MetaEnvelope`), INCLUDING bookmark zero, must make
  `apply_replicated_batch` FAIL with the typed refusal (`LegacyBookmarkFormat`); a floor
  marker without a bookmark refuses `apply_replicated_batch` with `LegacyFollowerState` and
  a LEADER repo with the same floor still OPENS and preserves its allocation floor
  (construct the repo, read the gate high-water mark); refusals hold across reopen
  (keep/adapt the fjall reopen case); a valid v3 record (any bookmark value incl. 0) applies
  through the receipt path; `admit_follower` unit coverage: Fresh → Fresh, Native → Native,
  conflicting `SeedInstallState` → refusal, floor → refusal.
- `receipt_apply_tests.rs` — DELETE the forced-legacy negative control (~lines 430-459) and
  its `disable_receipt_lookup` import; everything else stays.
- `crash_subprocess_tests.rs` — DELETE the legacy subprocess replay/convergence block
  (~lines 590-665); KEEP the receipt crash matrix (~563-588) and every other receipt test.
  Keep the harness plumbing the matrix needs.
- `checkpoint_uncertain_tests.rs`, `cancellation_tests.rs`, `ddl_interlock_tests.rs` —
  verify they don't reference deleted items; fix imports only.

### 3. `src/tx/follower_durability/tests/`
- `crash_image_tests.rs` — REWRITE per design: the old fake-store matrix exercised
  `finish_batch`/barrier/v2-writes (deleted). New content (acceptance test 10): construct a
  legacy crash image by hand (floor marker + v2 or u64 bookmark bytes written directly into
  the store), reopen → the apply/open path REFUSES with the typed error; repeat the attempt
  → refuses again; nothing is rewritten, no v3 is minted, counters unchanged. Keep any
  still-valid helper infrastructure.
- `gap_tests.rs` — port the legacy apply caller (~line 49) onto `apply_replicated_batch`;
  retain the J/gap invariants (upstream bookmark vs follower-local clean horizon stay
  distinct; gaps recorded through the receipt path). Remove expectations of successful
  legacy replay.
- `apply_failure_tests.rs`, `apply_crash_tests.rs` — inspect: they drive the legacy phases
  and seams (`AfterPerEventHistoryWrites`, `barrier_action`, `fail_history_flush`,
  `history_written`, `BookmarkPersisted`). Port what the RECEIPT pipeline can still express
  (its own phases + `fail_after` + park/abort machinery, `InjectedMaterializationBatch`
  seams in `stage.rs`); delete tests whose subject is only the retired executor.
- `range_tests.rs` — should be unaffected (`in_flight_range` kept); fix imports only.
- Update `tests/mod.rs` manifest for any file you delete/rename.

### 4. `src/tx/cold_reconcile/tests/`
- `follower_cold_tests.rs` — port the legacy apply/caller (~line 118) onto the receipt entry;
  keep the meaningful J, retained-gap and repeated-reopen invariants (design: "remove
  expectations of successful legacy replay").
- `classification_tests.rs`, `reconcile_tests.rs`, `receipt_recovery_tests.rs`,
  `helpers.rs` — check for deleted-name references; port refusal coverage where the fixture
  used legacy bytes (a legacy fixture now REFUSES the cold open with the typed error — that
  replaces any "legacy follower replays" expectation). The `lifecycle/` tests: fix
  deleted-name references only.

### 5. `src/repo/tests/repl_bookmark_tests.rs` — rewrite
`advance_replication_bookmark` is gone. Cover what survives: `replication_bookmark()` v3
projection (0 on fresh; value from `save_bookmark_v3`); refusal on legacy bytes; delete the
monotonic-advance tests (receipt checkpointing owns advancement; no public advance API).

### 6. `src/repo/seed_install/tests/` — port
- `image_tests.rs` (~line 172): replace `advance_replication_bookmark(8)` with
  `save_bookmark_v3(&info, &BookmarkV3 { fence: None, bookmark: 8, clean_through: Some(1),
  outstanding_receipt: None })` (match the surrounding fixture's semantics).
- Sweep the other files (`lifecycle`, boundary/failure tests) for legacy bookmark writers or
  assumptions (`save_replication_bookmark`, v2 reads); seed completion now writes a v3
  baseline (bookmark = source version, clean_through = 1) — update assertions that read the
  bookmark after `finish` accordingly.

### 7. Engine-wide residue sweep
`rg -n` for every deleted name (list in the W3 brief §7) across `crates/shamir-engine/` —
ALL remaining hits in this crate must be gone when you finish (tests included). Also sweep
`follower_bookmark` (directory gone since W1). Report zero residue or fix it.

## Last step
`cargo fmt -p shamir-engine`.

## Report back
Per file: ported / deleted / rewritten with one-line rationale; the acceptance coverage you
can point to by test name for design tests 1, 2, 3, 5, 10 (state which exist and where);
zero-residue confirmation or remaining hits; files over 900 lines; deviations.
