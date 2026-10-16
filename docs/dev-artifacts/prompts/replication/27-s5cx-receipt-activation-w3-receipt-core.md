# 27 — S5c-X PR6b-2c receipt activation, W3: receipt pipeline core on the merged foundations

Parent: `docs/dev-artifacts/prompts/remediation/119-s5cx-receipt-activation.md`. Binding design:
`git show feat/s5cx-follower-receipt-2:docs/dev-artifacts/prompts/remediation/106-s5cx-follower-receipt-2.md`.
The old branch `feat/s5cx-follower-receipt-2` is a SOURCE OF PARTS: port its receipt pipeline onto the
foundations that now exist IN YOUR WORKING TREE (W1+W2 landed: `RepoWriteDomain`/`RepoWritePermit`
admission + `repo.begin_repo_atomic_batch(permit)` + `BoxRepo` atomic-batch plumbing +
`RecordCounter::adopt_absolute` + receipt codec with `ops_per_event` at tuple version 2 +
`save_bookmark_v3`/`has_v3_record`/refuse-v3-downgrade + lifecycle foundation generalized:
`RepoWorkerLifecycle<J, R>` / `ForegroundLifecycle<J, R>` / `LifecycleJob` / `WorkerExecutionState` /
`WorkerLease` / `install_worker` / `submit` (all exported from
`crate::tx::cold_reconcile::lifecycle`) + shamir-tx `install_durable_events` / `resume_version_guard` /
`ts_index_insert pub` / publication-bounded reads).

Read CLAUDE.md first. Hard rules (non-negotiable): ⛔ no git-mutating commands (read-only git only:
`git show feat/s5cx-follower-receipt-2:<path>` is your source of parts); NO cargo/build/test/clippy;
stay in FILE SCOPE; ≤ 7 folder entries, ≤ 1000 lines/file (receipt/ lands at EXACTLY 7 entries);
imports at top; `mod.rs` = re-exports only; rustfmt-clean by hand; no machine-specific paths; verify
every symbol against the current tree before calling it.

## FILE SCOPE

- crates/shamir-engine/src/tx/follower_durability/receipt/{mod.rs, stage.rs, apply.rs, checkpoint.rs,
  publish.rs} (NEW)
- crates/shamir-engine/src/tx/follower_durability/receipt/resume/{mod.rs, activation.rs, lookup.rs,
  identity.rs, worker.rs} (NEW)
