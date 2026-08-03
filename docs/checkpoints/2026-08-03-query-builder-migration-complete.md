# Checkpoint — 2026-08-03 11:05 [query-builder-migration-complete]

## Session summary

This session continued directly from the prior #922-926 follow-up wave (F-68b hang fix, hnsw recall-flake fix, client API-completeness fixes) and its two `@oh` review rounds. The user asked to "актуализируй таски" (refresh the tasks) — a `/triage` pass found the TaskList clean (no hygiene issues), and per the user's choice, the 7 completed tasks from the prior wave (#915-921) were deleted (their history survives in CHANGELOG.md and prior checkpoints). Five real findings from the two `@oh` review rounds were kept as tasks #928-932 (Node engine-strict enforcement, two observability.rs doc-accuracy issues, missing TS e2e coverage for `setReplicator`, missing protocol-spec docs).

The user then flagged a separate, larger problem: `tests/e2e/tests` still hand-assembles wire-request objects almost everywhere — tasks #919/#920 (from an earlier wave) had only converted 6 of 18 files, and only their destructive/DDL/index ops, leaving the rest (reads, writes, filters, aggregations, sorting, replication ops) hand-rolled. The user explicitly noted this had already been attempted and "ничего не изменилось" (nothing changed) — an important, repeat request. After one round of scope calibration (first a 5-cluster task breakdown, then the user asked for one task per file instead), 18 individual tasks (#939-956) were created, chained sequentially via `blockedBy`, and executed one by one via `Agent(subagent_type: "sh")` — each preceded by careful, independent verification (reading the diff, cross-checking builder function signatures against the actual `@shamir/client` source in `crates/shamir-client-ts/src/core/builders/*.ts`, and re-running the full `tests/e2e` suite) before committing. All 18 completed with the test suite unchanged throughout: 130 passed / 0 failed, start to finish. One process violation occurred and was caught: task #949's agent ran `git commit` itself (against the standing "orchestrator commits" rule) — the commit's content was still verified and found correct before pushing, and subsequent agent prompts were strengthened with an explicit "DO NOT commit" instruction, with no recurrence.

After the migration completed, CHANGELOG.md was updated to document it, and the babysit cron (`cdfc5c2e`, armed earlier for the #928-932 work) picked up the remaining tasks automatically via its own tick logic. #928 (Node engine-strict enforcement) was completed by adding `tests/e2e/.npmrc` with `engine-strict=true`, empirically verified (temporarily set an impossible Node version requirement, confirmed `npm install` hard-fails with `EBADENGINE`, then reverted and confirmed normal operation). #929 (doc-accuracy fix in `observability.rs`'s shutdown comment, aligning its "between loop iterations" framing with `scheduler.rs`'s more precise "before the spawned task is first polled" framing for the identical bug class) was also completed directly. Both were done by the orchestrator directly (not delegated to an agent), since they were small, well-specified, low-risk changes.

The babysit cron is still armed and will pick up #930 (ObservabilityHandle Drop-safety) next on its own, per its tick logic — no goal or explicit instruction beyond the TaskList itself is driving this forward.

## Active goal

None (no `/goal` Stop-hook in force). Work is being driven entirely by the babysit cron ticking through the TaskList — the operative standing intent is simply "keep working through pending tasks," established implicitly by the two `/babygoal`-style task chains executed this session (the #922-926 wave's follow-ups, and the 18-file query-builder migration).

## TaskList

### in_progress
(none)

### pending
- #930 ObservabilityHandle leaks tasks on Drop -- fix doc or add DropGuard
- #931 Add e2e test coverage for shamir-client-ts's setReplicator against a real server
- #932 Document set_replicator in the client-server protocol spec

### recently completed
- #929 Fix imprecise "between loop iterations" doc claim in observability.rs's shutdown fix
- #928 Enforce (or explicitly decide not to enforce) tests/e2e's Node >=22.12 engines constraint
- #956 Query builders: 18-vectors.test.js (residual sweep) -- final file of the 18-file migration
- #955 Query builders: 17-replication-convergence.test.js (residual sweep)
- #954 Query builders: 16-replication.test.js (residual sweep)
- #953 Query builders: 15-transactions.test.js
- #952 Query builders: 14-index2-types.test.js (residual sweep)
- #951 Query builders: 13-migration.test.js
- #950 Query builders: 12-hmac-gate.test.js (residual sweep)
- #949 Query builders: 11-buffer-config.test.js
- (#939-948 also completed -- the remaining 9 of the 18-file migration chain, omitted here for brevity; see CHANGELOG.md's entry for the full list)

## Decisions

- Deleted the 7 completed tasks from the prior #915-921 wave per the user's explicit `/triage` choice, relying on CHANGELOG.md + prior checkpoints as the durable record instead of the live TaskList.
- Decomposed the query-builder migration into ONE task PER FILE (18 tasks, #939-956) rather than clustered groups, per the user's explicit correction ("заведи таску на каждый файл") -- this was a direct course-correction mid-conversation, not the orchestrator's own initial choice (which had been 5 clusters).
- For each of the 18 migration tasks, verified every non-trivial/novel builder function's signature against the actual TypeScript source in `crates/shamir-client-ts/src/core/builders/*.ts` before trusting an agent's claimed conversion -- caught nothing wrong in any of the 18, but this zero-trust discipline was applied uniformly regardless.
- After #949's agent self-committed (a process violation), did not revert or discard the commit (its content was independently verified correct) -- instead corrected the pattern going forward by adding an explicit "DO NOT commit" instruction to every subsequent agent prompt in the chain.
- For #928, chose a directory-scoped `tests/e2e/.npmrc` over a repo-wide one, to avoid any interaction with `crates/shamir-client-node`'s own, different `>=18` engines constraint -- verified the choice empirically rather than assuming it would work.
- For #928 and #929, did the work directly (no agent delegation) since both were small, fully-specified, low-risk changes where delegation overhead wasn't justified -- reserved agent delegation for the larger, more investigative work.

## Open questions

None outstanding. The babysit cron (`cdfc5c2e`, still armed) will autonomously resume with #930 next tick unless the user intervenes.

## Repo state

```
(clean -- nothing to commit; this checkpoint file itself is the only new untracked file)
```
```
f1821aa9 docs(server): #929 -- align shutdown doc's missed-wakeup framing with scheduler.rs
7d28a809 fix(e2e): #928 -- enforce Node >=22.12 engines constraint via engine-strict
9f27f143 docs: CHANGELOG entry for the completed tests/e2e query-builder migration
fe48cca9 refactor(e2e): #956 -- 18-vectors.test.js residual sweep (final file, migration complete)
e270c72e refactor(e2e): #955 -- 17-replication-convergence.test.js residual sweep
```
