# Task 121 round 4 — stage briefs (S5c-X PR6b-3, draft PR #189)

Round 4 fixes three independent-review TEST defects in worktree
`worktrees/s5cx-retire-legacy-follower`, branch
`feat/s5cx-retire-legacy-follower` (round 3 committed as `54397e8a`).
Production behaviour must NOT change unless a finding says so. The policy
and tooling block below is carried VERBATIM in every stage brief.

## Policy + tooling (verbatim, part of every stage brief)

## 🚦 Compile / build / test policy — MANDATORY, highest priority

**Compilation, builds and tests run on GitHub CI, not on this machine.**
This rule overrides every other section of this file, every skill, every
brief and every habit ("let me just run it once to be sure"). Where any
other text contradicts it, this section wins. It binds the main agent,
every sub-agent and rush executor, and every verifier child.

**Banned unless the user explicitly asks for it in the current
conversation** (a request for one run does not extend to later runs, and an
approval from an earlier session does not carry over):

* `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t` / `cargo tl`
* `cargo build`, `cargo run`, `cargo bench`, `cargo doc`
* `npm` / `tsc` / `vitest` builds and test runs, WASM builds, e2e and
  integration harnesses, loom, any release or full-workspace build
* any other command whose purpose is to build the project or to execute its
  tests or benchmarks

**Allowed locally before a push — only these**, scoped to the crates you
touched (`-p <crate>`, plus dependents when a public signature changed;
`--workspace` only when asked):

* `cargo check -p <crate> --all-targets` — type-check, runs no tests
* `cargo clippy -p <crate> --all-targets -- -D warnings`
* `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`
* `python -I scripts/ci/check_layout.py` — the instant layout gate

Never push code that fails `check`, `clippy` or `fmt`. Everything else is
CI's job, and `./scripts/test.sh` is executed **in the PR's CI**:

* Push to a branch and open a draft PR: the fast tier (fmt, layout, clippy
  and `./scripts/test.sh` lib tests of the affected crates) runs.
  `gh pr ready` runs the full matrix (`./scripts/test.sh` and the
  integration run on Ubuntu/Windows/macOS), which is the merge gate;
  `gh workflow run ci.yml --ref <branch>` forces the full tier on a draft.
* A failing test, hang or flake is diagnosed from the CI log
  (`gh run view <id> --log-failed`), fixed in code and re-pushed. It is not
  reproduced locally; reading code and logs is the debugging tool. When a
  flake needs repeated runs, the loop runs in CI (a workflow job), not here.
* The fast tier stops at the first failing crate. Before pushing, read every
  changed file for the whole class of error (module paths after a move,
  imports, privacy, `cfg(test)` gating, unused items) instead of letting CI
  reveal them one at a time.

**Delegated work.** Every brief for a sub-agent, rush executor or verifier
child carries this policy verbatim, including the allowed list. A child that
believes it needs a build or test run says so in its final message; the
orchestrator decides whether to ask the user.

The sections below that mention `./scripts/test.sh`, `cargo t` / `cargo tl`
or benches describe HOW they are run — in CI, or locally only on the user's
explicit request. They never grant permission to run them.

=== END OF POLICY ===

TOOLING WARNING (verbatim from the user): never use the shell builtin
`xargs` (it crashes the whole rush session) — in your commands or any
child's; capture gate exit codes without pipes
(`cmd > log 2>&1; echo exit=$?`). Write ALL scratch logs under `.scratch/`
(git-ignored) — NEVER `target-scratch/`. Run `CARGO_BUILD_JOBS=4`, one
cargo process at a time; do not use `CARGO_BUILD_JOBS=1`. If rustc dies
with an allocation/stack-buffer failure (machine OOM), retry the SAME
jobs=4 command; never drop the job count.

Hard rules for every stage: ⛔ no git mutations (read-only status/diff
allowed; the orchestrator commits). Never silence a lint with `#[allow]`.
Imports at the top of the file. NO new files anywhere. Surgical edits
only. Re-read the current file region in full before each edit — line
numbers below are from the review and may have drifted.

---

## Stage 1 — ENGINE worker (P1: cold-test handle hygiene)

Files (exclusive): `crates/shamir-engine/src/tx/cold_reconcile/tests/follower_cold_tests.rs`, `crates/shamir-engine/src/tx/cold_reconcile/tests/helpers.rs`. Another worker owns all server files.

