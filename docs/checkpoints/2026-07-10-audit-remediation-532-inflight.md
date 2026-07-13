# Checkpoint — 2026-07-10 [audit-remediation-532-inflight]

## Session summary

Continuation of the very long audit-remediation campaign against
`docs/audits/2026-07-06-*.md`. This checkpoint picks up from
`2026-07-10-audit-remediation-520-inflight.md`. Since that checkpoint,
completed and committed: task **#520** (Rust client connect/request
timeout, commit `8d3fcff2` — 5 new tests, all genuinely proving the
timeout fires; mechanical update of 14 `ConnectOptions` construction
sites across 12 files; `shamir-server`'s own replication client got
real non-`None` defaults since it's production traffic).

Then worked through the remaining PERF-scope tasks:
- **#523** (fjall-backend bench) — investigated directly, found
  `storage_fjall_pump.rs` (already existing from an earlier task)
  fully satisfies the requirement; no new code, closed with baseline
  numbers recorded in #524's task description.
- **#524** (fjall worker-loop batching prototype) — **the most
  significant finding this window.** `@oh` implemented a single-
  dedicated-worker-thread design (all point-ops funneled through one
  OS thread, eliminating per-op `spawn_blocking`), benched it, and
  reported BOTH throughput-under-contention and uncontended latency
  improving — satisfying the brief's own "revert on regression" gate
  on its face. **An `@fl` adversarial review, launched because the
  orchestrator independently suspected an architectural risk (verified
  against fjall/lsm-tree's actual source before even asking for
  review), confirmed the concern**: fjall reads are explicitly
  designed for genuine multi-threaded concurrent execution (fjall's
  own README says so outright), so funneling every read through ONE
  worker thread discards real, exploitable read parallelism — a >10×
  regression risk under realistic cold-cache, high-fan-out production
  load (16+ cores, ≥64 concurrent readers on a hot table) that the
  prototype's own bench (1MB, entirely memtable-resident, zero disk
  I/O) was structurally incapable of exposing. **Reverted the
  prototype from the working tree (`git checkout` + `rm` on the new
  files — never committed, so this was a clean, harmless revert of
  disposable prototype code)**, documented the full investigation and
  a redesign recommendation (write-only worker + sharded read pool) in
  `docs/design/fjall-worker-loop-524-findings.md` (commit `e21e3c48`),
  filed follow-up **#536** for the actual redesign.
- **#506, #512** were completed in the prior window (see previous
  checkpoint) — investigated directly by the orchestrator (no agent),
  both concluded "defer/no-fix-warranted" with documented design-doc
  reasoning (KeyBytes INLINE_CAP raise not worth the `unsafe` risk for
  a write-path-only cost; TLS-exporter-equality channel binding is
  cryptographically impossible per RFC 9266, existing ticket-rotation
  counter is the correct mitigation already in place).

**Currently IN FLIGHT: #532** (re-key `MvccStore`'s Bytes-keyed
coordination maps — `cells`, `locks`, `VersionedOverlay`'s `OverlayKey`
— to `RecordKey`/`KeyBytes`, eliminating a round-trip allocation found
during G1/#525's `@fl` review). Wrote brief 54
(`docs/prompts/audit/54-perf-532-mvccstore-rekey-recordkey.md`,
committed `b032058a`), launched `@oh` as a background agent — **this had
NOT yet returned when the session was interrupted for this checkpoint.**
`git status` shows extensive files already modified (mid-agent-run, NOT
yet independently verified or committed): `crates/shamir-tx/src/{
cell_reservation_guard.rs, mvcc_store/{mod,drain,mvcc_gc}.rs}`,
`crates/shamir-engine/src/{table/table_manager_{crud,locks,replication,
streaming,tx_ops}.rs, tx/{commit,pre_commit}.rs}`, plus several test
files and `crates/shamir-tx/benches/{overlay_gc_cost_vs_depth,
tx_overhead}.rs` and `crates/shamir-engine/benches/tx_concurrent.rs`.
This is a genuine, deliberate public-API break within `shamir-tx`
(changing `key: Bytes` → `key: RecordKey` on several public methods) —
the brief explicitly asked the agent to verify `shamir-tx` is
internal-only before proceeding, and to scope down if the caller-side
blast radius proved too large; the actual outcome (kept vs.
scoped-down) is unknown until the agent's report returns.

