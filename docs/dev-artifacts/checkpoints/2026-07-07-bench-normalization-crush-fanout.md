# Checkpoint — 2026-07-07 [bench-normalization-crush-fanout]

## Session summary

This session continued a long bench-tooling effort (see the earlier checkpoint
`2026-07-07-bench-scale-tool-migration.md` for the morning's work: migrating
all 47 workspace bench targets from Criterion to `bench-scale-tool`, a
fixed-iteration harness in a sibling repo `D:/dev/rust/bench-scale-tool`).

Today's session, after that migration was committed, focused on making the
whole bench suite actually *fast* and *correct*. We chased a real problem the
user kept flagging as "тормоза" (slowness/lag) through several layers:

1. **Harness-level fixes** (in `bench-scale-tool`, its own git repo, already
   committed there — commits `1d39a88` and `77c5493`): replaced blind
   batch-of-64 calibration with proportional-jump calibration (measure once,
   jump straight to an estimate, refine — converges in ~1-2 rounds instead of
   ~log2(N) doubling rounds); made `run_harness`'s fixed-run mode
   self-healing (a workload missing from `bench-iters.txt` is JIT-calibrated
   on the spot instead of aborting the whole binary with "workload(s) missing
   from manifest"); built `bench-cli` (a real Rust binary, not a shell
   script) with subcommands `list/calibrate/calibrate-all/run/sweep/history/
   history-diff/calibrate-to-budget`, all continue-on-error + resume-friendly
   (skips already-calibrated targets, `--force` to override) + streaming
   stdout (was buffered, made multi-workload targets look "frozen" for
   minutes) + one-cargo-invocation `build_all_benches` instead of a `cargo
   bench` per target.

2. **shamir-db integration** (this repo, commits `8483bec4` through
   `2a5bfa6b`, already pushed... actually NOT pushed, just committed):
   removed the criterion dev-dependency everywhere; added a `cargo
   bench-tool <args>` alias in `.cargo/config.toml` that runs `bench-cli` via
   `cargo run --release --bin bench-cli --manifest-path <bench-scale-tool>`
   without needing `cargo install` — required extending this repo's
   perimeter test-guard runner (the one that blocks bare `cargo test`) with a
   narrow exception: pass through when the executable's basename is exactly
   `bench-cli`(.exe); fixed three REAL bugs found while calibrating the
   migrated benches (not tooling bugs — bugs in the migrated bench files
   themselves): `wire_latencies.rs` and `subscription_fanout.rs` each built
   their live server / bridge tasks on a disposable per-call
   `new_current_thread()` tokio runtime that gets dropped (and aborts every
   task spawned on it, including the live server accept-loop / bridge tasks)
   the instant the one setup call returns — fixed by using a persistent
   `new_multi_thread()` runtime (`server_rt`) held alive until after
   `h.run()`; `framing.rs`'s `write_only/write_frame/<size>` destructured a
   `tokio::io::duplex` pair as `(mut w, _r)` and never touched `_r` inside the
   async block, so it wasn't captured by the future and dropped immediately,
   closing the duplex pipe and causing `BrokenPipe` on write — fixed by
   binding the receiver to a live local and touching it via `black_box`.

3. **A very long "why is calibrate-to-budget slow" investigation** that
   turned into a proper root-cause fix rather than a workaround: measured
   that `calibrate-to-budget`'s confirm-sweep took 619s against a 60s target
   before any of today's Criterion-era-dataset-size fixes; traced this to
   dozens of individual bench workloads whose SINGLE call (at N=1, the
   calibrated floor) still cost tens to thousands of milliseconds because
   they still carried Criterion-era dataset sizes (10k-100k+ row tables,
   50k-record seeds, 20k-touch loops, etc.) — sizes that made sense under
   Criterion's own adaptive sampling but are wrong under a model where the
   harness owns repetition count externally and expects each call to be a
   cheap ~10ms unit.

