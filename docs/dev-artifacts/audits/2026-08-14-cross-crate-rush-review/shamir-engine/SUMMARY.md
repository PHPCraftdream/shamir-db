# shamir-engine — Synthesized 7-lens review (consolidated follow-up to the 2026-08-14 cross-crate review)

Crate: `crates/shamir-engine/` — the query/commit/drain execution core: batch planning and
execution, MVCC commit pipeline + drainer, validators, indexes, replication apply, migration,
and the changelog/DDL bookkeeping behind `shamir-db`.

Review basis: the seven 2026-08-14 lens files under this directory — `correctness-tdd.md`,
`concurrency-lockfree.md`, `security-crypto.md`, `performance-hotpath.md`,
`api-wire-protocol.md`, `error-handling-lifecycle.md`, `style-claude-md.md` — synthesized
into one deduplicated document. Structure/tone/rigor calibrated on the two exemplar
syntheses: `shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`.
Workspace context (pre-dedup lens-tagged counts, not copied): `SUMMARY.md` row
`shamir-engine | 0 | 12 | 23 | 33 | 19 | 87`, verdict **high-risk**. A handful of
file:line references were re-verified against the source during synthesis (drainer Phase
A/B, `changelog_store.rs:37-55`, `db_instance.rs:61-68`, the three `.ok()` pre-read sites,
`group_commit/mod.rs`, `parser.rs`, `commit.rs:744-751`) — all confirmed; no new defects
were found during spot-checking. Read-only synthesis — no build/test/lint, no source
modifications.

## Executive summary

No critical findings and unusually strong structural discipline (lock inventory, TDD seams,
`THasher`/scc conformance, fail-closed culture), but the crate earns its **high-risk**
verdict on two themes: **silent data-loss/liveness defects in the drain-commit-replicate
spine** — the drainer finalizes entries as durable while never re-applying data ops for
tables without an `MvccStore` behind a factually false comment (1.2), three write-path
pre-reads flatten every I/O error into "record absent" and mis-plan index ops (6.1), and a
panicking group-commit leader strands all later durability flushes (1.4) — and **pervasive
hidden quadratic/unbounded cost on hot paths** (O(N²·K) pre-commit re-planning, O(M²)
staged-overlay probes, per-value FK table scans, a changelog reader that buffers the entire
journal tail). Fix first: the drainer data loss (1.2), the `.ok()` pre-reads (6.1), the
`DbInstance` DashMap guards held across `.await` (2.1 — the exact runtime-wedge class this
crate already fixed in `repo_instance.rs`), the changelog unbounded buffer (1.1), and the
exported legacy query parser that turns a `.limit(20)` read into a full-table read (5.1).

---

## 1. correctness-tdd

Lens verdict: the commit pipeline, drainer, and write paths are unusually well-defended
(issue-numbered regression tests, deterministic pause seams instead of sleeps, self-flagged
vacuous tests; no outright vacuous test exists in the current suite). The genuine defects
concentrate in edge-of-path error handling and contract seams.

### 1.1 [HIGH] `StoreChangelog::range_from` streams the ENTIRE journal tail before applying `limit`
- **File:line:** `crates/shamir-engine/src/repo/changelog_store.rs:37-55`
- **Issue:** the `while let Some(chunk) = stream.next().await { pairs.extend(chunk); }` loop
  consumes every key `>= from_key` into a `Vec<(RecordKey, Bytes)>`, sorts it, and only then
  truncates to `limit`. `batch = limit.clamp(1, 1024)` only sizes stream chunks — it never
  bounds total accumulation. This is the durable backend for `RepoInstance::read_changelog_from`
  (follower replication catch-up and late-subscriber resync).
- **Failure scenario:** a repo with millions of committed events whose changelog is tailed
  with `limit = 10` loads and sorts every remaining event into RAM per call — O(N) memory and
  O(N log N) CPU where O(limit) was promised. Under a replication loop this is sustained
  unbounded allocation (OOM/DoS), violating CLAUDE.md pillar 3. The changelog journal has no
  retention (server repl handler documents "R0: no retention"), so N grows forever.
- **Suggested fix:** break out of the drain loop once `pairs.len() >= limit + batch` (slack
  for the defensive in-window sort), or take only the first `limit` pairs batch-wise (keys are
  big-endian commit versions, so chunks arrive ascending on disk backends). Add a regression
  test asserting `range_from(from, 1)` on a large seeded journal performs bounded reads (e.g.
  a counting Store double).
- *Also flagged by performance-hotpath (#4 — same defect, perf framing; counted once).*

### 1.2 [HIGH] Drainer Phase B silently drops Put/Delete for tables with no `MvccStore` — and its justification comment is false
- **File:line:** `crates/shamir-engine/src/tx/drainer.rs:419-437` (Phase A), `:504-519` (Phase B)
- **Issue:** Phase A's op loop explicitly skips `WalOpV2::Put | Delete` for ALL tables ("Data
  ops: handled below via batch accumulation") — `replay_v2_op` is never called with data ops
  on the warm path. Phase B then writes history only for tables found in `per_table_mvcc`;
  for a table with no `MvccStore` it does nothing, with the comment *"data ops were already
  handled by replay_v2_op in Phase A (which skips Put/Delete for MVCC tables)"* — a
  description of behavior that does not exist. Cold recovery (`recover_inflight_v2` →
  `replay_v2_op`, `recovery.rs:80-117`) DOES write `data_store` for unattached tables, so the
  cold and warm paths diverge.
- **Failure scenario:** the ack path's `apply_data_batch` writes unattached (non-MVCC) tables
  via `base.transact` inline; on persistent failure the tx is reported
  `MaterializationState::Deferred` with the documented contract "recovery / drainer will
  reconcile" (`tx_out.rs`, `materialize.rs`). The drainer then finalizes the entry anyway —
  `gate.mark_durable(v)`, and `wal.commit(txn_id)` if A5-safe — without ever re-applying the
  failed table's data ops. If the WAL marker is truncated, the sole durable copy is destroyed:
  the reconciliation the client was promised cannot happen even after the storage fault heals.
  Reachability is narrow (unattached/system tables, or a table whose `per_table_mvcc` entry
  was removed by a concurrent `remove_table` mid-tx) — which is exactly why no test catches
  it: every fixture in `tx/tests/drainer_tests.rs` goes through `repo.get_table(...)`, which
  attaches an `MvccStore` in `create_table_context`.
- **Suggested fix:** in Phase B, when `read_sync(table_id, ...)` returns `None`, route the
  batch's ops through `replay_v2_op` (which already implements the non-MVCC `data_store`
  writes and NotFound-tolerant deletes), or record the table in `failed_tables` so Phase C
  stops finalization. Correct the Phase B comment. Add a `drain_step` test with a hand-built
  entry whose `Put` targets a token absent from `per_table_mvcc`, asserting the data lands in
  `data_store` (or that the entry is NOT marked durable).

### 1.3 [HIGH] *(primary: error-handling-lifecycle 6.1)* — write-path pre-reads swallow all read errors as "record does not exist"
- Full write-up at 6.1; listed here because it is a correctness defect at heart (mis-planned
  index/counter ops on the committed WAL entry). correctness-tdd rated the same sites medium;
  consolidated at the error-handling lens' **high** with its fuller failure scenario.
