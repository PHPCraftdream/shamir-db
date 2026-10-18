# Task 121 round 5 — stage briefs (S5c-X PR6b-3, draft PR #189)

Round 5 fixes two P1 defects and three P2 defects found by the independent
review of round 4, worktree `worktrees/s5cx-retire-legacy-follower`,
branch `feat/s5cx-retire-legacy-follower` (round 4 committed externally as
`e026829a`, merged with master `184be75a`). Every stage brief below carries
the policy block VERBATIM (whole, including the CI workflow and logging
bullets) and the tooling warning VERBATIM.

## Policy + tooling (verbatim — the WHOLE block below is pasted into every child prompt)

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

TOOLING WARNING (verbatim from the user): never use the shell builtin
`xargs` (crashes the whole rush session) in your commands or any child's;
capture gate exit codes without pipes (`cmd > log 2>&1; echo exit=$?`);
scratch logs under `.scratch/` only; one cargo process at a time with
`CARGO_BUILD_JOBS=4` (never 1); do NOT run `cargo clean` or any command
outside the allowed list (if metadata is corrupt, re-run `cargo check`, and
report instead of cleaning); DO NOT run `git add`/`git commit`. If rustc
dies with an allocation/stack-buffer failure, retry the SAME jobs=4 command
and report the failure count.

Hard rules for every stage: surgical edits; imports at file top; NO new
files; re-read the current file region in full before each edit; trace the
production path that decides each outcome BEFORE writing an expectation,
and derive every oracle from code with file:line citations.

---

## Stage 1 — ENGINE worker (P1-1: completion evidence must not be J)

Files (exclusive): `crates/shamir-engine/src/tx/follower_durability/receipt/apply.rs`, plus ONE engine receipt test file chosen by line budget (see below). Do NOT touch `gap_tests.rs`.

Defect (confirmed by the reviewer): round 4's `certified_clean_through`
(`checkpoint.rs:233-248`) lets a completed, published chunk end with
`clean_through < chunk_local_end` (or `None`) while a gap is open —
correctly. But the post-checkpoint retry fast path in
`apply.rs:512-525` still recognises a completed chunk only by
`v3.clean_through.is_some_and(|clean| clean >= chunk_local_end)`
(:515-517). Checkpoint publication (bookmark advance + receipt retirement)
happens BEFORE the fallible final info-store flush
(`checkpoint.rs:428-457`). Trace with a gap at version 2: chunk local end
2 checkpoints; bookmark 2, receipt retired, J correctly stays 1; the final
flush returns `DbError::Storage`; the inline retry (attempt 2) finds no
outstanding receipt and `v3.bookmark (2) >= leader_end (2)` but rejects the
fast path because `1 >= 2` is false; it falls through to
`stage::commit_prepared` (`stage.rs:434-481`), which re-stages the OLD
bookmark and J (`stage.rs:388-397`) and recreates a receipt for an already
committed and published chunk; with the failure sticky, attempts exhaust
(`apply.rs:405-426`) and the pipeline returns an error despite completed
publication, having durably rewound the cursor to the predecessor.

Fix (root cause — J must not be completion evidence):
1. In `apply.rs`'s fast path, DROP the `clean_through` clause. The
   completion evidence is exactly what the checkpoint's Uncertain read-back
   already uses (`checkpoint.rs:328-333`: durable bookmark advanced AND
   receipt retired — they are written in ONE atomic batch,
   `checkpoint.rs:263-280`): `v3.outstanding_receipt.is_none() &&
   v3.bookmark >= leader_end && gate.last_committed() >= chunk_local_end &&
   gate.durable_watermark() >= chunk_local_end`. Update the surrounding
   comment: J is gap-capped and no longer evidences chunk completion; the
   bookmark advance + receipt retirement + watermarks do. Keep every other
   use of `certified_clean_through` as is.
2. New deterministic test (counterfactual stated below). FIRST run
   `wc -l` on `receipt/tests/receipt_apply_tests.rs` and check
   `scripts/ci/layout_baseline.txt` for its recorded size: if the file is
   at/over 1000 or baselined such that growth would fail the ratchet, put
   the new test in `receipt/tests/checkpoint_uncertain_tests.rs` instead
   (the `set_fail_final_flush` seam is `pub(crate)` and usable from any
   engine receipt test module). Report the placement decision + line
   numbers. The test mirrors the existing no-gap case
   (`post_checkpoint_flush_failure_fresh_chunk_is_done`,
   receipt_apply_tests.rs:868-920) WITH an open gap:
   - open a fjall repo, apply event (1,1) to seed bookmark 1 / J Some(1);
   - `repo.changefeed()` then `feed.feed.mark_gap(2)` (the exact seam
     gap_tests.rs:58 uses);
   - arm `set_fail_final_flush(&name, true)`;
   - `apply_replicated_batch(&repo, &[event(2,2)], 1, Trusted)` must
     converge to `Applied { local_version: 2 }` WITHOUT re-staging;
   - disarm; assert: history versions for the new key == [2]; table counter
     == 2 (adopted absolute); `load_bookmark_v3` == bookmark 2 AND
     `clean_through == Some(1)` (gap-capped — proving completion no longer
     rides on J); receipt key `NotFound`; `load_clean_horizon == Some(1)`;
     a repeat apply of event (2,2) → `Skipped`; close + reopen → bookmark
     2 still durable.
   - COUNTERFACTUAL (comment in the test): restoring the round-3
     completion evidence (`clean_through >= chunk_local_end`) in the fast
     path turns this red — attempt 2 rejects the fast path, re-stages
     Prepared over the advanced bookmark, and the sticky final-flush
     failure exhausts the 3-attempt budget, so the apply returns
     `DbError::Storage` instead of `Applied { 2 }`.

