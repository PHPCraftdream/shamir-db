# Checkpoint — 2026-08-01 00:20 [p0-p1-wave-complete]

## Session summary

This session executed the full P0/P1 remediation wave from the standing `/babygoal` directive: implement tasks #896-#909 (labels F-69 through F-82, derived from two independent readonly code reviews of shamir-db), committing between tasks, then finish with `/checkpoint` + changelog update + commit all `.md` + an `@oh` review of the whole wave. All 13 remediation tasks (F-69..F-81, #896-#908) are now DONE, committed, and personally zero-trust-verified — full diff read, `fmt`/`clippy --workspace --all-targets -D warnings`/`./scripts/test.sh` re-run independently, plus a genuine red-then-green sabotage-then-restore proof of each task's load-bearing mechanism.

Delegation transport shifted mid-wave: F-69 through F-76 ran via `Agent(subagent_type: "sh")`; from F-77 onward, per the user's explicit instruction, everything moved to the `crush` CLI (`crush run --role smart --session "<name>" ...`), still following the same prompt-first discipline (a committed brief in `docs/dev-artifacts/prompts/post-alpha/<NN>-*.md` before every launch) and the same zero-trust verification after every hand-off. Two crush runs this session (F-80, F-81) needed a second `crush run` invocation on the SAME `--session` after the first hit "Context deadline exceeded" partway through (F-80 first pass wrote the bench but never touched the CI job or committed; F-81's crush run itself completed cleanly but left the diff uncommitted, which I committed myself after verification) — both resumed with an itemized correction prompt naming exactly what was missing, per the lesson learned earlier in the wave (F-76's first attempt).

Verification highlights worth remembering: F-79's `PredicateSet` migration to lock-free `scc::TreeIndex` was proven via sabotage (disabling the length-mirror increment) — RED then GREEN, clean restore. F-80's loom CI job required TWO sabotage attempts to get a meaningful proof: the first (reordering `fetch_sub` before the write) did NOT fail, revealing that the loom test's assertion checks `writer_wrote` AFTER `writer.join()`, which can never observe an interleaving hole since `join()` always waits for full thread completion — a genuine pre-existing weakness in F-56's test design, now flagged as a follow-up finding rather than silently fixed (out of F-80's stated scope). Removing the write entirely DID fail, confirming the CI gate is load-bearing for at least the "missing write" bug class. Also hit a real infrastructure flake mid-verification: `sccache` dropped its connection ("An existing connection was forcibly closed by the remote host, os error 10054"), causing a spurious compile failure and, before that, a stale-cache false negative (a rebuild that silently reused pre-sabotage object code, requiring an explicit `touch` to force a real recompile) — both diagnosed and worked around, not blindly retried.

F-81's investigation (delegated to an Explore-style research agent first, since the exact builder/validation gap wasn't obvious from the task title alone) found `CreateIndex::build()` already existed but was infallible, while the TS client (`ddl.ts`) already validated the same invalid combinations synchronously — Rust's builder was behind its own sibling. Added `try_build()` mirroring the existing `Query`/`Batch::try_build()` precedent exactly, plus msgpack parity fixtures. Investigated whether a vector-family validation check was needed and confirmed it was NOT (all vector fields have server-side defaults; the only vector-specific check is a runtime registry-state check, not constructible-time-validatable).

After all 13 tasks closed, this session updated `CHANGELOG.md`'s `[Unreleased]` section with one detailed entry per task (F-69..F-81), matching this repo's existing dense-changelog-prose convention, and deleted two empty stray debug log files (`stress_affinity_err.log`/`stress_affinity_out.log`, 0 bytes each, leftover from earlier stress-test work) per CLAUDE.md's "clean up stray debug files" rule. The `docs/checkpoints/`, `docs/dev-artifacts/research/`, and `docs/dev-artifacts/roadmap/` `.md` files listed as untracked below are prior session artifacts (research reviews and roadmap docs that fed this wave's task list) still awaiting a commit — that is the very next step (F-82's "commit all .md" instruction), followed by launching an `@oh` review agent over the whole wave's diff.

Babysit cron (recurring, off-minute schedule) ticked throughout and correctly reported `still running #<id>` / `resumed #<id>` based on live git-commit and process-table signals, without ever restarting live work or looping tighter than its schedule.

## Active goal

Standing `/babygoal` directive (Stop-hook-enforced): реализуй задачи с помощью @sh (later: crush), между тасками делай коммиты. После завершения всей работы сделай /checkpoint, обнови чейнджлог, закомить все мд и запусти ревью агента @oh — пусть проверит всю работу. Status: checkpoint ✅ (this file), changelog ✅ (CHANGELOG.md updated, not yet committed), commit-all-.md — IN PROGRESS (next step), @oh review — NOT YET STARTED (final step, after the .md commit).

## TaskList

### in_progress
- #909 F-82: post-wave wrap-up — checkpoint, changelog, commit .md, @oh review. Description carries an additional discovered follow-up item: the `loom_model` test in `writer_drain_barrier.rs` asserts `writer_wrote` after `writer.join()`, not immediately after `run_drainer()` returns, so it cannot actually observe an interleaving hole between drain-return and write-landing (verified during F-80 sabotage-proof) — flagged for the `@oh` pass as a candidate follow-up, not fixed in F-80 (out of that task's stated scope).

### pending
- #910 F-68b (P2, tracked, not blocking): residual risk — 2 unresolved 600s CI hangs, instrumented. User has twice explicitly declined active pursuit of this task — do not restart work on it without a fresh explicit ask.
- #894: crush-fallback persistent sentinel, `STATUS: dormant` — never complete/delete, it's a marker.

### recently completed (this wave, all zero-trust verified + committed)
- #908 F-81: typed `CreateIndex::try_build()` + msgpack parity fixtures against the server's own validation. Commit `01fcef65`.
- #907 F-80: writer-drain overhead bench (4 cells, ~60 ns/op fast-path tax) + hard-gate loom CI job. Commit `33f5636b`.
- #906 F-79: `PredicateSet` migrated to lock-free `scc::TreeIndex`; `RepoTxGate::pending_commits` formally narrowed as sanctioned dead-scaffolding exception. Commit `f22a9ede`.
- #905 F-78: streaming legacy regular-index CREATE build (O(table)→O(batch) peak memory). Commit `a3970b8b`.
- #904 F-77: `Store::supports_atomic_transact()` capability flag replaces an overpromising prose contract. Commit `cff22d54`.
- #903 F-76: DROP INDEX visibility window closed for index2/regular/unique; new `shamir_index::lifecycle` doc. Commit `c256b85e`.
- #902 F-75: fixed F-65's own invalid test oracle with a genuine 3-table cascade chain. Commit `504c72b4`.
- #901 F-74: reordered `apply_index_batch` to bump the sorted-index AsOf epoch before applying postings. Commit `511c6478`.
- #900 F-73: `rederive_index2_ops_post_stage` now returns `Result` and fail-closes. Commit `85417a5d`.
- #899 F-72: legacy regular/sorted CREATE INDEX now planner-invisible until backfill completes. Commit `d87e97e7`.
- #898 F-71: AsOf epoch survives restart, CREATE floors, RENAME carries it. Commit `be5ff1c4`.
- #897, #896: F-70/F-69 — completed in the prior (pre-compaction) portion of this session.

## Decisions

- Both F-80 and F-81 crush runs needed a resumed `--session` with an itemized correction prompt rather than a fresh restart — cheaper and preserves the agent's own context of what it already did, per the established F-76 lesson.
- F-80's loom-test assertion-timing gap (found via sabotage-proof) was NOT fixed inline — it's a pre-existing F-56 test-design issue outside F-80's stated scope (measurement + CI wiring, not a behavior change); flagged in task #909's description for the closing `@oh` review instead of silently patched.
- F-81's vector-family validation: investigated and deliberately did NOT add a new `try_build()` check — confirmed via reading `table_manager_index_mgmt.rs` that every vector field has a server-side default and the one real vector-specific constraint (one vector index per table) is a runtime registry-state check, not something a construction-time builder can validate.
- Deleted two empty (0-byte) stray stress-test log files from the repo root rather than committing them — no content, no value, matches CLAUDE.md's stray-debug-file cleanup rule.

## Open questions

- None outstanding from the user for this checkpoint. The loom-test assertion-timing finding (see task #909's description) is a candidate follow-up to surface in the `@oh` review, not a blocking question.

## Repo state

```
 M CHANGELOG.md
?? docs/checkpoints/2026-07-29-review-remediation-wave.md
?? docs/checkpoints/2026-07-30-f70-verification-in-flight.md
?? docs/checkpoints/2026-07-30-post-remediation-review-and-ci.md
?? docs/checkpoints/2026-07-31-p0-p1-wave-in-flight.md
?? docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md
?? docs/dev-artifacts/research/2026-07-30-new-wave-readonly-review.md
?? docs/dev-artifacts/research/2026-07-30-post-remediation-readonly-review.md
?? docs/dev-artifacts/roadmap/2026-07-29-pre-alpha-remediation.md
```
```
01fcef65 feat(query-builder): F-81 -- typed CreateIndex::try_build + parity fixtures (#908)
75028422 docs(prompts): brief for F-81 -- typed CreateIndex try_build + parity fixtures (#908)
33f5636b fix(engine,ci): F-80 -- measure writer-drain overhead + add loom CI gate (#907)
7db52f5b docs(prompts): brief for F-80 -- writer-drain overhead + loom CI job (#907)
f22a9ede fix(tx): F-79 -- remove remaining std::sync::Mutex from runtime paths (#906)
8f68f8bb docs(prompts): brief for F-79 -- remove remaining std::sync::Mutex from runtime paths (#906)
a3970b8b fix(engine,index): F-78 -- stream legacy regular-index build instead of materializing the whole table (#905)
a4c21882 docs(prompts): brief for F-78 -- stream legacy regular/unique index build (#905)
```
