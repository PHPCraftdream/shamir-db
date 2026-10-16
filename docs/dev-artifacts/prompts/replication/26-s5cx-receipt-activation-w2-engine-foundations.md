# 26 — S5c-X PR6b-2c receipt activation, W2: engine foundations (codecs, repo plumbing, lifecycle)

Parent task: `docs/dev-artifacts/prompts/remediation/119-s5cx-receipt-activation.md`.
The old branch `feat/s5cx-follower-receipt-2` is a SOURCE OF PARTS: read files with
`git show feat/s5cx-follower-receipt-2:<path>`; port onto the CURRENT tree, verifying every symbol
against the working tree (master gained #181/#182/#183 since that branch diverged; #183 already wired
`RepoWriteDomain`/`RepoWritePermit` into repo_instance.rs — do NOT re-add the old branch's
`ddl_admission` mutex or `DdlGuard`, that mechanism is replaced by the domain).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
  mutates the working tree or index. Only edit files; the orchestrator commits. Read-only git allowed.
- Do NOT run cargo, rustc, nextest, or any build/test/clippy command. CI is the gate.
  Compiles-by-inspection is the bar.
- Stay inside FILE SCOPE; minimal out-of-scope edits only if required to compile, and list them.
- Layout ratchet: ≤ 7 direct entries per folder, ≤ 1000 lines per file; files ALREADY over the limit
  must NOT grow (net-zero-or-negative). Measure with `wc -l` after editing.
- Imports at the top. `mod.rs` = re-exports only. Tests in `tests/` folders, `#[cfg(test)]`-gated.
- rustfmt-clean by hand. No machine-specific absolute paths. No new dependencies. No unrelated changes.

## FILE SCOPE