Gates until clean (real exit codes): `cargo check -p shamir-engine
--all-targets` → `.scratch/r5e_check.log`; `cargo clippy -p shamir-engine
--all-targets -- -D warnings` → `.scratch/r5e_clippy.log`. Then
`python -I scripts/ci/check_layout.py` → `.scratch/r5e_layout.log` (must
pass with the chosen test placement).

Report: the guard diff (file:line, before/after condition), the placement
decision + wc numbers, the full test body location, the traced production
lines that decide every assertion, the counterfactual, every gate's real
exit code, deviations.

---

## Stage 2 — SERVER worker (P1-2 + P2-1 + P2-2)

Files (exclusive): `crates/shamir-server/src/replication/identity_fence/tests/contact_tests.rs`, `crates/shamir-server/src/replication/identity_fence/tests/adoption_durability_tests.rs`, `crates/shamir-server/src/replication/tests/supervisor_tests.rs`. NO new files; do not change production.

### P1-2 — codec round trip must use the writer that really stores an arbitrary bookmark
`bookmark_codec_round_trips_boundaries_and_rejects_trailing_bytes`
(contact_tests.rs:239-316) still fails at the first `u64::MAX` iteration:
for an ABSENT key `recorded::save` (alias of
`save_fence_record_v3_aware`, bookmark.rs:59-93) writes a fresh follower
baseline with the flat AND fence bookmark forced to ZERO
(`bookmark.rs:82-91`), so the equality at :264 (loaded bookmark == supplied
`u64::MAX`) is deterministically false.
Purpose of the test: boundary round trip of the V3 BOOKMARK CODEC through
the writer that really stores an arbitrary bookmark, plus trailing-byte
rejection and the legacy-refusal block. Fix:
- Per boundary value `[0, 1, u64::MAX - 1, u64::MAX]`: remove the key;
  `recorded::save` a baseline (the observed_identity variant from the
  current matrix) to establish the fence; then write the boundary through
  the v3 writer the receipt advance path is built on —
  `save_bookmark_v3(&info, &BookmarkV3 { fence: Some(FenceRecord {
  bookmark: n, ..fence_from_baseline }), bookmark: n, clean_through: None,
  outstanding_receipt: None })` — syncing `fence.bookmark` to `n` exactly
  like the receipt checkpoint does (`checkpoint.rs:259-262`, required by
  the v3 decode invariant `fence.bookmark == record.bookmark`,
  bookmark_v3.rs:116-123). Assert: `recorded::load` returns
  `Some(FenceRecord { bookmark: n, ..baseline })`; `load_bookmark_v3`
  decodes bookmark n; `repo.replication_bookmark() == n`
  (`bookmark.rs:25-29` projects the v3 flat bookmark).
- Keep the trailing-byte probe per boundary (read the REAL saved bytes,
  push one byte, set, `load` errs, `save` errs) and the per-iteration
  fixture reset.
- Keep the final clean-v1 legacy-refusal block unchanged
  (contact_tests.rs:282-315) — it already asserts the derived reason
  (`bookmark_v3.rs:63-70`: version Some(1) → LegacyBookmarkFormat) plus
  save-refusal, then removes the key.
- COUNTERFACTUAL (comment): reverting to the old "save the supplied record
  on an absent key and expect it back" oracle turns the u64::MAX
  iterations red, because the fresh-baseline writer forces bookmark 0
  (`bookmark.rs:82-91`).

### P2-1 — the cancelled first contact must actually be DROPPED before the retry
`cancelled_first_contact_recovers_visibility_for_zero_event_follower`
(adoption_durability_tests.rs:540-607) pins `contact` and polls it through
`&mut contact`; tokio's `pin!` stores the future in a hidden block-scoped
local (tokio-1.49 `macros/pin.rs:125-135`), so the future stays suspended
inside `AdoptionStore::flush` while `prepare_existing` and the second
contact run — the test exercises retry BESIDE a live operation, not
recovery AFTER cancellation. Fix: enclose the first contact + `pin!` +
`select!` in a `{ ... }` block that ENDS before the recovery section
(exactly the shape of the older helper at adoption_durability_tests.rs
:289-298), with a comment stating that the future (and pin!'s hidden
local) drop at the block end, and that the Notify arrival proves the drop
happened at the parked flush seam. Keep every assertion as is.

