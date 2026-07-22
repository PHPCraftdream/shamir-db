# Checkpoint — 2026-07-22 [pre-ri13-changelog-audit]

## Session summary

Continuation of the long `/babygoal`-driven campaign ("реализуй задачи с помощью `/crush`, между задачами делай коммиты, покрой их тестами. Если /crush войдет в пиковые часы, то переключайся на агентов @sh"). This window resumed from a checkpoint (`2026-07-21-fg3-in-flight.md`) with FG-3 (tx-scan read-your-own-writes) in flight via an `@sh` continuation agent. Since then, EVERY remaining task in the RI+FG backlog has been completed and zero-trust-verified: FG-3 (RYOW overlay, plus a self-found FK cross-table gap and a perf regression I fixed myself in `validator_db.rs`), FG-4 (`KNOWN_LIMITATIONS.md`, a new public citation-backed doc), RI-14 (2 stale TS e2e tests fixed to match already-landed behavior), FG-6 (Big-value Eq filter + ORDER BY fix — the follow-up gap FG-1 surfaced), and FG-7 (isolation-independent `expected_version` CAS validation — the follow-up gap FG-2 surfaced, and the deepest/most architecturally significant task this window).

Key events this window: (1) the user reinforced "when `/crush` runs out of limits, switch to `@sh`" as a standing rule; a genuine `/crush` attempt was made for FG-6 mid-window (per stop-hook feedback demanding real evidence) and hit the same hard weekly/monthly quota exhaustion (resets 2026-07-23 17:01:11) already known from before this window — confirmed with fresh evidence, then correctly fell back to `@sh`. (2) The user then explicitly said "забудь про /crush - используй @sh пока не скажу обратного" (forget `/crush`, use `@sh` until told otherwise) — this is now the standing delegation target, no more `/crush` attempts unless the user says otherwise. (3) For FG-7, since the task's own description flagged the fix approach as a genuine open design decision, I first asked the user (AskUserQuestion) who deferred to consulting `@fm` for an independent architectural read; `@fm` produced a rigorous analysis rejecting the three original options (a/b/c) in favor of a 4th ("(d)": decouple CAS validation from Serializable/SSI entirely via a new independent `TxContext.cas_set`, validated at commit time regardless of isolation level) — the user chose (d). I then independently re-verified `@fm`'s key file:line citations myself before writing the implementation brief, mirroring FG-3's "investigate before delegating" discipline for engine-core work.

