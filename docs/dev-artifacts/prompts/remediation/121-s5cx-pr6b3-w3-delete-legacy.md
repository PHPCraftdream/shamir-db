# S5c-X PR6b-3 / W3 — delete the legacy executor, codecs' last callers, seams, advance API

Task 121 brief: `docs/dev-artifacts/prompts/remediation/121-s5cx-retire-legacy-follower.md`
(read it first). W1 landed strict v3 + typed refusals; W2 made
`apply_replicated_batch` unconditionally route the receipt pipeline, replaced the activation
selector with `resume::activation::admit_follower`, converted the seed writer and the cold
open, and mapped typed refusals through the server. The legacy apply machinery is now dead
code. You DELETE it.

## Hard rules
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
  command that mutates the working tree or index. Only edit files; the orchestrator commits.
  To delete a FILE, remove it with your file tool (never git) and list it in your report.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other compile or test
  command. Only `cargo fmt -p shamir-engine` (and `-p shamir-server` only if you touched it)
  as your LAST step.
- Never silence a lint with `#[allow]` — delete the dead code instead.
- ≤7 entries per folder, ≤1000 lines per file. Imports at top. `mod.rs` re-exports only.
  No machine-specific absolute paths. No new heavy awaits (heap-pin if one appears).

## Deletions

### 1. `crates/shamir-engine/src/tx/apply_replicated.rs`
- DELETE: `pub async fn apply_replicated` (free fn), `async fn apply_prepared`,
  `struct BatchDurabilityGuard` + its `Drop`, `pub async fn apply_replicated_batch_legacy`,
  `pub(crate) async fn apply_batch_with_info`, `async fn reserve_floor` +
  `async fn reserve_floor_in`, and the `guard: Option<shamir_tx::VersionGuard>` field of
  `PreparedEvent` including its `guard: None` initializer and any destructure of it.
  KEEP `PreparedEvent.version` (the receipt pipeline reads it), `prepare_event`,
  `reproject_for_downstream`, `ApplyOutcome`, `ApplyRejectReason`, `ApplyPolicy`,
  `PreparedTable`, and `apply_replicated_batch` (receipt-only since W2).
- Clean now-unused imports (e.g. `single_barrier`, the recovery_marker save/snapshot fns,
  `publish_journaled` if nothing else uses it — VERIFY with rg before removing; the receipt
  modules import `reproject_for_downstream` via `super::super::super::`, so that fn stays).
- REWRITE the module doc: it currently narrates the retired allocate→reserve→publish→
  flush→bookmark executor. New doc: the follower apply entry is `apply_replicated_batch` →
  the receipt pipeline (`follower_durability::receipt`); this module retains the shared
  preparation/validation/interner-translation/downstream-projection semantics
  (`prepare_event`, `reproject_for_downstream`) that the receipt path calls, plus
  `ApplyOutcome`/rejection reasons/policy. Keep the trust-precondition and local-version
  rationale paragraphs that still describe receipt execution; delete the reserve-floor /
  finalize-tail / BatchDurabilityGuard narrative that describes only the retired executor.

### 2. Whole-file deletions
- `crates/shamir-engine/src/tx/follower_durability/batch.rs` (persist_batch_record,
  finish_batch — the retired ordering).
- `crates/shamir-engine/src/tx/follower_durability/barrier.rs` (single_barrier,
  durable_journal_barrier, verify_events_and_flush, verify_events_without_flush,
  verify_events, serialize_event). The journal durability obligation SURVIVES through the
  receipt implementation's own exact-byte read-back + explicit flush + FIFO confirmation
  (`receipt/checkpoint.rs::verify_journal/confirm_journal`) and cold repair through the sole
  writer (`cold_reconcile/receipt_recovery.rs`) — do NOT remove those.