- crates/shamir-engine/src/meta/repl/receipt.rs
- crates/shamir-engine/src/meta/repl/bookmark_v3.rs
- crates/shamir-engine/src/meta/repl/mod.rs
- crates/shamir-engine/src/meta/repl/tests/receipt_tests.rs
- crates/shamir-engine/src/meta/repl/tests/bookmark_v3_tests.rs
- crates/shamir-engine/src/meta/tests/follower_bookmark/v2.rs (this IS the legacy codec — loaded via
  `#[path]` from meta/repl/bookmark.rs; the old branch's refuse-v3 hunks go here)
- crates/shamir-engine/src/repo/repo_types.rs
- crates/shamir-engine/src/repo/repo_instance.rs (OVER: 2069 — NET NEGATIVE required)
- crates/shamir-engine/src/repo/version_provider.rs
- crates/shamir-engine/src/repo/mod.rs
- crates/shamir-engine/src/repo/changelog_store/follower_durability.rs
- crates/shamir-engine/src/table/record_counter.rs
- crates/shamir-engine/src/tx/cold_reconcile/lifecycle/owner.rs
- crates/shamir-engine/src/tx/cold_reconcile/lifecycle/mod.rs
- crates/shamir-engine/src/tx/cold_reconcile/lifecycle/tests/* (only if the generalization requires it)

## Work items

1. **Receipt codec** (`meta/repl/receipt.rs`): port the old branch's diff
   (`git diff origin/master...feat/s5cx-follower-receipt-2 -- crates/shamir-engine/src/meta/repl/receipt.rs`):
   add `pub ops_per_event: Vec<u32>` to `ReplicationReceipt` (parallel to `mapping`; documented as the
   per-event op counts summing to `ops.len()`); the writer helpers `receipt_key()`, `put_receipt`,
   `load_outstanding_receipt`, `delete_receipt` (pub(crate)). THEN one deliberate deviation: BUMP the
   receipt tuple version from 1 to 2 in BOTH `encode_receipt` and `decode_receipt` (and the `version !=
   2` rejection) — no production writer ever shipped, and bincode is not self-describing, so a strict
   version bump guarantees the same binary that writes a receipt can always recover it (review finding:
   "must not write a receipt format that the same merged version cannot recover"). Update
   `receipt_tests.rs` (fixture gains `ops_per_event: vec![1, 1]`; add an assertion that a v1-tagged
   envelope fails closed). Keep validate() strictness; extend it to reject
   `ops_per_event.len() != mapping.len()` or a sum mismatch with `ops.len()`.
2. **Bookmark v3 writer + capability sniff** (`meta/repl/bookmark_v3.rs`): port `save_bookmark_v3`
   (set + flush) and `has_v3_record` from the old diff; export `save_bookmark_v3` from `meta/repl/mod.rs`.
   Port the `meta/tests/follower_bookmark/v2.rs` hunks: `is_v3` sniff, v3-aware `decode` fallback, and
   `refuse_v3` guards at the top of `save_fence_record` / `save_replication_bookmark` /
   `save_bookmark_and_clean_horizon` (legacy writers must never downgrade a v3 record — the receipt
   pointer would be lost). Port the `legacy_v2_loaders_read_v3_records_and_saves_refuse_downgrade` test
   into bookmark_v3_tests.rs.
3. **BoxRepo atomic-batch plumbing** (`repo/repo_types.rs`): port the old
   `supports_atomic_batch` + `begin_atomic_batch` impl for `BoxRepo` (InMemory/Fjall support;
   MemBuffer/Cached/Hybrid unsupported — wrappers keep caches outside a batch's scope; Hybrid splits
   stores across two physical databases). Verify the `BoxRepo` variant set on CURRENT master first.
4. **RepoInstance accessors** (`repo/changelog_store/follower_durability.rs`): port ONLY the
   capability/accessor additions — `storage_supports_atomic_batch()`, `raw_store(name)`,
   `begin_repo_atomic_batch(&self, permit: &RepoWritePermit)` (calls
   `self.write_domain.debug_check_permit(permit)` then `self.repo.begin_atomic_batch().await` — the
   permit is the admitted-API capability; there is NO second admission mechanism). Do NOT port
   `DdlGuard`/`ddl_admission_lock`/`ddl_admission_guard`/`ddl_hook` — that wrapper mechanism is replaced
   by the merged write domain.
5. **Counter adoption** (`table/record_counter.rs`): port `adopt_absolute(&self, after: u64)` — cache
   and `last_persisted` set to `after` under `persist_lock` so periodic `persist()` can never overwrite
   a committed count with a stale cache. Verify `ensure_cache`/`persist_lock` names on current master.
6. **repo_instance.rs slimming + lifecycle field** (file is 2069 lines — over the ratchet; the diff must
   be NET NEGATIVE): move `repo_token`, `register_token`, `to_mvcc_retention` out of repo_instance.rs
   into `repo/version_provider.rs` exactly as the old branch did (incl. the `repo/mod.rs` re-export
   change to `pub use version_provider::{repo_token, to_mvcc_retention};` and the `register_token`
   import inside repo_instance.rs). Do NOT touch anything else in repo_instance.rs in THIS task — the
   `worker_lifecycle` field type change lands in the next stage (W3).
7. **Lifecycle generalization** (`tx/cold_reconcile/lifecycle/`): make `ForegroundLifecycle` generic —
   `pub struct ForegroundLifecycle<J: Send, R: Send> { lifecycle: Arc<RepoWorkerLifecycle<J, R>> }` with
   `new()`, `from_lifecycle(Arc<RepoWorkerLifecycle<J, R>>)`, `lifecycle()`, `reopen_required()`; keep
   the `RepoUnitLifecycle = RepoWorkerLifecycle<(), ()>` alias (foundation tests keep compiling).
   In `lifecycle/mod.rs` un-gate the exports the receipt pipeline needs in production:
   `owner::{ForegroundLifecycle, RepoUnitLifecycle, WorkerTerminal, RepoWorkerLifecycle, InstallError,
   ShutdownError}` and `job::JobSubmitError` and `worker::WorkerLease` — i.e. remove the `#[cfg(test)]`
   gates on exactly those re-exports, keep every other export as-is. For any item that becomes exported
   but unused in non-test builds (dead_code under `-D warnings`), apply the file's existing
   `#[cfg_attr(not(test), allow(dead_code))]` / `#[allow(dead_code)]` + reason-comment pattern so clippy
   stays clean. Do not change the owner/worker/interlock state machines.

## Acceptance criteria

- All listed files edited; `wc -l` reported for repo_instance.rs (must be < 2069), lifecycle files, and
  every other touched file.
- No DdlGuard/ddl_admission anywhere. No second admission mechanism introduced.
- Every symbol verified against the current tree; deviations vs the old branch listed.
- Final message: files changed with line counts, deviations, anything unverified.
