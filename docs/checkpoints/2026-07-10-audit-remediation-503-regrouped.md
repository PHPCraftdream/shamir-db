# Checkpoint — 2026-07-10 [audit-remediation-503-regrouped]

## Session summary

Continuation of the long-running audit-remediation campaign against
`docs/audits/2026-07-06-*.md`. This window: closed out task #500 (WAL
segment-open sidecar) after applying two cheap `@fl`-review nits (reworded
a misleading warn-log message in `crate::segment_meta::remove_blocking` to
take a per-call-site `context: &str`; filed nit #2 as follow-up #522
rather than expanding scope) and committing (`a48e8b66`).

Then ran the full #501 pipeline for the Interner reverse-spine
(`docs/audits/2026-07-06-perf-radical-o-notation.md` finding 2.3): wrote
brief 44 (deliberately steering `@oh` AWAY from the audit's literal
chunked/segmented-spine suggestion, toward a flat-Vec doubling-growth
design to avoid touching the hottest decode-path indexing shape),
launched `@oh`. **Independent verification by the orchestrator (BEFORE
sending to `@fl`) found a genuine, reproducible data-loss race** in
`@oh`'s first implementation (a two-phase lock-free "ensure capacity, set,
confirm-and-retry" protocol) — a concurrent-growth stress test failed
with a lost touch. Root-caused precisely: a grower's non-atomic
per-cell clone-forward can race a concurrent in-place `OnceLock::set` on
the same still-live vec, the setter's own "am I still live" check can
spuriously pass (grow hasn't swapped yet), and the write is then silently
dropped when the grow's swap lands moments later using a stale clone.
Fixed directly (not re-delegated): replaced the two-phase protocol with a
single `std::sync::Mutex<()>` (`reverse_write_lock`) serializing ALL
reverse-spine WRITES while the READ path stays 100% lock-free — CLAUDE.md's
sanctioned low-frequency/setup-only exception. Re-verified (5x isolated
reruns of the stress test + full suite + bench: mutex design is flat
O(N)/touch and even slightly FASTER single-threaded than the buggy
lock-free attempt). Sent through `@fl` review a second time — SHIP IT WITH
NITS (5 nits, all cosmetic/doc/test-coverage, no new bugs); applied all 5
(reworded stale doc comments, added a `debug_assert` — caught my OWN bug
mid-edit where the `.set(arc)` call would've been silently elided in
release builds if left inside `debug_assert!`'s condition — fixed before
it shipped, and added a trailing-doubling-capacity gap test). Committed
(`82d04e7b`) with an extensive commit message.

Task #502 (fjall per-op `spawn_blocking` overhead, finding 3.3, deferred
once already from #490): investigated by reading fjall 3.1.6's actual
source from the local cargo registry cache — confirmed the audit's
suggested "skip spawn_blocking when cached" approach is infeasible without
forking fjall (no cache-only/non-blocking API exists anywhere in its
public surface). The alternative (sharded worker-loop + MPSC batching) was
deferred with documented reasoning (no fjall bench exists yet to honestly
measure before/after; architecturally invasive; batching classically
trades tail latency for throughput, unmeasured). Wrote
`docs/design/fjall-per-op-spawn-blocking-investigation.md`, committed
(`d439a3fd`), filed two scoped follow-ups (#523 add the missing fjall
bench first, #524 prototype worker-loop batching once that bench exists,
blocked on #523).

Started task #503 (RecordKey alias cutover `Bytes`→`KeyBytes`, step 2 of
`docs/design/record-key-128-migration-plan.md`, task #491's step 1 already
landed). Wrote brief 45, committed (`73da52d3`), launched `@oh` as a
background agent — **this is still running, not yet returned, not yet
verified or committed** — `git status` currently shows ~100 tracked files
modified workspace-wide (RecordKey is used almost everywhere), consistent
with this being mid-flight.

**Mid-session: two methodology changes, both explicitly requested and
confirmed by the user via `AskUserQuestion`:**
1. **Lighter per-task gate.** Per-task verification is now `cargo check`
   + scoped `./scripts/test.sh -p <touched crate(s)>` only, with immediate
   commit — NOT the full build+fmt+clippy+test gate used for #500/#501/#502.
   A single **FINAL-GATE** task (#529) at the very end of the whole
   remaining series runs the full `cargo fmt --all -- --check` +
   `cargo clippy --workspace --all-targets -- -D warnings` +
   `./scripts/test.sh --full`, fixing everything found in one pass. This
   trades faster per-task iteration for deferred/batched regression
   attribution risk — the user explicitly weighed this trade-off and chose
   scoped-test-per-task (not zero-test-per-task) as the compromise.
2. **Task regrouping.** The user asked to group related pending tasks into
   single combined briefs/passes. Wrote
   `docs/roadmap/2026-07-10-audit-remediation-regroup.md` (committed
   `961408cd`) and restructured the TaskList: created #525 (G1 = former
   #504+#505, KeyBytes migration steps 3+4), #526 (G2 = former #517+#518,
   keyset-pagination fixes), #527 (G3 = former #513+#514, security
   residual), #528 (G4 = former #509+#511+#521, test flakes/failures), and
   #529 (FINAL-GATE). The 9 absorbed original tasks were explicitly marked
   `deleted` (confirmed to the user when asked). Dependency edges updated:
   #525 blockedBy #503; #506 blockedBy #525; #529 (FINAL-GATE) blockedBy
   #525, #526, #527, #528, #506, #507, #512, #515, #516, #520, #522, #523,
   #524.