- crates/shamir-engine/src/tx/follower_durability/test_seam.rs (extend with the old branch's additions)
- crates/shamir-engine/src/repo/repo_instance.rs (MINIMAL: one lazy field + clone line + init line +
  imports; the file is at 2015 lines after W2's slimming — keep it net-negative vs origin/master)

Do NOT touch follower_durability/{mod.rs, batch.rs, record.rs}, apply_replicated.rs, cold_reconcile/,
shutdown_impl.rs, lifecycle/*, meta/repl/* — other stages own them.

## Design contract (this replaces the old branch's own worker AND its wrapper admission — the two
## mandated replacements of the parent task)

**Worker = the merged lifecycle foundation.** `RepoInstance` owns ONE
`Arc<ForegroundLifecycle<ReceiptApplyJob, DbResult<ApplyOutcome>>>` (the existing `worker_lifecycle`
field, re-parameterized by this task: change the field's `<(), ()>` spelling to the receipt types and
add `use` lines). The worker is installed lazily on FIRST submit via `install_worker(lease, handler)`;
`ReceiptApplyJob` is the payload (repo clone + `Arc<dyn Store>` + `Vec<ReplicatedEvent>` + bookmark +
policy); the reply is `DbResult<ApplyOutcome>`. Jobs carry payload + reply ONLY (`LifecycleJob`). The
worker holds NO sender and NO owner handle; shutdown = `shutdown_clean`'s existing
`close()` + `shutdown_until(deadline)` on the SAME lifecycle (repo/clean_horizon/shutdown_impl.rs —
a later stage fixes that file's accessor signature, not you). Repository drop drops the foreground
token → `close()` → the worker drains and the supervisor joins → the lease is dropped after join:
parked guards and any paused reservation are quarantined/cleared exactly once, through the owner.

**Admission = the repository permit.** `run_receipt_pipeline` runs INSIDE the worker and begins with
`repo.write_domain().admit()` — or `admit_with_reservation(&res)` when the lease carries a paused
reservation (take it out of the lease's mutex BEFORE awaiting, restore on failure — never hold a
std-sync guard across `.await`; the future must stay `Send`). Then
`repo.marker_write_mutex().lock().await` (lock order: permit → marker, per #183). The permit threads
into every atomic-batch staging function: `stage_prepared_batch`, `stage_materialized_batch`,
`stage_checkpoint_batch` take `permit: &RepoWritePermit` and call
`repo.begin_repo_atomic_batch(permit)` (which debug-checks it). There is NO second admission mechanism
anywhere; do NOT port the old branch's `ddl_admission_lock`/`marker-mutex-as-admission` — the DOMAIN
permit excludes DDL/mutations for the pipeline's whole run.

**Paused reservations.** When `apply_fresh` gives up on a chunk (3-strike or non-retryable) while a
receipt is OUTSTANDING (lookup finds one), it parks the chunk guards in the lease AND places
`repo.write_domain().try_place_reservation()` into the lease (store the `RepoWriteReservation`; its
RAII clears the domain slot on drop). While parked, ordinary `admit()` is rejected repo-wide
(`RepoWriteError::Reserved`); the NEXT pull resumes through `admit_with_reservation`. A successful
pipeline (receipt retired) clears the reservation. A failure with NO outstanding receipt (before the
Prepared commit) places NO reservation. `RepoWriteError` maps to `DbError::Internal` (follow
repo_instance.rs's existing mapping style).

**Lease** (`resume/worker.rs`): `ReceiptLease` must be `Clone` (inner `Arc<LeaseInner>`) and impl
`WorkerLease`. Contents: `gate: Arc<RepoTxGate>` (resolved at install time), parked guards
`Mutex<Vec<VersionGuard>>`, paused reservation `Mutex<Option<RepoWriteReservation>>`, and an idle
signal (`AtomicUsize in_flight` + `tokio::sync::Notify idle`). Methods: `park`/`retire_parked`/
`parked_count` (cfg(test))/`wait_idle` (cfg(test), arm-then-check `enable()` pattern — no sleeps, no
yield loops)/`place_pause_reservation`/`take_reservation`/`restore_reservation`/`clear_reservation`/
`quarantine_parked_if_any`. `WorkerLease::on_quarantine` and `Drop for ReceiptLease`: if parked guards
are non-empty → `gate.set_requires_reopen()` then drop them (safe durable pause; Aborted marks are
acceptable after reopen — same semantics as the old branch's shutdown quarantine; add the contention
comment: failure/shutdown path only, never a hot path).

**Handler loop**: `while let Some(job) = state.next_job().await { in_flight += 1; let _tail =
job.repo.commit_tail().enter(); let result = run_receipt_pipeline(&job.repo, job.info, job.events,
job.bookmark, job.policy, state.lease()).await; job.reply(result); in_flight 1→0 → idle.notify_waiters(); }`
then, after the channel closes: `state.lease().quarantine_parked_if_any()`. The reply send error is
ignored (caller cancellation drops only the receiver — the worker keeps running the job).

**Repo-side accessors** (`impl RepoInstance` in resume/worker.rs, mirroring the old branch's placement):
`receipt_worker_installed()` (lazy: `self.receipt_worker` OnceCell<Arc<ReceiptLease>> — ADD this field
to repo_instance.rs with the `Arc<OnceCell<...>>` clone + init pattern of `tx_gate`; on init resolve
`repo.tx_gate().await?`, build the lease, `install_worker(lease.clone(), handler)` on
`self.worker_lifecycle` (background clones have `None` → typed error), map `InstallError`);
`apply entry` = submit + await the reply (map `JobSubmitError::{Closing, NotInitialized, Full}` to
`DbError::Internal` messages matching the old branch's wording); cfg(test)
`receipt_worker_parked_guards() -> Option<usize>` and `receipt_worker_wait_idle()` keeping the OLD
branch's exact names so its tests port cleanly.

## Work items

1. **Port + adapt** `stage.rs`, `apply.rs`, `checkpoint.rs`, `publish.rs`, `resume/mod.rs`,
   `resume/activation.rs`, `resume/lookup.rs` from
   `git show feat/s5cx-follower-receipt-2:crates/shamir-engine/src/tx/follower_durability/receipt/...`.
   Keep: the BatchPlan/EventPlan/EventOps/MaterializedWork shapes, `ops_per_event` segmentation,
   Uncertain read-back reconciles + their cfg(test) negative-control seams
   (SIMULATE_UNCERTAIN / DISABLE_UNCERTAIN_RECONCILE / UNCERTAIN_WITHOUT_COMMIT / CHECKPOINT_RECONCILES
   / checkpoint test lock / COMMIT_PARK + RECONCILE park gates), `verify_journal` + `confirm_journal`,
   `install_durable_events` journal repair, the one-poll commit→publication rule in `checkpoint()`,
   `apply_publication`'s guard-ownership proof comment, chunking under MAX_RECEIPT_EVENTS, the
   `fail_after` fast-path seam call. Adapt: `receipt_key`/`put_receipt`/`load_outstanding_receipt`
   (now real, no `#[allow(dead_code)]` — W2 defined them), `save_bookmark_v3` instead of hand-encoded
   bookmark writes ONLY where the old code wrote bookmarks outside atomic batches (atomic batches still
   stage `encode_bookmark_v3` bytes directly), `resume_version_guard` (shamir-tx, landed),
   `ts_index_insert` (now pub), `adopt_absolute` (W2), and every symbol name drift vs current master.
   The pipeline's admission + marker-mutex block replaces the old `_admission`/`_ddl` guards; thread
   `permit` through try_chunk → build_plan callers → stage/checkpoint staging fns as specified above.
2. **Finding 1 — no repeated durable phase; full identity.**
   a. In `try_chunk`'s fast path (durable Materialized receipt matches this batch): build the
      publication with `publish::publication_from_receipt(repo, stored)` — NOT
      `publication_from_plan` — then go STRAIGHT to `durable_tail` (checkpoint). The ordinary resume
      arm in `run_receipt_pipeline` already does this for `ReceiptPhase::Materialized`; keep it.
   b. Replace `same_batch`'s range-only content match with a phase-invariant comparison of:
      `mapping` (exact local allocations), `upstream_start`, `upstream_end`, `predecessor_bookmark`,
      `lineage_id`, `upstream_epoch`, `bindings`, and `downstream_events` (byte equality) — the fresh
      plan carries all of these at Prepared time (`build_plan` reads fence lineage/epoch and serializes
      downstream bytes). Document why `receipt_id` itself is unusable across phases.
   c. New `resume/identity.rs`: `verify_identity(repo, info, receipt, offered) -> DbResult<()>` running
      BEFORE any resume work: (i) current fence `lineage_id`/`history_epoch` must equal the receipt's;
      (ii) every receipt-mapped upstream version must be offered (the old `match_offered` check);
      (iii) the union of offered `field_bindings` for covered versions must equal `receipt.bindings`
      (id→name pairs); (iv) BYTE identity: for each covered version, re-translate the offered event
      exactly like `apply_fresh` does (`prepare_event` with `applied_watermark =
      receipt.predecessor_bookmark`), reproject with the receipt's local version, serialize, and
      compare against `receipt.downstream_events[index]`. Mismatch → `DbError::Storage` ("outstanding
      receipt identity mismatch …; refusing to allocate replacements") — the receipt stays, the pull is
      refused; interner touches from the rejected translation are monotonic-unused (failure-matrix row
      1: no rollback). Call (i)+(ii) from the resume arm before loading bookmark v3; call (iii)+(iv)
      with the same `verify_identity` entry so tests can target it.
3. **test_seam.rs**: port the old branch's additions verbatim (Phase::{ReceiptPrepared,
   ReceiptMaterialized, ReceiptCheckpointed}, `abort_at`, `fail_after`/`fail_after_sticky`/
   `fail_after_reopen`, `take_fail_after`, `fail_after_reopen`, and the abort branch in `phase()`
   printing `SHAMIR_RECEIPT_ABORT_FIRED {phase:?}` then `std::process::abort()`). Keep existing phases/
   hooks untouched.
4. **Line budgets**: receipt/ folder = mod, stage, apply, checkpoint, publish, resume/, (tests/ lands in
   W5 — do NOT create it now) = 6 entries now. stage.rs (old 757 + permit plumbing) and apply.rs must
   each stay ≤ 1000 — if a file would overflow, split along its seams into a NEW file inside the same
   folder-with-room (resume/ has room), never by shrinking comments that carry proofs.
5. **repo_instance.rs**: ONLY the field/type/import edits this design requires (field re-parameterize +
   `receipt_worker` OnceCell + clone + init + imports). Must stay net-negative vs origin/master (2015
   now; you may add ≈5 lines).

## Acceptance criteria

- All files exist; `wc -l` for each (receipt/* ≤ 1000; repo_instance.rs < 2020 and < origin/master's 2069).
- The words `ddl_admission` appear NOWHERE in your diff. No `tokio::spawn` outside the lifecycle's
  `install_worker`. No unbounded channel. No std-sync guard held across `.await`.
- Final message: files + line counts, how each review-finding fix is realized (file:line), every
  deviation from the old branch, anything unverified.