Update `crates/shamir-engine/src/tx/follower_durability/mod.rs`: drop `mod barrier; mod
batch;` and the `durable_journal_barrier`/`single_barrier`/`finish_batch` re-exports; keep
`range` (`pub use range::in_flight_range` — cold reconcile uses it), `receipt`, `test_seam`,
`tests`.

### 3. `crates/shamir-engine/src/tx/follower_durability/test_seam.rs` — prune legacy members
- DELETE `Phase::{HistoryWritten, AfterPerEventHistoryWrites, JournalConfirmed,
  AfterJournalFlush, BeforeCombinedHistoryFlush, BookmarkPersisted}`; KEEP
  `Phase::{ReceiptPrepared, ReceiptMaterialized, ReceiptCheckpointed}`.
- DELETE the now-orphan helpers `history_written`, `barrier_action`, `fail_history_flush`,
  and the `Hook` fields `fail_journal_barrier`, `bypass_journal_barrier`,
  `fail_history_flush`.
- KEEP: `install`/`remove`, the `park_at`/`abort_at`/`panic_after` machinery inside
  `phase`, `fail_after`/`fail_after_sticky`/`fail_after_reopen` + `take_fail_after`,
  `reached`/`resume` notifies, and the `Phase` enum itself (the crash harness and receipt
  tests drive these).

### 4. `crates/shamir-engine/src/repo/repo_instance.rs`
- DELETE `advance_replication_bookmark` and its doc (search for it; ~line 1202) — receipt
  checkpointing owns the bookmark advance; seed completion stays an explicitly authorized
  initialization op. There is NO replacement API (tests are ported to `save_bookmark_v3` in
  W4/W5).
- DELETE the `apply_replicated` method wrapper and its doc (~line 1150) — the standalone
  single-event entry is retired; `apply_replicated_batch` is the only follower apply API.
- KEEP `replication_bookmark()` (strict v3 projection; used by the server loop and shamir-db).

### 5. Re-exports
`crates/shamir-engine/src/tx/mod.rs`:
`pub use apply_replicated::{apply_replicated, ApplyOutcome, ApplyPolicy};` →
`pub use apply_replicated::{apply_replicated_batch, ApplyOutcome, ApplyPolicy};`

### 6. Bench port
`crates/shamir-engine/benches/replication_apply.rs` calls the deleted free
`apply_replicated`. Port it to `apply_replicated_batch` (one-event batches; a fresh repo
self-adopts on the first batch via W2's admission). Keep the bench name/target so CI
invocations stay valid, and the file's harness conventions
(bench_scale_tool::Harness — see `crates/shamir-engine/benches/tx_pipeline.rs` for the
template). Measure the receipt pipeline; do not keep a stale benchmark shape.

### 7. Production reference sweep (fix, then report)
`rg` the whole workspace (production trees: `src/` and `benches/`, excluding `tests/`
directories) for: `apply_replicated_batch_legacy`, `apply_batch_with_info`, `apply_prepared`,
`BatchDurabilityGuard`, `reserve_floor`, `single_barrier`, `durable_journal_barrier`,
`verify_events_and_flush`, `verify_events_without_flush`, `finish_batch`,
`persist_batch_record`, `advance_replication_bookmark`, `disable_receipt_lookup`,
`HistoryWritten`, `AfterPerEventHistoryWrites`, `JournalConfirmed`, `AfterJournalFlush`,
`BeforeCombinedHistoryFlush`, `BookmarkPersisted`, `history_written`, `barrier_action`,
`fail_history_flush`. FIX every production (non-test) reference yourself. TEST references
are expected residue — list them all with file:line for W4/W5 (expected: the
`follower_durability/tests/` files, `receipt/tests/*`, `cold_reconcile/tests/*`,
`tx/tests/apply_replicated*`, `repo/tests/repl_bookmark_tests.rs`,
`seed_install/tests/image_tests.rs`, server/db tests).

## Report back
Deleted files; every deleted symbol; the bench port decision; the full test-residue list
(file:line); files you touched now over 900 lines; deviations with reasons.