Every task followed the established discipline throughout: investigate → write a precise brief to `docs/dev-artifacts/prompts/correctness/` → commit the brief → delegate (now `@sh` only) → zero-trust re-verify (full diff read + independent re-run of fmt/clippy/tests, never trusting the delegate's own envelope — this window this caught and I personally fixed a real perf regression in FG-3's validator_db.rs fix, an unnecessary O(n) full-scan fallback the delegate's own item-6 fix introduced) → commit → mark task complete.

**As of this checkpoint, ALL RI+FG tasks are done except #742 (RI-13, frozen commit)** — every one of its blockers (#745, #746, #748) is resolved. #747 (FG-5) and #749 (RI-15) remain explicitly deferred post-alpha and do not block anything. The user then asked two things in sequence, currently IN FLIGHT: (1) "обнови чейнджлог" (update the CHANGELOG) — I discovered via `git log --oneline -- CHANGELOG.md` that RI-11 (restore command), RI-12 (README positioning), and RI-14 (stale TS test fixes) commits never added a CHANGELOG.md bullet, unlike every other RI/FG task this campaign — this needs to be fixed by adding the 3 missing bullets before anything else; (2) "сделай коммиты всех мд" (commit all outstanding .md files) — referring to the 9 untracked checkpoint files in `docs/checkpoints/` (normally left uncommitted per the checkpoint skill's own convention, but the user has now explicitly asked for them to be committed, overriding that default); (3) THEN run #742 (RI-13: full local gate, push, tag, verify green remote CI).

**This checkpoint was written mid-flight** (user typed `/checkpoint` while I was still investigating the CHANGELOG gap, right after the `git log --oneline -- CHANGELOG.md` command that revealed the missing RI-11/RI-12/RI-14 entries) — the changelog update itself has NOT yet been written, the checkpoint .md files have NOT yet been committed, and #742 has NOT yet been started.

## Active goal

A `/goal` Stop hook is armed with this exact condition text (originally set at campaign start, reinforced multiple times since, most recently generalized to "use `@sh` until told otherwise" for the `/crush` fallback specifically):

> реализуй задачи с помощью /crush , между задачами делай коммиты, покрой их тестами. Если /crush войдет в пиковые часы, то переключайся на агентов @sh

Standing amendment (this window, explicit user message, supersedes the `/crush`-first default until further notice): **use `@sh` directly, do not attempt `/crush`.**

## TaskList

### pending
- #742 RI-13. Frozen commit: полный локальный gate + push + зелёный удалённый CI (только по явной команде пользователя) — READY (all blockers resolved), awaiting the changelog/md-commit work below, then explicit go
- #747 FG-5 (пост-альфа). Server-side cursors / streaming результатов — deferred, does not block #742
- #749 RI-15 (пост-альфа). Глобальный inflight response-memory budget — deferred, does not block #742

### recently completed
- #751 FG-7 (isolation-independent CAS validation), #750 FG-6 (Big-value filter/ORDER BY fix), #748 RI-14, #746 FG-4, #745 FG-3, #744 FG-2, #743 FG-1, #741 RI-12, #740 RI-11, #739 RI-10, #738 RI-9, #737 RI-8, #736 RI-7, #735 RI-6, #734 RI-5, #733 RI-4, #732 RI-3, #731 RI-2, #730 RI-1 — the entire RI+FG backlog except #742/#747/#749.

## Decisions

- **`/crush` is off the table for now** — explicit user instruction this window ("забудь про /crush - используй @sh пока не скажу обратного"). Do not re-attempt `/crush` on new tasks until the user says otherwise, even after the 2026-07-23 quota reset.
- **FG-7's design: option (d)** (isolation-independent `cas_set` on `TxContext`, validated at commit via a widened `commit_lock` guard) — chosen by the user after an `@fm` architectural consultation that rejected auto-upgrading isolation (option a, real side effects on co-batched non-CAS ops) and document-only (option b, an empirically-reproduced silent lost-update) in favor of decoupling CAS from SSI entirely. Implemented, verified, committed (`0463e8d9`).
- **FG-6/FG-7 do NOT block #742** — explicit user decision earlier this window (AskUserQuestion), consistent with the FG-5/RI-15 "follow-up, not blocker" precedent. (Now moot — both are done anyway.)
- **CHANGELOG.md has 3 missing entries** (RI-11, RI-12, RI-14) discovered via `git log --oneline -- CHANGELOG.md` — none of those three commits touched the file, unlike every other RI/FG task. This is being fixed now per the user's "update the changelog" request.
- **Checkpoint .md files will be committed** — a deliberate override of the checkpoint skill's own default ("do NOT add to git automatically"), per the user's explicit "commit all .md" request this turn.

## Open questions

None outstanding that require a stop — the three-step request (changelog fix → commit .md files → run #742) is clear and sequenced; the only in-flight uncertainty is exactly which CHANGELOG bullets to write for RI-11/RI-12/RI-14 (mechanical, not a design decision) and whether any OTHER commits besides those three are also missing a bullet (needs one more `git log`/diff sweep to confirm before declaring the changelog complete).

## Repo state

```
?? docs/checkpoints/2026-07-17-1600.md
?? docs/checkpoints/2026-07-19-1015.md
?? docs/checkpoints/2026-07-20-0230.md
?? docs/checkpoints/2026-07-20-0245.md
?? docs/checkpoints/2026-07-20-1615.md
?? docs/checkpoints/2026-07-20-full-campaign-recap.md
?? docs/checkpoints/2026-07-20-storage-readme.md
?? docs/checkpoints/2026-07-21-fg3-in-flight.md
?? docs/checkpoints/2026-07-21-ri7-in-flight.md
```

```
0463e8d9 fix(tx,engine): isolation-independent expected_version CAS validation (FG-7)
55b9549c docs(prompts): brief for FG-7 -- isolation-independent CAS validation
530f200c fix(engine): Eq filter + ORDER BY correctly see promoted Big values (FG-6)
1dfd4513 docs(prompts): brief for FG-6 -- Big-value Eq filter + ORDER BY fix
6ff94b71 test(client-ts): fix 2 stale e2e tests pinned to already-fixed behavior (RI-14)
```

## Active timers

A babysit cron has been ticking every 15 minutes throughout this window (re-verify via `CronList` — job ids rotate), reporting "still running #<id>" while a task was in progress and "blocked" now that only #742 (awaiting explicit authorization) and the two post-alpha-deferred tasks remain pending. `/crush`'s weekly/monthly quota resets 2026-07-23 17:01:11, but per this window's explicit user instruction, do not re-attempt it proactively — wait for the user to lift that restriction.
