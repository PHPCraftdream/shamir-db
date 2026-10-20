# Task 121 round 6 — CI regression briefs (S5c-X PR6b-3, PR #189 head bbd7c347)

Full CI matrix: lib tests pass on all 3 OSes, clippy/fmt/loom/ts pass, Windows
integration passes. FAILING on Ubuntu AND macOS (seed installer is Unix-only):
3 `shamir-server::seed_recovery_e2e` scenarios (a_crash_reseed,
b_second_crash, d_restarts) time out waiting for convergence on `items`
(harness panic at `crates/shamir-server/tests/seed_recovery_e2e/harness/follower.rs:234`).
Follower diagnostic at timeout: subscription `state=active`,
`status_message=null`, `last_terminal_reason=null`, identities recorded and
observed present and equal, `upstream_bookmark=7`, `next_pull_version=8`,
`local_consistent_version=1`, `certified_gap_first=1`,
`certified_gap_through=1`, `installation_state=complete`,
`seed_source_version=7`, `local_baseline_version=1`. Leader Hello:
`current_version=13`, `settled_version=13`, `gap_first=1`, `gap_through=7`.
The follower sits at the seed bookmark 7 and never applies the first batch
(upstream 8..13); the loop does not surface an error.

## Policy + tooling (pasted VERBATIM into every child prompt)

## 🚦 Compile / build / test policy — MANDATORY, highest priority

**Compilation, builds and tests run on GitHub CI, not on this machine.**
This rule overrides every other section of this file, every skill, every
brief and every habit ("let me just run it once to be sure"). Where any
other text contradicts it, this section wins. It binds the main agent,
every sub-agent, rush executor, and verifier child.

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
touched (`-p <crate>`, plus their dependents when a public signature
changed; `--workspace` only when asked):

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

TOOLING WARNING (verbatim): never use the shell builtin `xargs`; no
`cargo clean`; no `git add`/`git commit`; scratch logs under `.scratch/`;
one cargo process at a time with `CARGO_BUILD_JOBS=4`; never `xargs`.
If rustc dies with 0xc0000409 / STATUS_STACK_BUFFER_OVERRUN or a paging-file
error, rerun the SAME unchanged jobs=4 command once and report the failure.