The `/goal` Stop-hook ("решить все задачи") from earlier in this campaign
is presumed still active (not re-confirmed this window) and the `/babysit`
15-minute cron has been ticking throughout, each tick correctly reporting
"still running #503" since no new commit/signal landed for that specific
task during this window.

## Active goal

`/goal`: **"решить все задачи"** (solve all tasks) — presumed still
active from earlier in this campaign; not re-verified via CronList this
window. If stale, the user should re-arm with:
```
/goal решить все задачи
```

## TaskList

### in_progress
- #503 PERF-RADICAL-STRUCTURAL step 2: RecordKey alias cutover to KeyBytes (mechanical, no logic change) — `@oh` running in background, NOT yet returned/verified/committed

### pending
- #525 G1: KeyBytes шаги 3+4 — alloc-free key-конструкторы + sweep residual copy_from_slice  (blockedBy: #503)
- #526 G2: keyset-пагинация — record-id tie-breaker + short-page на stale index
- #527 G3: security-residual — subscription-cap slot leak + SSRF DNS-rebind/octal
- #528 G4: тестовые флейки/падения — oversample + trusted_pure_scalar + argon2id
- #506 PERF-RADICAL-STRUCTURAL step 5 (optional, measure-first): raise KeyBytes INLINE_CAP or add a posting-key tier  (blockedBy: #525)
- #507 CLEANUP: fix stray backslash comment typos in read_exec.rs (found during #492 review)
- #508 TEST: add real fault-injection regression test for WalGroupCommit::append_many all-or-nothing (finding 1.6 residual)
- #512 SECURITY: design a correct fix for resumption-ticket channel-binding (finding 1d, reverted attempt in #495)
- #515 PERF: MemBuffer merge-overlay scan (finding 5, deferred from #496)
- #516 PERF: fused SQ8 rescore + weighted-SIMD distance kernels (finding 4 items a/b, deferred from #496)
- #519 CLIENT: node-binding typed error .code/.retryable (found during #497 review, needs napi-rs 3.x — version bump needs explicit user permission, not yet requested)
- #520 CLIENT: Rust client roundtrip has no request timeout (found during #497 review)
- #522 TEST: strengthen reactivated_segment_sheds_stale_sidecar to exercise poison-rotation path (found during #500's second @fl review)
- #523 PERF: add fjall-backend bench (point get/set/scan, real tempdir)
- #524 PERF: prototype sharded worker-loop batching for fjall point-ops  (blockedBy: #523)
- #529 FINAL-GATE: полный fmt + clippy + test --full по workspace + фикс всего найденного  (blockedBy: #525, #526, #527, #528, #506, #507, #512, #515, #516, #520, #522, #523, #524)

### recently completed
- #502 PERF: investigate fjall per-op spawn_blocking overhead (commit d439a3fd, deferred with design doc)
- #501 PERF: Interner reverse-spine doubling growth (commit 82d04e7b)
- #500 PERF: WAL segment-open sidecar (commit a48e8b66)
- #499 PERF-RADICAL-3.2 (commit eb34e955)
- #498 RUSTSEC triage (commit 07add530)
- #497 HIGH-client residual (commit 6c2297cf)
- #496 HIGH-perf residual (commit b89172da)
- #495 HIGH-security residual (commit b41a4842)
- #494 MEDIUM-durability residual (commit 83fe85f3)
- #493 bench-scale-tool migration (already-done, no commit needed)

(9 tasks deleted this window as absorbed-into-groups: #504, #505, #509,
#511, #513, #514, #517, #518, #521 — see
`docs/roadmap/2026-07-10-audit-remediation-regroup.md` for what each
became.)

## Decisions