### Defect
`legacy_bookmark_refuses_reopen_before_replay` (~:315-393): the baseline
scope opens `tx = raw_store(path, "__tx__")` and, while `tx` is still
alive, opens `changelog = raw_store(path, "__changelog__")`; the
post-refusal scope repeats this with `tx_after` / `changelog_after`. Each
`raw_store` call builds a fresh `BoxRepoFactory::fjall_raw(...).create()`
(helpers.rs:212) → new `FjallRepo` → a NEW `Database::open`
(storage_fjall.rs:373) whose stores retain `Arc<Database>` (:394); fjall
takes an EXCLUSIVE database lock (fjall-3.1.6/src/db.rs:576 →
`LockedFileGuard::try_acquire`; locked_file.rs:53 retries 3× then
`Error::Locked`). The second open inside one scope fails on the lock
before the typed-refusal assertion.

### Fix (exact shape)
1. Split the baseline into two scopes: read floor + legacy bytes from
   `__tx__`, let the handle DROP at scope end, only then open
   `__changelog__` for the clean-horizon/gap reads. Same four baseline
   values (`floor_before, legacy_before, clean_before, gap_before`), same
   later assertions.
2. Same one-store-at-a-time split after the refused open for the four
   unchanged-value assertions.
3. Add to helpers.rs:
   ```rust
   pub(super) async fn raw_store_with_retry(path: &Path, name: &str) -> Arc<dyn Store>
   ```
   up to 25 attempts of the existing `raw_store` body; on `Err` sleep
   20 ms and retry; on exhaustion panic with the last error. One doc line:
   a fresh exclusive-lock open right after a close can hit brief fjall
   lock lag. Use it for the FOUR raw opens in this test only; leave
   `raw_store` untouched for the other tests. The refused repo reopen
   already goes through `open_refusal_with_retry` — keep that.
4. One comment line above each of the four raw-open scopes: the handle
   must be dropped before the next open — each raw store owns an
   exclusive fjall database lock.
5. Keep unchanged: the real in-flight WAL block (`repo.repo_wal()` →
   `begin_grouped` → `sync_wal` → pre-close non-empty `recover()`
   assertion), the legacy-bytes injection, `close(repo)`,
   `open_refusal_with_retry`, and every assertion.

Trace first: confirm from helpers.rs / storage_fjall.rs that each
`raw_store` call opens its own Database — that is why one-at-a-time
matters. No production code changes.

Gates until clean (real exit codes): `cargo check -p shamir-engine
--all-targets` → `.scratch/r4_engine_check.log`; `cargo clippy -p
shamir-engine --all-targets -- -D warnings` →
`.scratch/r4_engine_clippy.log`.

Report: new block shape (file:line), helper added (file:line), gates'
real exit codes, `wc -l` of both files, deviations.

---

## Stage 2 — SERVER worker (P2-a + P2-b)

Files (exclusive): `crates/shamir-server/src/replication/supervisor.rs`,
`crates/shamir-server/src/replication/tests/recovery_gap_consumer_tests.rs`,
`crates/shamir-server/src/replication/identity_fence/tests/adoption_durability_tests.rs`.
NO new files — `replication/tests/` sits at its 8-entry baseline and a new
file is invisible to the layout checker until commit.

### P2-a — make the early gate-refusal regression real
`legacy_bookmark_gate_refusal_marks_typed_terminal`
(recovery_gap_consumer_tests.rs:440) builds the DB FIRST, then injects
legacy bytes. Trace (verified by the reviewer): `build_db()` →
`ShamirDb::add_repo_as` (supervisor_tests.rs:67) →
`add_repo_as_with_durability` (db_management.rs:419) →
`RepoInstance::from_factory_with_durability`
(durability_policy/repo_impl.rs:46) → `recover_v2_inflight_admitted`
(repo_impl.rs:72); recovery step 1 calls `repo.tx_gate()`
(tx/recovery.rs:373) BEFORE reading entries, so the gate `OnceCell`
(repo_instance.rs:860) is initialized during OPEN — and production
refuses legacy/malformed bytes at open itself (repo_impl.rs:64-65
classify). Review option (a) — seed provenance before first open — is
therefore infeasible: a repo the supervisor can see always hands it the
cached, successfully-initialized gate. Chosen fix = option (b):

1. Extract the inline error-kind mapping at supervisor.rs:344-365 into a
   `pub(super)` method on `SubscriptionSupervisor` (behaviour parity is
   mandatory: typed refusal → mark `resync_required`, mark failure →
   `Start`, return the typed error; every other error → `Start`, nothing
   marked):

   ```rust
   /// Map a gate-initialization failure by KIND: a typed follower-state
   /// refusal stays machine-readable and terminal — the subscription is
   /// marked `resync_required` and the typed error is returned; every
   /// other error keeps the generic retryable `Start` mapping and marks
   /// nothing. Extracted (round 4) so tests can drive every kind
   /// directly: the supervisor cannot reach a FAILING `tx_gate()` through
   /// this fixture — the gate OnceCell is initialized during open
   /// recovery (tx/recovery.rs:373 → repo_instance.rs:860) and open
   /// itself refuses legacy/malformed bytes
   /// (durability_policy/repo_impl.rs:64-65), so the supervisor always
   /// sees the cached gate.
   pub(super) async fn refuse_gate_init(
       &self,
       sub_name: &str,
       error: shamir_db::storage::error::DbError,
   ) -> ReplError
   ```

   Call site becomes:

   ```rust
   let gate = match destination.tx_gate().await {
       Ok(gate) => gate,
       Err(error) => return Err(self.refuse_gate_init(&sub.name, error).await),
   };
   ```

   Keep/adjust the surrounding comment to point at the helper.

