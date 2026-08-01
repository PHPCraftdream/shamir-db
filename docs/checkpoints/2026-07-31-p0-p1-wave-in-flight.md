# Checkpoint — 2026-07-31 [p0-p1-wave-in-flight]

## Session summary

This session executed the standing `/babygoal` directive: implement the remediation-wave TaskList (#896-#909, labels F-69 through F-82, derived from two independent readonly code reviews of shamir-db), committing between tasks, with a final wrap-up (checkpoint + changelog + commit all .md + `@oh` review) once everything is done. All P0 tasks (F-69 through F-74) and P1 tasks F-75 through F-78 are DONE, committed, and personally zero-trust-verified (full diff read, fmt/clippy/tests re-run independently, and a genuine red-then-green sabotage-revert proof for the load-bearing mechanism of each). F-79 (#906) is currently IN PROGRESS in the background via a `crush run` session (`f79-remove-std-mutex`) — not yet returned.

Delegation strategy shifted mid-session: F-69 through F-76 were implemented via `Agent(subagent_type: "sh", ...)`; from F-77 onward, per the user's explicit "давай перейдем на агентов /crush" instruction (to take effect after the then-in-flight F-75 agent completed), all delegation moved to the `crush` CLI (`crush run --role smart --session "<name>" ...`). Every task still follows the prompt-first discipline: a brief is written to `docs/dev-artifacts/prompts/post-alpha/<NN>-<name>.md` and committed BEFORE the agent/crush launch, and the orchestrator (this session) personally re-verifies fmt/clippy/`./scripts/test.sh` and performs an independent sabotage-then-restore proof before committing (crush is instructed NOT to self-commit — it proposes a commit message and the orchestrator reviews the diff, runs the gate, and commits).

Two crush-specific process lessons learned this session: (1) `crush run --session <id>` alone continues a session; combining it with `-C`/`--continue` is a CLI error ("If any flags in the group [session continue] are set none of the others can be") — use `--session` alone to resume. (2) A `crush run` can hit "Context deadline exceeded" mid-task and stop without committing, sometimes leaving a doc file with FALSE completion claims (F-76's first attempt: `lifecycle.rs` claimed "✅ FIXED + tested" and cited a test file that did not exist on disk) — always verify claims against actual repo state (`git status`, `ls` the claimed file) before trusting a crush summary, and re-invoke the same `--session` with an explicit correction prompt naming exactly what's missing if this happens.

The user separately flagged mid-session that the dev machine is "шумная" (noisy) with minimal process priority right now, which explains two transient test flakes encountered during verification: `rename_populated_survives_cold_restart` (TIMEOUT in a full-suite run, PASS in 1.2s isolated) and `f32_graph_dropped_after_fit_and_search_survives` (TIMEOUT in a full-suite run, PASS in 0.3s isolated) — both confirmed as machine-load noise, not regressions, via isolated reruns.

Babysit cron (`2b284a4b`, `3,18,33,48 * * * *`) has been ticking throughout and correctly reports `still running #<current task>` by checking for fresh git commits / working-tree diffs / active `crush.exe`/`cargo`/`nextest` processes as progress signals, without ever restarting live work.

## Active goal

Standing `/babygoal` Stop-hook condition (not a `/goal`-command condition, but functionally enforced by a Stop-hook that has fired "feedback" messages throughout this session demanding continued work): реализуй задачи с помощью @sh (later: crush), между тасками делай коммиты. После завершения всей работы сделай /checkpoint, обнови чейнджлог, закомить все мд и запусти ревью агента @oh. NOT yet satisfied — F-79 (#906) is in flight, F-80/F-81 (#907/#908) not started, and #909 (the wrap-up task itself) is blocked on all three.

## TaskList

### in_progress
- #906 F-79 (P1): remove remaining std::sync::Mutex from runtime paths or narrow the invariant — crush session `f79-remove-std-mutex` running in background (task id `bwjzqookp`), not yet returned. Working tree currently shows uncommitted, in-flight changes to `CLAUDE.md`, `crates/shamir-tx/src/predicate_set.rs`, `crates/shamir-tx/src/repo_tx_gate.rs` — NOT yet verified or committed by the orchestrator.

### pending
- #907 F-80 (P1): measure writer-drain overhead + add a loom CI job (blockedBy: none currently listed, but per the wave's sequential-delegation strategy should start only after #906 is verified+committed)
- #908 F-81 (P1): typed CreateIndex builders + validating try_build + parity fixtures
- #909 F-82: post-wave wrap-up — checkpoint, changelog, commit .md, @oh review (blockedBy: #906, #907, #908)
- #910 F-68b (P2, tracked, not blocking): residual risk — 2 unresolved 600s CI hangs, instrumented. User has twice explicitly declined active pursuit of this task this/prior session — do not restart work on it without a fresh explicit ask.
- #894: crush-fallback persistent sentinel, `STATUS: dormant` — never complete/delete, it's a marker.

### recently completed (this session, all zero-trust verified + committed)
- #905 F-78: stream legacy regular-index CREATE build (O(table)→O(batch) peak memory); unique-family streaming explicitly deferred per its own escape hatch. Commit `a3970b8b`.
- #904 F-77: `Store::supports_atomic_transact()` capability flag replaces an overpromising prose contract; audit found the bug latent-but-transient/self-healing, not escalated. Commit `cff22d54`.
- #903 F-76: DROP INDEX visibility window closed for index2/regular/unique (retire-definition-before-posting-sweep, mirror of F-72's CREATE fix); sorted DROP was already safe. New `shamir_index::lifecycle` doc unifies the CREATE/DROP/RENAME contract across all 4 families. Commit `c256b85e`.
- #902 F-75: fixed F-65's own invalid test oracle (self-referential CASCADE never recurses at all — `is_self_ref` filter — plus an armed-everywhere injector that consumed the wrong site's read); replaced with two discriminating tests over a genuine 3-table chain. Commit `504c72b4`.
- #901 F-74: reordered `apply_index_batch` to bump the sorted-index AsOf epoch BEFORE applying postings (closing a real cross-thread TOCTOU window), fixed an inverted safety comment in two places. Commit `511c6478`.
- #900 F-73: `rederive_index2_ops_post_stage` now returns `Result` and fail-closes on every previously-swallowed error class. Commit `85417a5d`.
- #899 F-72: legacy regular/sorted CREATE INDEX now planner-invisible until backfill completes (`IndexState::Building`/`Ready` state machine, mirroring index2's existing pattern). Commit `d87e97e7`.
- #898 F-71: AsOf epoch now survives restart (persisted `ready_at_version`), CREATE (`mark_ready_at` floors at table watermark), and RENAME (epoch carried to new name). Commit `be5ff1c4`.
- #897, #896: F-70/F-69 — completed in the prior (pre-compaction) portion of this session.

## Decisions

- Switched delegation transport from `Agent(subagent_type: "sh")` to `crush run` starting with F-77, per explicit user instruction, while keeping the same prompt-first + zero-trust-verify discipline unchanged.
- For F-77 (MirroredStore atomicity), chose the capability-flag resolution (`supports_atomic_transact`) over a real snapshot/overlay rewrite, because the audit found zero production callers actually exercise the durable-subset path and the overlay rewrite would touch every read path's scan-merge logic — a disproportionate risk for a transient, self-healing anomaly.
- For F-76 (DROP visibility), chose "retire definition first, sweep postings second" (no new `Dropping`/`Failed` enum variant) — full removal from the planner-visible structure is strictly stronger than an intermediate state the planner would also need to learn to filter.
- For F-78 (streaming CREATE), landed ONLY the regular-hash family per its own documented escape hatch; unique-hash duplicate detection needs global knowledge and was deferred rather than shipping an unsound bounded-memory rewrite under time pressure.
- Rejected trusting crush's own claim of task completion at face value after the first F-76 attempt (a doc claimed tests existed and passed when the test file did not exist on disk) — this is now a standing extra verification step for every crush hand-off: confirm claimed artifacts actually exist on disk before trusting a "done" summary.

## Open questions

- None outstanding from the user. F-79's outcome (which of the two `std::sync::Mutex` sites got a lock-free migration vs. a documented narrowing) is not yet known — the crush run has not returned.

## Repo state

```
 M CLAUDE.md
 M crates/shamir-tx/src/predicate_set.rs
 M crates/shamir-tx/src/repo_tx_gate.rs
?? docs/checkpoints/2026-07-29-review-remediation-wave.md
?? docs/checkpoints/2026-07-30-f70-verification-in-flight.md
?? docs/checkpoints/2026-07-30-post-remediation-review-and-ci.md
?? docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md
?? docs/dev-artifacts/research/2026-07-30-new-wave-readonly-review.md
?? docs/dev-artifacts/research/2026-07-30-post-remediation-readonly-review.md
?? docs/dev-artifacts/roadmap/2026-07-29-pre-alpha-remediation.md
?? stress_affinity_err.log
?? stress_affinity_out.log
```
```
8f68f8bb docs(prompts): brief for F-79 -- remove remaining std::sync::Mutex from runtime paths (#906)
a3970b8b fix(engine,index): F-78 -- stream legacy regular-index build instead of materializing the whole table (#905)
a4c21882 docs(prompts): brief for F-78 -- stream legacy regular/unique index build (#905)
cff22d54 fix(storage): F-77 -- make Store::transact visibility-atomicity contract honest (#904)
9c7d6ed4 docs(prompts): brief for F-77 -- MirroredStore::transact visibility atomicity (#904)
```

Note: the `M CLAUDE.md` / `predicate_set.rs` / `repo_tx_gate.rs` changes above are the CURRENTLY-RUNNING crush session's uncommitted, unverified in-flight work for F-79 — do not assume they are correct or final; the next action after this checkpoint is to wait for that crush run to return, then perform the standard zero-trust verification before committing.