## Stage E — ENGINE worker (R6-1 root cause + regression)
Files: shamir-engine (+ shamir-tx ONLY if the pinned mechanism lands there —
state it and gate `-p shamir-tx` too). Do NOT touch shamir-server.
1. TRACE, deriving each outcome from code with file:line, the seeded-follower
   first-batch path and identify the FIRST step that prevents bookmark
   advance 7 -> 13 or repeats forever:
   `repo/seed_install/writer.rs:272-330` (finish: v3 baseline bookmark=7,
   clean_through=Some(1), fence=None, `record_recovery_gap(1,1)`,
   `record_clean_horizon(1)`; the seed writes NO changelog rows);
   identity adoption `identity_fence/checks.rs:199-274` +
   `meta/repl/bookmark.rs:59-94`; admission
   `receipt/resume/activation.rs:47-99`; the apply entry
   `receipt/apply.rs:88-235` (admission/visibility/resume) and
   `apply.rs:300-536` (prepare watermark, chunking, try_chunk, the round-5
   completion predicate at :512-528, commit_prepared); `run_chunk_tail`
   `apply.rs:541-632` and `durable_tail` `apply.rs:639-702` (verify_journal
   repair via install_durable_events, confirm_journal, checkpoint);
   `receipt/checkpoint.rs:174-216` (verify_journal/confirm_journal) and
   `:250-330` (stage_checkpoint_batch / certified_clean_through :233-248);
   the changefeed writer `shamir-tx/src/changefeed.rs:399-429` (confirm_through),
   `:453-526` (mark_gap / record_recovery_gap / record_clean_horizon /
   first_gap_version), `:855-1042` (writer loop, Barrier = unconditional
   fetch_max at :1036-1039, RecoveryGap persist at :1028-1032),
   `install.rs` (process_install).
   Orchestrator eliminations so far (verify, don't trust): Barrier confirm
   is unconditional; the loop's gap-stop (`follower_loop.rs:490-507`) needs
   `gap_at >= from_version(8)`; admission is Native for a valid v3; the
   fence sync keeps `fence.bookmark == v3.bookmark` during adoption.
   Candidate class to confirm or kill WITH EVIDENCE: a step that fails
   repeatedly with a retriable (`DbError::Storage`) error ONLY in the
   seeded state (open recovery gap at version 1 + J=Some(1) + fence=None
   baseline + changelog rows 2..6 written by the first chunk), causing the
   3-attempt budget (`apply.rs:405-426`) to surface Storage, which
   `follower_loop.rs:583-584` swallows with warn+backoff forever.
2. FIX the root cause minimally. If the mechanism is the round-4/5
   gap-cap design (`certified_clean_through` capping at `first_gap - 1`,
   `checkpoint.rs:233-248`, mirrored at `apply.rs:443-444`, or the
   completion predicate `apply.rs:512-528`), redo that design so BOTH the
   `gap_tests.rs` oracles (open upstream gap caps J) AND the
   seeded-follower first-batch path hold; explain precisely why the new
   form satisfies both (e.g. the seed gap interval vs. the chunk's local
   range: J certification must not be blocked by a gap interval entirely
   BELOW the newly written and journal-confirmed range).
3. LIB REGRESSION (no e2e harness): a `#[tokio::test]` in the engine
   receipt tests that constructs the seeded baseline (real
   `SeedInstaller`/`writer.rs::finish` or the exact durable equivalent),
   performs identity adoption, then applies a first batch
   (upstream > bookmark) and asserts the bookmark advances to the batch's
   upstream end, rows visible, counter adopted, `Applied` outcome.
   COUNTERFACTUAL comment: with the pre-fix code the test reproduces the
   CI stall (name the exact failing call/error). Also assert the loop-level
   surfacing contract is not engine-visible (that part is the server
   worker's).
Gates until clean (real exit codes, one cargo at a time, CARGO_BUILD_JOBS=4):
`cargo check -p shamir-engine --all-targets` (plus `-p shamir-tx` if
touched), clippy `-D warnings`, `cargo fmt -p <touched>`,
`python -I scripts/ci/check_layout.py`. Logs under `.scratch/r6e_*.log`.

## Stage S — SERVER worker (loop surfacing, independent)
Files: `crates/shamir-server/src/replication/follower_loop.rs` (+ supervisor
and replication tests ONLY as needed). Do NOT touch shamir-engine, do NOT
touch e2e tests or their deadlines.
Make a persistent no-progress condition VISIBLE: the loop already swallows
transient apply/pull errors with warn+backoff (`follower_loop.rs:391-395`,
`:550-585`). Add a bounded consecutive-failure counter that RESETS on any
progress (bookmark advance) and, when exceeded (pick a constant > the
engine's inline budget of 3, justify it), sets a visible diagnostic on the
subscription row — reuse the existing supervisor plumbing
(`mark_subscription_terminal` / status_message path; inspect
`supervisor.rs`/`mark_subscription_*` first) — either a typed terminal
classification or at minimum a persistent `status_message`. The existing
`receipt_io_pause_retry_convergence` test (3 failures then convergence)
MUST still pass: the bound must not fire for it (counter resets on
progress/convergence; the surfaced state must not be permanent-unique).
Keep all other loop semantics unchanged. Update/extend a server unit test
covering: repeated failures without progress -> status_message (or
terminal) set; subsequent progress clears/does not block convergence.
Gates until clean: `cargo check -p shamir-server --all-targets`, clippy
`-D warnings`, fmt, layout — real exits, `.scratch/r6s_*.log`.

## Stage V — VERIFIERS (two, read-only, AFTER the last edit)
Engine verifier: re-derive the mechanism from code with file:line; trace
the new lib regression under (a) fix and (b) counterfactual revert, naming
the failing assertion; confirm gap_tests oracles still hold under the new
J design; confirm no OTHER clean_through/J completion predicate remains
(INCLUDING `cold_reconcile/follower.rs:35-48` and
`cold_reconcile/receipt_recovery.rs:43-143` — R6-2, which an earlier
verifier wrongly dismissed as nonexistent). Server verifier: the surfacing
counter (reset-on-progress, bound, plumbed state), no deadline/e2e changes,
budget-3 test still green by construction, diff limited to the authorized
files. Output: (a) MUST-FIX file:line; (b) SHOULD-FIX; (c) verified-clean
checklist with file:line; exact "MUST-FIX: none" if empty.