4. **The core insight that reshaped the rest of the session** (explicitly
   stated by the user, and it is important to preserve verbatim): not every
   multi-N "scaled" bench workload is the same kind of problem. Two cases:
   - **(a)** N is literally an inner sequential-repeat loop
     (`for _ in 0..N { single_op().await }`) inside ONE timed call — the
     harness's own external repetition already provides this signal, so an
     inner loop duplicating it is pure waste. These should be COLLAPSED to
     the smallest N (or N=1), full stop.
   - **(b)** N is a genuine STRUCTURAL parameter — a real single bulk
     operation moving N records in one call (e.g. `Batch::insert` with N
     rows in ONE `execute()`), or a real scaling-curve demonstration (does
     GC cost grow O(depth) or O(depth²)? does fan-out delivery scale
     linearly with subscriber count? does a matrix bench compare
     backend×concurrency×batch on purpose?). These must NOT be silently
     deleted — that would delete the actual signal the bench exists to
     produce. The resolution pattern settled on: keep the SMALLEST tier as
     the default (fast sweep stays fast), and gate the full ladder behind an
     opt-in env var, e.g.:
     ```rust
     let wide = std::env::var("BENCH_<NAME>_SCALING")
         .map(|v| matches!(v.as_str(), "1"|"true"|"yes"|"on"))
         .unwrap_or(false);
     let sizes: &[usize] = if wide { &[/* full ladder */] } else { &[/* smallest */] };
     ```
   This distinction was discovered midway through reviewing sub-agent work
   (task #465/#466) and retrofitted into every subsequent task's brief.

5. **Execution model**: the user explicitly invoked `/babygoal` asking for
   "/crush agents" to do the mechanical per-crate normalization work, capped
   at **at most 2 concurrent crush sessions** (an explicit user constraint,
   stated mid-session after 4 were briefly running at once — do not exceed 2
   again). A `/babysit` cron job (id `a32d171e`, every 15 min, session-only)
   is armed and ticking to resume this if the session dies. TaskList
   decomposition is per-crate (one task per crate's `benches/` directory, or
   per logical group for the large `shamir-engine` crate which was split
   into 3 groups A/B/C). Each task's own description embeds the exact
   /crush brief used, with the case (a)/(b) refinement injected into every
   task whose prompt hadn't been launched yet at the time the refinement was
   discovered.

6. **Zero-trust verification discipline actually followed**: every
   completed crush agent's diff was read in full by the orchestrator (not
   just the agent's own claimed summary), independently re-verified with
   `cargo check`/`cargo fmt -- --check`/`cargo clippy -- -D warnings` scoped
   to the touched crate, and in two cases (task #465's `engine_perf.rs`
   `bulk_insert*` workloads, task #466's `filter_eval.rs` +
   `fts_indexed.rs`, task #467's `backend_matrix.rs`) the orchestrator
   personally corrected the agent's over-aggressive deletion back to the
   opt-in-flag pattern after finding it violated the case (b) rule found
   later.

## What's currently in flight (as of this checkpoint)

Two crush sessions running in the background right now, ~2 min old, both
progressing normally per their own tool-call logs (not yet verified by the
orchestrator — that happens when their completion notification arrives):

- **Task #471** (`bench-471` crush session) — `crates/shamir-server/benches/`
  (7 files: db_handler_rps, duplex_throughput, subscription_throughput,
  wire_latencies, subscription_delivery, subscription_fanout,
  wire_pipelining). Extra-hazardous scope: `wire_latencies.rs` and
  `subscription_fanout.rs` carry the persistent-runtime fix from item 2
  above — the brief explicitly forbids touching that plumbing, and mandates
  a live `cargo bench -- --calibrate 0.05` re-run of both files after any
  edit to confirm no `ConnectionRefused`/`fanout loss` regression. Last seen
  log output: applying the opt-in-flag pattern to
  `subscription_delivery.rs`, `subscription_fanout.rs`, `wire_pipelining.rs`
  (N=[1,8,32,128] concurrency ladder), `duplex_throughput.rs` (N=[10,32]
  batch ladder); about to run the mandatory verification commands.
- **Task #473** (`bench-473` crush session) — `crates/shamir-tx/benches/`
  (tx_overhead.rs, overlay_gc_cost_vs_depth.rs — the latter's name literally
  says "cost vs depth", flagged in its own brief as almost certainly case
  (b), needing the opt-in pattern not deletion). Just started, reading the
  two files.

## Active goal

None (`/goal` Stop hook not used this session — progress is tracked purely
via the TaskList + `/babysit` cron per the `/babygoal` skill's own design,
which explicitly does not synthesize a `/goal` line unless the user types it
themselves).

## TaskList

### in_progress
- #471 Нормализовать бенчи shamir-server под ≤10мс/итерацию
- #473 Нормализовать бенчи shamir-tx под ≤10мс/итерацию

### pending
- #441 CRIT-7: пустой ответ FTS/index2 → полный скан; VectorSimilarity на miss отдаёт ВСЕ строки (~400×)
- #442 CRIT-8: TS-клиент .limits() всегда роняет запрос
- #443 HIGH-durability: WAL/recovery/MemBuffer кластер
- #444 HIGH-security: TLS/WS accept timeout+per-IP cap, Argon2 gating, ticket binding, subscription fanout, WASM fuel/SSRF
- #445 HIGH-perf: keyset O(N²), unbounded ts_index/cells, MemBuffer drain amplification, UPDATE de-interning, SQ8 fusion
- #446 HIGH-client: error-code collapse, zero timeouts, query_version staleness, wire-type drift, e2e parity gaps
- #447 HIGH-concurrency-isolation: MVCC/SSI кластер
- #448 CLEANUP: устаревшие/лживые doc-комментарии + мёртвый код
- #449 COMPLIANCE-1: cargo-deny/cargo-audit CI-гейт + SECURITY.md + captrack path-pin
- #450 COMPLIANCE-2: plaintext username в auth-логах + wasmtime advisory-политика
- #451 COMPLIANCE-3: docs/guide-docs/security/data-protection.md
- #452 PERF-RADICAL-1: fjall zero-copy Bytes
- #453 PERF-RADICAL-2: CachedStore read-after-write
- #454 PERF-RADICAL-3: posting-list Arc + sorted-slice
- #455 PERF-RADICAL-4: funclib distinct() O(N²) + WAL segment-open replay + interner reverse-vec clone
- #456 PERF-RADICAL-5: CREATE INDEX полная материализация + fjall spawn_blocking-per-op + TCP framing memcpy
- #457 PERF-RADICAL-STRUCTURAL: RecordKey=Bytes → инлайн Key128(u128)
- #458 FLAKE: vr5_cofilter_sees_staged_and_filters_residual intermittent failure
- #459 Migrate remaining 41 criterion benches (superseded in spirit — all 47 targets ARE migrated; this task was never explicitly closed)
- #463 Финальный прогон calibrate-to-budget 60 через новый алиас (BLOCKED on #474/#475 finishing — this is the "prove it actually works end to end" step)
- #474 Нормализовать бенчи shamir-types под ≤10мс/итерацию (not yet launched)
- #475 Нормализовать бенчи shamir-wal (wal_append.rs) под ≤10мс/итерацию (not yet launched)

### recently completed
- #472 Нормализовать бенчи shamir-storage (store_raw.rs) — verified clean, opt-in flags for scan/prefix_scan/set_many/get_many/remove_many
- #470 Нормализовать бенчи shamir-query-types (batch_planner.rs) — verified clean, opt-in flag for chain/independent sizes
- #469 Нормализовать бенчи shamir-index (sq8_hot_path.rs) — verified: file already fine, zero changes needed
- #468 Нормализовать shamir-engine группа C — verified clean, orchestrator confirmed all 3 opt-in-flag applications correct
- #467 Нормализовать shamir-engine группа B — verified; orchestrator personally fixed backend_matrix.rs (agent had deleted the whole matrix axis, restored via opt-in flag)
- #466 Нормализовать shamir-engine группа A — verified; orchestrator personally fixed filter_eval.rs + fts_indexed.rs (agent deleted O(N²)→O(N) regression-check tiers, restored via opt-in flag)
- #465 Нормализовать shamir-db — verified; orchestrator personally fixed engine_perf.rs bulk_insert* (agent collapsed a genuine bulk-batch-size comparison to one tier), plus restored filter_eval-style tiers in changelog_read.rs and record_size_axis.rs
- #464 Нормализовать shamir-connect (hot_paths.rs) — verified clean, no corrections needed
- #462 Cargo alias для bench-cli — committed (`438257f1`)
- #461 calibrate_all_at resume + --force — committed (bench-scale-tool `1d39a88`)
- #460 calibrate_all_at continue-on-error — committed (bench-scale-tool `1d39a88`)

## Decisions

- **Chose** the case (a)/(b) distinction (inner-loop-artifact vs genuine
  structural axis) over the original blanket instruction ("delete all
  larger-scale variants, keep only smallest") once the user pointed out
  concrete counter-examples (`bulk_insert/100 vs /1000`, `overlay_gc_cost_vs_depth`).
  Rejected: blind collapse-to-smallest for every multi-N bench, which would
  have silently deleted several genuine before/after regression-check
  benches (the O(N²)→O(N) memoisation-fix demonstrations in
  `filter_eval.rs`/`fts_indexed.rs`) and an entire concurrency×batch×backend
  comparison matrix (`backend_matrix.rs`).
- **Chose** an opt-in env-var-gated ladder (`BENCH_<NAME>_SCALING=1`) as the
  uniform resolution pattern for case (b), over either (i) leaving the full
  ladder in the default sweep (keeps sweep slow) or (ii) deleting the larger
  tiers outright (loses the signal permanently). This preserves both "fast
  default sweep" and "the scaling signal is one env var away, not gone."
- **Chose** to cap crush-agent concurrency at 2 (explicit user instruction
  after 4 ran briefly at once) — rejected running all remaining tasks in
  one big parallel batch, which is what the `/babygoal` skill's own
  "parallel sub-agents" strategy would otherwise imply for genuinely
  independent per-crate work.
- **Chose** to build `bench-cli` as a real Rust binary with a `cargo
  bench-tool` alias (requiring a narrow perimeter-guard exception) over a
  `.sh` wrapper script, specifically for Windows cmd.exe/PowerShell
  compatibility (a `.sh` script needs Git Bash).
- **Rejected** (for now, explicitly deferred) chasing down whether Windows
  Defender real-time scanning contributes measurable per-binary first-exec
  overhead — the user confirmed `D:\dev` is already in the Defender
  exclusion list, closing that line of investigation.

## Open questions

- None outstanding that require user input right now — the two in-flight
  crush tasks (#471, #473) will report back, get zero-trust-verified, and
  the next pending task (#474 or #475) will be launched to keep exactly 2
  concurrent, continuing until all of #464-#475 are done.
- Once #474/#475 are done, task #463 ("Финальный прогон calibrate-to-budget
  60") is the natural next step — it was explicitly deferred by the user
  earlier ("Нужно прекратить все запуски, пока мы не нормализуем внутренние
  итерации") until this whole normalization pass is complete. That
  condition is now almost met.
- `bench-iters.txt` currently holds a mix of stale/valid entries from
  earlier partial runs this session — it should probably be treated as
  fully stale and regenerated via `--force` once #463 actually runs, rather
  than trusted as-is.
- Nothing has been pushed to any remote this session (multiple commits
  exist locally in both `shamir-db` and the sibling `bench-scale-tool` repo)
  — no push was requested and none should happen without an explicit ask.

## Repo state

```
 M bench-iters.txt
 M crates/shamir-connect/benches/hot_paths.rs
 M crates/shamir-db/benches/changelog_read.rs
 M crates/shamir-db/benches/durability_axis.rs
 M crates/shamir-db/benches/engine_perf.rs
 M crates/shamir-db/benches/record_size_axis.rs
 M crates/shamir-engine/benches/backend_matrix.rs
 M crates/shamir-engine/benches/distinct.rs
 M crates/shamir-engine/benches/drain_cost_vs_depth.rs
 M crates/shamir-engine/benches/drain_throughput.rs
 M crates/shamir-engine/benches/durable_concurrent_commit.rs
 M crates/shamir-engine/benches/filter_eval.rs
 M crates/shamir-engine/benches/filtered_vector_search.rs
 M crates/shamir-engine/benches/fts_indexed.rs
 M crates/shamir-engine/benches/order_by_pipeline.rs
 M crates/shamir-engine/benches/select_projection.rs
 M crates/shamir-engine/benches/tx_concurrent.rs
 M crates/shamir-engine/benches/tx_pipeline.rs
 M crates/shamir-engine/benches/vector_bulk_compaction.rs
 M crates/shamir-engine/benches/vector_search.rs
 M crates/shamir-engine/benches/wasm_invoke.rs
 M crates/shamir-query-types/benches/batch_planner.rs
 M crates/shamir-server/benches/duplex_throughput.rs
 M crates/shamir-server/benches/subscription_delivery.rs
 M crates/shamir-server/benches/subscription_fanout.rs
 M crates/shamir-server/benches/subscription_throughput.rs
 M crates/shamir-server/benches/wire_pipelining.rs
 M crates/shamir-storage/benches/store_raw.rs
 M crates/shamir-tx/benches/overlay_gc_cost_vs_depth.rs
?? wire_lat_calib.log
```

(`crates/shamir-server/benches/wire_latencies.rs` is presumably about to
show as modified too, mid-edit by the #471 crush agent as this checkpoint
is written — not yet reflected in the status snapshot above.)

```
2a5bfa6b chore: remove obsolete bench-quick-all.sh, add migration checkpoint
4077518a perf(benches): normalize per-call cost — harness owns iteration count now
4b3a836f fix(benches): fix three real bugs found calibrating the migrated benches
438257f1 feat(cargo): bench-tool alias — run bench-cli without cargo install
8257a34c docs(claude): ban sleep/tasklist polling on backgrounded commands
```

None of today's normalization work (tasks #464-#472, in-flight #471/#473)
has been committed yet — all still uncommitted in the working tree, pending
completion of the remaining tasks (#474, #475) and the final
calibrate-to-budget proof run (#463).