- *Also flagged by error-handling-lifecycle (#1 — one defect, two lenses; counted once).*

### 1.4 [MEDIUM] `GroupCommit`: a panicking leader strands `leader_busy = true` forever
- **File:line:** `crates/shamir-engine/src/repo/group_commit/mod.rs:48-117`
- **Issue:** the detached `leader_loop` task (added to fix the cancellation DoS, audit §2.1)
  resets `leader_busy = false` only on its normal exit path. If `flush()` panics, the task
  aborts without unwinding to the reset: the current batch's waiters correctly get
  `Err("group-commit flush task dropped")` from their dropped oneshots, but `leader_busy`
  stays `true`, so every subsequent `run()` caller pushes its oneshot and parks forever —
  all future `synced_flush` calls on that repo hang (durability-flush DoS, the same class
  the cancellation fix aimed to eliminate).
- **Failure scenario:** any panic inside the `flush` closure (`repo.flush_buffers()` — e.g. a
  poisoned lock or an `unwrap` deep in a backend) converts one failed flush into a permanent
  hang of every later `synced` commit on the repo.
- **Suggested fix:** catch unwind around `flush().await` (or guard the leadership with an
  RAII struct whose `Drop` resets `leader_busy` under the state lock), propagating the panic
  message to the batch as `Err`. Add a test with a panicking flush asserting a subsequent
  `run()` still completes. (Same panic-unsafe-latch pattern as 6.6.)

### 1.5 [HIGH] *(primary: performance-hotpath 4.1)* — `rederive_stale_value_ops_post_stage` is quadratic in staged ops (O(R×W) per commit)
- Full write-up at 4.1; listed here because it sits on the commit critical path and is the
  same defect class this crate already fixed twice nearby (#1099, #1108 —
  `released_unique_cache` was made incremental for exactly this reason). correctness-tdd
  rated it medium; consolidated at performance-hotpath's **high**.

### 1.6 [LOW] Stale doc asserts a footprint-ordering invariant the AsyncIndex path no longer has
- **File:line:** `crates/shamir-engine/src/tx/finalize.rs:21-26` vs `crates/shamir-engine/src/tx/commit.rs:731-751` (verified: footprint is recorded at `:744`, strictly before `version_guard.commit()` at `:751`)
- **Issue:** `finalize.rs` justifies NOT unifying the AsyncIndex tail by claiming its SSI
  footprint (`record_commit_writes`) runs AFTER `version_guard.commit()`. The code records
  the footprint strictly BEFORE publish (with its own F-28/S3-C comment explaining the
  missed-phantom window this order closes). Divergence axis 1 of the three listed is
  therefore false. A future refactor trusting this doc either preserves a phantom constraint
  or — worse — re-orders footprint-after-publish on some path, reintroducing the real window
  the ordering exists to close.
- **Suggested fix:** update the doc to the current order (and re-evaluate whether the
  remaining two axes still justify the duplication). Doc-only, but on a concurrency-critical
  ordering it should be corrected before it misleads.

### 1.7 [LOW] *(primary: security-crypto 3.7)* — `SessionPermissions` dead loop + by-construction `unwrap()`s in test-only RBAC scaffolding
- Full write-up at 3.7 (public export + dead loop + unwraps, one consolidated finding);
  performance-hotpath's nits also flag the dead loop's wasted O(N) scan. Counted once.

### 1.8 [MEDIUM] *(primary: style-claude-md 7.3)* — two implementation files embed inline `#[cfg(test)] mod tests`
- Full write-up at 7.3 (`writer_drain_barrier.rs:410-474`, `hashable_query_value.rs:250+`).
  correctness-tdd rated it low; consolidated at style-claude-md's **medium**.

### 1.9 [LOW] *(primary: concurrency-lockfree 2.3)* — `RecordCounter` dirty-flag race can drop the last persist trigger
- Full write-up at 2.3 (the concurrency lens' write-up covers both the `set` and `persist`
  shapes and is the superseding analysis). correctness-tdd rated it nit; consolidated at
  concurrency-lockfree's **low**.

**Coverage verdict (TDD lens).** No vacuous tests in the current suite; discipline is
demonstrably strong (issue-keyed regression tests, one-shot pause seams with
`reached`/`armed` handshakes, nextest-process-isolation rationale, honest in-code
self-corrections — the F-84 loom-model fixup and the #1003 vacuous-hook fixup). The gaps
align exactly with the findings above: no test drives `drain_step` with a data op against a
table lacking an `MvccStore` (1.2), no test covers a panicking group-commit leader (1.4), and
no test asserts a transient pre-read error on `set`/`update_tx`/`delete` surfaces as an error
rather than an insert-shaped plan (6.1) — each a natural Red test under CLAUDE.md's
Red/Green/Refactor protocol.

## 2. concurrency-lockfree

Lens verdict: exceptionally good shape against CLAUDE.md's five pillars — every production
lock is a `tokio::sync::Mutex` with an inline contention-model comment, the two
`std::sync::Mutex` sites are `#[cfg(test)]` failure-injection hooks (plus the sanctioned
`InFlightCreateSet` DDL guard), concurrent maps are `scc::*`/`DashMap` with `THasher`
everywhere, `ArcSwap` RCU for read-heavy snapshots, scc `len()` ack'd or atomically
mirrored, and `WriterDrainBarrier` carries a SeqCst memory-model proof, the F-70 lock-order
invariant, and loom coverage. Findings below are residual.

### 2.1 [HIGH] DashMap shard read-guard held across `.await` in `DbInstance` accessors
- **File:line:** `crates/shamir-engine/src/db_instance/db_instance.rs:61-68` (verified) plus
  `:172-184`, `:187-200`, `:203-214`, `:217-228`, `:231-242`, `:245-256`, `:259-271`
- **Issue:** `self.repos.get(repo_name)` returns a `dashmap::Ref` — a synchronous
  `std::sync::RwLock` read guard on one shard of
  `repos: Arc<DashMap<String, RepoInstance, THasher>>`. In `get_table` and all seven
  index-routing methods the `Ref` is kept alive across the delegated `.await`
  (`repo_manager.get_table(...).await`, `repo.create_index(...).await`,
  `lookup_by_index(...).await`, …). DashMap guards must not cross an await point: the
  guard-holder parks at the `.await` while the shard's OS RwLock stays held by its
  (unscheduled) thread, and any writer needing that shard (`add_repo`, `remove_repo`,
  `rename_repo` — all take the write lock) blocks its **worker thread** synchronously. This
  is verbatim the deadlock class this crate itself documents and fixed in
  `RepoInstance::get_table` (`src/repo/repo_instance.rs:311-320`: "holding the `entry()`
  write guard across … an `.await` … under runtime oversubscription every worker thread can
  become wedged on the OS RwLock of a shard whose guard-holder is parked at an `.await`, and
  a synchronous lock cannot yield").
- **Failure scenario:** (a) a cold `get_table` lazily constructs a `TableManager` — store
  opens, index loads, potentially `mgr.repair().await` (full legacy-index rebuild, seconds to
  minutes per `TableManager::create`) — all with the shard read-locked. (b) Worse:
  `create_index`/`drop_index` hold the guard across an entire online backfill (minutes, per
  `KNOWN_LIMITATIONS` §3). Concurrently, one `remove_repo`/`rename_repo` call blocks its
  tokio worker on the shard write lock for that whole duration; a handful of such callers
  under a small worker pool wedges the runtime — a `TIMEOUT [test]`-class hang per
  CLAUDE.md's "hangs are bugs" rule.
- **Suggested fix:** mirror the `repo_instance.rs` pattern: clone the `RepoInstance` out
  (cheap `Arc`-field clone) and drop the guard before awaiting — exactly what `get_repo` at
  `:109-111` already does. Apply to all eight sites.

### 2.2 [MEDIUM] `ValidatorRegistry::add_binding` — check-then-act lost update on a lock-free map
- **File:line:** `crates/shamir-engine/src/validator/registry.rs:162-169` (caller:
  `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:504`, unserialized)
- **Issue:** `add_binding` runs two separate critical sections:
  `entry_sync(id).and_modify(|set| set.insert(table))` (no-op while vacant) followed by
  `insert_sync(id, BTreeSet::from([table])).ok()` — the `.ok()` discards the `Err` scc
  returns when the key already exists (scc `insert` never overwrites; see
  `repo_instance.rs:500`'s "silently no-op" note). Two concurrent `bind_validator_as` calls
  for the same validator id (different tables) can both observe the entry vacant, both
  `and_modify` no-op, and the loser's `insert_sync` errors with its table binding silently
  dropped from `bound_in`. The bind path takes no lock that serializes them.
- **Failure scenario:** concurrent `BindValidator` DDL for validator V on tables T1 and T2 →
  T2's `bound_in` entry lost → `is_bound(V)` under-reports → a later `drop_validator`'s
  still-bound refusal (the registry's documented referential-integrity contract,
  `registry.rs:6-7`) is defeated and the dropped validator leaves a dangling binding on T2;
  step 7's `persist_validator_bound_in` then persists the incomplete set, compounding the drift.
- **Suggested fix:** collapse into one atomic critical section using scc's entry API:
  `let mut e = self.bound_in.entry_sync(*id).or_insert_with(BTreeSet::new);
  e.get_mut().insert(table);` — `or_insert_with` occupies the slot under the bucket lock,
  closing the race.

### 2.3 [LOW] `RecordCounter` — `dirty` flag clobbered by concurrent `increment` during `set`/`persist` awaits *(also flagged by correctness-tdd, as a nit — counted once)*
- **File:line:** `crates/shamir-engine/src/table/record_counter.rs:88-94` (`set`),
  `:143-163` (`persist`)
- **Issue:** `set()` does `cache.store(count)` → `write_through(count).await` →
  `dirty.store(false)`. A concurrent `increment()` landing during the `.await` does
  `fetch_add` + `dirty.store(true)`; `set()` then resumes and its unconditional
  `dirty.store(false)` erases that mark. `persist()` has the same shape (`:159-161`):
  `write_through(cur).await` then `dirty.store(false)`, with `cur` snapshotted before the
  await. The `persist_lock` doesn't help — `increment` is deliberately lock-free. Result:
  the incremented delta is invisible to the next `persist()` (fast-path skip at `:144`) and
  the durable count drifts until a *later* increment re-dirties the flag; on crash/boot the
  counter seeds stale. In-memory `get()` stays correct and the doctor reconciles, bounding
  blast radius to metadata drift.
- **Failure scenario:** doctor `set_to`/`set` reconciling a count while writers are
  inserting → the writes' bumps are never persisted → after crash, `RecordCounter::get`
  reports a pre-reconcile+delta count until the next `repair()`.
- **Suggested fix:** replace the boolean with a generation/epoch `AtomicU64` bumped on every
  `increment`/`set`; `persist` snapshots the epoch before the write and clears `dirty` only
  if the epoch is unchanged (CAS), or re-reads `cache` after the write and stores that as
  `last_persisted` so the skip comparison can't mask the delta.

### 2.4 [LOW] Watchdog thread runs `log::warn!` inside `iter_sync` — sync log I/O under scc bucket locks
- **File:line:** `crates/shamir-engine/src/query/batch/op_watchdog.rs:118-130`
- **Issue:** the 1 Hz diagnostic thread scans `REGISTRY` via `iter_sync`, whose closure
  executes while scc holds each bucket's lock. The closure calls `log::warn!` (potentially
  synchronous stderr I/O) under that lock, momentarily stalling `insert_sync`/`remove_sync`
  — which `register_op_watchdog`/`OpGuard::drop` call on the batch-op path. Mitigations:
  warnings are one-shot per stuck op and the registry is small, so the window is rare and
  short — but it's I/O under a lock the hot path shares.
- **Suggested fix:** collect `(id, alias, elapsed)` triples inside the closure and emit the
  `log::warn!` lines after `iter_sync` returns (the existing second pass over
  `ids_to_update` already shows the pattern).

### 2.5 [NIT] `MigrationCoordinator::drain_until_caught_up` — unbounded catch-up loop under sustained writes
- **File:line:** `crates/shamir-engine/src/migration/coordinator.rs:247-260`
- **Issue:** the loop breaks only when `shadow_lag() <= max_lag` or a pass applies zero
  entries. A shadow log fed faster than the drain applies entries keeps `applied > 0`
  forever — a livelock (no locks held, but the caller never returns) on a sustained-write
  source table during migration.
- **Suggested fix:** add an attempt/pass budget or backoff and return the residual lag to
  the caller (an admin/migration path can poll again) rather than spinning indefinitely.

### 2.6 [NIT] `FkReverseCache::get_or_build_by_parent` — unbounded CAS-loss retry while holding `build_lock`
- **File:line:** `crates/shamir-engine/src/repo/fk_reverse_cache.rs:342-355`
- **Issue:** on a publish-CAS loss the build loop retries indefinitely, still holding
  `build_lock`, so a continuous `invalidate()` storm (continuous DDL) starves both the
  rebuilder and every other waiter on the cache. Practically bounded because invalidation is
  DDL-rare and the design (single-flight + pointer-identity CAS) is otherwise exemplary;
  flagged for completeness.
- **Suggested fix:** none required for correctness; if ever needed, a `yield_now` between
  retries or a bounded retry + error return would make the starvation window explicit. Do
  NOT release `build_lock` mid-retry — that breaks single-flight.

## 3. security-crypto

Lens verdict: no crypto primitives of its own (HMAC/SCRAM/Argon2/TLS all live in sibling
crates); no `unsafe` in library code; corrupt-record reporting leaks only `(table, id)`;
WASM validator failures fail closed (`stop = true`). The surface is: client-supplied
filter/expr trees (DoS), actor threading toward the real enforcement gate
(`ShamirDb::execute_as`, outside this crate), the explicitly trusted replication apply
boundary, and the WASM validator bridge. Main gaps: the designated untrusted-input DoS
guards are incomplete, and a per-row recompile amplification in `$cond` evaluation.

### 3.1 [MEDIUM] Filter-depth DoS guard misses `when`, `having`, and all `FilterValue` nesting — designated guard is incomplete
- **File:line:** `crates/shamir-engine/src/query/batch/batch_validate.rs:78-97` (guard),
  `query_runner.rs:136-157` (`when` compiled, never depth-checked),
  `query/read/aggregate.rs:1304-1311` (`having` compiled, never depth-checked),
  `shamir-query-types/src/filter/filter_enum.rs:219-238` (`check_filter_depth` walks only
  `And`/`Or`/`Not` — never descends into `FilterValue` operands)
- **Issue:** `validate_filter_depth` collects filters from exactly three places:
  `Read(q.r#where)`, `Delete(d.where_clause)`, `Update(u.where_clause)`. Three classes of
  client-supplied filter trees reach recursive compilation/evaluation **without any depth
  check**: (1) `QueryEntry::when` (Epic03/B) — compiled by `compile_filter` in
  `resolve_skip` (`query_runner.rs:156`); the planner only rejects *field-based comparisons*
  inside `when`, not depth; (2) `GroupBy::having` — compiled at `aggregate.rs:1306`; (3)
  `FilterValue` trees (`$cond`/`$expr`/`$fn` args/`Array`) nested inside a WHERE *value* —
  `check_filter_depth` treats `Filter::Eq{..}` as a leaf, so a depth-1 `Eq` whose value is a
  100k-deep `$cond`/`Array` chain passes the guard, then `resolve_filter_query`
  (`resolve.rs:272-431`) and `compile_filter` recurse unbounded at eval time — per row.
- **Failure scenario:** a client sends a Read whose `where` embeds a deeply nested
  `{"$cond": ...}` chain (or a deep `when`). The batch passes `validate_filter_depth`, then
  the recursive walk overflows the tokio worker stack → process abort, not a catchable
  `Err`. (Transport-layer serde recursion is the first line of defense, but it is equally
  unbounded and lives in sibling crates; this engine guard exists precisely as the second
  line — #670 even extended it to the interactive-tx path — and it silently covers only 3 of
  the reachable filter surfaces.)
- **Suggested fix:** (a) extend the collector in `validate_filter_depth` to include
  `entry.when` and `Read(q.group_by.and_then(having))`; (b) add an iterative
  `FilterValue`-tree depth walk (mirroring `prescan_filter`'s dispatch shape in
  `cond_cache.rs:104-150`) to `check_filter_depth` so value-nesting counts toward
  `MAX_FILTER_DEPTH`; (c) optionally make `compile_filter`/`resolve_filter_query`
  depth-bounded (return `FilterNode::False`/`None` past a cap) as a final backstop.

### 3.2 [MEDIUM] Per-row recompile of `$cond` conditions on the WHERE path — client-driven CPU amplification (incl. `Regex::new` per row)
- **File:line:** `crates/shamir-engine/src/query/filter/resolve.rs:397-403`;
  `cond_cache.rs:1-16` (module doc admits WHERE/`when`/write-value callers do not populate
  the cache); `compile.rs:101-108` (`Regex::new` inside `compile_filter`)
- **Issue:** when a WHERE clause's comparison value is a `$cond` (`FilterValue::Cond`),
  `resolve_filter_query`'s Cond arm calls `compile_filter(&cond.condition, ctx.interner)` on
  every evaluation — once per record scanned — because the #643 `CondCache` is only wired
  into `SelectProjection::new`. If the `$cond`'s condition contains a `Filter::Regex` or
  `Like` node, that is a full `Regex::new` compile per row. The `regex` crate is linear-time
  at match (no ReDoS), but *compilation* is not free (tens of µs to ms for large patterns,
  default 10 MB compiled-program budget per pattern) and the #666 cooperative deadline only
  checkpoints **between ops**, never inside one.
- **Failure scenario:** one Read op over a large table with
  `where: {"op":"eq","field":"x","value":{"$cond":{"if":{"op":"regex",...},"then":1,"else":0}}}`
  recompiles the regex once per row — minutes of single-op CPU with no deadline trip; the op
  watchdog only logs it afterwards. Repeat across connections for sustained amplification.
- **Suggested fix:** thread a `CondCache` through the WHERE compile path the same way
  `SelectProjection::new` does (prescan the compiled `FilterNode`'s embedded `FilterValue`s
  once per query), or cache the compiled `FilterNode` keyed by the (static-per-query)
  `&FilterValue` pointer — the same identity argument `CondCache` already documents.

### 3.3 [LOW] Engine boundary performs no authorization — enforcement is a single upstream wrapper (`execute_as`), `trace_access` is observability only
- **File:line:** `crates/shamir-engine/src/query/batch/query_runner.rs:563-578` (explicit
  doc: `trace_access` "always `Ok`, NOT the enforcement gate"); `batch_execute.rs:79-100`
  (public `execute_batch` takes an `Actor` but never checks it);
  `db_instance/db_instance.rs` (raw facade, no actor parameter at all)
- **Issue:** every public engine entry point (`execute_batch`, `execute_in_open_tx`,
  `DbInstance` methods) is a full-power API; DAC enforcement happens only if the embedding
  calls `ShamirDb::execute_as` first. The code comments this honestly and even warns future
  readers not to mistake `trace_access` for enforcement — but nothing structural prevents a
  new call path (a new server route, a WASM host bridge, an internal job) from skipping the
  wrapper and silently running as `Actor::System`. The only `Actor::System` hardcode in
  non-test engine code is inside the `#[cfg(test)]` `execute_batch_with_permissions`.
- **Suggested fix:** consider a type-level seam (engine executors take an
  `Authorized<BatchRequest>` token minted by the enforcement layer, or `trace_access` gains
  an enforcing sibling behind a feature flag), so "forgot the wrapper" becomes a compile
  error rather than a silent bypass.

### 3.4 [LOW] Replication apply is a trusted raw write — no re-validation of leader events
- **File:line:** `crates/shamir-engine/src/tx/apply_replicated.rs:124-271` (raw
  `(key, value)` straight into `apply_committed_ops` / `base.transact`); module doc lines
  4-9 state the trust model
- **Issue:** the follower applies leader `ChangelogEvent`s with **no validators, no schema
  check, no DAC, no integrity check** on the payload — raw bytes go directly into the
  version-log of any table named in the event. The entire security of this path rests on the
  replication transport being authenticated/integrity-protected (outside this crate). A
  compromised or spoofed upstream can plant arbitrary/corrupt record bytes that the follower
  then serves to its own clients.
- **Failure scenario:** unauthenticated replication endpoint (or a compromised peer in a
  chain — events are re-emitted downstream via `reproject_for_downstream` without any
  re-check) writes garbage or forged records; follower-side reads surface them (at best as
  `corrupt_records` refs) and downstream replicas chain-replicate the same bytes.
- **Suggested fix:** at minimum document this as a hard precondition on the transport crates
  in REPLICATION.md's threat model; consider an opt-in "validate-on-apply" mode (record
  decode + schema/validator gates on follower ingest) for deployments that cannot fully
  trust the wire.

### 3.5 [LOW] Pointer-keyed caches (`CondCache`, `FieldPathCache`, `QueryRefCache`) expose a public type alias whose safety invariant is documentation-only — stale *hit* hazard unaddressed
- **File:line:** `crates/shamir-engine/src/query/filter/cond_cache.rs:27-49`
  (`pub type CondCache = TMap<usize, Arc<FilterNode>>` keyed on
  `&*cond.condition as *const Filter as usize`); same pattern in `field_path_cache.rs` and
  `query_ref_cache.rs`
- **Issue:** the doc's safety analysis covers only the clone case (a cloned tree's nodes
  live at new addresses → cache *miss* → benign recompile). It does not cover **address
  reuse**: if the owning `Filter`/`FilterValue` tree is dropped while a cache built from it
  survives, a freshly allocated tree can land on the same addresses and the cache returns a
  **stale `FilterNode` for a different predicate** — a silent wrong-results failure, not a
  soft miss. Nothing in the type system ties cache lifetime to tree lifetime; the invariant
  is enforced only by a comment at current call sites.
- **Failure scenario:** a future caller caches across requests (natural temptation for a
  "compiled query cache") while request trees are dropped between uses; allocator reuse
  serves another query's compiled predicate → wrong data.
- **Suggested fix:** wrap the key in a newtype (`CondKey<'a>(&'a Filter)`) that borrows the
  tree, making "cache outlives tree" a compile error; or key on a hash of the filter tree
  instead of the address.

### 3.6 [LOW] Client-supplied `Regex`/`Like` patterns: no size/length cap, and invalid patterns silently compile to `False`
- **File:line:** `crates/shamir-engine/src/query/filter/compile.rs:81-110`
  (`Regex::new(pattern)`; `Err(_) => FilterNode::False`; `None => FilterNode::False`),
  `fts.rs:6-25` (`like_pattern_to_regex`, `.ok()`)
- **Issue:** (a) pattern length is unbounded — a repeated batch of ops each carrying a
  near-10 MB pattern burns seconds of compile CPU per op, again inside the no-checkpoint
  window of a single op (compounds 3.2). (b) an **invalid** pattern folds to
  `FilterNode::False` — fail-closed for a bare predicate, but `Not(<invalid regex>)`
  compiles to `True`, i.e. *matches everything*: a `DELETE ... WHERE NOT (regex typo)`
  deletes all rows with no error surfaced. The engine's convention elsewhere is that
  malformed client input is a hard `Err` (e.g. `WriteValueError::MalformedMarker`), so this
  silent fold is inconsistent.
- **Suggested fix:** reject invalid regex/like patterns at batch validation with a coded
  `BatchError` instead of folding to `False`; cap pattern length in `validate_filter_depth`'s
  pass (e.g. 64 KiB) like `MAX_FILTER_DEPTH` caps depth.

### 3.7 [LOW] `SessionPermissions` RBAC remains publicly exported while being test-only scaffolding — plus a dead authorization loop inside it *(also flagged by correctness-tdd #7 and performance-hotpath nits — counted once)*
- **File:line:** `crates/shamir-engine/src/query/auth/mod.rs:10` (unconditional
  `pub use session::SessionPermissions`), `session.rs:26-34` (doc: "test-only scaffolding …
  NOT wired into the server's live request path"), `session.rs:162-170` (first loop body is
  empty — dead code with an inline "we need a different approach" TODO),
  `session.rs:238-254` (`extract_action_resource` unwraps `op.table_ref()` for all five
  data-op variants — safe today only because every `BatchOp::Read/Insert/Update/Delete/Set`
  construction carries a table ref, an invariant not asserted anywhere),
  `batch/mod.rs:168-170` (only `execute_batch_with_permissions` is `#[cfg(test)]`-gated)
- **Issue:** the non-enforcing permission type is part of the crate's public API, so a
  downstream embedder can reasonably construct `SessionPermissions` and believe it is the
  access model; its companion consumer is test-gated. The retained implementation also
  carries the unfinished half of `row_filter()` (the dead loop), which invites "fixes" to the
  wrong loop, and `unwrap`s on `op.table_ref()` that a future op variant could panic.
  `SecretString` is likewise re-exported (`auth/mod.rs:11-14`) but never used here
  (harmless — redaction/zeroize live in `shamir-types::secret`). Production impact is nil
  today (live access control is Shomer DAC); this is boundary fragility.
- **Suggested fix:** gate `SessionPermissions` behind `#[cfg(test)]` alongside
  `execute_batch_with_permissions`, or move it to a `test-support` module; delete the dead
  loop; replace the unwraps with `debug_assert!` + fallback resource (or return
  `Resource::Global`-deny).

## 4. performance-hotpath

Lens verdict: largely disciplined against pillar 3 — zero un-annotated `scc::*::len()` in
non-test code, THasher/Fx everywhere, textbook hoisting in several paths
(`table_manager_crud.rs:285-291`) — but five hot-path quadratic/unbounded shapes remain,
plus a long tail of per-row allocations. All HIGH/MEDIUM findings below were re-verified
line-by-line in the original review; LOW/nit entries pattern-verified against the same files.

### 4.1 [HIGH] O(N²·K) re-planning scans inside `rederive_stale_value_ops_post_stage` *(also flagged by correctness-tdd #5 — counted once)*
- **File:line:** `crates/shamir-engine/src/tx/pre_commit.rs:1999-2329` (rebuild at
  `:2014-2030`; linear rescans at `:2089-2101` and `:2209-2300`; gates at `:1875`,
  `:1969-1975`)
- **Issue:** for each staged row, the code (a) rebuilds `staged_removals_by_rid` by
  re-iterating the whole `tx.index_write_set` filtered to the table and cloning every
  matching `RemovePosting.key` (`key.clone()` at `:2027`), and (b) for each re-planned op
  runs a `.iter().filter(|(t,_)| *t == table_token).any(...)` linear rescan of the same set.
  With N staged rows × K index ops each, cost is O(N²·K) map rebuilds, key clones and
  scans. Unlike its generation-gated siblings, the gate here
  (`version_allocation_high_water_mark > snapshot_version`, `:1970`) fires under *any*
  concurrent write traffic — the normal production case for any table with base indexes.
- **Failure scenario:** bulk DELETE/UPDATE of 10k rows over 3 indexed fields under
  concurrent commits → ~10^8-element linear scans plus ~30k full map rebuilds with key
  clones, executed inside the locked pre-commit validate phase, directly widening commit
  latency for all same-table writers.
- **Suggested fix:** build `staged_removals_by_rid` (and a `TFxSet` of staged regular
  posting keys) once per table before the row loop, updating it when appending ops; replace
  both `.any()` rescans with O(1) set lookups (mirroring `refresh_released_unique_cache`'s
  incremental shape). Assert via the existing `p1107_stale_value_gate` bench that ns/op does
  not regress super-linearly with batch size.

### 4.2 [HIGH] Staged-overlay probe clones and linearly scans the whole tx write-set per validated record
- **File:line:** `crates/shamir-engine/src/validator/validator_db.rs:312`
  (`staged_field_matches`) and `:427-440` (`exists_in_self` step 3); root cost in
  `crates/shamir-tx/src/staging_store.rs:172-180` (`snapshot_ops()`)
- **Issue:** `staging.snapshot_ops()` materializes a fresh `Vec<KvOp>` cloning every staged
  key and value bytes, then `.any(...)` linearly scans it — invoked once per unique/FK
  schema-rule probe, i.e. per record being validated.
- **Failure scenario:** batch-insert of M rows into a table with a schema-level `unique` or
  FK rule in one tx → M probes × O(M) clone+scan = O(M²) time and O(M) transient allocations
  per record, on the hot pre-commit write path (the autocommit path threads an implicit tx,
  so `ctx.db()` is `Some`).
- **Suggested fix:** add a non-materializing `for_each_op` iterator to `StagingStore` that
  matches on borrowed bytes (no key clone), and/or maintain a per-table staged-value set
  keyed by (interned field id, scalar) updated at stage time for O(1) probes.

### 4.3 [HIGH] FK RESTRICT: one full child-table scan per parent value, values not deduplicated
- **File:line:** `crates/shamir-engine/src/query/batch/fk_restrict.rs:145-164` (per-value
  loop); `collect_parent_values` `:220-282` (un-deduped push at `:273`); full-scan fallback
  in `child_has_reference` `:373-391`
- **Issue:** `for parent_val in values_for_field { child_has_reference(...) }` — when the
  child FK column has no single-field index, each value pays a full `list_stream_tx` scan of
  the child table. `collect_parent_values`' doc says "distinct values" but `:273` pushes
  every matched row's value, so duplicates multiply identical scans. The index fast path
  (`:325-352`) exists but only when an index covers the field.
- **Failure scenario:** bulk delete of 10k parent rows whose FK value repeats (deleting one
  customer's orders) against a 1M-row child table without a child-side index → up to 10k
  full child scans inside one delete op — minutes of latency.
- **Suggested fix:** dedupe parent values into a `TFxSet`, then invert: ONE pass over the
  child table testing each row's FK field against a coercing membership set (the shape
  `fk_actions::classify_row` already uses), keeping the index fast path as an early-out per
  distinct value. (4.18 is the same shape on the ON UPDATE path.)

### 4.4 [HIGH] *(primary: correctness-tdd 1.1)* — changelog `range_from` buffers the entire journal tail, then truncates to `limit`
- Full write-up at 1.1; listed here because it is one of the five lens-defining
  unbounded/quadratic shapes. Counted once.

### 4.5 [HIGH] ON UPDATE discovery: repo-wide table scan on EVERY UPDATE, before the no-op gate
- **File:line:** `crates/shamir-engine/src/query/batch/fk_on_update.rs:734-783`
  (`discover_on_update_refs`); invoked at `:188`, before the set-fields ∩ ref-fields gate at
  `:196-204`
- **Issue:** `repo.list_table_names()` then per table `resolver.resolve(...)` +
  `child_table.collect_fk_refs()` — an O(tables) schema walk per UPDATE op, paid even when
  the update touches no FK-referenced field (the intersection gate runs after it). This is
  exactly the scan F-28 Step 4 (#831) removed from the delete path via
  `RepoInstance::fk_reverse_cache`; the ON UPDATE path never migrated, contradicting the
  module's own "zero scan overhead on the hot path" claim.
- **Failure scenario:** repo with 500 tables under update-heavy load → 500 table resolves +
  FK collections per UPDATE statement, dominating op latency.
- **Suggested fix:** route through `repo.fk_reverse_cache()` entries filtered on
  `on_update != NoAction` (mirror `discover_action_refs` in `fk_actions.rs`), then apply the
  cheap intersection gate first.

### 4.6 [MEDIUM] Per-row `iter_unique_indexes()` clone storm in tx batch insert
- **File:line:** `crates/shamir-engine/src/table/table_manager_tx_ops.rs:705-722`
  (`insert_tx_many`), `:920-938` (`insert_tx_many_bytes`); clone source
  `crates/shamir-index/src/base_index/index_info.rs:310-314`
- **Issue:** `for (i, v) in values.iter().enumerate() { for def in
  self.index_manager.iter_unique_indexes() {...} }` — the iterator yields owned
  `IndexDefinition` clones (`snap[i].clone()`), so a batch pays O(rows × U ×
  deep-clone(IndexDefinition)). The sibling non-tx path fixed precisely this with an
  explicit flamegraph-justified hoist (`table_manager_crud.rs:285-291`); the two tx paths
  (the wire path for transactional INSERT) did not.
- **Failure scenario:** 10k-row transactional bulk insert into a table with 4 unique indexes
  → 40k deep `IndexDefinition` clones + iterator setups instead of 4.
- **Suggested fix:** hoist `let unique_defs: Vec<IndexDefinition> =
  self.index_manager.iter_unique_indexes().collect();` above the row loop in both methods,
  mirroring `crud.rs:290`.

### 4.7 [MEDIUM] Phase 5a data batch deep-cloned on every commit for a retry that almost never happens
- **File:line:** `crates/shamir-engine/src/tx/commit_phases.rs:145-154`
  (`apply_data_phase`); `crates/shamir-engine/src/tx/materialize.rs:80-89` (`materialize`)
- **Issue:** `retry_materialize(MATERIALIZE_ATTEMPTS, || { apply_data_batch(repo, table_id,
  base.clone(), ops.clone(), ...) })` — the `FnMut` closure body runs on attempt 1 too, so
  every clean commit allocates and memcpys the entire per-table `Vec<KvOp>` (full record
  bodies) once, purely so a rare retry could re-run it.
- **Failure scenario:** every commit pays an extra O(tx payload) alloc+memcpy on the
  latency-critical ack path; large batch commits double the write path's memory bandwidth.
- **Suggested fix:** take `&[KvOp]` in `apply_data_batch` (the MVCC arm already only reads
  it), or hold `Option<Vec<KvOp>>` and clone lazily from attempt 2 onward.

### 4.8 [MEDIUM] `$in` probe heap-allocates per string/binary row value
- **File:line:** `crates/shamir-engine/src/query/filter/filter_node.rs:98-99`
- **Issue:** `ScalarRef::Str(s) => set.contains(&QueryValue::Str(s.to_string()))` (and
  `Bin(b.to_vec())`) — a fresh heap allocation per field probe per row on the `InSet` arm,
  the engine's hottest CPU filter path (see the `filter_eval` bench). The F6 borrow-based
  `scalar_at` design (documented "zero clone" at `:547`) is defeated for the two
  variable-length scalar types.
- **Failure scenario:** `$in` filter over a 1M-row string column with a 100-element literal
  set → 1M unnecessary String allocations per query.
- **Suggested fix:** give the set a borrow-friendly probe: a wrapper key type that
  `Hash`es/`Eq`s over `&str`/`&[u8]` bytes against the `TSet<QueryValue>`, or compile a
  bytes-keyed mirror set at filter-compile time.

### 4.9 [MEDIUM] `$contains_all` deep-clones the required-values set per record
- **File:line:** `crates/shamir-engine/src/query/filter/filter_node.rs:808`
- **Issue:** `let mut remaining = values.clone();` per record — deep-copies every
  `QueryValue::Str` in the client-supplied `TSet` for every row evaluated. The in-comment
  acknowledgment ("The only allocation is the cloned scratch set") is per-row, not per-query.
- **Failure scenario:** `$contains_all` with a 50-element string list over 1M rows →
  1M × 50-string deep clones — O(rows × M) time and allocation churn.
- **Suggested fix:** reuse a per-node scratch of *positions* (`Vec<u32>` cleared in place
  between rows) or a ≤64-bit bitmask like the neighboring `FtsMatch` code (`:897`).

### 4.10 [MEDIUM] Raw msgpack pre-filter allocates per Compare node per record despite the "zero-alloc" contract
- **File:line:** `crates/shamir-engine/src/query/filter/eval_bytes.rs:527-539`, conversion
  helper `:655-667` (contract comment at `:642`)
- **Issue:** for every record × every `Compare` node,
  `query_value_to_filter_value_lit(pre)` builds an owned `FilterValue` —
  `FilterValue::String(s.clone())` / `Binary(b.clone())` for Str/Bin literals — or falls
  back to `value.clone()`. The module header (`:32-36`, `:642`) claims a zero-alloc raw
  cursor; string/binary comparisons break that.
- **Failure scenario:** a `WHERE str_field = ?` bytes pre-filter over a large scan
  allocates one String per record per Compare node ahead of the full decode it is supposed
  to cheaply gate.
- **Suggested fix:** add `(RawScalar, &QueryValue)` compare arms that compare the raw slice
  directly (`RawScalar::Str(a)` vs `&QueryValue::Str(s)` → `a.cmp(s.as_bytes())`), removing
  both the conversion and the clone.

### 4.11 [MEDIUM] Drainer Phase A applies index postings one awaited store op at a time
- **File:line:** `crates/shamir-engine/src/tx/drainer.rs:419-437` →
  `crates/shamir-engine/src/tx/recovery.rs:147-239`
- **Issue:** data ops are deliberately accumulated and applied per-table in Phase B, but
  every `IndexPut`/`IndexDel` routes through `replay_v2_op` → one awaited
  `info_store().set/remove` per posting (plus a `table_by_token` resolve per op), although
  `Store::transact(Vec<KvOp>)` batching exists and the same file's own Phase B rationale is
  to coalesce exactly this shape.
- **Failure scenario:** an index-heavy drain window (bulk ingest, backfill) does one await +
  one backend op per posting instead of one per table; drain throughput lags,
  `MAX_UNDRAINED_VERSIONS` backpressure engages sooner and brakes live commits.
- **Suggested fix:** accumulate `IndexPut`/`IndexDel` per `table_id` in Phase A and transact
  them per table in Phase B (per-op error semantics can be preserved per batch).

### 4.12 [MEDIUM] ForEach re-plans and re-validates an identical body every iteration
- **File:line:** `crates/shamir-engine/src/query/batch/query_runner.rs:846-944`; the
  duplicated setup at `:210-212` (`BatchPlanner::plan` + `validate_tables` +
  `validate_filter_depth`)
- **Issue:** each of up to 100k iterations (`ITERATION_CAP` at `:35`) recurses into
  `run_nested_body_in_outer_tx` / `execute_batch_impl`, which re-runs planning + async table
  resolution + filter-depth validation for a body that is byte-identical across iterations;
  only params differ.
- **Failure scenario:** `for_each` over a 10k-element list with a 5-query body in a
  200-table repo pays 10k × (plan + 5 table resolves + validation) of redundant work before
  any execution.
- **Suggested fix:** plan/validate once before the loop; per iteration run only
  deadline-check, param injection, and execution.

### 4.13 [MEDIUM] Shadow log `read_from` rescans the whole prefix and buffers unbounded
- **File:line:** `crates/shamir-engine/src/migration/shadow_log.rs:105-123`
- **Issue:** every drain scans the full `__shadow_<id>_` prefix from lsn 0 (skipping
  deserialize, not reads, of old entries), buffers every entry ≥ `start_lsn` without cap,
  then `sort_by_key` defensively — although keys are big-endian-LSN-suffixed and thus
  already lexicographically ordered.
- **Failure scenario:** long migration with L entries drained D times
  (`drain_until_caught_up` loops at `coordinator.rs:247-260`; admin drains at start and
  cutover) → O(D·L) storage reads and O(tail) RAM per drain.
- **Suggested fix:** range-scan starting at `ShadowKey::new(id, start_lsn)`, drop the sort
  (or assert ordering), and cap the returned page like `MAINT_SCAN_BATCH` does elsewhere.

### 4.14 [MEDIUM] Shadow drain applies entries via per-entry `set`/`remove` round-trips
- **File:line:** `crates/shamir-engine/src/migration/coordinator.rs:212-229` and duplicated
  at `:288-303` (batched contrast in the same file at `:166-185`)
- **Issue:** `self.dst_data.set(key, Bytes::from(value.clone())).await?` per entry, while
  the same file's snapshot path correctly uses `set_many`. N entries → N individual async
  store ops (each potentially its own WAL/fsync) + a `value.clone()` per entry.
- **Failure scenario:** final drain before cutover after hours of dual-writes can hold
  hundreds of thousands of entries; per-op overhead makes cutover latency grow linearly
  where a batched sweep would be an order of magnitude faster.
- **Suggested fix:** group Puts into `set_many` chunks and Deletes into `remove_many` (API
  already used by `shadow_log.purge:137`).

### 4.15 [MEDIUM] `index2_on_insert` re-snapshots all index2 backends per record on the batched non-tx insert path
- **File:line:** `crates/shamir-engine/src/table/table_manager_crud.rs:373-375` calling
  `:89-97`; contrast the tx path's hoist at `table_manager_tx_ops.rs:771-777`
- **Issue:** `for (id, value) in pairs_iter() { self.index2_on_insert(id, value).await?; }`
  — each call walks `index2_registry.all_backends()` (an async scc traversal + `Arc` clone +
  fresh `Vec`, whose own `// O(N) ack` says "off hot path") against a backend set that
  cannot change mid-batch. The file's own doc (`:236`) acknowledges "index updates still
  loop per-record"; the two tx batch paths already solved this.
- **Failure scenario:** 1000-row bulk insert into a table with fts/functional/vector
  backends → 1000 scc-map walks + 1000 Vec allocations to plan against the same unchanged
  backends.
- **Suggested fix:** take `let backends = self.index2_registry.all_backends().await;` once
  before the pair loop and inline the plan/apply body (same shape as `insert_tx_many`
  step 4).

### 4.16 [LOW] Shadow log never purged after a successful migration commit
- **File:line:** `crates/shamir-engine/src/migration/coordinator.rs:274-310` (ends at
  `Committed` with no purge; only `rollback` purges at `:319` — grep confirms no other caller)
- **Issue:** every committed migration leaves all `__shadow_<id>_<lsn>` records (full row
  values) in the store forever; repeated migrations accumulate monotonically.
- **Failure scenario:** disk grows without bound across migrations; future prefix scans
  (including 4.13's rescans) get slower with each migration.
- **Suggested fix:** call `shadow_log.purge()` after the phase flips to `Committed` (or in
  the admin cleanup that removes the coordinator).

### 4.17 [LOW] FK `exists_in_table` fallback: full parent-table scan per child record
- **File:line:** `crates/shamir-engine/src/validator/validator_db.rs:261-283`
- **Issue:** when the referenced parent field has no single-field index, each child-row FK
  validation streams the whole parent table (`list_stream` + per-row `record_field_matches`
  with interner lookups). Documented behavior, but nothing warns or wires an index.
- **Failure scenario:** inserting M children whose FK targets an unindexed parent column
  scans O(M·P) parent rows.
- **Suggested fix:** batch the statement's FK values and semi-join against one parent pass,
  or warn/refuse FK rules whose field lacks a ready single-field index.

### 4.18 [LOW] ON-UPDATE RESTRICT gate repeats the per-changed-value scan shape with un-deduped values
- **File:line:** `crates/shamir-engine/src/query/batch/fk_on_update.rs:337-355` (gate),
  `:304-316` (un-deduped `restrict_fields` build)
- **Issue:** same family as 4.3: per-old-value child probes, duplicates included, on the ON
  UPDATE path.
- **Suggested fix:** same as 4.3 — dedupe + single child pass against a membership set.

### 4.19 [LOW] `$contains` ignores the borrowed `str_at` fast path its siblings use
- **File:line:** `crates/shamir-engine/src/query/filter/filter_node.rs:670-684` (contrast
  `Like|Regex` at `:663-668`, `FtsMatch` at `:880-883`)
- **Issue:** `Contains` always `materialize_at` (owned copy) + converts to `QueryValue` per
  row, though the dominant Str-contains-Str case could be served by the borrowed
  `record.str_at(field_path)`.
- **Failure scenario:** substring filter over a 1M-row string column pays a full owned
  String materialization + conversion per row.
- **Suggested fix:** try `str_at` first (borrowed `s.contains(sub)` when the filter value is
  a str literal); fall back to materialize only for containers.

### 4.20 [LOW] `classify_row*` re-parses RecordView per probe per row
- **File:line:** `crates/shamir-engine/src/query/batch/fk_actions.rs:1192-1246`; mirrored in
  `fk_on_update.rs:1017-1079`
- **Issue:** each field probe constructs its own `RecordView::new(bytes)` (map-header walk),
  and a failed view falls back to `Bytes::copy_from_slice` + full msgpack decode per
  (row, probe) pair instead of once per row.
- **Suggested fix:** hoist one `RecordView` (and one fallback decode) per row and match all
  probes against it.

### 4.21 [LOW] `read_history` issues one awaited `history_of` per matched record
- **File:line:** `crates/shamir-engine/src/table/read_temporal.rs:481-488`
- **Issue:** sequential per-record MVCC round-trips on a path whose siblings are batched
  (`read_as_of` uses `get_at_many` at `:218-219`).
- **Failure scenario:** `HISTORY OF` over a WHERE matching 50k rows = 50k serialized store
  awaits.
- **Suggested fix:** add a vectored `history_of_many` mirroring `get_at_many`, chunked at
  the page size.

### 4.22 [LOW] `execute_update_tx`: per-row `table_token()` recompute + probe allocation
- **File:line:** `crates/shamir-engine/src/table/write_exec.rs:664-667`; token derivation
  at `table_manager.rs:15-20`
- **Issue:** inside the matched-rows loop, `tx.write_set.get(&self.table_token())` re-hashes
  the immutable table name (SipHash over the full name) per row, plus a 16-byte
  `id.to_bytes()` allocation just to probe staging.
- **Failure scenario:** UPDATE matching 500k rows → 500k redundant name hashes + staging
  probes that hoisting to two lines before the loop removes.
- **Suggested fix:** hoist `let token = self.table_token(); let staging =
  tx.write_set.get(&token);` above the loop; probe with `id.as_bytes()`.

### 4.23 [LOW] `doctor::repair` deep-clones the entire materialized table per index rebuild
- **File:line:** `crates/shamir-engine/src/table/doctor.rs:600-613`
- **Issue:** `all_records.clone()` per regular/unique index definition — O(D × N) deep
  `InnerValue` clones and transient RAM, while a streaming alternative
  (`create_index_from_stream`) already exists and is used by `create_index`.
- **Failure scenario:** `repair()` on a 50M-row table with 6 indexes holds ~7 full-table
  tree copies in RAM simultaneously.
- **Suggested fix:** use the streaming rebuild for the regular family (keep collect only for
  unique until F-78 lands).

### 4.24 [LOW] DDL op-log cap is documented but never enforced (no-op eviction stub)
- **File:line:** `crates/shamir-engine/src/table/ddl_op_log.rs:106-120` (cap constant at `:26`)
- **Issue:** `maybe_evict_terminal_records` is a permanent `Ok(())` stub; terminal DDL
  status records accumulate one blob per CREATE/DROP/RENAME forever (ingestion rate is
  DDL-only, hence low).
- **Suggested fix:** implement the documented FIFO sweep (keep newest `DDL_OP_LOG_CAP`
  terminal records), triggered at open time and/or post-terminal-write.

### 4.25 [LOW] A8 pre-commit scan re-decodes every staged record value on every commit
- **File:line:** `crates/shamir-engine/src/tx/pre_commit.rs:366-377`
- **Issue:** whenever the tx staged any writes, each staged value is msgpack-decoded again
  (`InnerValue::from_bytes`) to collect referenced interner ids — a second full decode pass
  of the commit payload before Phase 5a writes it. Bounded by the tx's own payload and
  mandated by A8 fail-safety, but large batches pay 2-3 full decode passes per commit.
- **Suggested fix:** capture referenced ids at stage time (when bytes are first encoded into
  staging), amortizing the scan to ~zero.

### 4.26 [LOW] Staged vector payloads deep-cloned twice per vector commit
- **File:line:** `crates/shamir-engine/src/tx/commit_phases.rs:237-277` and
  `promote_vectors` at `:453-508`
- **Issue:** whole `Vec<(RecordId, Vec<f32>)>` embedding lists are cloned once in the delta
  phase and again in promote; bounded by the tx's own staged vectors, but for 1536-dim
  batches the avoidable memcpy is significant.
- **Suggested fix:** borrow from `tx.staged_vectors.get(&token)` across those awaits (the
  map is not otherwise mutated there).

### 4.27 [LOW] Validator registry helpers do full-map scans, one nested inside a loop
- **File:line:** `crates/shamir-engine/src/validator/registry.rs:128-145` (`remove`
  reverse-scan), `:195-218` (`unbind_all_for_table` → `name_for_id` full scan per candidate)
- **Issue:** O(V²) worst case for V validators bound to a table; V is schema-sized so small,
  but the inverse map would make it O(1).
- **Suggested fix:** store the name alongside the artifact (or keep a direct id→name map) so
  `name_for_id` is a lookup.

### 4.28 [LOW] `one_of` rule: linear `Vec::contains` with a materializing clone per record
- **File:line:** `crates/shamir-engine/src/validator/schema/field_rule.rs:264-280` (linear
  `allowed.contains(&actual)` at `:280`, fed by allocating `materialize_as_qv`)
- **Issue:** K-element linear scan (K is user-supplied and unbounded) plus a per-check value
  allocation, per record × per rule.
- **Suggested fix:** precompute a `TFxSet` of allowed values once at validator build time;
  probe with a borrowed scalar.

### 4.29 [LOW] Per-record/per-rule small allocations across the schema validation path (grouped)
- **File:line:** `crates/shamir-engine/src/validator/schema/schema_validator.rs:130`;
  `crates/shamir-engine/src/validator/schema/cross_field.rs:80`;
  `crates/shamir-engine/src/validator/record_fields.rs:93`
- **Issue:** `rule.path.iter()...collect::<Vec<&str>>()` per rule per record (SmallVec
  pattern already exists in `validator_binding.rs:15`); `ViewFields::resolve_path`
  re-collects a fresh `Vec<InternerKey>` and repeats interner lookups on every
  scalar/str/present probe, several times per rule per record.
- **Suggested fix:** SmallVec for path refs; resolve the interner path once per
  (validate-call, field) and reuse.

### 4.30 [NIT] Per-row small-allocation nits (grouped; item 4 merged into 3.7 — counted once)
1. `query/filter/resolve.rs:297-306` — FieldRef cache hit clones the path SmallVec per row
   (inline ≤4 segments, heap beyond); pass the cached slice by reference since
   `materialize_at` only needs `&[InternerKey]`.
2. `query/read/select_projection.rs:231-241` — output-key `String` clone per field per
   record; documented deliberate tradeoff, `Rc<str>` would remove it.
3. `table/table_manager.rs:15-20` + `:992-994` — `table_token()` re-derives a SipHash over
   the immutable table name on every call (2-5× per write op); compute once and store the
   `u64`.
4. `repo/fk_reverse_cache.rs:419-428` — warm-cache lookup clones the parent's
   `Vec<ReverseFkEntry>` (String fields) per hit; `Arc<[ReverseFkEntry]>` values remove it.
5. `repo/repo_types.rs:205-213` — `!names.contains(&disk_name)` per store is
   O(names × stores); bounded (schema-sized), a `TFxSet` flattens it.
6. `validator/registry.rs:237-239` — scc `is_empty()` shares O(N)-flavored cost with the
   annotated `len()` at `:231` but carries no ack (clippy bans only `len`); by pillar-3
   spirit it deserves the same treatment.
7. `validator/native_adapter.rs:56` — legacy adapter pays a full encode→decode `QueryValue`
   round-trip (≥3 allocs) per invocation even on the empty-error accept path; modern
   `NativeRecordValidator` avoids it.
8. `query/auth/session.rs:164-170` — dead `row_filter` loop iterates the whole
   `row_filters` vec producing nothing *(merged into finding 3.7 — same dead loop)*.

**Verified clean (theme-relevant).** No un-annotated `scc::*::len()` in non-test code
anywhere (all four sites carry sound `// O(N) ack:` justifications;
`Drainer::window_depth` is the canonical AtomicUsize mirror). THasher/Fx discipline holds
throughout (no `HashMap::new`/`RandomState` collections). `cond_cache`/`query_ref_cache`/
`field_path_cache` are pointer-keyed, per-query-lifetime structures — no unbounded
cross-query growth. The drainer window, group-commit waiter queue, changefeed footprint,
and FTS/regex precompilation patterns are all correctly bounded.

## 5. api-wire-protocol

Lens verdict: the production wire path (BatchOp → serde-derived DTOs in
`shamir-query-types`, produced by `shamir-query-builder`) is well designed — tagged enums,
`skip_serializing_if` wire stability, fail-closed version byte on the DDL op log.
Builder-only query construction is genuinely enforced: zero `serde_json` in `src/`, ~40
test files import `shamir-query-builder`. The two real weaknesses: a parallel hand-written
parser family speaking a dead wire dialect, and inconsistent application of the crate's own
`MetaEnvelope` versioning convention.

### 5.1 [HIGH] Exported hand-written query parser speaks a dead wire dialect and silently drops query semantics
- **File:line:** `crates/shamir-engine/src/query/read/parser.rs:14-78` (verified),
  `crates/shamir-engine/src/query/common/parser.rs:576-612` (re-exported at
  `query/read/mod.rs:19`, `query/mod.rs:13`)
- **Issue:** two parsers exist for the same logical message. The canonical one
  (`BatchOp::Read` → `qv_to::<ReadQuery>` via serde,
  `shamir-query-types/src/batch/batch_op.rs:288`) accepts pagination as the internally-tagged
  nested object `{"pagination": {"mode": "LimitOffset"|"Page"|"After", ...}}` (builder
  output, pinned by `shamir-query-builder/src/query/tests/query_tests.rs:806-1027`) plus
  `temporal`/`with_version`/`explain`. The engine's public `query_from_value` instead reads
  a top-level `"limit"` key, parses an *untagged* `{"page"|"page_size"|"limit"|"offset"}`
  shape (pinned by `query/read/tests/pagination_tests.rs:211-238`), cannot represent keyset
  `After` pagination at all, and hardcodes `temporal: Latest, with_version: false,
  explain: false` (`parser.rs:74-76`).
- **Failure scenario:** any caller that routes a builder/serde-shaped `ReadQuery` payload
  through this exported function gets `map.get("limit") == None` → `Pagination::None` → the
  `.limit(20)` query returns the **entire table**; an `as_of` temporal read silently becomes
  a `Latest` read; `with_version`/`explain` are silently ignored. In-workspace only tests
  call it today, which is exactly why the drift was never caught — it is a public-API trap
  for the next consumer (SDK, FFI, tooling). (The workspace SUMMARY also carries this as a
  cross-crate item: "shamir-engine + shamir-query-types — two query parsers for one logical
  message".)
- **Suggested fix:** either delete the parser family, or reduce `query_from_value` to a thin
  serde round-trip (`rmp_serde` QueryValue → `ReadQuery`, same as `BatchOp`'s `qv_to`) so
  there is exactly one wire grammar; failing that, `#[doc(hidden)]` + `#[deprecated]` it and
  fix `pagination_from_value` to accept the tagged `pagination` object. Add a differential
  test asserting builder-serialized queries round-trip through it.

### 5.2 [MEDIUM] `pagination_from_value` coerces invalid wire input instead of rejecting it
- **File:line:** `crates/shamir-engine/src/query/common/parser.rs:576-606`
- **Issue:** four separate lenient coercions: (a) `Some(Value::Str(_))` for `limit` falls to
  `_ => None` — a string `"10"` silently means **no limit**; (b) negative ints are cast
  unchecked: `{"limit": -1}` → `(-1i64) as u64` = `u64::MAX` (same for `offset` and `page`);
  (c) a non-Int `page` (e.g. `"2"`) fails the `if let` and silently falls through to the
  limit/offset branch, dropping pagination entirely; (d) when `page` is present,
  `page_size` is required but the error mislabels the field as `limit.page_size` (line 583).
- **Failure scenario:** a client sending `{"limit": "10"}` or `{"limit": -1}` (both
  plausible from dynamic TS) receives an unbounded result set instead of a parse error — a
  correctness *and* DoS-amplification hazard, in the same module that exists to validate the
  wire. Severity was rated "high if the function stays reachable per finding 1" — hence the
  P0 pairing with 5.1.
- **Suggested fix:** type-mismatch → `InvalidType("limit", "integer")`; `i < 0` →
  `InvalidField("limit", "non-negative")` (likewise offset/page/page_size); non-Int `page` →
  error rather than fallthrough; fix the error label to `pagination.page_size`.

### 5.3 [MEDIUM] `MetaEnvelope` convention not applied to three persisted bincode blobs (no version dispatch possible)
- **File:line:** `crates/shamir-engine/src/table/buffer_config.rs:33,47`;
  `crates/shamir-engine/src/migration/shadow_log.rs:79,97,114`;
  `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1388-1393`
- **Issue:** `meta/envelope.rs` documents that *"every persisted `__meta__/*` payload"* is
  wrapped in the versioned `MetaEnvelope` (`magic=SDB2, version u16`), and
  `recovery_marker.rs` / `validator/persistence.rs` honor it. But: `MemBufferConfig` (a
  `MetaKey::BufferConfig` payload, written by DDL today) is raw `bincode::serialize`;
  `ShadowEntry` (crash-recovery-critical — `recover()` reads it on open) is raw bincode; the
  index2 drop tombstone `Vec<(u32, String, Option<String>)>` is raw bincode under
  `RecordId::system("_m.idx.drop")`. `bincode` 1.x is neither self-describing nor versioned,
  and the tombstone tuple has already changed shape once (#1051 added the `Option<String>`
  op_id) — the churn class is real, and each change is a silent old-file hard-failure
  (`Codec` error) with no dispatch point. Contrast `table/ddl_op_log.rs:34,90-95`, which
  does this right (explicit version byte, fail-closed on unknown version).
- **Failure scenario:** `MemBufferConfig` gains a sixth knob (the struct's own doc lists
  five tunables and calls them evolvable) → after upgrade, every table with a persisted
  config fails `buffer_config::load` → `TableManager::create` errors on open; there is no
  version byte to migrate on.
- **Suggested fix:** route all three through `MetaEnvelope` (the key space is already
  reserved), or at minimum prepend the `DDL_OP_LOG_VERSION`-style version byte + migration
  shim in each reader, as `ddl_op_log` already demonstrates.

### 5.4 [LOW] `order_by` parser silently swallows invalid `nulls` values while `order` errors strictly
- **File:line:** `crates/shamir-engine/src/query/common/parser.rs:536-544`
- **Issue:** an unrecognized `order` string errors (`InvalidField("order", "asc or desc")`),
  but an unrecognized `nulls` string (`"middle"`, typo'd `"fist"`) maps to `_ => None` —
  silently "no placement preference". The serde side models this as a `NullsOrder` enum, so
  the hand parser is strictly weaker than the canonical grammar for the same field.
- **Failure scenario:** client typos the nulls placement; rows come back in a different
  order than requested with a 200-OK response; the bug surfaces as an application-level sort
  mystery, not a parse error.
- **Suggested fix:** return `InvalidField("nulls", "first or last")` for unrecognized
  strings.

### 5.5 [LOW] `filter_stream_tests.rs` constructs filters from raw wire maps, not the builder
- **File:line:** `crates/shamir-engine/src/table/tests/filter_stream_tests.rs:81` (and ~30
  more `filter_from_value(&mpack!({...}))` sites in the same file)
- **Issue:** `Cargo.toml:113-115` states the project rule — *"Tests build queries via the
  typed query builder instead of raw wire values"* — and sibling evaluation tests
  (`write_exec_tests.rs`, `fk_*_tests.rs`, `doctor_tests.rs`) do use
  `shamir_query_builder::filter`. `filter_stream_tests` is a filter-*evaluation* suite (its
  subject is streaming eval, not the wire format), so it doesn't fall under the documented
  serde-round-trip exception; it uses the legacy parser as a convenience constructor, which
  also keeps 5.1's dead dialect alive as if it were a supported input path.
- **Suggested fix:** migrate the file to `shamir_query_builder::filter::*`; keep
  raw-`mpack!` construction only in files whose subject is the parser itself
  (`parser_tests.rs`, `query_tests.rs`).

### 5.6 [LOW] Validator-result decoder is strict on `code` but silently lenient on `stop`
- **File:line:** `crates/shamir-engine/src/validator/decode.rs:59-63`
- **Issue:** a non-string `"code"` errors (`NonStringCode`), but a non-bool `"stop"` (e.g.
  `"stop": "yes"` from a WASM guest) silently becomes `false`. The validator's intent to
  halt the chain is lost and later validators still run — the write may be accepted on a
  different basis than the author intended. This is an ABI-convention boundary, so leniency
  here is a correctness hazard, not convenience.
- **Suggested fix:** add a `BadStopType` variant and error, mirroring `NonStringCode`.

### 5.7 [NIT] `ShadowKey`/`MigrationShadowLog` public constructors don't enforce the documented id constraint
- **File:line:** `crates/shamir-engine/src/migration/shadow_key.rs:6-8,53-59`;
  `migration/shadow_log.rs:38-46`
- **Issue:** the key codec's layout (`__shadow_<id>_<lsn_be>`) is documented as safe only
  because *"migration_ids are UUIDs or short ASCII identifiers"*, but production ids are
  `format!("mig_{table}_{ns}_{rand}", ...)`
  (`shamir-db/src/shamir_db/execute/admin_migration.rs:88`) — they embed a user-controlled
  table name and contain `_`. `parse_lsn` never validates the prefix shape, so a
  `_`-bearing id whose bytes prefix another migration's id would make one migration's
  `scan_prefix` match (and `purge` delete) another's entries. Today's trailing `{:08x}`
  random suffix makes an actual prefix collision practically impossible, but the invariant
  is load-bearing and unenforced at the only place that could enforce it.
- **Suggested fix:** validate the id charset (e.g. reject `b'_'`) in `ShadowKey::new` /
  `MigrationShadowLog::new`, or length-prefix the id in the key layout so the constraint
  disappears.

### 5.8 [NIT] *(primary: style-claude-md 7.1)* — `repo/group_commit/mod.rs` contains full implementation logic
- api-wire-protocol flagged the same defect as a nit (spot-checked: all other `mod.rs` in
  `query/*`, `meta/*`, `validator/`, `table/` are re-export-only). Full write-up and fix at
  7.1; counted once.

## 6. error-handling-lifecycle

Lens verdict: generally excellent — zero `panic!`/`todo!`/`unimplemented!` and no `anyhow`
in production code; `thiserror` on the principal error enums; pervasive RAII-guard culture
(`VersionGuard`, `CellReservationGuard`, `WriterDrainGuard`, `WriteBarrierGuard`,
`InFlightCreateGuard`, `OpGuard`, `CascadePathGuard`) with documented release-on-every-exit
semantics; canonical NotFound-vs-real-error discipline that past audits (#435, #881, #891,
#900, #1013) progressively enforced. The residual defects cluster in older code that
predates that discipline, plus fault-injection seams that don't yet reach the specific gaps.

### 6.1 [HIGH] Write-path pre-reads swallow all read errors as "record does not exist" *(also flagged by correctness-tdd #3, rated medium there — counted once)*
- **File:line:** `crates/shamir-engine/src/table/table_manager_crud.rs:431`
  (`delete_returning_version`, verified), `crates/shamir-engine/src/table/table_manager_crud.rs:511`
  (`set_returning_version`, verified), `crates/shamir-engine/src/table/table_manager_tx_ops.rs:1075`
  (`update_tx`, verified)
- **Issue:** `let old_value = self.get(id).await.ok();` (and
  `self.read_one_tx(id, Some(&*tx)).await.ok()`) conflates `Err(DbError::NotFound)` — the
  expected "row absent" signal — with `Storage` I/O errors and `Codec` decode errors (`get()`
  maps a corrupt stored record to `Err(Codec)` via `InnerValue::from_bytes`,
  `table_manager_crud.rs:591-601`). Non-NotFound errors must propagate; only NotFound means
  "absent".
- **Failure scenario:** a corrupt or unreadable stored record turns `delete()` into a silent
  successful no-op returning `(false, 0)` "record did not exist" — the record survives, no
  error is surfaced, index cleanup and history archiving are skipped. `set()`/`update_tx()`
  instead take the create path: `validate_unique_for_create` runs against a row that still
  exists (spurious unique violations, or wrong exclusion), old postings are never removed
  (`plan_insert_ops` instead of `plan_update_ops` — the OLD unique key stays claimed
  forever, permanently blocking future inserts of that value), and the record counter
  double-counts (+1 for an existing row). For `update_tx` the commit-time
  `rederive_stale_value_ops_post_stage` repair only fires when the repo-busy gate passes
  (`pre_commit.rs:1969-1977`) — a quiet repo commits the wrong plan. This is the exact
  fail-open defect class the crate itself fixed elsewhere (F-73 `read_pre_tx_bytes` doc;
  `meta/recovery_marker.rs::load_u64`'s `Err(NotFound) => Ok(None), Err(e) => Err(e)` is the
  canonical pattern; F-65/#891 hardened the sibling `delete_tx`/`read_one_tx_bytes` path to
  fail-closed on exactly this class).
- **Suggested fix:** replace `.ok()` with a match that maps `NotFound` to `None` and
  propagates every other error via `?` (mirror `read_pre_tx_bytes`). Add a
  corrupt-record/se injected-read regression test per site (the
  `TEST_READ_ONE_TX_BYTES_FAILURE` seam already exists for the tx path).

### 6.2 [MEDIUM] Commit-entry `?` on `tx_gate()`/`repo_wal()` bypasses pessimistic-lock release
- **File:line:** `crates/shamir-engine/src/tx/commit.rs:589-590` (`commit_tx_inner`)
- **Issue:** every other early-exit in
  `commit_tx_inner`/`commit_tx_lockfree`/`commit_tx_inner_legacy_async` (lines 574-576,
  580-582, 603-608, 686-689, 929-932, 952-965) explicitly calls
  `release_pessimistic_locks(&tx, repo).await` before returning `Err`. The two `?`s at
  function entry — `let gate = repo.tx_gate().await?; let wal = repo.repo_wal().await?;` —
  do not. Per the crate's own documentation (`batch_execute.rs:66-68`: "TxContext has no
  Drop impl for Level-3 locks"), a `Pessimistic` tx aborted through this path leaks its
  `locked_keys` permanently; younger waiters park in `lock_key` with no timeout (only an
  *older* waiter can wound the holder, and the holder never runs again).
- **Failure scenario:** `repo_wal()` lazily performs real I/O on first touch
  (`create_dir_all`, `SegmentSet::open`, `repo_instance.rs:741-820`); an interactive
  Pessimistic tx created via the engine API (`RepoInstance::begin_tx(Pessimistic)`) whose
  commit is the repo's first WAL touch fails on a permissions/disk error → `?` propagates →
  locks leak → subsequent lockers of those keys hang. (Currently mitigated by the wire layer
  only exposing Snapshot/Serializable for interactive txs, `shamir-db/.../db_tx.rs:83-84` —
  direct embedders of the engine crate are exposed.)
- **Suggested fix:** wrap both resolutions so the `Err` arm calls
  `release_pessimistic_locks(&tx, repo).await` before returning, matching the sibling
  paths. Add a fail-injection test (force `repo_wal()` init failure on a Pessimistic tx;
  assert the key is re-lockable).

### 6.3 [MEDIUM] `RecordCounter` lazy init swallows read and decode errors → durable counter silently zeroed
- **File:line:** `crates/shamir-engine/src/table/record_counter.rs:175-178` (`ensure_cache`)
- **Issue:** `Err(_) => 0` treats *any* info_store read error as "no persisted count", and
  `bincode::from_bytes(&bytes).unwrap_or(0)` treats a corrupt count blob as 0 — both
  silently. `last_persisted` is seeded to 0 alongside, so the first `persist()` after any
  increment sees `cur != last` and `write_through(cur)` **overwrites** the previously
  durable count (e.g. 10 000 → 1). This contradicts the same crate's interner policy
  (`interner_manager.rs:143-151`, audit §2.6: corruption is fatal, not skippable, precisely
  because silently-truncated state is worse than a failed open).
- **Failure scenario:** transient I/O error on the counter key at first `count()` after open
  → in-memory count = 0 → next write increments and persists 1 → durable count destroyed
  until an operator runs `doctor.repair()` (which does reconcile via full scan — the
  mitigation that keeps this medium, not high).
- **Suggested fix:** distinguish `NotFound` (→ 0) from other errors (→ propagate `Err`); on
  decode failure either fail or at least `log::warn!` and skip the write-back (never persist
  a value derived from a defaulted init). Add tests for both error classes.

### 6.4 [MEDIUM] FK parent-value scans silently skip rows that fail to decode
- **File:line:** `crates/shamir-engine/src/query/batch/fk_actions.rs:661`, `:1142`
  (`collect_parent_values`), `crates/shamir-engine/src/query/batch/fk_on_update.rs:845`,
  `crates/shamir-engine/src/query/batch/fk_restrict.rs:270`; same class in
  `crates/shamir-engine/src/validator/validator_db.rs:98-108`
  (`record_field_matches_by_id` returns `false` on decode failure)
- **Issue:** `if let Ok(view) = RecordView::new(&bytes) { ... }` drops the ref-field values
  of any matched parent/grandchild row whose bytes fail to decode — no error, no log, no
  corrupt-record counter. F-65 (#891) fixed the *read-error* half of exactly this defect
  class in these same files (its module doc names "storage error, and decode error" as the
  outcomes that must abort), but the *decode-error* half remains swallowed at the
  value-collection layer.
- **Failure scenario:** one corrupt parent row in a CASCADE / SET NULL / ON UPDATE fan-out:
  its ref-field values silently vanish from the collected set → the corresponding children
  are never visited → a silently-shrunk RI action set with a success result — the precise
  "RI violation with no error surfaced" F-65's doc describes. (The read path reports corrupt
  rows via `CorruptRecordRef` (F-10); these FK scans have no equivalent.)
- **Suggested fix:** on `RecordView::new` failure, fall back to `InnerValue::from_bytes` (as
  `record_field_matches_by_id` already tries) and only then return `Err(DbError::Codec(...))`
  — fail closed, mirroring `read_pre_tx_bytes`. Extend
  `fk_indexed_action_read_error_tests.rs`-style injection to cover a decode-corrupt matched
  row.

### 6.5 [MEDIUM] `per_table_mvcc` attach discards the error signal that indicates split-brain
- **File:line:** `crates/shamir-engine/src/repo/repo_instance.rs:399` (`create_table_context`)
- **Issue:** `let _ = self.per_table_mvcc.insert_sync(token, Arc::clone(&mvcc));` —
  `scc::HashMap::insert` returns `Err` when the token is already present, and
  `remove_table`'s own A13 comment (lines 496-510) states that a stale entry at attach time
  means "a split-brain where committed transactions silently vanish" (the commit pipeline
  resolves the MvccStore *by token through this map*, while the new `TableManager` reads
  through its own store). Discarding the `Err` hides exactly that condition instead of
  surfacing it.
- **Failure scenario:** `remove_table(X)` racing a `get_table(X)` init (the shared
  `OnceCell` is removed by `remove_table`, so a second init can run): the second
  `insert_sync` fails silently, the map keeps the *old* store, and all subsequent commits
  for the re-created table write into the detached store.
- **Suggested fix:** on `Err((_old, _new))`, `log::error!` and/or return `DbError::Internal`
  (fail the open) — the presence of the error is itself the invariant-violation signal A13
  documents.

### 6.6 [LOW] Background `verify` never clears its single-flight latch if the task panics
- **File:line:** `crates/shamir-engine/src/table/table_manager.rs:946-969` (`bump_write_counter`)
- **Issue:** the spawned task clears `verify_running` only on the normal path
  (`self_clone.verify_running.store(false, ...)` at line 968). A panic inside `verify()` is
  swallowed by the un-awaited `JoinHandle`, so the CAS latch stays `true` forever —
  background consistency verification is permanently disabled for that table with zero
  signal. (Same panic-unsafe-latch pattern as 1.4.)
- **Failure scenario:** any panic in `verify()` (e.g. an unwrap on unexpected state during a
  scan) silently kills all future background verifies; inconsistencies the gauge exists to
  catch go unreported.
- **Suggested fix:** wrap the body so the flag is reset in a `Drop` guard (or a
  `scopeguard`-style local), or `std::panic::AssertUnwindSafe(...).catch_unwind()` with a
  `log::error!` on the JoinError path.

### 6.7 [LOW] Silent best-effort operations: errors dropped without even a log line
- **File:line:** `crates/shamir-engine/src/table/table_manager.rs:703` and
  `crates/shamir-engine/src/table/doctor.rs:716` (`let _ = save_index2_metadata(...)`),
  `crates/shamir-engine/src/tx/recovery.rs:180` and `:231` (`if let Ok(tbl) =
  repo.get_table(&name).await` in broadcast replay),
  `crates/shamir-engine/src/tx/commit_phases.rs:345-370` (`tx_gate()` failure → `ok = false`
  with no log)
- **Issue:** these are legitimate best-effort choices (documented as such in comments), but
  unlike every sibling best-effort site in the crate (e.g. `flush_buffers`' first-error
  pattern, `replay_v2_op`'s warn-on-skip, the DDL op-status writers' loud `log::error!` at
  `table_manager_index_mgmt.rs:1129-1140`), these drop the error with no observability at
  all. In `recovery.rs`, a table whose `get_table` fails during broadcast
  `IndexPut`/`IndexDel` replay is skipped silently while the neighboring "token not found"
  branches warn — an operator debugging a missing posting replay gets no trace.
- **Suggested fix:** add a `log::warn!`/`log::error!` at each site (the messages can state
  the accepted consequence, as the DDL sites already do).

### 6.8 [LOW] `apply_replicated` conflates attach failure with "unattached table"
- **File:line:** `crates/shamir-engine/src/tx/apply_replicated.rs:208`
- **Issue:** `let _ = repo.get_table(&table_name).await;` intentionally ignores NotFound
  ("table not configured on this follower"), but it also ignores every other error (store
  I/O, open-time index recovery failure). A non-NotFound failure falls through to
  `mvcc_found == None` → the direct `base.transact` branch — precisely the non-MVCC write
  path the attach exists to prevent (the R1-d divergence: replication writes invisible to
  subsequent MVCC reads, worked around once before with "a throwaway SELECT").
- **Failure scenario:** follower whose `TableManager::create` fails on one index's recovery
  during replication apply: event data lands in `__data__` only, MVCC reads never see it,
  and the bookmark advances so re-delivery won't fix it.
- **Suggested fix:** match on the error: `NotFound` → proceed to the unattached branch;
  anything else → propagate (the caller must not advance the watermark — the function's own
  idempotency contract supports retry).

### 6.9 [LOW] Missing error-path tests for the specific gaps above
- **File:line:** `crates/shamir-engine/src/query/batch/tests/executor_tests/error_handling_tests.rs`
  (covers only planner-level errors: circular dependency, unknown table, id echo); no tests
  at the sites in 6.1-6.5
- **Issue:** the crate has a strong fault-injection culture (`SHAMIR_TEST_CRASH_AFTER`
  seams, `TEST_READ_ONE_TX_BYTES_FAILURE`, `TEST_REDERIVE_PRE_TX_READ_FAILURE`,
  `FAIL_HISTORY_SEED_TX_ID`, the `test-util` feature, `p967`/`r0d`/`f73` fail-closed
  suites) — but none of it reaches: (a) the `delete`/`set`/`update_tx` pre-read `.ok()`
  conflation, (b) commit-entry lock release on `tx_gate`/`repo_wal` init failure, (c)
  `RecordCounter` init read/decode errors, (d) decode-corrupt rows inside FK parent-value
  collection. Each of those paths would fail silently today even though deterministic
  injection seams for adjacent sites already exist.
- **Suggested fix:** one regression test per fixed finding, reusing the existing seam
  conventions (a corrupt-blob fixture for the counter; the `read_one_tx_bytes` injector for
  the tx-path pre-read; a `get_table`-failing store double via `install_table_for_test`
  where needed).

### 6.10 [NIT] `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` repeated 10×
- **File:line:** `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1135, 1215,
  1263, 1367, 1570, 1765, 1858, 2274, 2435, 2636`
- **Issue:** panics on a pre-1970 system clock; `migration/coordinator.rs:62` already uses
  the safe `.unwrap_or(0)` form for the identical computation.
- **Suggested fix:** extract one `unix_millis()` helper using the safe form.

### 6.11 [NIT] `ids.lock().unwrap()` poisoning cascade
- **File:line:** `crates/shamir-engine/src/table/in_flight_create_guard.rs:100, 113, 143`
- **Issue:** the `std::sync::Mutex` itself is the documented sanctioned DDL exception (inline
  justification present) and the critical sections are panic-free BTreeMap ops, so
  poisoning is unreachable in practice — but a panic anywhere in a future edit of these
  sections would cascade into `degraded_index_count()` panics.
- **Suggested fix:** `.lock().unwrap_or_else(|p| p.into_inner())` (state is always
  structurally valid) or a brief comment noting the invariant.

### 6.12 [NIT] `QueryParseError` / `WriteValueError` lack `thiserror`
- **File:line:** `crates/shamir-engine/src/query/common/parser.rs:17-18` (hand-written
  `Display` + `impl std::error::Error`, so functionally compliant) and
  `crates/shamir-engine/src/query/batch/param_subst.rs:145-146` (`pub(super)`, stringified
  at the boundary)
- **Issue:** cosmetic deviation from CLAUDE.md's "`thiserror` for library error enums"; no
  behavioral impact.
- **Suggested fix:** mechanical conversion when either file is next touched.

**Verified non-issues (checked, no finding).** All `unwrap()`/`expect()` sites in production
code outside the above are structurally guarded invariants verified at their guard site
(`session.rs:196,238-254,626-628`, `read_exec.rs:1056-1058` / `read_temporal.rs:184-186`,
`order.rs:95,295`, `query_runner.rs:512`, `table_manager_streaming.rs:725,785`,
`tx_scan_overlay.rs:213,231`, `fk_*: get_mut(field).unwrap()`,
`interner_manager.rs:154,224` / `record_counter.rs:183`, `format.rs` regexes,
`writer_drain_barrier.rs:444`). `let _ =` on scc map ops (`drainer.rs:592,710`,
`validator/registry.rs`, `repo_instance.rs:493,510,583`) are idempotent or benign-by-contract
with inline justification. Resource lifecycles are sound and documented (drainer/group-commit/
`spawn_gc_task`/watchdog exits; `WriteBarrierGuard` drop-order; `InFlightCreateGuard`
refcounting; `BackgroundCommitHandle` panic → `Deferred`; `flush_buffers`/`flush_all_history`
first-error pattern; crash seams use `process::abort` with `#[cfg(debug_assertions)]`
gating).

## 7. style-claude-md

Lens verdict: structural conformance is strong but not clean — 15 of the crate's 16
`mod.rs` files are re-export-only and the per-module `tests/` layout is otherwise exemplary,
yet three explicit CLAUDE.md rules are violated (one of them at scale).

### 7.1 [HIGH] `mod.rs` contains a full implementation, not re-exports *(also flagged by api-wire-protocol #8, as a nit — counted once)*
- **File:line:** `crates/shamir-engine/src/repo/group_commit/mod.rs:1-128` (verified: 128 lines)
- **Issue:** CLAUDE.md: *"mod.rs files contain re-exports only. Types and logic live in
  sibling files."* This file implements `GroupCommit`, `GcState`, `run()`, `leader_loop()`,
  and `recv()` (125 lines of logic plus the `mod tests;` decl). It is the only `mod.rs` in
  the crate with logic — every other one (`table/`, `tx/`, `repo/`, `validator/`, `meta/`,
  `migration/`, `db_instance/`, `index/`, `query/*`) complies.
- **Failure scenario:** the precedent invites the next module to grow logic in its `mod.rs`;
  diffs against this file mix structural moves with behavioural edits, diluting `git blame`
  exactly as the rule intends to prevent. (Practical coupling: 1.4's panic-safety fix and
  4.x-adjacent changes all land in this file.)
- **Suggested fix:** move the implementation to a sibling `group_commit.rs` (module dir
  keeps `mod.rs` with `mod group_commit; pub use group_commit::GroupCommit;`, adding
  `#[allow(clippy::module_inception)]` — same precedent as `table/table.rs` and
  `db_instance/db_instance.rs`). Style-only commit per the CLAUDE.md sweep rule.

### 7.2 [HIGH] Systemic mid-function `use` imports (~25 sites, ~15 production files)
- **File:line:** representative sites: `migration/shadow_log.rs:50,109,129` (the *same*
  `use futures::StreamExt;` repeated inside three different fns); `table/table_manager.rs:16`
  (`table_token_for`), `:935`, `:975` (`use std::sync::atomic::Ordering`), `:1582`;
  `table/read_exec.rs:1717-1719, 1774-1778`; `table/read_planner.rs:38-39, 78`;
  `table/read_index_scan.rs:412, 508`; `table/table_manager_index_mgmt.rs:37-40`;
  `table/table_manager_sorted_index.rs:48`; `table/table_manager_tx_ops.rs:94`;
  `table/table_manager_validators.rs:316`; `tx/commit.rs:832-833`;
  `tx/commit_phases.rs:889` (`:566` is inside a `#[cfg(test)]` block — borderline under the
  cfg exception); `query/batch/fk_actions.rs:1284`; `query/batch/fk_on_update.rs:1092`;
  `query/filter/compile.rs:196`; `query/filter/eval_bytes.rs:656`;
  `query/read/parser.rs:178` (import nested inside a match arm);
  `validator/validator_db.rs:117`; `validator/schema/cross_field.rs:37` (`use CompareOp::*`),
  `:118`; `repo/repo_instance.rs:1702`
- **Issue:** CLAUDE.md "Imports at the top" bans `use` inside function/block bodies outside
  three documented exceptions (test-mod `use super::*`, trait-name collision with a comment,
  cfg-gated bodies). None of these sites fit an exception.
- **Failure scenario:** hidden per-function dependencies: a reader scanning the file header
  misses that `shadow_log.rs` methods need `StreamExt`, and the same import is already
  duplicated three times in that one file — the pattern propagates by copy-paste.
- **Suggested fix:** hoist all listed imports to file headers in one `style:` commit (some
  may need no other change; `use CompareOp::*` at top of `cross_field.rs` is safe since
  `CompareOp` is defined there). Flag `parser.rs:178` (match-arm-nested import) as the most
  misleading of the set.

### 7.3 [MEDIUM] Inline `#[cfg(test)] mod tests` embedded in implementation files *(also flagged by correctness-tdd #8, rated low there — counted once)*
- **File:line:** `crates/shamir-engine/src/query/read/hashable_query_value.rs:250-379`;
  `crates/shamir-engine/src/table/writer_drain_barrier.rs:410-534`
- **Issue:** CLAUDE.md test-organisation rule 5: "Never embed `#[cfg(test)] mod tests
  { ... }` inline inside implementation files. Move them to the `tests/` directory."
  `hashable_query_value.rs` is ~34% inline tests (129 of 379 lines) even though
  `query/read/tests/` already exists; `writer_drain_barrier.rs` carries ~124 lines of inline
  tests (a sibling `#[cfg(loom)] mod loom_model` at `:535` also exists, but that one is a
  deliberate, `build.rs`-coupled model-checker module with documented rationale — a
  defensible cfg-gated exception; the plain `#[cfg(test)] mod tests` is not).
- **Failure scenario:** test and impl edits collide in one file's history; the inline block
  grows unbounded because the `tests/` split discipline is invisible at the point of editing.
- **Suggested fix:** move to `query/read/tests/hashable_query_value_tests.rs` and
  `table/tests/writer_drain_barrier_tests.rs` (manifest entries added to the respective
  `tests/mod.rs`), converting `use super::*` to explicit `crate::…` paths. Keep the loom
  module where it is.

### 7.4 [LOW] Test manifests deviate from the documented `pub mod` form and duplicate cfg gating
- **File:line:** `crates/shamir-engine/src/repo/tests/mod.rs:1-16`;
  `crates/shamir-engine/src/repo/group_commit/tests/mod.rs:1-4`;
  `crates/shamir-engine/src/query/*/tests/mod.rs` (private `mod` decls); mixed forms in
  `query/read/tests/mod.rs:3-7` and `query/batch/tests/executor_tests/mod.rs`
- **Issue:** CLAUDE.md prescribes the manifest form `pub mod value_tests;`. The `query/**`
  trees use private `mod x_tests;` instead, two manifests mix both forms in the same file,
  and `repo/tests/mod.rs` + `group_commit/tests/mod.rs` add a redundant `#[cfg(test)]` to
  every line although the parent (`repo/mod.rs:9-10`) already gates the whole `tests`
  module. Spirit (manifest-only, no test code) is honored everywhere; the form is
  inconsistent across siblings (`table/`, `tx/`, `validator/`, `meta/`, `migration/`,
  `db_instance/` all use the documented `pub mod` form).
- **Failure scenario:** none functional; drift makes the "which tests exist" grep different
  per subtree (`pub mod` vs `mod` changes what `cargo doc`/IDE exposes and what a
  `pub`-visibility grep finds).
- **Suggested fix:** one `style:` commit normalising manifests to `pub mod x_tests;` and
  dropping the redundant per-line `#[cfg(test)]`.

### 7.5 [NIT] Test files missing the `_tests` suffix
- **File:line:** `crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check.rs`,
  `p1097_remove_posting_owner.rs`, `p1100_stale_snapshot_delete_posting.rs`,
  `p1101_released_skip_durable_check.rs`; `crates/shamir-engine/src/table/tests/f53b_step3_cursor_after_spike.rs`
- **Issue:** CLAUDE.md's test layout prescribes one `*_tests.rs` file per topic; ~95% of the
  crate's test files follow it, these five don't (helper files like `test_helpers.rs` /
  `stream_utils.rs` / `helpers.rs` are correctly exempt — they aren't test files).
- **Suggested fix:** rename in a `style:` commit (git-mv to preserve history); add the
  `_tests` suffix to the manifest entries.

### 7.6 [LOW] Test file nests a redundant `#[cfg(test)] mod tests` and splits helpers from tests
- **File:line:** `crates/shamir-engine/src/query/batch/tests/watchdog_tests.rs:146-163`
- **Issue:** the file defines `TestResolver` + `setup_resolver()` at top level, then wraps
  the actual `#[test]` fns in a nested `#[cfg(test)] mod tests { use super::*; … }`. The
  whole file is already test-gated by the parent manifest chain (`query/batch/mod.rs:187-188`
  → `tests/mod.rs`), so the inner `cfg` is dead and the helper/test split is arbitrary —
  unlike sibling test files, which put helpers and tests at one level.
- **Failure scenario:** copy-paste template risk: new test files imitate the nested form,
  spreading a second layout convention.
- **Suggested fix:** drop the inner `mod tests` wrapper (hoist its contents to file level)
  or move `TestResolver`/`setup_resolver` next to the tests they serve.

### 7.7 [NIT] Tail-of-file `pub use` re-exports outside `mod.rs`
- **File:line:** `crates/shamir-engine/src/query/batch/query_runner.rs:1856-1861`
- **Issue:** a comment "Re-export public items used outside this module" introduces
  `pub use crate::query::batch::batch_execute::execute_batch;` (plus a `#[cfg(test)]`
  sibling and the interactive-tx trio) at the bottom of an impl file, creating a second
  valid path (`…batch::query_runner::execute_batch`) alongside the canonical
  `query/batch/mod.rs:159-161` re-export of the same names. The crate's own convention (and
  CLAUDE.md) keeps re-exports in `mod.rs`.
- **Suggested fix:** fold into the existing `pub use query_runner::{…}` block in
  `query/batch/mod.rs` and delete the tail block.

### 7.8 [NIT] `repo_types.rs` stretches "one file = one primary export" to 11 public types
- **File:line:** `crates/shamir-engine/src/repo_types.rs:28-377`
- **Issue:** `BoxRepo` + 3 composites + `RepoFactory` trait + 5 factory types/enums live in
  one file. They form one conceptual family (repo backend + its factory variants), so this
  is defensible under the "closely-coupled group" clause — but it is the largest export
  surface in a single non-mod file in the crate, and the composites vs. factories split is a
  natural seam.
- **Suggested fix:** optional: split composites (`BoxRepo` + `*RepoComposite`) from
  factories (`RepoFactory` + `*RepoFactory`) when the file is next touched for substance;
  not worth a dedicated churn commit.

---

## Finding counts

Raw lens-tagged totals across the 7 source files (matches the workspace SUMMARY's
pre-dedup row `0 | 12 | 23 | 33 | 19 | 87` exactly):

| Severity | Lens-tagged findings | Distinct defects after dedup | Dedup groups (severity carried by the group) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 12 | 11 | 1.1+4.4 (changelog unbounded buffer) · 1.3+6.1 (`.ok()` pre-reads) · 1.5+4.1 (rederive quadratic) · 5.8+7.1 (`mod.rs` implementation) |
| medium | 23 | 21 | 1.8+7.3 (inline test modules) |
| low | 33 | 31 | 1.7+3.7+perf-nit#8 (SessionPermissions) · 1.9+2.3 (RecordCounter dirty race) |
| nit | 19 | 16 | (members of the groups above, absorbed) |
| **total** | **87** | **79** | 7 cross-lens dedup groups covering 15 lens-tagged findings |

Deduplicated defect census: **0 critical, 11 high, 21 medium, 31 low, 16 nit = 79 distinct
defects** (87 lens-tagged findings). Severity of each dedup group = the highest severity any
lens assigned it (e.g. correctness-tdd rated the `.ok()` pre-reads medium and the rederive
scans medium; error-handling/performance rated the same defects high — consolidated high).

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Make the drainer warm path actually reconcile (1.2).** In Phase B, route batches for
   tables absent from `per_table_mvcc` through `replay_v2_op` (or mark them in
   `failed_tables` so Phase C refuses to finalize); correct the false Phase B comment; add
   the `drain_step` Red test with a hand-built entry whose `Put` targets an unattached
   token. Closes **1.2** — the crate's one silent-permanent-data-loss defect.
2. **Fail closed on write-path pre-read errors (6.1/1.3).** Replace the three `.ok()`s with
   NotFound-only mapping + `?` propagation; regression tests via the existing
   `TEST_READ_ONE_TX_BYTES_FAILURE`-style seams. Closes **6.1/1.3** (stale unique postings,
   counter drift, silent no-op deletes).
3. **Drop the DashMap guards before `.await` in `DbInstance` (2.1).** Clone the
   `RepoInstance` out at all eight sites (pattern already proven by `get_repo:109-111`).
   Closes **2.1** — removes the runtime-wedge/hang class on a path every query takes.
4. **Bound `range_from` (1.1/4.4).** Early-exit the collect loop at `limit` (+ batch slack)
   instead of buffering the whole journal tail; bounded-read test with a counting store
   double. Closes **1.1/4.4** (replication catch-up OOM).
5. **One wire grammar for queries (5.1 + 5.2 + 5.4).** Delete the hand-written parser family
   or reduce `query_from_value` to a serde round-trip; if kept temporarily,
   `#[doc(hidden)]` + `#[deprecated]` it, fix the coercions (`"10"`/`-1`/non-Int `page`),
   and migrate `filter_stream_tests.rs` to the builder. Closes **5.1, 5.2, 5.4, 5.5** —
   removes the "`.limit(20)` returns the whole table" public-API trap.

**P1 — soon**

6. **Quadratic hot-path cluster.** Hoist `staged_removals_by_rid`/dedup keys out of the
   rederive loops (**4.1**/1.5); non-materializing `for_each_op` staged probe (**4.2**);
   dedupe + single-pass FK RESTRICT/ON-UPDATE scans (**4.3, 4.18**); route ON UPDATE
   discovery through `fk_reverse_cache` behind the intersection gate (**4.5**). Closes
   **4.1, 4.2, 4.3, 4.5, 4.18**.
7. **Panic-safe latches.** RAII/unwind guard for the group-commit leader's `leader_busy`
   (**1.4**) and the background-verify `verify_running` CAS latch (**6.6**) — same pattern,
   two sites; tests with a panicking flush/verify. Closes **1.4, 6.6**.
8. **Atomic validator binding (2.2).** Single `entry_sync(..).or_insert_with(..)` critical
   section in `ValidatorRegistry::add_binding`. Closes **2.2**.
9. **Complete the untrusted-input hardening.** Extend `validate_filter_depth` to `when` +
   `having` + iterative `FilterValue` walk (**3.1**); cap pattern length and turn invalid
   regex/like into coded errors (**3.6**); thread `CondCache` through the WHERE compile path
   (**3.2**). Closes **3.1, 3.2, 3.6**.
10. **Restore error signals on lifecycle paths.** Release pessimistic locks on the
    commit-entry `?`s (**6.2**); `RecordCounter::ensure_cache` NotFound-vs-error split
    (**6.3**); FK scans fail closed on decode-corrupt rows (**6.4**); `per_table_mvcc`
    attach `Err` → log/error (**6.5**); `apply_replicated` distinguishes attach failure from
    unattached (**6.8**). Closes **6.2, 6.3, 6.4, 6.5, 6.8** — with one regression test per
    site (**6.9**).
11. **Version the three raw-bincode blobs (5.3).** `MemBufferConfig`, `ShadowEntry`, and the
    index2 drop tombstone through `MetaEnvelope` (or a `DDL_OP_LOG_VERSION`-style byte +
    shim). Closes **5.3**.
12. **Gate the test-only RBAC scaffolding (3.7/1.7).** `#[cfg(test)]`-gate
    `SessionPermissions` (or move to test-support), delete the dead `row_filter` loop,
    replace the `table_ref()` unwraps. Closes **3.7** (and the perf-nit duplicate).

**P2 — backlog**

13. **Per-clone/per-row batch-path costs:** hoist unique defs in tx inserts (**4.6**), drop
    the attempt-1 Phase 5a clone (**4.7**), `$in` borrow-probe (**4.8**), `$contains_all`
    scratch (**4.9**), eval_bytes raw-compare arms (**4.10**), drainer Phase A per-table
    posting batches (**4.11**), ForEach plan-once (**4.12**), shadow-log range-scan + page
    cap (**4.13**), `set_many`/`remove_many` shadow drain (**4.14**), index2 backends hoist
    (**4.15**).
14. **Perf low/nit tail:** purge shadow log on commit (**4.16**); FK parent-scan
    semi-join/warn (**4.17**); `$contains` `str_at` fast path (**4.19**); per-row
    `RecordView` hoist (**4.20**); vectored `history_of_many` (**4.21**); hoist token/staging
    probes (**4.22**); streaming doctor rebuild (**4.23**); DDL op-log FIFO sweep (**4.24**);
    stage-time interner-id capture (**4.25**); vector borrow (**4.26**); registry inverse map
    (**4.27**); `one_of` TFxSet (**4.28**); validator-path SmallVec/path-reuse (**4.29**);
    the seven standalone perf nits (**4.30** items 1-3, 5-7).
15. **Security/architecture hardening:** type-level `Authorized<BatchRequest>` seam or
    enforcing `trace_access` sibling (**3.3**); REPLICATION.md threat-model precondition +
    optional validate-on-apply (**3.4**); borrowed-key newtype for the pointer-keyed caches
    (**3.5**).
16. **Concurrency nits:** collect watchdog triples then log (**2.4**); drain_until_caught_up
    pass budget (**2.5**); FkReverseCache retry note (**2.6**).
17. **API nits:** `BadStopType` for validator `stop` (**5.6**); `ShadowKey` id-charset
    validation or length-prefixed layout (**5.7**).
18. **Lifecycle nits:** `log::warn!` at the silent best-effort sites (**6.7**); shared
    `unix_millis()` helper (**6.10**); poisoning-tolerant lock (**6.11**); `thiserror` for
    the two hand-written error enums (**6.12**).
19. **Style sweep (one or two `style:` commits):** move `GroupCommit` to a sibling file
    (**7.1**/5.8); hoist ~25 mid-function imports (**7.2**); relocate the two inline test
    modules (**7.3**/1.8); normalize test manifests to `pub mod` + drop redundant cfg
    (**7.4**); `_tests` renames (**7.5**); flatten `watchdog_tests.rs` wrapper (**7.6**);
    fold the tail `pub use` (**7.7**); optional `repo_types.rs` split (**7.8**).