2. Three unit tests in recovery_gap_consumer_tests.rs next to the
   integration test. Each `#[tokio::test]`: `build_db()`,
   `create_profile_and_subscription(&follower).await`, a throwaway
   `ReplSourceFactory` (a `PartialSource` with empty events is fine),
   `SubscriptionSupervisor::new(follower.clone(), factory, "unit")` with
   `.with_poll_wait_ms(20).with_max_iterations(2)` — then call
   `sup.refuse_gate_init("sub_a", …)` DIRECTLY; never reconcile:

   - `gate_init_error_legacy_refusal_is_typed_and_marks_terminal`:
     pass `DbError::FollowerStateRefused { reason:
     FollowerRefusalReason::LegacyBookmarkFormat }`; assert the returned
     error matches `ReplError::FollowerStateRefused { reason:
     LegacyBookmarkFormat }` AND `sub_state(&follower, "sub_a")` is
     `Some("resync_required")`.
   - `gate_init_error_malformed_refusal_is_typed_and_marks_terminal`:
     same with `FollowerRefusalReason::MalformedBookmark`.
   - `gate_init_error_other_error_keeps_start_and_marks_nothing`: capture
     `sub_state` before; pass `DbError::Storage("gate init i/o".into())`;
     assert the return matches `ReplError::Start(_)` and the state is
     UNCHANGED (not resync_required).

   Counterfactual comment on the first two (REQUIRED by the reviewer's
   caller): flattening the helper's match to a blanket
   `ReplError::Start(error.to_string())` (the round-2 shape) turns them
   red — they would return `Start` and never persist `resync_required`.

3. Re-comment the existing integration test honestly: it seeds legacy
   bytes into the ALREADY-OPEN repo, so it exercises the admission-time
   classifier arm (supervisor.rs:390) and terminal persistence, NOT the
   early gate-init arm — the unit tests cover that directly. Keep its
   assertions unchanged.

### P2-b — warm visibility through the REAL metadata store
`warm_visibility_enabled_on_retry_over_existing_v3_zero_event_follower`
(adoption_durability_tests.rs:447) wraps a FRESH `InMemoryStore` in
`AdoptionStore` while passing the REAL repo handle (`:480`, `:496`) and
asserting the real bookmark (`:508`). The repo's own `__tx__` store
(repo_instance.rs:791) is never wrapped, so bookmark 0 is equally
consistent with an untouched bookmarkless repo.

Fix — same `AdoptionStore` struct, but wrap the repository's REAL store:
`inner: repo_handle.tx_info_store().await.unwrap()`. Rewrite the test:

1. Warm gate disabled (as today).
2. First contact `checks::fence_pull_identity(&info, &pull, None,
   Some(&repo_handle))` → `Err(ReplError::Bookmark(_))`; the fence record
   is READABLE: `recorded::load(&info)` is Some AND
   `load_bookmark_v3(&repo_handle.tx_info_store().await.unwrap())` is
   Some (a fresh handle returns the same underlying store —
   storage_in_memory.rs:42-50); `!probe.durable`; gate still disabled.
3. `checks::prepare_existing(&info)` → `probe.durable` true.
4. Retry `fence_pull_identity(…, Some(&repo_handle))` → Ok; gate ENABLED
   (`receipt_native_visibility_enabled()`).
