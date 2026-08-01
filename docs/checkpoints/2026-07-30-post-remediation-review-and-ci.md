# Checkpoint — 2026-07-30 05:26 [post-remediation-review-and-ci]

## Session summary

This session continued from `docs/checkpoints/2026-07-29-review-remediation-wave.md`.
Picked up F-65/F-66/F-67 (the P1 "wave 3" from the 2026-07-29 review), each
delegated to `Agent(subagent_type: "sh")` (crush-fallback still active,
hard-limit trigger, marker task #894) with a prompt-first brief committed
before launch, then personally zero-trust-verified (full diff read,
personal `fmt`/`clippy`/`./scripts/test.sh` re-run, and a genuine
red-then-green repro by temporarily sabotaging the exact mechanism) before
committing. All three landed:

- **F-65** (`28d39f31`): FK indexed CASCADE/SET NULL/ON UPDATE fast-path
  candidate re-reads no longer swallow read errors (`Ok(Some) / Ok(None) /
  Err` now properly separated at 4 sites in `fk_actions.rs`/
  `fk_on_update.rs`); added a `#[cfg(test)]` failure-injection seam
  (`TEST_READ_ONE_TX_BYTES_FAILURE`) and 3 new tests.
- **F-66** (`829f1227`): `TxContext::ri_barrier_tokens` moved from
  `std::sync::Mutex<TFxSet<u64>>` to a lock-free `scc::HashSet`.
- **F-67** (`e7a8c707`): `SortedIndexManager::last_mutation_version`
  became per-index (keyed by `name_interned`) instead of manager-wide, so
  mutating one sorted index no longer disables the AsOf cursor seek fast
  path for cursors reading a different index on the same table.

TaskList #891/#892/#893 all marked `completed`; the `/babygoal доделай
задачи c помощью @sh` goal was satisfied and its babysit cron
(`bd7bffb6`) was deleted once the TaskList held only the persistent
crush-fallback sentinel (#894).

**User then asked to push and monitor CI** ("сделай пуш, проконтролируй
ci"). Pushed 23 commits (`e145b1d3..28d39f31`) to `origin/master`. Watched
the three triggered workflows (`CI`, `numa`, `supply-chain`) via
`gh run watch` (backgrounded, notification-driven, no manual polling).
`numa` and `supply-chain` passed first try. The main `CI` run
(`30515165549`) FAILED on `cargo test lib (macos-latest)`:
`shamir-index::vector::tests::crash_recovery_tests::
restart_preserves_recall_at_10_against_brute_force` — "recall@10 after
restart (0.800) below 0.90 floor". Confirmed this is NOT a regression
from our wave (git history shows the last touch to `crates/shamir-index/
src/vector/` was `f6016bce`, itself a *prior* "fix flaky recall" attempt,
unrelated to F-55..F-67). Reran only the failed job
(`gh run rerun 30515165549 --failed`) — it passed on rerun, confirming
this is a flaky (not deterministic) failure.

**User then asked for two independent readonly reviews of the just-landed
wave**, run in parallel-ish fashion:
1. One review appeared already written by a separate process/session at
   `docs/dev-artifacts/research/2026-07-30-new-wave-readonly-review.md`
   (36 KB) — I read it fully and personally re-verified its 4 headline
   P0s directly against the code (not taking the doc on faith) before
   accepting them.
2. I separately launched `Agent(subagent_type: "oxx")` (explicit user
   request: "запусти @oxx ревью агента... исследуй в readonly режиме") —
   its report landed at `docs/dev-artifacts/research/
   2026-07-30-post-remediation-readonly-review.md`. This second review
   found **3 NEW P0s not in review #1**, two of which are regressions
   introduced by our own just-landed F-56/F-57/F-67, plus 6 P1 and 8 P2
   (only the P0/P1 summary has been read in full so far — the complete
   list of 8 P2s has NOT yet been read).

**User then said, twice, explicitly and emphatically: fix ALL CI first,
before anything else from the reviews, and root-cause every flake —
NO workarounds** ("но так же самая первая таска должна быть - наладить
все ci (пока ничего не начинаем)" then "в первую очередь нужно будет
починить все ci, все флаки - найти причину и исправить, а не сделать
обход"). This is a hard, explicit constraint on F-68/#895 and governs the
whole next wave: no threshold-loosening, no retry-wrapper, no
`#[ignore]`, no raised timeout, no dropped platform — root cause or
nothing, unless a threshold change is derived and justified in writing.

Then invoked `/oh заведи таски даорботок по ревью` — I created 14 new
TaskList items (#895–#908, F-68 through F-81) covering: CI stabilization
first (blocking everything), the 3 new P0s from review #2 (writer-barrier
torn read, DDL lock-order inversion, AsOf epoch init), the 4 P0s from
review #1 (partial-index visibility on CREATE, tx epoch-bump ordering +
an inverted safety comment I personally approved during F-67's review,
commit-time re-derivation fail-open), plus P1 follow-ups (F-65's own
grandchild test has an invalid oracle, DROP/RENAME lifecycle, Mirrored
Store visibility atomicity, streamed index build, remaining
`std::sync::Mutex` sites, writer-drain benchmarking + loom CI, typed
CreateIndex builders). Deliberately did NOT ticket perf-runner/
baseline/changelog/version/tag items — those are release-process
bureaucracy, explicitly out of this session's scope per an earlier user
decision ("мы занимаемся только кодом").

