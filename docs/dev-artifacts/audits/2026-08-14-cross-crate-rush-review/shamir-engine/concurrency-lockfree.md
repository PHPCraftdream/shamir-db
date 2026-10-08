<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-engine — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The guard, binding, counter and watchdog findings are repaired. FK-cache retry now yields but remains unbounded under invalidation.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 12 | 0 | 5 | 1 | 0 | 1 | 5 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — DashMap shard read-guard held across await in DbInstance accessors

Status: `fixed`. Current risk: —.

All eight awaited accessors clone RepoInstance before awaiting, ending the shard guard's lifetime at the lookup statement.

Evidence: [crates/shamir-engine/src/db_instance/db_instance.rs:70](../../../../../crates/shamir-engine/src/db_instance/db_instance.rs#L70); [crates/shamir-engine/src/db_instance/db_instance.rs:191](../../../../../crates/shamir-engine/src/db_instance/db_instance.rs#L191); [crates/shamir-engine/src/db_instance/db_instance.rs:298](../../../../../crates/shamir-engine/src/db_instance/db_instance.rs#L298).

<a id="review-2"></a>

### Claim 2 — ValidatorRegistry::add_binding — check-then-act lost update on a lock-free map

Status: `fixed`. Current risk: —.

One entry_sync/or_default/get_mut operation inserts into the existing set without a separate losing insert.

Evidence: [crates/shamir-engine/src/validator/registry.rs:190](../../../../../crates/shamir-engine/src/validator/registry.rs#L190); [crates/shamir-engine/src/validator/tests/registry_tests.rs:43](../../../../../crates/shamir-engine/src/validator/tests/registry_tests.rs#L43); [crates/shamir-engine/src/validator/tests/mod.rs:8](../../../../../crates/shamir-engine/src/validator/tests/mod.rs#L8).

<a id="review-3"></a>

### Claim 3 — RecordCounter — dirty flag clobbered by concurrent increment during set/persist awaits

Status: `fixed`. Current risk: —.

No independent dirty flag remains. set/persist record only their written count; later cache divergence causes another persist.

Evidence: [crates/shamir-engine/src/table/record_counter.rs:103](../../../../../crates/shamir-engine/src/table/record_counter.rs#L103); [crates/shamir-engine/src/table/record_counter.rs:169](../../../../../crates/shamir-engine/src/table/record_counter.rs#L169); [crates/shamir-engine/src/table/tests/record_counter_tests.rs:374](../../../../../crates/shamir-engine/src/table/tests/record_counter_tests.rs#L374).

<a id="review-4"></a>

### Claim 4 — Watchdog thread runs log::warn! inside iter_sync

Status: `fixed`. Current risk: —.

The iterator collects warning data; log calls run after iter_sync returns.

Evidence: [crates/shamir-engine/src/query/batch/op_watchdog.rs:139](../../../../../crates/shamir-engine/src/query/batch/op_watchdog.rs#L139); [crates/shamir-engine/src/query/batch/op_watchdog.rs:156](../../../../../crates/shamir-engine/src/query/batch/op_watchdog.rs#L156).

<a id="review-5"></a>

### Claim 5 — MigrationCoordinator::drain_until_caught_up — unbounded catch-up loop under sustained writes

Status: `fixed`. Current risk: —.

The loop has a 32-pass cap and returns residual lag. Its admin caller currently ignores that residual result.

Evidence: [crates/shamir-engine/src/migration/coordinator.rs:126](../../../../../crates/shamir-engine/src/migration/coordinator.rs#L126); [crates/shamir-engine/src/migration/coordinator.rs:288](../../../../../crates/shamir-engine/src/migration/coordinator.rs#L288); [crates/shamir-db/src/shamir_db/execute/admin_migration.rs:178](../../../../../crates/shamir-db/src/shamir_db/execute/admin_migration.rs#L178).

<a id="review-6"></a>

### Claim 6 — FkReverseCache::get_or_build_by_parent — unbounded CAS-loss retry while holding build_lock

Status: `partially-fixed`. Current risk: `nit`.

CAS losses now yield cooperatively, but retries remain unlimited and build_lock remains held; continuous invalidation can still starve waiters.

Evidence: [crates/shamir-engine/src/repo/fk_reverse_cache.rs:354](../../../../../crates/shamir-engine/src/repo/fk_reverse_cache.rs#L354); [crates/shamir-engine/src/repo/fk_reverse_cache.rs:369](../../../../../crates/shamir-engine/src/repo/fk_reverse_cache.rs#L369); [crates/shamir-engine/src/repo/fk_reverse_cache.rs:384](../../../../../crates/shamir-engine/src/repo/fk_reverse_cache.rs#L384).

<a id="review-positive-conformance-notes-lock-inventory"></a>

### Claim Positive conformance notes/Lock inventory — Only sanctioned explicit synchronous mutexes

Status: `not-applicable`. Current risk: —.

Explicit synchronous mutex fields remain DDL-only or test-gated. This does not make DashMap or scc operations structurally lock-free.

Evidence: [crates/shamir-engine/src/table/in_flight_create_guard.rs:79](../../../../../crates/shamir-engine/src/table/in_flight_create_guard.rs#L79); [crates/shamir-engine/src/tx/pre_commit.rs:1037](../../../../../crates/shamir-engine/src/tx/pre_commit.rs#L1037); [crates/shamir-engine/src/table/table_manager_streaming.rs:77](../../../../../crates/shamir-engine/src/table/table_manager_streaming.rs#L77).

<a id="review-positive-conformance-notes-lock-ordering"></a>

### Claim Positive conformance notes/Lock ordering — Canonical token ordering and DDL drain-before-lock protocol

Status: `not-applicable`. Current risk: —.

Sorted/deduplicated table tokens and the centralized barrier protocol remain present; this is source conformance, not executed concurrency proof.

Evidence: [crates/shamir-engine/src/tx/pre_commit.rs:527](../../../../../crates/shamir-engine/src/tx/pre_commit.rs#L527); [crates/shamir-engine/src/table/writer_drain_barrier.rs:74](../../../../../crates/shamir-engine/src/table/writer_drain_barrier.rs#L74).

<a id="review-positive-conformance-notes-memory-model"></a>

### Claim Positive conformance notes/Memory model — SeqCst proof and loom coverage

Status: `unverified`. Current risk: —.

The proof, packed flag and opt-in model exist. The model samples drain-return state but explicitly does not distinguish production atomic orderings; external loom behavior was not verified.

Evidence: [crates/shamir-engine/src/table/writer_drain_barrier.rs:448](../../../../../crates/shamir-engine/src/table/writer_drain_barrier.rs#L448); [crates/shamir-engine/src/table/writer_drain_barrier.rs:550](../../../../../crates/shamir-engine/src/table/writer_drain_barrier.rs#L550); [crates/shamir-engine/build.rs:12](../../../../../crates/shamir-engine/build.rs#L12).

<a id="review-positive-conformance-notes-scc-len-discipline"></a>

### Claim Positive conformance notes/scc len() discipline — Annotated traversals and atomic cardinality mirrors

Status: `not-applicable`. Current risk: —.

The relevant remaining traversals are annotated/test-only; validator len/is_empty now use an atomic mirror.

Evidence: [crates/shamir-engine/src/tx/drainer.rs:260](../../../../../crates/shamir-engine/src/tx/drainer.rs#L260); [crates/shamir-engine/src/validator/registry.rs:249](../../../../../crates/shamir-engine/src/validator/registry.rs#L249); [crates/shamir-engine/src/tx/commit.rs:306](../../../../../crates/shamir-engine/src/tx/commit.rs#L306).

<a id="review-positive-conformance-notes-fx-hash-pillar"></a>

### Claim Positive conformance notes/Fx-hash pillar — THasher-backed concurrent collections

Status: `not-applicable`. Current risk: —.

Reviewed registries retain THasher, including the new local condition cache; no production RandomState/default HashMap construction was found.

Evidence: [crates/shamir-engine/src/db_instance/db_instance.rs:29](../../../../../crates/shamir-engine/src/db_instance/db_instance.rs#L29); [crates/shamir-engine/src/validator/registry.rs:55](../../../../../crates/shamir-engine/src/validator/registry.rs#L55); [crates/shamir-engine/src/query/filter/cond_cache.rs:77](../../../../../crates/shamir-engine/src/query/filter/cond_cache.rs#L77).

<a id="review-positive-conformance-notes-o-x-0"></a>

### Claim Positive conformance notes/O(x→0) — Batch snapshots and coalesced history writes

Status: `not-applicable`. Current risk: —.

These specific hoists/coalescing mechanisms remain present; they do not establish constant-time behavior throughout the crate.

Evidence: [crates/shamir-engine/src/table/table_manager_tx_ops.rs:804](../../../../../crates/shamir-engine/src/table/table_manager_tx_ops.rs#L804); [crates/shamir-engine/src/tx/drainer.rs:566](../../../../../crates/shamir-engine/src/tx/drainer.rs#L566).

## Corrections and qualified non-findings

- The DbInstance regression is a registered concurrent hammer, not a deterministic scheduler proof, and does not separately exercise all seven index routes.
- Yielding on CAS loss improves cooperation; it does not bound completion or release the single-flight lock.
- Distinguish the repository's approved concurrency primitives from a literal lock-free progress guarantee.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-engine — Concurrency & lock-free invariants

## Summary

Judged against CLAUDE.md's five pillars, `shamir-engine` is in exceptionally good shape: every production lock is a `tokio::sync::Mutex` carrying an inline contention-model comment, the two `std::sync::Mutex` sites are `#[cfg(test)]` failure-injection hooks (plus the CLAUDE.md-sanctioned `InFlightCreateSet` DDL guard set), concurrent maps are `scc::*`/`DashMap` with `THasher` everywhere, `ArcSwap` RCU is used for read-heavy snapshots, scc `len()` is either ack'd or replaced by atomic mirrors (`Drainer::window_depth`, `TableManager::bindings_len`), and the `WriterDrainBarrier`/`begin_write_barrier` hierarchy comes with a SeqCst memory-model proof, a documented lock-order invariant (F-70), and loom coverage. The findings below are residual: one genuine DashMap-shard-guard-across-`.await` in `DbInstance` (the exact deadlock class `RepoInstance::get_table` documents and avoids), one check-then-act lost-update on the lock-free validator registry, and three smaller flag/livelock nits.

## Findings

### 1. DashMap shard read-guard held across `.await` in `DbInstance` accessors
- **File:** `crates/shamir-engine/src/db_instance/db_instance.rs:61-68` (also `:172-184`, `:187-200`, `:203-214`, `:217-228`, `:231-242`, `:245-256`, `:259-271`)
- **Severity:** high
- **Issue:** `self.repos.get(repo_name)` returns a `dashmap::Ref` — a synchronous `std::sync::RwLock` read guard on one shard of `repos: Arc<DashMap<String, RepoInstance, THasher>>`. In `get_table` and all seven index-routing methods the `Ref` is kept alive across the delegated `.await` (`repo_manager.get_table(table_name).await`, `repo.create_index(...).await`, `lookup_by_index(...).await`, …). DashMap guards must not cross an await point: the guard-holder parks at the `.await` while the shard's OS RwLock stays held by its (unscheduled) thread, and any writer needing that shard (`add_repo`, `remove_repo`, `rename_repo` — all take the write lock) blocks its **worker thread** synchronously. This is verbatim the deadlock class this crate itself documents and fixed in `RepoInstance::get_table` (`src/repo/repo_instance.rs:311-320`: "holding the `entry()` write guard across … an `.await` … under runtime oversubscription every worker thread can become wedged on the OS RwLock of a shard whose guard-holder is parked at an `.await`, and a synchronous lock cannot yield").
- **Failure scenario:** (a) A cold `get_table` lazily constructs a `TableManager` — store opens, index loads, and potentially `mgr.repair().await` (full legacy-index rebuild, seconds to minutes per `TableManager::create`) — all with the shard read-locked. (b) Worse: `create_index`/`drop_index` hold the guard across an entire online backfill (minutes, per `KNOWN_LIMITATIONS` §3). Concurrently, one `remove_repo`/`rename_repo` call blocks its tokio worker on the shard write lock for that whole duration; a handful of such callers under a small worker pool wedges the runtime — a `TIMEOUT [test]`-class hang per CLAUDE.md's "hangs are bugs" rule.
- **Suggested fix:** Mirror the `repo_instance.rs` pattern: clone the `RepoInstance` out (it's a cheap `Arc`-field clone) and drop the guard before awaiting — `let repo = self.repos.get(repo_name).map(|r| r.clone())…; repo.get_table(...).await` (exactly what `get_repo` at `:109-111` already does). Apply to all eight sites.

### 2. `ValidatorRegistry::add_binding` — check-then-act lost update on a lock-free map
- **File:** `crates/shamir-engine/src/validator/registry.rs:162-169` (caller: `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:504`, unserialized)
- **Severity:** medium
- **Issue:** `add_binding` runs two separate critical sections: `entry_sync(id).and_modify(|set| set.insert(table))` (no-op while vacant) followed by `insert_sync(id, BTreeSet::from([table])).ok()` — the `.ok()` discards the `Err` that scc returns when the key already exists (scc `insert` never overwrites; see `repo_instance.rs:500`'s "silently no-op" note). Two concurrent `bind_validator_as` calls for the same validator id (different tables) can both observe the entry vacant, both `and_modify` no-op, and the loser's `insert_sync` errors with its table binding silently dropped from `bound_in`. The bind path (`validator_management.rs`) takes no lock that serializes them.
- **Failure scenario:** Concurrent `BindValidator` DDL for validator V on tables T1 and T2 → T2's `bound_in` entry lost → `is_bound(V)` under-reports → a later `drop_validator`'s still-bound refusal (the registry's documented referential-integrity contract, `registry.rs:6-7`) is defeated and the dropped validator leaves a dangling binding on T2; step 7's `persist_validator_bound_in` then persists the incomplete set, compounding the drift.
- **Suggested fix:** Collapse into one atomic critical section using scc's entry API: `let mut e = self.bound_in.entry_sync(*id).or_insert_with(BTreeSet::new); e.get_mut().insert(table);` — `or_insert_with` occupies the slot under the bucket lock, closing the race.

### 3. `RecordCounter` — `dirty` flag clobbered by concurrent `increment` during `set`/`persist` awaits
- **File:** `crates/shamir-engine/src/table/record_counter.rs:88-94` (`set`), `:143-163` (`persist`)
- **Severity:** low
- **Issue:** `set()` does `cache.store(count)` → `write_through(count).await` → `dirty.store(false)`. A concurrent `increment()` landing during the `.await` does `fetch_add` + `dirty.store(true)`; `set()` then resumes and its unconditional `dirty.store(false)` erases that mark. `persist()` has the same shape (`:159-161`: `write_through(cur).await` then `dirty.store(false)`, with `cur` snapshotted before the await). The `persist_lock` doesn't help — `increment` is deliberately lock-free. Result: the incremented delta is invisible to the next `persist()` (fast-path skip at `:144`) and the durable count drifts until a *later* increment re-dirties the flag; on crash/boot the counter seeds stale. In-memory `get()` stays correct, and the doctor reconciles, which bounds the blast radius to metadata drift.
- **Failure scenario:** Doctor `set_to`/`set` reconciling a count while writers are inserting → the writes' bumps are never persisted → after crash, `RecordCounter::get` reports a pre-reconcile+delta count until the next `repair()`.
- **Suggested fix:** Replace the boolean with a generation/epoch `AtomicU64` bumped on every `increment`/`set`; `persist` snapshots the epoch before the write and clears `dirty` only if the epoch is unchanged (CAS), or re-reads `cache` after the write and stores that as `last_persisted` so the skip comparison can't mask the delta.

### 4. Watchdog thread runs `log::warn!` inside `iter_sync` — sync log I/O under scc bucket locks
- **File:** `crates/shamir-engine/src/query/batch/op_watchdog.rs:118-130`
- **Severity:** low
- **Issue:** The 1 Hz diagnostic thread scans `REGISTRY` via `iter_sync`, whose closure executes while scc holds each bucket's lock. The closure calls `log::warn!` (potentially synchronous stderr I/O) under that lock, momentarily stalling `insert_sync`/`remove_sync` — which `register_op_watchdog`/`OpGuard::drop` call on the batch-op path. Mitigations: warnings are one-shot per stuck op and the registry is small, so the window is rare and short — but it's I/O under a lock the hot path shares.
- **Suggested fix:** Collect `(id, alias, elapsed)` triples inside the closure and emit the `log::warn!` lines after `iter_sync` returns (the existing second pass over `ids_to_update` already shows the pattern).

### 5. `MigrationCoordinator::drain_until_caught_up` — unbounded catch-up loop under sustained writes
- **File:** `crates/shamir-engine/src/migration/coordinator.rs:247-260`
- **Severity:** nit
- **Issue:** The loop breaks only when `shadow_lag() <= max_lag` or a pass applies zero entries. A shadow log fed faster than the drain applies entries keeps `applied > 0` forever — a livelock (no locks held, but the caller never returns) on a sustained-write source table during migration.
- **Suggested fix:** Add an attempt/pass budget or backoff and return the residual lag to the caller (an admin/migration path can poll again) rather than spinning indefinitely.

### 6. `FkReverseCache::get_or_build_by_parent` — unbounded CAS-loss retry while holding `build_lock`
- **File:** `crates/shamir-engine/src/repo/fk_reverse_cache.rs:342-355`
- **Severity:** nit
- **Issue:** On a publish-CAS loss the build loop retries indefinitely, still holding `build_lock`, so a continuous `invalidate()` storm (continuous DDL) starves both the rebuilder and every other waiter on the cache. Practically bounded because invalidation is DDL-rare and the design (single-flight + pointer-identity CAS) is otherwise exemplary; flagging only for completeness.
- **Suggested fix:** None required for correctness; if ever needed, a `yield_now` between retries or a bounded retry + error return would make the starvation window explicit. Do not release `build_lock` mid-retry — that breaks single-flight.

## Positive conformance notes (for the record, no action)

- **Lock inventory:** the only non-test, non-sanctioned-exception `std::sync::Mutex`/`RwLock`/`parking_lot` in `src/` is zero. The two `std::sync::Mutex` hits (`tx/pre_commit.rs:1006`, `table/table_manager_streaming.rs:56`) are `#[cfg(test)]` one-shot failure-injection hooks with inline justification; `InFlightCreateSet::ids` (`table/in_flight_create_guard.rs:79`) is CLAUDE.md F-9/#1076 category 2 (DDL-only, never held across `.await`, own contention comment). All production locks are `tokio::sync::Mutex` (sanctioned exception) each with an inline contention-model comment: `unique_write_lock`, `ddl_admission`, `InternerManager::persist_lock`, `RecordCounter::persist_lock`, `FkReverseCache::build_lock`, `GroupCommit::state`, `MigrationCoordinator::state`, `marker_write_mutex`.
- **Lock ordering:** `uwl_guards` acquired in sorted token order (`pre_commit.rs:493-541`); DDL side canonicalized through `begin_write_barrier` (`table_manager.rs:1203-1231`) enforcing ddl_admission → raise-bit → drain → lock; the F-70 inversion history is documented at `writer_drain_barrier.rs:50-146` with a deterministic regression test (`tx/tests/f70_lock_order_inversion_tests.rs`).
- **Memory model:** `WriterDrainBarrier` carries a full SeqCst total-order proof (`writer_drain_barrier.rs:190-258`), F-69 collapsed seven flags into one packed `AtomicU16`, and an opt-in loom model exists (`Cargo.toml` `loom` feature).
- **scc `len()` discipline:** `Drainer::window_len` (`drainer.rs:243-247`) and `ValidatorRegistry::len` (`registry.rs:230-234`) both carry `#[allow(clippy::disallowed_methods)] // O(N) ack:` comments; `Drainer` keeps an O(1) `window_depth` atomic mirror; `commit.rs:293-315` documents the `is_empty` bucket-walk tradeoff.
- **Fx-hash pillar:** every `DashMap`/`scc::HashMap` found uses `THasher` (`db_instance.rs:14`, `repo_instance.rs:25-26`, `validator/registry.rs:36-40`, `repo_types.rs:83`, `op_watchdog.rs:65`); no `RandomState` or bare `HashMap::new()` in production code; ordered cases use `TMap`/`TFxMap`/`TFxSet`.
- **O(x→0):** batch planners snapshot `all_backends()` once per batch (`table_manager_tx_ops.rs:770-800`), `insert_many` snapshots unique defs once per batch (`table_manager_crud.rs:285-291`), the drainer coalesces E×T → T history transacts, and hot-path guards-drop-before-await is systematically documented (`repo_instance.rs:311-320`, `:439-442`).

</details>
