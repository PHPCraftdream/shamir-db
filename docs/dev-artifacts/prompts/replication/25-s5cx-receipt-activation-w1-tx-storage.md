# 25 — S5c-X PR6b-2c receipt activation, W1: shamir-tx + shamir-storage primitives

Parent task: `docs/dev-artifacts/prompts/remediation/119-s5cx-receipt-activation.md`.
Binding design: `docs/dev-artifacts/prompts/remediation/106-s5cx-follower-receipt-2.md` (git show
feat/s5cx-follower-receipt-2:docs/dev-artifacts/prompts/remediation/106-s5cx-follower-receipt-2.md).
The old branch `feat/s5cx-follower-receipt-2` is a SOURCE OF PARTS, not a base: read its files with
`git show feat/s5cx-follower-receipt-2:<path>` and port them onto the CURRENT tree, verifying every
symbol against the working tree before use (master gained #181/#182/#183 since that branch diverged).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
  mutates the working tree or index. Only edit files; the orchestrator commits. Read-only git
  (`git show`, `git diff`, `git ls-tree`, `git log`) is allowed and expected.
- Do NOT run cargo, rustc, nextest, or any build/test/clippy command. CI is the gate. Compiles-by-inspection
  is the bar: read the real current definition of every type/trait/fn you touch (Read/Grep the working
  tree, never from memory) before calling it.
- Stay inside FILE SCOPE. If an out-of-scope edit is strictly required to compile, make it minimal and
  list it in the final message.
- Layout ratchet: ≤ 7 direct entries per folder, ≤ 1000 lines per source file; files ALREADY over the
  limit must NOT grow (net-zero-or-negative diff). Measure with `wc -l` after editing.
- Imports at the top of the file. `mod.rs` = re-exports only. Tests live in `tests/` folders,
  `#[cfg(test)]`-gated, one topic per file.
- Write rustfmt-clean code by hand (4-space indent, trailing commas). No machine-specific absolute paths.
- No new dependencies. Do not rename or reformat unrelated code. Do not touch unrelated comments.

## FILE SCOPE (only these paths)

- crates/shamir-tx/src/changefeed.rs (OVER 1000 lines: 1096 — NET ZERO OR NEGATIVE diff)
- crates/shamir-tx/src/changefeed/journal_read.rs (NEW)
- crates/shamir-tx/src/changefeed/install.rs (NEW)
- crates/shamir-tx/src/mvcc_store/mod.rs (OVER: 1720 — NET ZERO OR NEGATIVE)
- crates/shamir-tx/src/mvcc_store/mvcc_history.rs (674 → ≤ 1000)
- crates/shamir-tx/src/mvcc_store/version_entry.rs
- crates/shamir-tx/src/visibility_metrics/resume_access.rs (NEW; the dir exists with gate_access.rs,
  metrics.rs, mod.rs, snapshot.rs, tests/)
- crates/shamir-tx/src/repo_tx_gate.rs (OVER: 1411 — NET ZERO OR NEGATIVE)
- crates/shamir-tx/src/mvcc_store/write_receipt/tests/publication_bound_tests.rs (218 → ≤ 1000)
- crates/shamir-storage/src/commit_window_park_seam.rs (NEW)
- crates/shamir-storage/src/storage_fjall.rs (928 → ≤ 1000)
- crates/shamir-storage/src/lib.rs (+1 mod declaration only)

## Work items

1. **Journal durable install** (design step 3: serialized durable-install command owned by the sole
   journal writer; never the lossy `try_send` enqueue path):
   - Port `git show feat/s5cx-follower-receipt-2:crates/shamir-tx/src/changefeed/install.rs` VERBATIM
     into `crates/shamir-tx/src/changefeed/install.rs`, then verify against CURRENT master:
     `journal_tx` field, `version_key`, `ChangelogStore`, `RepoChangefeed` — adapt imports/signatures
     to what exists today.
   - Port `crates/shamir-tx/src/changefeed/journal_read.rs` verbatim (it MOVES the `JournalRead` struct
     + `RepoChangefeed::read_from` out of changefeed.rs).
   - Edit `changefeed.rs`: add `mod install;` + `mod journal_read;` + `pub use journal_read::JournalRead;`
     next to the existing `pub mod cold;` / `mod repair_pending;` declarations; DELETE the local
     `JournalRead` struct and the `read_from` method (moved); add the `JournalMsg::InstallRecords`
     variant; add the dispatch arm calling `install::process_install(store, records,
     last_persisted_version, ack).await`. Locate `async fn process_journal_msg` on CURRENT master and
     confirm `store` and `last_persisted_version` are in scope with the types install.rs expects; adapt.
   - `JournalRead`'s external callers must keep compiling (`read_from` keeps its exact signature).
   - Line budget: changefeed.rs must END ≤ 1096 (target ≈ 1070; the two moved-out blocks exceed the
     additions).
2. **Publication-bound reads (invariant I3) — the zero-floor fix** (review finding: the activation must
   never expose materialized-but-unpublished rows, including at floor 0):
   - Port ALL `crates/shamir-tx/src/mvcc_store/mod.rs` hunks from
     `git diff origin/master...feat/s5cx-follower-receipt-2 -- crates/shamir-tx/src/mvcc_store/mod.rs`:
     `ts_index_insert` pub(super)→pub; the cold path in `get_current` using the new bounded
     `seek_latest_published` (replacing `seek_latest_version` — grep for remaining callers first; port
     `seek_latest_published` exactly as the old branch wrote it and DELETE `seek_latest_version` only if
     nothing else references it); removal of the `floor > 0` exemptions in `get_current` and in the
     multi-get slot routing; `#[cfg(test)] pub(crate) fn evict_current_cell_for_test`; REMOVAL of
     `snapshot_stream_impl` (moves to mvcc_history.rs) and of the now-unused
     `use version_entry::StreamingGroupByState;` import.
   - Port the matching `mvcc_history.rs` and `version_entry.rs` hunks (snapshot_stream_impl moved in as
     `pub(super) fn`, StreamingGroupByState import; version_entry floor check loses the `floor > 0`
     exemption).
   - mvcc_store/mod.rs must END ≤ 1720 (the old branch's diff was net ≈ −85).
3. **Resume guard** — port `git show feat/s5cx-follower-receipt-2:crates/shamir-tx/src/visibility_metrics/resume_access.rs`
   (moves `fresh_tx_id` out of repo_tx_gate.rs and adds `RepoTxGate::resume_version_guard`). Wire with
   the SAME `#[path = "visibility_metrics/resume_access.rs"] mod resume_impl;` pattern repo_tx_gate.rs
   already uses for its other `#[path]` includes (check how gate_access/metrics are wired and mirror it).
   Verify every `Arc::clone(&self.<field>)` in `resume_version_guard` matches CURRENT `RepoTxGate` fields
   and `VersionGuard::new`'s CURRENT parameter list (it may have drifted since the old branch) — adapt.
   repo_tx_gate.rs must END ≤ 1411.
4. **Zero-floor publication tests** — append the three new tests from the old branch's
   `publication_bound_tests.rs` diff (`materialized_unpublished_invisible_at_zero_floor`,
   `zero_floor_cold_path_does_not_seed_unpublished_cell`, and the round-4 bounded-scan regression test)
   into `crates/shamir-tx/src/mvcc_store/write_receipt/tests/publication_bound_tests.rs`. Adjust to the
   helpers that exist in that file on master (`make_mvcc`, `set_op`, `collect_current`); `history_store()`
   exists (mvcc_store/mod.rs ~1297). Multi-line indented literals per AGENTS.md.
5. **Commit-window park seam** (test seam for cancellation tests; the fjall file is at 928 lines and
   +104 would break the 1000-line ratchet, so the seam module moves to its own file):
   - Create `crates/shamir-storage/src/commit_window_park_seam.rs` with the module the old branch wrote
     inside storage_fjall.rs (`git show feat/s5cx-follower-receipt-2:crates/shamir-storage/src/storage_fjall.rs`
     — the `pub mod commit_window_park_seam` block, doc comment included). Declare it in
     `crates/shamir-storage/src/lib.rs` as `#[doc(hidden)] pub mod commit_window_park_seam;` next to the
     existing `membuffer_clear_race_hook` pattern.
   - In storage_fjall.rs: add `pub use crate::commit_window_park_seam;` (keeps the old
     `shamir_storage::storage_fjall::commit_window_park_seam::…` path working) and the `exec_atomic_commit`
     hook (`if commit_window_park_seam::ARMED… { commit_window_park_seam::park(); }` placed where the old
     branch put it — after staging, before `batch.commit()`).
   - storage_fjall.rs must END ≤ 936.
   - Check the parking thread's Condvar/Mutex usage against rust-intel rules: it runs on a
     `spawn_blocking` thread (blocking primitive is correct); keep the arm-then-check `wait_reached`
     Notify pattern exactly (no sleeps, no yield budgets).

## Acceptance criteria

- All listed files exist/edited; every `wc -l` budget above holds (report measured numbers).
- Every symbol verified against the CURRENT tree; any adaptation vs the old branch listed in the final
  message.
- No builds/tests run. Final message: files changed with line counts (before → after), what each new
  test proves, deviations, anything unverified.