### P2-2 — receipt_io_pause_retry_convergence must prove the PAUSE/RE-PULL path
One injected error is retried INSIDE the same apply call
(`apply.rs:405-426`), so it never requires a re-pull; the fixture ignores
`_from` and serves the same event every pull (:811-829), so `pulls >= 2`
can be an ordinary later iteration. Fix by exhausting the inline budget
(so the loop MUST surface the error, back off, and re-pull):
- `TransientReadHistory`: replace `armed: AtomicBool` with a budget
  counter (`remaining: AtomicUsize`, started at 3). `iter_range_stream`
  injects `DbError::Storage` while the budget lasts — use
  `self.remaining.fetch_update(Ordering::AcqRel, Ordering::Acquire,
  |n| n.checked_sub(1))` (checked_sub prevents wrap at 0; on `Err` it
  delegates) — then `failures.fetch_add(1)`. `get` stays a plain
  delegate; update the struct doc.
- Fixture: `remaining: AtomicUsize::new(3)`.
- Trace to cite in comments: attempts 1-3 of `try_chunk` each fail in
  `compute_counters` (`stage.rs:600`) — attempt 1 AFTER `commit_prepared`
  committed the durable Prepared receipt (so the park at `apply.rs:415`
  places the paused reservation and the re-pull takes the RESUME path);
  attempt >= 3 parks the guards and returns `DbError::Storage`;
  `follower_loop.rs:583-584` backs off and re-pulls; pull #2 resumes the
  receipt (4th scan call — budget exhausted, delegates) and converges.
- Assertions: `wait_for_bookmark` FIRST (the real condition — after it,
  the pulls counter is deterministic); then `failures == 3` (the inline
  budget was exhausted, not merely "a failure happened"); `pulls >= 2`
  (now implies the re-pull after the surfaced error); convergence to
  `leader_version`; `version_allocation_high_water_mark() == 1` and
  `items_row_counter == 1` (no replacements — the resume re-owns the
  receipt's version); `sub_state == "active"` (transient ≠ terminal).
- COUNTERFactual (comment): with the budget at 1 the old one-shot shape
  returns, the single failure is retried inline inside the same apply, and
  a passing `pulls >= 2` proves nothing about pause/re-pull — the
  exhaustion (3) is what forces the loop-level pause.

Gates until clean (real exit codes; one cargo at a time): scoped
`cargo check -p shamir-server --all-targets` → `.scratch/r5s_check.log`
and clippy → `.scratch/r5s_clippy.log`; then the final mandated combined
pass: check → `.scratch/r5_final_check.log`, clippy →
`.scratch/r5_final_clippy.log`, `cargo fmt -p shamir-engine -p
shamir-server` → `.scratch/r5_final_fmt.log`,
`python -I scripts/ci/check_layout.py` → `.scratch/r5_final_layout.log`.
If a combined gate fails on an ENGINE file, STOP and report.

Report: per fix file:line + traced deciding lines + counterfactual, every
gate's real exit code, `wc -l` of all three files, deviations.

---

## Stage 3+4 — VERIFIER children (READ-ONLY; two, engine and server)

Whole policy + tooling block above is pasted verbatim into each verifier
prompt. They re-read EVERY changed file AND the production paths behind
P1-1 and P1-2, and — because previous verifiers reported MUST-FIX none and
were wrong twice — every oracle must be DERIVED FROM CODE with file:line
citations, actively hunting for counterexamples (counterfactual
reverts that would NOT turn a test red, unsynchronized assertions,
futures kept alive, stale mirror state, layout-budget violations).

Engine verifier focus: apply.rs guard (old vs new condition; does any
OTHER consumer still treat J as completion evidence? grep
`clean_through` across the receipt pipeline and cold reconcile);
`certified_clean_through` unchanged; the new gap flush-failure test —
trace attempt-by-attempt under (a) the fix and (b) the counterfactual
revert, and state which assertion fails in (b); gap_tests.rs untouched;
layout numbers.
Server verifier focus: contact_tests round trip for 0/1/u64::MAX-1/
u64::MAX against `bookmark.rs:82-91` and `checkpoint.rs:259-262`;
trailing-byte + legacy blocks intact; the cancellation block truly ends
(pin! scoping) before recovery; the budget fixture — trace all four
`iter_range_stream` calls and the resume path; assertions ordered after
`wait_for_bookmark`; supervisor_tests.rs diff limited to the fixture +
assertions; layout numbers.

Output per verifier: (a) MUST-FIX with file:line; (b) SHOULD-FIX;
(c) verified-clean checklist with file:line evidence; explicit
"MUST-FIX: none" if empty.