- **Interner concurrency bug (#501): fixed with a mutex, not a fancier
  lock-free retry scheme.** A `std::sync::Mutex<()>` serializing only the
  rare WRITE path (first-touch population) — while the hot READ path stays
  fully lock-free — is simpler, provably correct, and (measured) even
  slightly faster single-threaded than the buggy two-phase CAS-retry
  design it replaced. Chose this over hand-rolling a seqlock-style
  generation-counter scheme, which would have been correct but far more
  error-prone to verify by hand.
- **fjall spawn_blocking overhead (#502): deferred, not implemented.**
  Verified from actual fjall 3.1.6 source that the audit's "skip
  spawn_blocking when cached" idea is infeasible without forking the
  dependency. The remaining option (worker-loop batching) needs a
  benchmark that doesn't exist yet — filed as a prerequisite follow-up
  (#523) before attempting the architectural change (#524).
  Structural/high-complexity findings continue to get investigation-first
  treatment rather than blind implementation.
- **Per-task gate lightened.** Explicit user trade-off: `cargo check` +
  scoped test per task (not full build+fmt+clippy+test) to speed up
  iteration through the remaining ~15 tasks, with a single FINAL-GATE task
  catching everything at the end. User considered and rejected
  "check-only, no tests at all" (too risky) in favor of scoped-test
  (fast AND attributable).
- **Task regrouping.** Merged 9 originally-separate review-follow-up tasks
  into 4 grouped tasks (G1-G4) covering the same work, on the reasoning
  that adjacent/same-file/same-theme findings are cheaper to fix in one
  agent pass than in N separate brief→agent→review→commit cycles. Original
  tasks explicitly `deleted` (not just closed) since their content is fully
  captured in the new grouped tasks' descriptions and the roadmap doc.

## Open questions

- Task #503: has `@oh`'s background run finished? Check via the
  completion notification (do NOT poll `git status` or tail the agent's
  raw JSONL transcript file) — the last check before this checkpoint
  showed ~100 tracked files modified, consistent with an in-flight
  workspace-wide alias cutover, not yet independently verified by the
  orchestrator (per this campaign's zero-trust rule: build/check, run the
  byte-identity test suite, review the diff for any non-mechanical
  change, THEN send to `@fl`, THEN commit).
- Task #519 (napi-rs 3.x bump for node-binding typed errors): blocked on
  an explicit user decision to bump a dependency's major version — not
  yet asked this session. Needs to be raised separately when reached.
- Whether the `/goal` Stop-hook is still armed — not reconfirmed this
  window; if the session behaves as though it can stop early, the user
  should re-run `/goal решить все задачи`.

## Repo state

```
 M CLAUDE.md
 M bench-iters.txt
 M crates/shamir-engine/benches/interner_cold_growth.rs
 M crates/shamir-engine/benches/membuffer_concurrent.rs
 M crates/shamir-engine/benches/tx_pipeline.rs
 M crates/shamir-engine/src/meta/recovery_marker.rs
 M crates/shamir-engine/src/migration/coordinator.rs
 M crates/shamir-engine/src/repo/changelog_store.rs
 M crates/shamir-engine/src/table/buffer_config.rs
 M crates/shamir-engine/src/table/interner_manager.rs
 M crates/shamir-engine/src/table/record_counter.rs
 M crates/shamir-engine/src/table/table.rs
 M crates/shamir-engine/src/table/table_manager_crud.rs
 M crates/shamir-engine/src/table/table_manager_index_mgmt.rs
 M crates/shamir-engine/src/table/table_manager_replication.rs
 M crates/shamir-engine/src/table/table_manager_streaming.rs
 M crates/shamir-engine/src/table/table_manager_tx_ops.rs
 ... (workspace-wide, ~100 tracked files modified total — this is @oh's
     in-flight #503 RecordKey alias cutover, NOT YET independently
     verified or committed by the orchestrator)
 ?? (153 untracked entries — mostly stray *.log files accumulated across
     this entire campaign's gate runs in the repo root, pre-existing
     clutter not part of any pending commit; plus a handful of new
     docs/checkpoints/*.md and docs/prompts/audit/*.md files from this
     and earlier sessions)
```

```
961408cd docs(roadmap): regrouped remaining audit-remediation tasks + lighter per-task gate
73da52d3 docs(prompts): brief for #503 RecordKey alias cutover to KeyBytes (structural step 2)
d439a3fd docs(design): investigate fjall per-op spawn_blocking overhead (task #502, audit 3.3)
82d04e7b perf(types): interner reverse-spine doubling growth, no full-vec clone per touch (audit 2.3)
cb6c1f5b docs(prompts): brief for #501 interner reverse-spine doubling growth (audit 2.3)
```
