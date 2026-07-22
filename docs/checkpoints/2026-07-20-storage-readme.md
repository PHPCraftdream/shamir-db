# Checkpoint — 2026-07-20 [storage-readme-in-flight]

## Session summary

Continuation of a very long `/babygoal`-driven session working through
`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`,
standing directive: "реализуй задачи с помощью /crush, между задачами делай
коммиты, покрой их тестами." This session resumed from checkpoint
`2026-07-20-1615.md` (Этап 7 just completed). Since then: TaskList was
cleaned of all completed Этап 1-7 tasks; the work plan's Этап 8
(Performance, explicitly "post-blocker, не гейт релиза") was decomposed into
6 leaf tasks (8a-8f) and the two standalone leftovers (#695, #715) were kept.

**Этап 8 (Performance) — fully completed, all 6 tasks (8a-8f), each via
investigate → write brief → commit brief → `/crush` → zero-trust verify
(full diff read + fmt/clippy + `./scripts/test.sh` run TWICE) → commit**:
- 8a (F1, `9ba703e8`): pointer-keyed `FieldPathCache` for `resolve_filter_query`'s
  `FieldRef` arm, mirroring `CondCache` (#643).
- 8b (F2, `efe7c3b3`): lazy per-scan `QueryRefCache` (`OnceLock`-per-node,
  populated on first row) for `$query` resolution — structurally different
  from 8a since it needs runtime scan data not available at compile time.
- 8c (F3+F7, `22d30e7c`): score HNSW candidates inside the `read_sync`
  closure instead of cloning `Vec<u8>`/`Vec<f32>` per candidate; bonus
  `Arc<TFxSet>` hoist for co-filter's allow-set.
- 8d (F4, `82cfb962`): new SIMD kernels for `approx_l2_sq`/`fused_dot`;
  `dequant_norm_sq` canonicalized to reuse the EXISTING `weighted_bilinear_f32`
  kernel via a math identity (zero new kernel code); optional VR-7 query-norm
  hoist for Cosine graph traversal ATTEMPTED and PROVEN SAFE via a
  thread-local pointer-keyed stack (verified against `hnsw_rs` source that
  `.search()` is single-threaded per query). Highest-risk task in the batch;
  measured 2.7x Cosine / 3.8x L2 speedup on `sq8_hot_path` bench.
- 8e (F5, `e4305c33`): replaced ForEach's per-iteration msgpack round-trip
  with a custom `serde::Serializer` targeting `QueryValue` directly
  (`query_value_serializer.rs`) — chosen over hand-mirroring because
  `QueryRecord::Inserted` has non-obvious sorted-`_id`-interleaving
  serialize logic that would have been a maintenance trap to duplicate.
  12-case differential test proves parity against the OLD msgpack path.
- 8f (F6+F10, `8fa78cd1`): `CompactPath` now stores `InternerKey` directly
  (removes a per-row rebuild in 15 `matches()` arms); `InSet::matches`
  switched to `scalar_at`+`set_contains_coercing` (was `materialize_at`,
  inconsistent with sibling `In`). F6's optional Hash-compatible probe
  wrapper, F9 (Cow sort keys — hit a real self-referential-struct wall in
  the top-K heap), and F11 (write-value marker cache) were investigated and
  explicitly declined per the brief's own risk/effort calibration — all
  three declines were well-reasoned, not shortfalls.

**#695 (investigation) — completed.** A dedicated Explore-style sub-agent
confirmed a REAL silent data-corruption bug (not the FK-cascade-specific
concern originally suspected, but a general "two ops touch the same row in
one batch/tx" staging gap): `execute_update_tx` scans matched rows from the
COMMITTED store only (blind to this tx's own `write_set`), then stages its
merge result via `StagingStore::set` — an unconditional overwrite. If a
prior op in the same tx (e.g. FK cascade fan-out) already staged a
different value for that row, it is silently clobbered, no error. User
explicitly chose "full fix now" over a narrower stopgap or deferring.

**New task #729 (fix) — completed, commit `7801009a`.** `execute_update_tx`
now probes `tx.write_set`'s `StagingStore::staged_op` for each matched row
before merging, using the ALREADY-staged bytes (not the stale commit-store
scan) as the merge base for both the byte-level merge AND
`update_tx_bytes`'s index-delta planning. A staged `Remove` for the row
causes an UPDATE to skip it entirely (no resurrection). Two new regression
tests, BOTH independently demonstrated by the implementing agent to FAIL
without the fix and PASS with it (agent temporarily reverted the fix to
prove this) — verified independently by me via a full diff read + 2x green
`./scripts/test.sh` runs + clippy/fmt.

**#715 (doc rewrite) — IN FLIGHT right now, not yet verified/committed.**
Investigation found `shamir-storage/src/README.md` describes SIX on-disk
backends (Sled/Redb/Fjall/Nebari/Persy/Canopy); FIVE of those six do not
exist anywhere in the codebase (confirmed via `ls`, `lib.rs` module list,
`Cargo.toml` `[features]` — only `fjall = ["dep:fjall"]` exists, the
`all-backends` meta-feature expands to `["fjall"]` alone). A user aside
mid-task ("redb мы ведь удалили?") was answered directly: yes, fully
removed in code, only docs were stale. A detailed brief (committed
`e2f71a4b`) directs a full rewrite (not a word-swap) of the README's file
tree, `Store`/`Repo` trait sections (real signatures include `get_many`/
`set_no_flag`/`remove_no_flag`/`insert_many`/`set_many`/`remove_many`, not
just the 4 methods currently shown), backend comparison table, and the
stale `cargo test test_sled/test_redb/...` block (replace with
`./scripts/test.sh`). It also documents `storage_membuffer.rs` (a moka-based
write-back cache wrapper) which the README never mentions at all. Part 2 of
the brief covers `shamir-server/src/backup.rs`'s 4 stale redb doc-comment
references — explicitly NOT a simple rename, since redb's page-based
CRC32/atomic-commit properties don't necessarily transfer to fjall's
LSM-tree/journal design; the agent was told to investigate fjall's actual
durability model before rewriting those claims.

**Current literal state**: `crash sessions locks storage-readme-backup-rewrite`
shows the session `alive` (heartbeat fresh as of the last check).
`git status --short` shows `crates/shamir-storage/src/README.md` as
modified (uncommitted, mid-write by the agent) — this is expected, NOT yet
verified or committed. When this session's crush run completes, the
zero-trust verification protocol (read full diff, re-run
`./scripts/test.sh -p shamir-storage -p shamir-server --full` TWICE,
`cargo fmt -p shamir-server -- --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, remove stray log files) must be run before
committing, exactly as done for every prior task this session.

## Active goal

None. No `/goal` Stop hook is armed this session — the TaskList is the sole
source of truth for what's in flight. A babysit cron (`459f9e60` or a
successor — re-check via `CronList`, IDs may have rotated) has been
ticking every 15 minutes throughout, correctly reporting "still running
#715" on each tick while this crush session was active.

## TaskList

### in_progress
- #715 Rewrite shamir-storage/src/README.md and shamir-server/src/backup.rs's stale redb content (blockedBy: none)

### pending
(none)

### recently completed
- #729 Fix silent lost-update: execute_update_tx must merge over already-staged tx bytes, not stale pre-scan bytes
- #728 8f. Misc low-risk perf tail: F6/F9/F10/F11
- #727 8e. F5: ForEach direct QueryResult->QueryValue conversion
- #726 8d. F4: SQ8 Cosine query-norm hoist + SIMD kernels
- #725 8c. F3+F7: score HNSW candidates inside the read closure
- #724 8b. F2: hoist $query/$param operand resolution
- #723 8a. F1: compiled value-IR for FieldRef
- #695 Investigate row-overlap lost-update in UPDATE-cascade pipeline

(Full earlier history — Этапы 1-7, ~50 tasks — all completed and deleted
from the live TaskList per the user's explicit request this session to
clean up finished tasks before re-planning; see git log for the full
commit trail if needed.)

## Decisions

- **User explicitly chose "full fix now"** for the #695 lost-update bug
  over a narrower plan-time-rejection stopgap or deferring to a separate
  task — this was a real, consequential decision point (the bug required
  touching core tx-staging/index-delta-planning code), confirmed via
  `AskUserQuestion`.
- **8d's optional query-norm hoist (VR-7 option 2) was attempted and
  shipped**, not deferred — the delegated agent built a genuinely safe
  mechanism (thread-local stack, pointer-keyed, soft-miss-on-mismatch by
  construction so correctness never depends on `hnsw_rs`'s undocumented
  argument-order convention) and proved it with targeted concurrency tests;
  I verified this reasoning independently before accepting it.
- **8e's Strategy A (custom Serializer) was chosen over Strategy B
  (fast-path-with-fallback)** by the delegated agent, and the differential
  test proved this was the RIGHT call: a naive fast-path clone would have
  wrongly preserved `Dec`/`Big`/`Set` as-is instead of coercing to
  `Str`/`Str`/`List` the way the real msgpack round-trip does.
- **8f: F9 (Cow-based sort keys) was correctly declined**, not attempted —
  the agent found a genuine self-referential-struct wall in the top-K
  heap's `HeapItem` (which holds both a would-be-borrowing key and the
  value it would borrow from) and stopped per the brief's own explicit
  "revert rather than force `unsafe`" instruction.
- **#715's brief explicitly forbids a word-swap fix for backup.rs** — the
  redb-specific technical claims (CRC32/atomic-commit) needed genuine
  re-investigation against fjall's actual durability model, not a rename.

## Open questions

None outstanding from the user. The only open item is mechanical: #715's
crush session needs to finish, then go through this session's standard
zero-trust verification before commit — no user input is blocking this.

## Repo state

```
 M crates/shamir-storage/src/README.md
?? docs/checkpoints/2026-07-17-1600.md
?? docs/checkpoints/2026-07-19-1015.md
?? docs/checkpoints/2026-07-20-0230.md
?? docs/checkpoints/2026-07-20-0245.md
?? docs/checkpoints/2026-07-20-1615.md
```

```
e2f71a4b docs(prompts): brief for #715 -- storage README + backup.rs stale-backend rewrite
7801009a fix(engine): execute_update_tx merges over already-staged tx bytes, not stale scan
5ecf54f0 docs(prompts): brief for execute_update_tx staged-merge fix (silent lost-update)
8fa78cd1 perf(engine): CompactPath as InternerKey + InSet/In parity (F6+F10)
4d092159 docs(prompts): brief for 8f -- F6/F9/F10/F11 misc low-risk perf tail
```

## Active timers

A babysit cron has been running throughout this window (visible via
repeated "# babysit tick" prompts), correctly holding at "still running
#715" while the crush session for #715 is active. Run `CronList` at the
start of the next session to confirm its current job id and that it is
still armed (crons auto-expire after 7 days regardless).
