# Brief — #1213 apply_replicated preflight

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.

## FILE SCOPE (only these paths may be edited)

- `crates/shamir-engine/src/tx/apply_replicated.rs` (and, only if the outcome type lives in another file under `crates/shamir-engine/src/tx/`, that file for the new `ApplyOutcome::Rejected` / reject-reason type)
- `crates/shamir-server/src/replication/follower_loop.rs`, `error.rs`, `supervisor.rs`
- tests: a NEW topic file in `crates/shamir-engine/src/tx/tests/` (+ one-line `pub mod`) for the direct `apply_replicated` cases; `crates/shamir-server/src/replication/tests/follower_loop_tests.rs` for the loop case; adjust the existing `apply_replicated_tests.rs` tests that the contract change affects (see 5)
- do NOT touch `read_after_replication_tests.rs` (another agent's PR owns it) and do NOT touch the commit/drainer files (`commit.rs`, `materialize.rs`, `drainer.rs`, `version_guard.rs`)

## Task

#1213 (+ GitHub issue #52) — `apply_replicated` must fail closed on an unknown table / incompatible follower, with a PREFLIGHT before any version is spent. (S2 of the replication plan; the opt-in `ApplyPolicy::ValidatePayload` from #1216 is already merged.)

Broken on master (verified by a read-only review): `apply_replicated.rs` swallows `NotFound` from `get_table` (~:289-290); then `table_by_token` returns `None`, which only logs a warning and `continue`s (~:346-358), the function returns `Ok(Applied)` and the follower's bookmark advances — a silent loss of that table's writes. For events touching several tables the apply runs table by table in hash-map order (~:239-266): a failure on the second table leaves the first table's rows visible while the version is marked Aborted (`apply_committed_ops` publishes before the history write). Any `Err` in `follower_loop.rs` goes into an endless backoff (~:379-390), so one poison event is a livelock visible only in logs.

Required result:
1. A preflight BEFORE `assign_next_version_guarded` (~:228) over ALL tables of the event: `get_table` must succeed; the table must have an MVCC store (collect the `Arc<MvccStore>` handles here and USE them in the apply phase — no second lookup, and remove the base-store fallback for data tables); the follower table must have NO secondary index (hash / unique / sorted / index2 / vector -> reject; this is the fail-closed answer for "indexes are not maintained on a follower", which is documented as unsupported); every Put has a value (move the existing check from ~:244-250 into the preflight); with `ApplyPolicy::ValidatePayload` every Put payload decodes.
2. A refusal returns `ApplyOutcome::Rejected { reason: ApplyRejectReason }` (`ApplyRejectReason` is a small `Copy` enum: unknown table, no MVCC store, indexed table, missing value, undecodable payload). No version is spent, nothing is written, nothing is re-emitted downstream.
3. `follower_loop.rs`: `Rejected` is a TERMINAL stop — surface it as a new `ReplError::ApplyRejected { .. }` (in `error.rs`), do not advance the bookmark, and let the supervisor (`supervisor.rs` ~:289-317, the place that already reacts to `ReplError::JournalGap`) call `mark_subscription_resync_required` for it. Transient storage errors keep the existing retry/backoff behaviour. The undecodable-payload rejection of #1216 becomes this terminal rejection instead of an endless retry (update the #1216 follower-loop test accordingly).
4. Red tests (deterministic, direct `apply_replicated` calls on in-memory repos; tests first in spirit, one commit is fine): (a) an event for a table that does not exist on the follower -> today `Ok(Applied)` and `last_committed` grows; expect `Rejected`, `last_committed` unchanged, no downstream re-emit; (b) an event touching two tables, one valid and one unknown -> today the valid table's row becomes visible; expect NO row and no version; (c) the follower table has a hash index -> expect `Rejected`; (d) the follower loop receiving `Rejected` stops with `ApplyRejected`, the bookmark does not move, and the subscription is marked `resync_required`.
5. Contract fallout to handle: the existing test `a12_failure_path` (in `apply_replicated_tests.rs`, ~:396-430) relies on a failure AFTER version allocation ("Put without value"); after moving that check into the preflight it needs another post-allocation trigger — use a failing history store (look at how other tests in this crate inject a failing store) so the post-allocation abort path stays covered.
