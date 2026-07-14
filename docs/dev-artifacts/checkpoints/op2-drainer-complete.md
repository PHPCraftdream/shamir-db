בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — Op #2 incremental drainer cursor (campaign complete)

Date: 2026-06-22.

## Session summary

Op #2 from `docs/dev-artifacts/perf/perf-roadmap-2026-06-21.md` (ROI 82, "incremental drainer cursor") implemented across 6 sequential sub-stages delegated to `o46l` agents, with main-thread oversight, diff verification, and one main-thread bisect to triage a pre-existing test failure. Campaign reached the SUCCESS verdict: drain cost is flat O(W) per entry across W=1k → W=20k (a 20× depth range), confirming removal of the prior O(W²) cliff. Final gate green except for one pre-existing failure documented in TaskList #163.

The core change: `Drainer` now owns a `scc::TreeIndex<u64, Arc<WalEntryV2>>` window. Every successful commit offers its WAL entry to the window after `begin_grouped` returns durable. `drain_step` consumes the contiguous (durable_watermark, last_committed] prefix from the window in O(log N + k); only on a gap does it fall back to one `wal.recover()` reseed. `offer` is best-effort with a soft 64K high-watermark — under overload, drops are recovered transparently via the same gap-reseed path, so commits never block on the drainer.

The mental model: the WAL remains the source of durable truth; the window is a pure in-memory accelerator that is provably a subset of durable-WAL state. `recover()` retreats from "per-drain primitive" to "cold-start + gap-reseed primitive". No new durability surface, no new persistence format, no migration.

## Active goal

None (`/goal` not in use).

## TaskList

### in_progress
- #162 Op #2 — checkpoint + await commit/push sanction (THIS task; will close after writing this file)

### pending
- #163 Pre-existing — fix crash_mid_drain_recovers_all (broken since b2b1280 backend swap; NOT a blocker for Op #2 ship)

### recently completed (Op #2 campaign)
- #161 Op #2 Stage 5 — bench proof + full gate (SUCCESS verdict)
- #160 Op #2 Stage 4 — best-effort offer backpressure
- #159 Op #2 Stage 3 — rewrite drain_step to consume window (CENTREPIECE)
- #158 Op #2 Stage 2 — drainer.offer() in commit path
- #157 Op #2 Stage 1 — Drainer window (TreeIndex) + seed-on-spawn
- #156 Op #2 Stage 0 — probes + baselines

## Stage results (Op #2)

| Stage | Subject | Tests added | Gate |
|---|---|---|---|
| 0 | Probes: commit_version order, baselines, history-write topology | (read-only) | n/a |
| 1 | TreeIndex window + seed-on-spawn | 3 | drainer 10/10 + fmt + clippy |
| 2 | Wire offer() into 4 of 5 commit-path wake-sites | 3 | drainer 13/13 + commit 82/82 |
| 3 | drain_step consumes window + one-shot gap-reseed | 4 | @oracle --full 1410/1410 + crash_recovery 2/2 |
| 4 | Soft 64K high-watermark; best-effort offer | 3 | drainer 19/19 + @oracle 1412/1413 (1 pre-existing skip) |
| 5 | drain_cost_vs_depth bench + flat-throughput proof | 1 bench | full gate 6/6 with #163 excluded |

## Bench result (drain_cost_vs_depth, AFTER)

| W (depth) | Time (mean) | Throughput | Per-entry |
|---|---|---|---|
| 1,000  | 46.9 ms  | 21.3 Kelem/s | ~47 µs |
| 5,000  | 210.7 ms | 23.7 Kelem/s | ~42 µs |
| 20,000 | 847.8 ms | 23.6 Kelem/s | ~42 µs |

Flat per-entry cost across a 20× depth range confirms O(W) scaling. Under the prior O(W²) regime, per-entry cost at W=20k would have been ~20× worse than at W=1k. BEFORE baseline not captured directly (sub-agent declined `git stash` in a shared workspace); architectural argument + flat-shape observation are the proof. Document at `docs/dev-artifacts/perf/op2-stage5-results.md`.

## Files modified (uncommitted; awaiting commit/push sanction)

Source (5):
- `crates/shamir-engine/src/tx/drainer.rs` — TreeIndex window, offer/seed/accessors, drain_step rewrite, soft high-watermark, telemetry.
- `crates/shamir-engine/src/tx/pre_commit.rs` — `Arc<WalEntryV2>` plumbed through `PreCommit`/`ValidatedPreCommit`.
- `crates/shamir-engine/src/tx/commit.rs` — two offer call-sites (legacy + lockfree).
- `crates/shamir-engine/src/tx/group_commit.rs` — batch + single-tx offer call-sites.
- `crates/shamir-engine/src/tx/mod.rs` — `pub(crate) mod drainer` → `pub mod drainer` (for bench access; minor surface expansion, intentional).

Tests (1):
- `crates/shamir-engine/src/tx/tests/drainer_tests.rs` — 13 new tests across Stages 1-4.

Bench (1, new):
- `crates/shamir-engine/benches/drain_cost_vs_depth.rs` — depth-varying bench, parameterised W ∈ {1k, 5k, 20k}.

