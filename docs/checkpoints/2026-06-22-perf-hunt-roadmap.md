בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-22 (perf-hunt roadmap delivered; awaiting pick)

## Session summary

Resumed from `2026-06-22-0500` (VersionWindow campaign closed at Stage 0
gate). First action this session: committed + PUSHED the entire unpushed
backlog. The uncommitted VersionWindow Stage 0 artifacts were triaged — kept
the probe test (a real bounded-cache-depth regression guard on the
TreeIndex-backed decode/deliver caches) + the design doc + checkpoints;
DROPPED the `cache_struct_tradeoff` bench (its DashMap-vs-TreeIndex verdict is
already consumed in the design doc and the Stage 2 decision is shipped). Two
commits landed: `816c402` (test) + `2e31854` (docs), gate green
(fmt + probe 2/2 + clippy --all-targets -D warnings), then `git push` took
master `b68fa1c..2e31854` to origin. **origin/master is now fully synced —
0 ahead.**

Then the user launched a `/workflows` perf-hunt with `oxx` (Opus 4.8 max
effort) agents: find O(N^2) / hidden-O(N) / multiplicative speedups across
9 hot-path subsystems → adversarial verify each candidate (refute-by-default;
"theoretical cliff that never bites" = real=false) → synthesize a
ROI-ranked roadmap. Workflow `wf_c9c2cc8e-094` (task `wucnvvqn7`) ran 56
agents / ~2.9M tokens / 8.5 min and confirmed **10 real hot-path findings**.

The roadmap (delivered to the user, NOT yet acted on):
- **#1 (Tier 1, M, up to ~10000x) — THE headline.** `$in @ref[].field`
  semi-join is a true O(outer×ref) cliff (both axes unbounded):
  `resolve_query_ref_column` is called INSIDE the per-row loop in
  `crates/shamir-engine/src/query/filter/filter_node.rs:286`, re-scanning +
  cloning the ref column per outer row. Fix: hoist to materialize the ref
  column ONCE into a `TSet<QueryValue>` at FilterContext build, O(1) probe —
  mirrors the existing `InSet` arm at filter_node.rs:241-243.
- **#2 (Tier 2, M, ~2-50x ×N subs) — filter AST (incl. Regex::new)
  recompiled per event** at `subscriptions/filter_eval.rs:43`. Compile the
  FilterNode once at subscribe-time in bridge_task, store alongside targets.
- **#4 (Tier 2, S, High-ROI cheap) — `value_qv` decoded before the
  deliver-cache check**, wasted on cache-hit / always in Keys mode, at
  `subscriptions/bridge.rs:345`. Move decode into Records cache-MISS branch.
  Stacks with #2 on the same fan-out loop.
- **#3 (Tier 3, M, ~50-500x const) — per-request §7.5 validity check** does
  2 fjall reads + full msgpack decode for one u64 at
  `shamir-connect/src/server/dispatch.rs:141`. Replace with in-memory
  `scc::HashMap<[u8;16], AtomicU64>` tickets-invalid-before, refreshed at the
  3 admin write_lock sites.
- **#5-#10 (Tier 4)** — real but single-digit-% end-to-end or behind rare
  isolation levels: WAL deep-clone per commit (S, pre_commit.rs:389),
  lookup_by_index BTreeSet clone (M, index_manager.rs:634), phantom-predicate
  O(P×W) rescan (S, Serializable-only, pre_commit.rs:332), min_alive
  full-scan (M, repo_tx_gate.rs:368), indexed_targets String alloc
  (M, ~1.02x), apply_distinct_qv extra collections (S, negligible).

Agent recommendation: start with **#1** under /opti measure-first (bench a
`$in @ref` shape → fix → re-measure), opportunistically grab **#4** (S,
stacks with hot fan-out). Nothing started — awaiting the user's pick.

Nothing in flight. No /loop or /babysit timers active. TaskList empty.

## Active goal

None (`/goal` not set). No babysit cron. TaskList empty.

## TaskList

Empty. The perf-hunt findings are captured in this checkpoint's roadmap, not
yet decomposed into tasks (awaiting the user's pick of which findings to
implement).

## Decisions

- **Committed + pushed the whole backlog** (user said "коммит" then "пуш").
  origin/master synced, 0 ahead.
- **Kept the cache-depth probe test, dropped the cache_struct_tradeoff
  bench** — the probe is a real bounded-depth regression guard on production
  eviction; the bench was a one-off decision-justification whose verdict is
  already in the design doc and whose Stage 2 outcome is shipped.
- **Ran the perf hunt as a verify-first workflow** (refute-by-default
  adversarial verify) so "theoretical cliffs that never bite" were filtered
  out — same measure-first discipline that closed VersionWindow.
- **Did NOT start any implementation** — awaiting the user's pick of which
  of the 10 findings to take.

## Open questions

- **Which perf findings to implement?** Agent recommends #1 (biggest
  multiplicative, O(N^2) semi-join) first under /opti, + #4 (cheap, stacks).
  Awaiting the user's selection before decomposing/implementing.
- **Strategy for the chosen work** — single-context vs sub-agents/workflow.
  Per standing rule, no sub-agents without explicit sanction.

## Repo state

```
(working tree clean)
```

```
2e31854 docs(perf): VersionWindow Stage 0 — design + closure checkpoints
816c402 test(subscriptions): cache-depth bound guard — VersionWindow Stage 0
94a88e4 docs(perf): hidden-O(N) sweep checkpoints + roadmap outcomes
0f5de6b chore(clippy): ban scc O(N) len() — hidden-O(N) sweep Stage 3
a37c950 fix(read): sorted-index ORDER BY+LIMIT must emit pagination — #128 sibling
```

Working tree clean. origin/master fully synced (0 ahead). Perf-hunt
workflow `wf_c9c2cc8e-094` completed; full result archived in the task
output (`wucnvvqn7.output`).
