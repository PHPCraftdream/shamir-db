# Brief — P1-4: long CREATE INDEX blocks all writes for the whole scan (alpha-minimum bar)

Task: #969 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P1-4 (adjacent §7.1-7.2). Read this brief in full; the review itself explicitly splits this into an "alpha-minimum" bar (this task) and a "post-alpha" bar (a full online-build redesign, explicitly OUT of scope here).

## What's already true — verified, do not re-derive

`TableManager::create_index` holds F-70's write barrier
(`begin_write_barrier(REGULAR_INDEX_CREATE)` → raise bit → drain → hold
`unique_write_lock`) across the ENTIRE Phase 1→2→3 backfill sequence —
already honestly documented in
`crates/shamir-index/src/base_index/index_manager.rs`'s
`create_index_from_stream` doc (~line 1044-1062): "the barrier holds
writers for the whole build in BOTH the old and new shapes... Reducing
writer-blocked time would require releasing the barrier between batches,
which is explicitly out of scope." F-78 (#905) already reduced PEAK MEMORY
for the regular family (streaming instead of materializing the whole
table) but explicitly did NOT reduce writer-blocked TIME (same doc,
confirmed) — and F-78 was **deferred for the unique family**
(`crates/shamir-engine/src/table/table_manager_index_mgmt.rs` ~line
675-679): unique still materializes the whole table (`Vec`) before one
`set_many`, so it ALSO carries O(table) peak memory, not just write-stall
time.

**A near-complete writer-latency benchmark for exactly this scenario
ALREADY EXISTS**: `crates/shamir-engine/benches/f78_writer_latency.rs`
measures concurrent-writer p50/p95/p99 during a real `create_index` call,
using `bench_scale_tool::Harness`. It currently only runs at `N_ROWS =
5_000`. **Do not write a new benchmark from scratch** — extend this one.

## The alpha-minimum checklist (do all 5; the "post-alpha online build"
redesign named in the review is explicitly OUT OF SCOPE — do not attempt
it)

### 1. Documented operational warning

Add a new entry to `docs/guide-docs/KNOWN_LIMITATIONS.md` §3 "Indexes"
(check the section's existing entries for tone/format first) stating:
- `CREATE INDEX` (regular/unique/sorted) holds a table-wide write lock for
  the ENTIRE backfill scan — every other writer queues for the full
  duration. On a medium-to-large table this is a write OUTAGE, not a brief
  pause.
- The unique family additionally materializes the whole table into memory
  before writing (O(table) peak memory) — F-78's streaming fix was NOT
  applied to unique.
- Recommend operators run `CREATE INDEX` on large tables during a
  maintenance window, and — for TS/JS client callers — explicitly raise or
  disable the client's default request timeout for this specific call
  (see item 3).
- Cross-reference `TableManager::verify()`/`doctor::repair()` (#966) for
  post-crash visibility, and note that a full lock-free "online build" is
  planned as a future improvement (do not design it here — just note it's
  tracked).

### 2. Progress visibility during backfill

For the regular family's streaming backfill
(`IndexManager::create_index_from_stream`, per-batch loop ~line
1071-1195): add a periodic progress log (e.g. `log::info!` every N
processed batches or every few seconds — your call on the right cadence,
avoid spamming on every single batch) reporting rows-processed-so-far, so
an operator watching logs can see the DDL is progressing, not hung. Check
whether the unique and sorted families' backfill loops have an analogous
per-batch/per-row point where a similar progress log makes sense given
their OWN structure (unique currently materializes-then-writes in one
shot per `create_unique_index_from_records` — if there's no natural
per-batch point, at minimum log a "starting backfill of N rows" line
before the scan and completion line after, so at least start/duration is
visible even without granular progress).

### 3. Client-side request-timeout consideration

Verified: there is NO server-side per-DDL-command timeout that could abort
a long `CREATE INDEX` mid-flight (checked `crates/shamir-server/src` — the
only `tokio::time::timeout` usages are TLS/WS handshake and shutdown
deadlines, unrelated). The relevant timeout is CLIENT-side:
`crates/shamir-client-ts/src/core/client.ts`'s `_requestTimeoutMs`
(`DEFAULT_REQUEST_TIMEOUT_MS`), which applies to EVERY request including
`create_index`. Add a JSDoc note on the `createIndex` builder
(`crates/shamir-client-ts/src/core/builders/ddl.ts`) and/or the
`execute`/`Batch.execute` call sites recommending callers pass a generous
`requestTimeoutMs` (or `0` to disable) for a `create_index` call against a
large table — this is a DOC-only change (JSDoc + the KNOWN_LIMITATIONS.md
entry from item 1), NOT a code/behavior change to the timeout mechanism
itself.

### 4. Health visibility

Already satisfied by #966 (`doctor::verify()` reports a `Building` index
as unhealthy with a diagnostic message) — no new code needed here. Just
cross-reference it in the KNOWN_LIMITATIONS.md entry from item 1.

### 5. Benchmark at realistic scale (100k / 1M rows)

Extend `crates/shamir-engine/benches/f78_writer_latency.rs` to ALSO run
the same `create_index_with_concurrent_writers` scenario at `100_000` and
`1_000_000` rows (in addition to the existing `5_000`), using the SAME
harness pattern (`h.bench_batched_async` with a distinct scenario name per
scale, e.g. `"create_index_with_concurrent_writers/100k_rows"`). Run it
(`CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p
shamir-engine --bench f78_writer_latency`) and update the bench file's own
"Measured results" doc comment (~line 39-49) with the NEW percentiles at
each scale — this is the concrete evidence backing the operational warning
in item 1 (cite the actual measured stall duration at 100k/1M rows in the
KNOWN_LIMITATIONS.md entry too, once you have real numbers).

## Gate (MANDATORY)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
```
Plus the bench run itself (item 5) — report its exact output/numbers.
If you touch `shamir-client-ts` (item 3's JSDoc), also run
`npx tsc --noEmit` in that package.

## Scope discipline

- Do NOT implement the "online build" redesign (persist Building →
  snapshot version → lock-free bulk scan → delta replay → short cutover →
  Ready) — that is explicitly the review's OWN "post-alpha" bar, a
  separate, much larger task.
- Do NOT implement the §7.2 unique-build-memory fix (partitioned hash
  files / external sort / durable keyspace with collision detection) —
  also explicitly post-alpha per the review.
- Do NOT change the write-barrier's actual locking behavior — this task
  is visibility/documentation/benchmarking only.
- Do NOT add a client-side or server-side timeout ENFORCEMENT mechanism —
  item 3 is documentation only.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

Confirm all 5 checklist items addressed (or explain why one doesn't
apply). Give the exact new benchmark numbers (build duration + writer
p50/p95/p99) at 100k and 1M rows. Give exact gate command output.