Config (1):
- `crates/shamir-engine/Cargo.toml` — `[[bench]]` entry for `drain_cost_vs_depth`.

Docs (untracked, new):
- `docs/dev-artifacts/perf/op2-stage0-probes.md` — Stage 0 probe findings.
- `docs/dev-artifacts/perf/op2-stage5-results.md` — bench proof + verdict.
- `docs/dev-artifacts/perf/phase2-routing-decision-2026-06-21.md` — campaign routing (from prior turn).
- `docs/dev-artifacts/checkpoints/2026-06-21-2206.md` — earlier mid-Phase-1 checkpoint.
- `docs/dev-artifacts/checkpoints/2026-06-21-2320.md` — prior session-end checkpoint.
- `docs/dev-artifacts/checkpoints/op2-drainer-complete.md` — this file.

## Decisions

- **TreeIndex over MPSC FIFO** — Stage 0 confirmed `commit_version` is dense-monotonic via atomic `fetch_add` (`repo_tx_gate.rs:285`) BUT the lockfree commit path can finalize WAL entries out of `commit_version` order, so an ordered structure is required. `scc::TreeIndex` is per the project's ideology pillar #5 (lock-free, sorted, O(log n)).
- **No double-write to collapse** — Stage 0 confirmed the D2 P1d-2b cutover already moved history writes drainer-only. Op #2's win is purely from removing `recover()`.
- **Gap-reseed over duplicate-handling** — under backpressure drops, the cost is one full `recover()` per drain pass that hits a gap, then linear scan from the window. Acceptable because the steady state is gap-free.
- **Best-effort offer, not blocking** — addresses rust-intel §B14 unbounded-handoff. Commits NEVER block on offer; the soft 64K watermark forces graceful degradation to today's path on overload, not deadlock.
- **`pub mod drainer` visibility expansion** — needed for the bench to construct `Drainer::new()`. Minor surface change; downstream callers within the engine crate already had access via `pub(crate)`. No external library consumer relies on this module.
- **Pre-existing test failure scoped out** — `crash_mid_drain_recovers_all` was broken by b2b1280 (backend swap redb→fjall). Confirmed pre-Op#2 via `git stash` bisect. NOT in scope for this campaign per CLAUDE.md "don't modify code unrelated to the task". Tracked as #163.

## Open questions

- **Commit / push sanction.** Six source files + 1 bench + 1 config + 6 docs are uncommitted. By standing instruction: "Ты никогда не делаешь коммит без явной моей просьбы" / "Ты никогда не делаешь пуш без явной моей просьбы". Awaiting "коммит и пуш".
- **Commit message decomposition.** Recommend either: (a) ONE feature commit `perf(tx): Op #2 incremental drainer cursor — window + offer + gap-reseed + backpressure (ROI 82)` covering Stages 1-5, plus a separate `docs(perf): Op #2 checkpoints + Stage 0/5 results`. OR (b) per-stage commits to mirror the campaign. Author preference required.
- **`pub mod drainer` surface.** If the public visibility is undesirable long-term, a follow-up could move `Drainer::new()` behind a `#[cfg(any(test, feature = "bench"))]` shim and revert to `pub(crate)`. Not pressing.
- **Pre-existing #163.** Address as a separate `fix(test): ...` work item when convenient.

## Repo state

```
 M crates/shamir-engine/Cargo.toml
 M crates/shamir-engine/src/tx/commit.rs
 M crates/shamir-engine/src/tx/drainer.rs
 M crates/shamir-engine/src/tx/group_commit.rs
 M crates/shamir-engine/src/tx/mod.rs
 M crates/shamir-engine/src/tx/pre_commit.rs
 M crates/shamir-engine/src/tx/tests/drainer_tests.rs
?? crates/shamir-engine/benches/drain_cost_vs_depth.rs
?? docs/dev-artifacts/checkpoints/2026-06-21-2206.md
?? docs/dev-artifacts/checkpoints/2026-06-21-2320.md
?? docs/dev-artifacts/checkpoints/op2-drainer-complete.md
?? docs/dev-artifacts/perf/op2-stage0-probes.md
?? docs/dev-artifacts/perf/op2-stage5-results.md
?? docs/dev-artifacts/perf/phase2-routing-decision-2026-06-21.md
```

```
c64fd41 fix(bench): cap s3_range_and N at 500K — bound 10-min wall-clock (#154)
d2d3504 fix(storage): MemBuffer *_many — publish dirty_nonempty before populating (#152)
35ebd40 perf(types): interner reverse-spine Arc<str> — O(N²)→O(N) cold growth (#151)
7140f82 perf(storage): scan_prefix_stream range-seek on sled — O(M²/bs)→O(log N+bs) (#155)
98f256d perf(storage): scan_prefix_stream range-seek — O(N+M²)→O(log N+M) on fjall (#146)
af2ff2d arch(storage): drop nebari/persy/canopy/redb engines, fjall as sole durable backend
```

Origin/master is 6 commits behind local; nothing pushed yet from yesterday's session either. Awaiting sanction.