**No implementation work has started on any of #895–#908** — this
checkpoint is written at that exact boundary, per the user's explicit
"пока ничего не начинаем".

## Active goal

None currently armed. The prior `/babygoal доделай задачи c помощью @sh`
was satisfied and its babysit cron was deleted once F-65/F-66/F-67
landed. No new `/babygoal` or `/goal` has been set for the F-68..F-81
wave yet — the user has not yet said "start", only "create the tasks."

## TaskList

### pending (ready, no blockers)
- #895 F-68 (CI): stabilize all CI workflows — flaky + persistently failing jobs. **User's explicit FIRST priority; do not start any other task until this is genuinely green with root causes found, not workarounds.**

### pending (blocked by #895, and by each other per the dependency chain below)
- #896 F-69 (P0): collapse writer-barrier predicate into one atomic — fix Relaxed torn read (blockedBy: #895)
- #897 F-70 (P0): fix lock-order inversion between commit drain guards and DDL lock-then-drain (blockedBy: #895, #896)
- #898 F-71 (P0): AsOf epoch initialization — restart, CREATE and RENAME floors — F-67 regression (blockedBy: #895)
- #899 F-72 (P0): make legacy regular/sorted CREATE INDEX planner-invisible until backfill completes (blockedBy: #895, #897)
- #900 F-73 (P0): make commit-time index re-derivation fail closed (blockedBy: #895)
- #901 F-74 (P0): bump tx sorted epoch before posting apply + fix inverted safety comment (blockedBy: #895, #898)
- #902 F-75 (P1): fix F-65's grandchild test — invalid oracle leaves sites 2/3 unverified (blockedBy: #895)
- #903 F-76 (P1): unified DROP/RENAME INDEX lifecycle + per-family error/cancellation semantics (blockedBy: #899)
- #904 F-77 (P1): MirroredStore::transact — deliver visibility atomicity or stop claiming it (blockedBy: #895)
- #905 F-78 (P1): stream legacy regular/unique index build instead of materializing the whole table (blockedBy: #899)
- #906 F-79 (P1): remove remaining std::sync::Mutex from runtime paths or narrow the invariant (blockedBy: #895)
- #907 F-80 (P1): measure writer-drain overhead + add a loom CI job (blockedBy: #895, #896)
- #908 F-81 (P1): typed CreateIndex builders + validating try_build + parity fixtures (blockedBy: #895)
- #894 crush-fallback state (persistent sentinel — do not complete; last known STATUS: active, AGENT: sh, TRIGGER: hard-limit — has not been re-checked this session, may have reset if the weekly/monthly quota's 2026-07-30 17:01:11 reset has passed by the time work resumes)

### recently completed (most recent 10)
- #893 F-67 (P1): per-index mutation epoch instead of manager-wide high-water — `e7a8c707`
- #892 F-66 (P1): remove std::sync::Mutex from TxContext::ri_barrier_tokens — `829f1227`
- #891 F-65 (P1): FK indexed-action fast path must not swallow read errors — `28d39f31`
- #889 F-63 (P1-R4): pin dtolnay/rust-toolchain to immutable commit SHA — `75c156ba`
- #888 F-62 (P1-R3): wire perf-gate into the release DAG — `8eaf16cf`
- #887 F-61 (P0-R2): restrict perf-gate.yml to workflow_dispatch — `015e9b5f`
- #886 F-60 (P0-R1): perf-gate fail-closed bench parser hardening — `a5c3a29d`
- #885 F-59 (P0-5): MirroredStore::transact whole-batch error atomicity — `a66d68d6`
- #884 F-58 (P0-4): AsOf index-seek TOCTOU race closed — `15b5a729`
- #883 F-57 (P0-3): unified online CREATE INDEX lifecycle — `fcaae001`

