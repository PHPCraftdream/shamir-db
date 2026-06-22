בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — VersionWindow campaign PAUSED at the Stage 0 gate

## Session summary

The VersionWindow unification campaign was started (/babygoal, o46m sequential
sub-agents, /opti measure-first discipline) to extract a shared
`VersionWindow<K,V>` primitive and collapse 3 "version-primary sliding-window
stores" (drainer window, commit_write_log, decode/deliver caches) into one —
the overlay is EXCLUDED (version-secondary, different shape). Stage 0 (probe +
design, #173) is DONE and its gate fired the honest verdict:

**Architectural consolidation, NOT a perf win.** Measured: the caches stay
tiny under a healthy consumer (decode ≤30, deliver ≤60 entries = 2×lag×changes),
so O(log N) get ≈ O(1) — no generational/Variant-C specialization ever needed.
Drainer window (1-3) and commit_write_log (single digits) are similarly thin.
The ONLY measurable win is `commit_log_len()` O(N)→O(1). Real value = one tested
primitive + extinct "forgot the depth mirror" bug-class.

Design confirmed: `VersionKey` trait + `VersionWindow<K,V>` API covers all 3
access patterns cleanly, no additions. Home = `shamir-collections`.
evict_through = exact two-pass count (evicted slice is tiny → nanoseconds).

The campaign is PAUSED awaiting the user's appetite decision (the measure-first
gate exists precisely to make this call):
- **A. Stop here** — Stage 0 already banked the design doc + the cache-depth
  proof (which also vindicates Stage 2's get "regression" as moot) + benches.
  ROI of a 3-crate migration for cosmetics + one O(1) telemetry is not worth it.
- **B. Stage 1 only** — build the `VersionWindow` primitive (zero-risk, nothing
  wired) + tests as a clean artifact for later.
- **C. Stage 1→2→3** — full consolidation (cache + commit-log), take the O(1)
  commit_log_len, kill the bug-class. Perf-neutral (proven via /opti before/after).
  Stage 4 (drainer) stays behind a separate sanction regardless.

Agent recommendation: **A or B** (no perf win; respect measure-first).

babysit cron `c08d4604` is being CANCELLED here (idle loop with nothing to
watch). Re-arm on B/C.

## TaskList

- #173 [completed] VersionWindow Stage 0 — probe + design (gate)
- #174 [pending, ready] Stage 1 — build the primitive (blockedBy #173 ✓)
- #175 [pending] Stage 2 — migrate caches (blockedBy #174)
- #176 [pending] Stage 3 — migrate commit_write_log SSI (blockedBy #175)
- #177 [pending] STOP-POINT report + checkpoint (blockedBy #176)
- #178 [pending] Stage 4 — drainer (DEFERRED, needs explicit sanction; blockedBy #176)

## Decisions

- **Overlay excluded from VersionWindow** — version-secondary key, different
  (harder) shape; its gc_upto O(N) cliff was already measured theoretical.
- **evict_through exact-count, not approximate** — evicted slice is tiny
  (proven), and approximate depth would break commit_log_len telemetry.
- **Paused at the gate per the user's own /opti measure-first principle** —
  Stage 0 measured "no perf win", so the appetite decision is surfaced rather
  than auto-proceeding (same discipline that deleted Stage 1 of the prior
  hidden-O(N) campaign as gold-plating).

## Open questions

- **A/B/C appetite decision** — the campaign's continuation. Awaiting user.
- **Commit/push** — many commits ahead of origin from prior campaigns still
  unpushed (Op #2 cleanup, hidden-O(N) sweep, pagination, docs). Plus this
  campaign's Stage 0 artifacts are uncommitted. Awaiting "пуш".

## Repo state

```
 M crates/shamir-server/Cargo.toml
 M crates/shamir-server/src/subscriptions/tests/mod.rs
?? crates/shamir-server/benches/cache_struct_tradeoff.rs
?? crates/shamir-server/src/subscriptions/tests/cache_depth_probe_tests.rs
?? docs/perf/versionwindow-stage0.md
?? docs/checkpoints/versionwindow-paused-stage0.md
```

Stage 0 changed NO production code — only a probe test + bench + design doc.
Origin/master is many commits behind (prior campaigns unpushed).
