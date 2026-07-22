# Checkpoint — 2026-07-20 [full-campaign-recap]

## Session summary

This is a continuation of a very long `/babygoal`-driven session working
through `docs/dev-artifacts/research/2026-07-17-release-audit/
00-WORK-PLAN.md`, standing directive: "реализуй задачи с помощью /crush,
между задачами делай коммиты, покрой их тестами." This checkpoint captures
BOTH the full history of earlier waves (Этапы 0-7, completed before this
visible window, per the user's explicit request to also document them) AND
the current window's work (TaskList cleanup, Этап 8, #695/#729, #715).

**Этап 0 — ACL bypass fix** (done before this campaign's visible window):
recursive `collect_required_access`, both entry points + WASM gateway, 16
tests, commit `6d33fe9e`.

**Этап 1 — Correctness blockers** (5 tasks, data-corruption class, from
report 04 + 05#1): FK on-update dedup bug (deduped the wrong thing,
`fk_on_update.rs`); `$contains_all` counted duplicate hits as distinct
(`filter_node.rs`+`compile.rs`); diamond FK cascade falsely rejected as a
cycle (per-path visited tracking needed, `fk_actions.rs`); Dec/Big-aware
comparison layer added across `compare_values`/aggregates/ORDER BY/`$expr`
coercion (one coherent task); UPSERT MERGE was overwriting `created_at`
(`write_exec.rs`).

**Этап 2 — Concurrency deadlock hazards** (5 tasks H1-H6, extends #589/
#671, from report 06): `active_snapshots`/`mvcc_locks` `entry_async→
entry_sync` (H1+H2); the SAME hazard class in 5 vector-index maps —
`deleted`/`vectors_u8`/`vectors`/`rid_to_internal`/`compaction_deleted_rids`
(H3, this is the fix whose `DEADLOCK FIX (#589 class)` comments were
repeatedly cited and preserved throughout this session's later Этап 8 work
on the same files); `per_table_mvcc`/`token_names` `read_async→read_sync`
(H4+H5); `layered_interner` `touch→touch_sync` hygiene + stale-comment
cleanup (H6, low).

**Этап 3 — Honesty fixes** (5 tasks, silent-gap class, from report 05 +
tail of 04): DDL-time rejection of nested-path `default`/`auto_now`/
computed-default (symmetric with existing `unique` rejection); reject
`Call` inside a transactional/interactive batch; warn-logs for fail-open
computed default + Null Call params; tail of report 04's MED/LOW findings
— coercing set-probes, threading `ScalarResolver` through SELECT/when/
bind/over, `count_distinct`/`mode` over Set/Map, self-referential FK,
FK Int↔F64 coercion, checked `$expr mod`/i64/Big compare+cast. (This tail
work is where task #695's row-overlap concern was FIRST noticed, during
self-referential FK cleanup verification, then deferred as a standalone
item rather than blocking the sequential chain.)

**Этап 4 — v0.10 funclib top-up** (6 tasks P0, from report 10 Part A §3):
null-functions `coalesce`/`if_null`/`nullif`/`is_null` (new `null/` folder);
wired `percentile`/`string_agg` args + honor-or-reject `distinct`;
`datetime/format(ts, pattern)` + `parse(s, pattern)` via chrono; `uuid_v4()`
(+ optional `random`/`random_bytes`) as `pure:false`; fixed `arrays/sort`'s
cross-type `compare()` + `sort_desc`; `parse_json`/`to_json`.

**Этап 5 — Compliance & Ops** (6 tasks, from report 03): audit-coverage doc
fix (AuditSink bridge flagged P1/deferred, docs corrected meanwhile); fixed
`07-operations.md`'s backup claim (stop-and-copy, not live); expanded
`data-protection.md` §2 erasure-remnants coverage (index snapshots,
replicas, FTS postings); fixed the dead `audit.retention_days` config knob;
added an SBOM artifact to the supply-chain CI workflow; moved
`CreateScramUser.password` + `VectorBackendRef::External.api_key_secret` to
`SecretString` (this required relocating `SecretString` from
`shamir-query-types` down to `shamir-types` behind a new `crypto` feature,
since `shamir-index` needs it but doesn't depend on `shamir-query-types`).

**Этап 6 — Documentation accuracy** (7 tasks, from report 09): rebalanced
`08-interconnect.md`'s stale "nothing works" framing (replication/
changefeed/subscriptions are actually implemented+tested); replaced raw
`cargo test` examples with `./scripts/test.sh`/`cargo t`/`cargo tl` in
README/CONTRIBUTING/CLAUDE.md (CLAUDE.md's OWN pre-commit gate section had
contradicted its own later ban); fixed stale `redb→fjall` references in
`03-storage.md` and doc comments (narrowly scoped — most grep hits were
legitimate historical/comparison prose, correctly left alone — this is
also where `shamir-storage/src/README.md`'s deeper drift was FIRST spotted
and deliberately deferred to become task #715); implemented the
previously-phantom `allow_public_metrics` config knob for real (new
`ObservabilityConfig` field, threaded through `server_launcher.rs`, 2 new
boot-path tests); fixed CLAUDE.md's incorrect "`shamir_bench_utils` is
gone" claim (it was narrowed, not removed); fixed ~10 phantom function
names in funclib docs (`05-functions.md`); misc — TS `skipped` field,
stale example port (13760→7331), 3 stale "not wired" Rust doc comments
(WalGroupCommit/RecordView/VersionedOverlay are all genuinely wired in
now), removed 2 now-provably-dead `#[allow(dead_code)]` attributes.

**Этап 7 — Test/CI robustness** (7 tasks, from report 08): pinned
`cargo-nextest@0.9.137` in CI (was unpinned despite `.config/nextest.toml`'s
own documented pinned baseline — a real guard-coupling risk); added
`shell: bash` to the Windows `test` job (parity with `integration`, which
already had it — Windows runners default `run:` steps to PowerShell
otherwise); investigated `[[profile.ci.overrides]]` for SCRAM/WASM tests —
the brief's own hypothesized gap turned out FALSE on investigation
(nextest's override precedence falls through per-setting from the selected
profile to `default`, proven via official docs + an empirical
`show-config` probe), so the fix shipped as documentation only, correctly
overriding the original ask rather than forcing an unneeded config change;
added 7 new tests covering write-value marker combination gaps (top-level
`$expr`, `$expr`/`$cond`+`$ref` pinning, `SetOp.key` marker, deep nesting)
— the exact structural gap class report 08 says produced a shipped
`$fn`+`$ref` bug; added a new nightly/scheduled stress lane
(`stress-nightly.yml` + a new `[profile.stress]` nextest profile, inverse
of `ci`'s low-parallelism tuning) targeting the Version Oracle area, since
no environment today recreates the contention that once caught a real
MvccStore deadlock; wrapped ONE genuinely-unbounded spin-wait
(`overlay_ordering_tests.rs`'s reader loops) in `tokio::time::timeout`,
after investigating FOUR other report-08-cited sites and correctly leaving
them as-is (already adequately bounded by a different mechanism — explicit
judgment-over-mechanical-application, not laziness); pinned
`cargo-cooldown` + swept for and fixed 4 MORE unpinned tool installs across
`numa.yml`/`supply-chain.yml` that earlier tasks had missed.

Every single task across ALL these Этапы (0-7, roughly 50 leaf tasks) and
every task in the current window followed the SAME disciplined loop without
exception: investigate the code myself first (never trust the work-plan's
or report's citations blindly — several turned out to be imprecise, stale,
or need correction — e.g. 7c's hypothesis disproven, this window's F4 brief
correcting the report's stale `read_async` citations to the ALREADY-fixed
`read_sync`), write a precise brief to `docs/dev-artifacts/prompts/<area>/
NN-*.md`, commit the brief BEFORE delegating, delegate to `/crush`,
independently read the full diff and re-run tests/fmt/clippy myself before
ever committing, and mark the task complete only after that verification
passed.

**Current window (this visible session, continuing from checkpoint
`2026-07-20-1615.md`)**: TaskList was cleaned of all ~50 completed Этап 0-7
tasks (explicit user request: "давай всё по порядку, сначала удалим
выполненные задачи. Потом пересмотрим план. Составим общий план из
оставшихся задач"). Этап 8 (Performance, explicitly "post-blocker, не гейт
релиза" per the work plan) was decomposed into 6 leaf tasks (8a-8f) and
executed in full — see the "Этап 8" section of the PRIOR checkpoint
(`2026-07-20-storage-readme.md`, written earlier this same window) for the
complete per-task breakdown; summarized here: 8a (F1, `9ba703e8`) FieldRef
caching, 8b (F2, `efe7c3b3`) lazy QueryRef caching, 8c (F3+F7, `22d30e7c`)
HNSW score-in-closure, 8d (F4, `82cfb962`) SQ8 SIMD kernels + query-norm
hoist (highest-risk task, attempted AND shipped the optional item after
proving safety against `hnsw_rs` source), 8e (F5, `e4305c33`) ForEach direct
serialization via a custom `serde::Serializer`, 8f (F6+F10, `8fa78cd1`)
CompactPath/InSet fixes (F9/F11/F6-optional correctly declined with
reasoning). Then #695 (investigation, completed) found a REAL silent
data-corruption bug — any two ops touching the same row in one batch/tx via
`execute_update_tx` could silently lose one mutation (not just an FK-cascade
edge case as originally suspected). User explicitly chose "full fix now."
New task #729 was created and completed (commit `7801009a`): made
`execute_update_tx` staging-aware via `StagingStore::staged_op`, with 2 new
regression tests each PROVEN by the implementing agent to fail without the
fix and pass with it. Finally task #715 (doc rewrite) is IN FLIGHT right
now: `shamir-storage/src/README.md` was found to describe 6 backends of
which 5 don't exist in the codebase at all (only Fjall is real — confirmed
via `lib.rs`/`Cargo.toml` feature list), and `shamir-server/src/backup.rs`
has 4 stale redb doc-comment references that need re-investigation (not a
word-swap) against fjall's actual LSM-tree durability model. A user aside
mid-task ("redb мы ведь удалили?") was answered directly (yes, fully
removed in code; only docs were stale). As of this checkpoint, BOTH target
files show as modified/uncommitted (`crates/shamir-storage/src/README.md`
and `crates/shamir-server/src/backup.rs`) — the crush session
`storage-readme-backup-rewrite` is still `alive` per its last heartbeat
check; this has NOT yet been through the zero-trust verification pass
(diff read + `./scripts/test.sh -p shamir-storage -p shamir-server --full`
x2 + fmt + clippy) or committed.

## Active goal

None. No `/goal` Stop hook is armed this session — the TaskList is the sole
source of truth for what's in flight. A babysit cron has been ticking every
15 minutes throughout (re-check current job id via `CronList` — IDs may
have rotated across this long session; crons auto-expire after 7 days
regardless), correctly holding at "still running #715" on every tick while
this crush session has been active.

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

(Этапы 0-7, ~50 further tasks, all completed and deleted from the live
TaskList earlier this window per explicit user request — see the Session
summary above for their full content, or `git log` for the exhaustive
commit trail.)

## Decisions

- **User explicitly chose "full fix now"** for the #695 lost-update bug
  over a narrower plan-time-rejection stopgap or deferring — confirmed via
  `AskUserQuestion`, a real consequential decision point since the bug
  required touching core tx-staging/index-delta-planning code.
- **8d's optional query-norm hoist (VR-7 option 2) was attempted and
  shipped**, not deferred — proven safe via a thread-local pointer-keyed
  stack whose correctness does NOT depend on `hnsw_rs`'s undocumented
  eval-argument-order convention (a wrong assumption there degrades to a
  cache miss, never a wrong answer) — I verified this reasoning myself
  before accepting it.
- **8e's Strategy A (custom `serde::Serializer`) was chosen over Strategy
  B (fast-path-with-fallback)** — proven the right call by the differential
  test, which caught that a naive fast-path clone would have wrongly kept
  `Dec`/`Big`/`Set` as-is instead of the real wire format's `Str`/`Str`/
  `List` coercion.
- **8f's F9 (Cow-based ORDER BY sort keys) was correctly declined** — a
  genuine self-referential-struct wall in the top-K heap's `HeapItem`, not
  a shortfall; the brief explicitly permitted this exact outcome.
- **#715's brief explicitly forbids a word-swap fix for `backup.rs`** —
  redb's page-based CRC32/atomic-commit claims needed genuine
  re-investigation against fjall's actual (LSM-tree/journal-based)
  durability model before rewriting.
- **TaskList was deliberately purged of ~50 completed Этап 0-7 tasks**
  early this window, per explicit user instruction, before re-planning
  Этап 8 — the full historical content is preserved in this checkpoint's
  Session summary rather than in live TaskList entries.

## Open questions

None outstanding from the user. The only open item is mechanical: #715's
crush session needs to finish, then go through this session's standard
zero-trust verification (diff read, 2x green test run, fmt, clippy, stray
log cleanup) before commit — no user input is blocking this.

## Repo state

```
 M crates/shamir-server/src/backup.rs
 M crates/shamir-storage/src/README.md
?? docs/checkpoints/2026-07-17-1600.md
?? docs/checkpoints/2026-07-19-1015.md
?? docs/checkpoints/2026-07-20-0230.md
?? docs/checkpoints/2026-07-20-0245.md
?? docs/checkpoints/2026-07-20-1615.md
?? docs/checkpoints/2026-07-20-storage-readme.md
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
still armed.