(#881, #882 also completed this/prior session; all prior-session tasks
#857–#880 completed before the previous checkpoint's window.)

## Decisions

- User: fix ALL CI first, root-cause every flake, no workarounds
  whatsoever (stated twice, verbatim in Russian, both quoted in the
  summary above) — this is the governing constraint on task #895 and,
  transitively, on starting anything else.
- User: "пока ничего не начинаем" — tasks are created but implementation
  has not started; wait for an explicit go-ahead.
- Both independent reviews' claims were personally re-verified against
  the actual code before being ticketed (not accepted on the reviewing
  agent's/process's word alone) — e.g. personally read `load()` to
  confirm the epoch map is never persisted/restored (P0-3/N-1), personally
  read `commit_phases.rs:596-617` to confirm apply-before-bump ordering
  and that the safety comment there is inverted (P0-2/F-74).
- Deliberately did NOT ticket release-process items (perf-runner
  registration, `bench-baseline.json` capture, CHANGELOG, version/tag
  reconciliation) that both reviews flagged as release blockers — out of
  scope per the standing "we only deal with code" boundary from the prior
  session. Told the user this explicitly and offered to ticket them
  separately if wanted.
- Ticketed F-75 (#902) as a NEW P1 against our OWN F-65 test
  (`cascade_grandchild_recursion_propagates_read_error` has an invalid
  oracle — arms the failure injector on the same table it deletes from,
  so it passes via the primary delete's own read failing rather than via
  the grandchild-recursion sites it claims to cover) rather than silently
  patching it — this was caught by review #2, not by my own F-65
  zero-trust pass, which is itself worth remembering: my red-then-green
  repro on F-65 covered site 1 only, not sites 2/3.
- Dependency ordering in the TaskList encodes real code-conflict
  reasoning, not just severity: F-70 depends on F-69 (same
  barrier/lock surface, must not run concurrently); F-72 depends on F-70
  (overlapping DDL/lock work); F-74 depends on F-71 (both rework the same
  epoch mechanism); F-76/F-78 depend on F-72 (extend the same lifecycle
  state machine F-72 introduces); F-80 depends on F-69 (benchmark numbers
  are only meaningful after the barrier collapses to one atomic).

## Open questions

- Whether to ticket the release-operational items (perf-runner
  hardening/registration, `bench-baseline.json` capture, CHANGELOG/
  version/tag reconciliation) that both reviews call release blockers —
  offered to the user, no answer yet. Do not start this without an
  explicit ask.
- The full P2 list (8 items) from review #2
  (`docs/dev-artifacts/research/2026-07-30-post-remediation-readonly-review.md`)
  has not yet been read in full — only the P0/P1 summary from the
  agent's final-turn message. Worth a full read before the P1 wave
  (#902-#908) is exhausted, in case a P2 turns out more urgent on close
  reading.
- Crush-fallback marker #894's actual current state (armed vs. active)
  has not been re-checked this session — worth a `TaskGet` before
  delegating F-68 onward, since the quota's stated reset
  (2026-07-30 17:01:11) may or may not have passed.

## Repo state

```
?? docs/checkpoints/2026-07-29-review-remediation-wave.md
?? docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md
?? docs/dev-artifacts/research/2026-07-30-new-wave-readonly-review.md
?? docs/dev-artifacts/research/2026-07-30-post-remediation-readonly-review.md
?? docs/dev-artifacts/roadmap/2026-07-29-pre-alpha-remediation.md
```

(All committed work is already pushed to `origin/master` — nothing
ahead/behind. The five untracked files above are review/roadmap/
checkpoint artifacts not yet added to git; leaving that to the user per
the checkpoint skill's convention.)

```
28d39f31 fix(engine): F-65 -- FK indexed-action fast paths must not swallow read errors (#891)
e7a8c707 perf(index): F-67 -- per-index mutation epoch instead of manager-wide high-water (#893)
0dce5c1f docs(prompts): brief for F-67 -- per-index mutation epoch (#893)
829f1227 fix(tx): F-66 -- remove std::sync::Mutex from TxContext::ri_barrier_tokens (#892)
6d3f95cf docs(prompts): brief for F-66 -- remove std::sync::Mutex from ri_barrier_tokens (#892)
```

CI status as of this checkpoint: `numa` and `supply-chain` green on
`28d39f31`; main `CI` run `30515165549` is green AFTER a rerun of the one
failed job (`cargo test lib (macos-latest)`, the recall-flake described
above) — this flake is task #895's first concrete item to root-cause, not
a currently-open incident.