5. Durable provenance: the fresh-handle `load_bookmark_v3` returns Some
   with `bookmark == 0` and `fence.is_some()`;
   `repo_handle.replication_bookmark()` == 0 (now meaningful — the
   repo's own store holds the v3 baseline).

New cancelled variant
`cancelled_first_contact_recovers_visibility_for_zero_event_follower`:
same real-store wrapper with `park: true`; replicate the park mechanics
of `adoption_retry`'s park branch (~:289-298): pin the
`fence_pull_identity(&info, &pull, None, Some(&repo_handle))` future and
`tokio::select! { biased; _ = probe.reached.notified() => {}, result =
&mut contact => panic!("contact escaped barrier: {result:?}") }`. Then:
v3 readable through a fresh real-store handle, `!probe.durable`, gate
disabled; then `prepare_existing` + a straight
`fence_pull_identity(…, Some(&repo_handle))` → Ok → gate enabled,
durable, v3 bookmark 0. (The existing cancellation tests pass NO repo
handle — this closes exactly that gap.) Keep `AdoptionStore`,
`adoption_retry`, `retry_contact` and every pre-existing test untouched.

Trace requirements (both findings): before writing any oracle read the
current production regions — supervisor.rs ~:344-435; checks.rs `decide` /
`fence_pull_identity` / `enable_visibility`; identity_fence/recorded.rs;
meta/repl/bookmark_v3.rs `save_bookmark_v3` (set + flush);
repo_instance.rs `tx_gate` OnceCell + `tx_info_store`;
storage_in_memory.rs store memoization. The final message states the
traced line that decides each assertion.

Gates until clean (real exit codes; one cargo at a time):
- scoped convergence: `CARGO_BUILD_JOBS=4 cargo check -p shamir-server
  --all-targets > .scratch/r4_server_check.log 2>&1; echo exit=$?` and
  `CARGO_BUILD_JOBS=4 cargo clippy -p shamir-server --all-targets --
  -D warnings > .scratch/r4_server_clippy.log 2>&1; echo exit=$?`
- final mandated combined pass: `CARGO_BUILD_JOBS=4 cargo check -p
  shamir-engine -p shamir-server --all-targets > .scratch/r4_check.log
  2>&1; echo exit=$?`; `CARGO_BUILD_JOBS=4 cargo clippy -p shamir-engine
  -p shamir-server --all-targets -- -D warnings >
  .scratch/r4_clippy.log 2>&1; echo exit=$?`; `CARGO_BUILD_JOBS=4 cargo
  fmt -p shamir-engine -p shamir-server > .scratch/r4_fmt.log 2>&1; echo
  fmt_exit=$?`; `python -I scripts/ci/check_layout.py >
  .scratch/r4_layout.log 2>&1; echo layout_exit=$?`

If the combined check fails on an ENGINE file, STOP and report — engine
files are not yours.

Report: per fix file:line, traced deciding lines, gates' real exit
codes, `wc -l` of the three files (supervisor.rs before/after),
deviations.

---

## Stage 3 — VERIFIER child (READ-ONLY)

You independently re-read every changed TEST and the production path each
oracle depends on. You do NOT edit anything, you do NOT run any cargo
command (read the `.scratch/r4_*.log` tails instead). If you believe you
need a banned command, say so in your final message instead of running it.

1. Read IN FULL: `follower_cold_tests.rs`,
   `cold_reconcile/tests/helpers.rs`, `supervisor.rs` (changed regions +
   the new helper), `recovery_gap_consumer_tests.rs`,
   `adoption_durability_tests.rs`; `git status` (no new files anywhere)
   and the diffs of the five files.
2. P1: no two raw opens are alive in one scope anywhere in the test; the
   retry helper retries 25×20 ms and panics with the last error; the four
   drop-before-next-open comments exist; the WAL block, pre-close
   non-empty `recover()` assertion, legacy injection and every assertion
   survived.
3. P2-a: behaviour parity of `refuse_gate_init` vs the old inline arms;
   the three unit oracles match the helper per kind; the counterfactual
   comments state the correct one-line revert; the re-commented
   integration test's claims match the traced reality (gate cached at
   open: tx/recovery.rs:373 → repo_instance.rs:860; open refuses legacy
   bytes: durability_policy/repo_impl.rs:64-65); `pub(super)` reaches
   `crate::replication::tests`.
4. P2-b: the wrapper wraps `repo_handle.tx_info_store()`; trace decide →
   recorded save (set + flush) → fail-once flush → readable-not-durable →
   `prepare_existing` → presence-based enable (checks.rs) → gate flag;
   fresh-handle provenance assertions; the cancelled variant mirrors
   `adoption_retry`'s park mechanics with `Some(&repo_handle)` and no
   pre-existing test was weakened.
5. Cross-cutting: imports at file tops; zero new `#[allow]`; hand line
   counts (`wc -l`) of every touched file and entry counts of
   `replication/tests/` and `identity_fence/tests/` (the layout checker
   measures git-tracked files only; there are no new files this round);
   no file > 1000 lines; no folder above its recorded count.
6. Read the `.scratch/r4_*.log` tails and quote each final line and the
   workers' echoed exit codes.

Output: (a) MUST-FIX with file:line; (b) SHOULD-FIX; (c) verified-clean
checklist with file:line evidence per item; explicit "MUST-FIX: none" if
empty.
