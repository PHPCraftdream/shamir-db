# 28 — S5c-X PR6b-2c receipt activation, W4: routing, cold recovery, shutdown, AS OF gate tests

Parent: `docs/dev-artifacts/prompts/remediation/119-s5cx-receipt-activation.md`. W1–W3 landed in the
working tree: shamir-tx install/resume-guard/zero-floor reads, engine codecs + repo plumbing +
lifecycle generalization, and the receipt pipeline core
(`crates/shamir-engine/src/tx/follower_durability/receipt/**`) with permit-based admission and the
lifecycle-owned worker. Read CLAUDE.md. Hard rules: ⛔ no git-mutating commands (read-only git only);
NO cargo/build/test/clippy; FILE SCOPE only; ≤ 7 folder entries, ≤ 1000 lines/file, over-limit files
net-zero-or-negative; imports at top; rustfmt-clean; verify every symbol against the current tree.

## FILE SCOPE

- crates/shamir-engine/src/tx/apply_replicated.rs (+6-line routing only; 778 → ≤ 784)
- crates/shamir-engine/src/tx/follower_durability/mod.rs (declare receipt module; drop record re-export)
- crates/shamir-engine/src/tx/follower_durability/record.rs (DELETE — content moves into batch.rs)
- crates/shamir-engine/src/tx/follower_durability/batch.rs (receive persist_batch_record; 78 → ≤ 100)
- crates/shamir-engine/src/tx/cold_reconcile/mod.rs (+1 line)
- crates/shamir-engine/src/tx/cold_reconcile/algorithm.rs (+7-line receipt hook)
- crates/shamir-engine/src/tx/cold_reconcile/receipt_recovery.rs (NEW, 107 lines from old branch)
- crates/shamir-engine/src/tx/cold_reconcile/tests/mod.rs (+1 manifest line)
- crates/shamir-engine/src/tx/cold_reconcile/tests/receipt_recovery_tests.rs (NEW, port from old branch)
- crates/shamir-engine/src/repo/clean_horizon/shutdown_impl.rs (accessor signature only)
- crates/shamir-engine/src/table/tests/as_of_unpublished_gate_tests.rs (NEW, port from old branch)
- crates/shamir-engine/src/table/tests/mod.rs (+1 manifest line)

## Work items

1. **Routing** (`apply_replicated.rs`): port the old 6-line hunk — in `apply_batch_with_info`, after the
   first `requires_reopen` check and BEFORE the prepare loop: `if
   follower_durability::receipt::receipt_path_selected(repo, info).await? { return
   follower_durability::receipt::apply_batch_receipt(repo, info, events, bookmark, policy).await; }`.
   Keep the legacy path byte-identical otherwise (activation rule: WAL-less follower + backend
   `supports_atomic_batch` + stored bookmark v3-or-absent → receipt path; v1/v2 bookmark keeps the
   legacy path; unsupported persistent backend is rejected with `DbError::AtomicBatchUnsupported`
   inside `receipt_path_selected` — already ported by W3).
2. **follower_durability module wiring**: port the old branch's mod.rs (replace `mod record;` +
   `pub(crate) use record::persist_batch_record;` with `pub(crate) mod receipt;`) and batch.rs (receive
   `persist_batch_record` as `pub(super)`, importing `save_bookmark_and_clean_horizon` /
   `save_replication_bookmark` from `crate::meta::repl_bookmark`). DELETE record.rs (the folder stays at
   7 entries: barrier, batch, mod, range, receipt, test_seam, tests).
3. **Cold recovery**: port `cold_reconcile/receipt_recovery.rs` verbatim from
   `git show feat/s5cx-follower-receipt-2:crates/shamir-engine/src/tx/cold_reconcile/receipt_recovery.rs`,
   with these adaptations: (a) wire `mod receipt_recovery;` into cold_reconcile/mod.rs; (b) in
   algorithm.rs's `reconcile_with_hook`, port the old hunk exactly — complete the outstanding receipt
   BEFORE the `h <= j` early return and the PR6c classification (move the `let info =
   repo.tx_info_store().await?;` line up as the old branch did); (c) `complete_outstanding_receipt`
   must begin with `let permit = repo.write_domain().admit().await.map_err(...)` (uncontended at open)
   and thread `&permit` into `begin_repo_atomic_batch` via the W3 staging functions' signatures — read
   the ACTUAL W3 signatures in the tree first and match them; (d) verify the
   `checkpoint::{verify_journal, confirm_journal, checkpoint}` and
   `stage::{materialize_from_prepared_receipt, materialize_from_materialized_receipt,
   commit_materialized_work, journal_records}` and `publish::{publication_from_receipt, adopt_counters}`
   call shapes against W3's actual code.
4. **Cold recovery tests**: port
   `crates/shamir-engine/src/tx/cold_reconcile/tests/receipt_recovery_tests.rs` (152 lines) — adapt to
   the current helpers in cold_reconcile/tests/helpers.rs and the current receipt codec (tuple version
   2, `ops_per_event` mandatory in any hand-built fixture; use `encode_receipt` rather than hand-rolled
   bytes where possible). Wire into tests/mod.rs preserving its alphabetical-ish grouping style.
5. **Shutdown accessor** (`shutdown_impl.rs`): the `worker_lifecycle()` accessor still names
   `RepoUnitLifecycle` — W3 re-parameterized the field to the receipt job/reply types. Change the
   accessor's return to the ACTUAL field type (grep repo_instance.rs for the `worker_lifecycle` field)
   and update imports; `close`/`shutdown_until`/`quarantine`/`reopen_required` calls are
   parameterization-independent and must not change. Nothing else in the file moves.
6. **AS OF gate tests**: port `crates/shamir-engine/src/table/tests/as_of_unpublished_gate_tests.rs`
   (212 lines) verbatim-adapted: verify every API it uses exists on current master
   (`Query::from(..).select([..]).as_of_version(..).build()`, `FilterContext::new(interner,
   &refs).with_read_visibility_budget(..)`, `TableManager::create(..).with_mvcc_store(..)
   .with_changefeed(..)`, `mvcc.apply_committed_visible`, `mvcc.materialize_committed_to_history`,
   `mvcc.publish_materialized`, `mvcc.version_at_or_before_ts`, `DbError::AsOfVisibilityTimeout {
   requested, visible_through, blocking_version }`, `.code()`); adapt names where master drifted. Wire
   into table/tests/mod.rs. This is the review finding demanding a REAL AsOf read (version route AND
   timestamp route) over a materialized-but-unpublished version — keep both routes and the parked-time
   (`start_paused`, 17 ms budget, poll-once-then-advance) structure; no sleeps, no yield budgets.

## Acceptance criteria

- All files exist/deleted as listed; `wc -l` reported; apply_replicated.rs ≤ 784; follower_durability/
  stays at 7 entries; cold_reconcile/ lands at exactly 7 entries.
- The legacy (non-receipt) follower path and the leader commit path are byte-identical to master apart
  from the routing hunk.
- Final message: files + line counts, the activation rule as implemented (file:line), every deviation,
  anything unverified.