**Unrelated observation**: `git log` shows two commits
(`8ed770f4` "permission-system security review (5 parallel @fxx
agents)" and `ea07f8c4` "synthesize permission-system audit summary")
that are NOT part of this orchestrator's task-tracked campaign work —
these appear to be from a separate `/fxx` skill invocation (referenced
in this session's skill-usage history: "sделай ревью проекта... используй
несколько агентов @fxx для ревью на разные области") that ran
independently of the TaskList-tracked audit-remediation series. Noting
for completeness; not something this checkpoint's resumption needs to
act on unless the user asks about it specifically.

The `/goal` Stop-hook ("решить все задачи") is presumed still active
(not re-confirmed this window). The `/babysit` cron has been ticking
throughout, correctly reporting progress signals at each tick.

## Active goal

`/goal`: **"решить все задачи"** (solve all tasks) — presumed still
active; not re-verified via CronList this window. If stale:
```
/goal решить все задачи
```

## TaskList

### in_progress
- #532 PERF: re-key MvccStore's Bytes-keyed coordination maps to RecordKey — `@oh` background agent running, NOT yet returned/verified/committed at checkpoint time

### pending
- #519 CLIENT: node-binding typed error .code/.retryable (needs napi-rs 3.x version bump → needs explicit user permission, not yet asked; does NOT block FINAL-GATE)
- #529 FINAL-GATE: full fmt+clippy+test --full + fix everything found  (blockedBy: #533, #534, #535, #536 — #532 itself will need to be added once resolved; NOT #519)
- #533 DESIGN: record-id tie-breaker for keyset pagination on ORDER BY value ties (needs a client-wire-protocol decision)
- #534 SECURITY/CORRECTNESS: close index2 CREATE INDEX lost-write race + crash-orphan-postings window
- #535 FIX: MemBuffer dirty_nonempty flag has a clear-race that can mask an ACKed write
- #536 PERF: redesign fjall worker-loop as write-only worker + sharded read pool (follow-up from #524's revert)

### recently completed (this window, in order)
- #524 fjall worker-loop prototype — implemented, reviewed, REVERTED with documented findings (commit e21e3c48), no code change landed
- #523 fjall-backend bench — already satisfied by existing bench, closed with no new commit
- #520 Rust client connect/request timeout (commit 8d3fcff2)

## Decisions

- **#524's prototype reverted despite passing its own bench gate** —
  the orchestrator independently suspected (before even requesting
  review) that funneling all fjall reads through one worker thread
  might discard real read parallelism, verified this against
  fjall/lsm-tree's actual source, and used `@fl` to confirm/quantify
  the concern rather than accept the prototype's own bench numbers at
  face value. This is the session's clearest example of NOT trusting
  a sub-agent's self-reported "it passed the bench" when the
  orchestrator has independent reason to suspect the bench itself is
  blind to the real risk.
- **#523 required no new code** — recognized an existing bench already
  satisfied the task's stated requirement rather than creating
  duplicate work.
- **#532 is a deliberate public-API break within shamir-tx** (not
  wire/client-facing) — the brief explicitly gated this on confirming
  shamir-tx has no external consumers before proceeding.

## Open questions

- Task #532: did `@oh`'s background run finish, and what was the
  outcome (kept as designed, or scoped-down with a documented
  blocker)? Check via the completion notification (do NOT poll `git
  status` repeatedly or tail the agent's raw JSONL transcript) —
  resume verification (cargo check + scoped `./scripts/test.sh -p
  shamir-tx -p shamir-engine`, spot-check the diff, confirm shamir-tx's
  internal-only assumption was actually verified by the agent rather
  than assumed) once it returns.
- Task #519 (napi-rs 3.x bump): still blocked on an explicit user
  decision not yet asked this session.
- Whether the `/goal` Stop-hook is still armed — not reconfirmed this
  window.
- The two `/fxx`-originated commits (`8ed770f4`, `ea07f8c4`) — unclear
  if the user wants anything further done with that separate
  permission-system review, or if it's already considered complete on
  its own track.

## Repo state

```
 M CLAUDE.md
 M crates/shamir-db/tests/purge_history.rs
 M crates/shamir-engine/benches/tx_concurrent.rs
 M crates/shamir-engine/src/repo/tests/repo_instance_tests.rs
 M crates/shamir-engine/src/table/table_manager_crud.rs
 M crates/shamir-engine/src/table/table_manager_locks.rs
 M crates/shamir-engine/src/table/table_manager_replication.rs
 M crates/shamir-engine/src/table/table_manager_streaming.rs
 M crates/shamir-engine/src/table/table_manager_tx_ops.rs
 M crates/shamir-engine/src/table/tests/asof_read_tests.rs
 M crates/shamir-engine/src/table/tests/covering_read_tests.rs
 M crates/shamir-engine/src/table/tests/history_read_tests.rs
 M crates/shamir-engine/src/tx/commit.rs
 M crates/shamir-engine/src/tx/pre_commit.rs
 M crates/shamir-tx/benches/overlay_gc_cost_vs_depth.rs
 M crates/shamir-tx/benches/tx_overhead.rs
 M crates/shamir-tx/src/cell_reservation_guard.rs
 M crates/shamir-tx/src/mvcc_store/drain.rs
 M crates/shamir-tx/src/mvcc_store/mod.rs
 M crates/shamir-tx/src/mvcc_store/mvcc_gc.rs
 (^ all from @oh's in-flight #532 work — NOT yet independently verified
    or committed by the orchestrator; likely more files touched than
    shown here, re-run `git status --short` on resume for the full list)
 ?? (many stray *.log files accumulated across this entire campaign's
    gate runs in the repo root — pre-existing clutter, not part of any
    pending commit)
```

```
ea07f8c4 docs(audits): synthesize permission-system audit summary
8ed770f4 docs(audits): permission-system security review (5 parallel @fxx agents)
b032058a docs(prompts): brief for #532 MvccStore Bytes-keyed maps -> RecordKey
e21e3c48 docs(design): revert fjall worker-loop prototype (#524), document findings + redesign
31d2357c docs(prompts): brief for #524 fjall worker-loop batching prototype
```
