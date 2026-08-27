# shamir-tx — Concurrency & lock-free invariants

## Summary

shamir-tx adheres strongly to CLAUDE.md's five pillars: every concurrent map is
`scc::*` keyed with `THasher` (cells / locks / pending_ts / active_snapshots /
read_set / cas_set / locked_keys / ri_barrier_tokens / interner_overlay /
CompletionTracker states), there is no `parking_lot` and no `std::sync::Mutex`
on any live path (the single `std::sync::Mutex` is the documented
dead-scaffolding exception, category 1 of the sanctioned list), and every
`tokio::sync::Mutex` is the sanctioned guard-across-`.await` case with an
inline contention-model comment (`commit_mutex`, `drain_exclusive` with
`try_lock` on the periodic path, `KeyLock::state` dropped before its `.await`).
The #589 sync-accessor discipline (never mix `entry_async` lock-handoff with
`*_sync` readers on the same map) is applied uniformly and backed by dedicated
regression tests (`active_snapshots_deadlock_tests.rs`, `locks_deadlock_tests.rs`).
The findings below are one latent monotonicity footgun in a public gate API, one
unbounded-growth lock registry, and several O(N)-shaped walks — two of which
evade the pillar-3 `len()` lint by using `range(..).count()` instead.

## Findings

### 1. `publish_committed` plain `store()` can regress `last_committed_version`; its safety contract is unsound
- **File:line:** `crates/shamir-tx/src/repo_tx_gate.rs:576-589`
- **Severity:** high (latent — zero production callers today; every in-tree caller is a test)
- **Issue:** The doc claims the plain `store(version, Release)` is safe because
  it "must be called under `commit_lock` on the tx commit path, where monotonic
  ordering is guaranteed by the lock". But `commit_mutex` only serialises tx
  commits **against each other** — the crate's own non-tx write paths
  (`MvccStore::set_versioned` / `set_versioned_many` / `delete_versioned` via
  `VersionGuard::advance_last_committed` → `fetch_max`, and
  `write_committed_*_to_history` → `publish_committed_max`) publish the same
  atomic **without** holding `commit_lock`, and allocate versions from the same
  counter. `commit_lock` therefore does not establish the global monotonicity
  the comment promises. Everything else in the gate enforces monotonicity
  (fetch_max / CAS loops); this one public method is the only site that can
  move the reader-visible floor backwards.
- **Failure scenario:** Tx T takes `commit_lock`, assigns v=95. Concurrently a
  non-tx writer assigns v=96, completes, and `fetch_max`s `last_committed` to
  96 (its write is acked). T then calls `publish_committed(95)` → floor drops
  96→95. Readers hit the R3 cap in `get_current_bytes` (`v > floor` →
  range-scan at the floor) and resolve the **pre-write** value: the acked
  non-tx write becomes invisible; a fresh snapshot opens at 95 and misses it
  (read-your-write break across the non-tx/tx boundary).
- **Suggested fix:** Make `publish_committed` body identical to
  `publish_committed_max` (or delete it and migrate the ~20 test callers to
  `publish_committed_max`). At minimum, correct the doc: monotonicity holds
  only if *every* publisher takes `commit_lock`, which is already false
  in-crate.

### 2. `MvccStore::locks` registry never evicts empty entries — unbounded growth under pessimistic workloads
- **File:line:** `crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:219-239` (insertion at `:75-82`; struct at `key_lock.rs:78-90`)
- **Severity:** medium
- **Issue:** `lock_key` `entry_sync`-inserts an `Arc<KeyLock>` (a
  `tokio::sync::Mutex<KeyLockState>` + `Notify` + `Vec<Holder>`) per key on
  first pessimistic acquire; `release_locks` removes holders but explicitly
  keeps the empty entry ("Leftover empty entries are kept in the map (cheap;
  GC is intentionally not done here)"). Grep confirms no removal/retain path
  exists anywhere for this map. For a Level-3 workload locking ever-fresh keys
  (the common shape — lock record by monotonic `RecordId`), the map grows
  without bound for the life of the table. This is a pillar-3 violation
  (unbounded growth on a per-operation path), and the "cheap" comment only
  addresses per-entry size, not cardinality. `locks_len()` exists solely to
  assert the map is empty when Level-3 is unused — nothing covers the used case.
- **Failure scenario:** A long-running server with `IsolationLevel::Pessimistic`
  txs touching N distinct keys accumulates N dead `KeyLock`s forever; memory
  grows linearly with total distinct locked keys, never reclaimed.
- **Suggested fix:** Evict on release when safe. Naive removal is unsound (a
  waiter parked on `notify` holds an `Arc<KeyLock>` clone; removing the map
  entry while it waits lets a new requester insert a fresh `KeyLock` and both
  txs would then "hold" the key via two independent mutexes). Add a
  `waiters: AtomicUsize` per `KeyLock` incremented around the parked
  `select!`, and in `release_locks` remove the entry (via `entry` /
  `remove_sync` under the state lock) only when `holders.is_empty() &&
  waiters == 0`; alternatively sweep in the existing GC tick
  (`retain_sync` dropping entries whose `Arc::strong_count` indicates no
  external clones).

### 3. Predicate-conflict validation is O(window × deps × postings) with linear scans, under `commit_lock`
- **File:line:** `crates/shamir-tx/src/repo_tx_gate.rs:860-884` (`predicate_conflicts_batch`), `:803-824` (single-dep variant), `:1004-1020` (`record_conflicts`)
- **Severity:** medium
- **Issue:** For every `CommitWriteRecord` in the `(snapshot, last_committed]`
  window and every dep, `record_conflicts`'s `IndexRange` arm runs
  `inserted_index_keys.iter().any(|k| key_in_interval(..))` — a linear scan.
  `build_footprint_from_tx` explicitly `sort_unstable()`s each table's keys
  ("frees a future binary_search optimisation") but the validation never uses
  the order. The doc on `predicate_conflicts_batch` states it runs **under
  `commit_lock`** on every live Serializable call path (CRIT-4), so this
  O(W×P×K) walk directly extends the hold time of the repo-wide commit mutex —
  a hidden super-linear cost in a helper on the commit hot path (pillar 3),
  and it serialises all Serializable commits behind it.
- **Failure scenario:** Busy repo with Serializable txs holding wide predicate
  sets (P deps) and a commit window containing many footprints (W records,
  K postings each): each Serializable commit holds `commit_mutex` for
  O(W×P×K) memcmp work, throttling commit throughput for every table in the
  repo.
- **Suggested fix:** For `IndexRange` deps, `partition_point` on the
  `Bytes`-sorted `inserted_index_keys` to find the `[lo, hi]` slice in
  O(log K), then check only that slice against the 9-byte index prefix (the
  prefix check can be hoisted to one slice-level comparison since all keys in
  one footprint share the table's index ids). Keep the linear path for
  `TableScan` (single map probe — already O(1)).

### 4. `vacuum_key` scan path issues a duplicate `lookup_ts` history read per reclaimed version
- **File:line:** `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:227-234` and `:243-250`
- **Severity:** medium (only under non-default retention; then it is per-write)
- **Issue:** When `max_age_secs` is set, the age-cap branch calls
  `self.lookup_ts(*version).await` (line 229) to classify the version, and the
  reclaim block calls `lookup_ts` again (line 245) for the *same* version after
  the guards pass. Two awaited `history.get` round-trips (potentially
  `spawn_blocking` + disk I/O per call on file backends) per reclaimed version
  — a "repeated lookups" pillar-3 violation in a loop. `vacuum_key` runs at the
  tail of **every** `set_versioned` / `set_versioned_many` /
  `delete_versioned` write; the scan path fires whenever retention is not
  `CurrentOnly` (or `vacuum_needs_scan` is set), so any production deployment
  using age/count retention pays doubled per-entry I/O on its write path.
- **Failure scenario:** Retention `{max_age_secs: 60}` + overwrite-heavy
  workload: each write's vacuum scans the key's versions and performs 2 ts
  lookups per reclaimable version instead of 1 — measurable write-latency and
  IOPS inflation on disk backends.
- **Suggested fix:** Hoist `let ts = self.lookup_ts(*version).await;` once per
  entry before the age branch, reuse it at the reclaim site (bind
  `reclaimed_ts = ts`), matching what `purge_below_ts` (`:452-463`) and
  `gc_below` (`:343`) already do correctly.

### 5. O(N) tree traversals use `range(..).count()` — sidesteps the `len()` disallowed-methods gate without the ack attribute
- **File:line:** `crates/shamir-tx/src/repo_tx_gate.rs:898-904` (`commit_log_len`), `:886-896` (`prune_commit_log_below`)
- **Severity:** low
- **Issue:** CLAUDE.md pillar 3 bans `scc::*::len()` (== `iter().count()`) on
  every code path via `clippy.toml` `disallowed-methods`, and requires
  legitimate O(N) uses to carry
  `#[allow(clippy::disallowed_methods)] // O(N) ack: <why>`. Both functions
  compute cardinality with `range(..).count()` — semantically identical full
  traversals that the lint cannot see — and carry only prose doc comments, not
  the sanctioned ack attribute. `prune_commit_log_below` additionally walks
  the pruned range **twice** (once to count, once in `remove_range_sync`) on
  the engine's GC tick. Both are off the hot path and test/telemetry-facing,
  hence low.
- **Suggested fix:** Add the `#[allow(clippy::disallowed_methods)] // O(N) ack:
  telemetry/GC tick, off hot path` attribute to both (matching the pattern
  already used on `pending_ts_len` / `ts_index_len` / `locks_len` in
  `mvcc_store/mod.rs`), and in `prune_commit_log_below` drop the pre-count
  pass if `remove_range_sync`'s return can serve the count, or fold the count
  into a single pass.

### 6. `min_alive()` is a full `active_snapshots` traversal per call
- **File:line:** `crates/shamir-tx/src/repo_tx_gate.rs:651-670`
- **Severity:** low
- **Issue:** `min_alive` does an `iter_sync` scan of every distinct live
  snapshot version to find the minimum. It is called once per
  `vacuum_key` **scan-path** invocation (i.e. per write under any non-default
  retention — `mvcc_gc.rs:156`), once per `gc()`/`prune_version_cache`, and by
  the engine GC tick. Cost is O(S) per write (S = distinct pinned snapshot
  versions) and takes bucket read locks that contend with the hot
  `bump_refcount`/`SnapshotGuard::drop` path on the same map. S is normally
  small (concurrent-reader count, refcount-deduped), so low — but it is a
  per-operation O(N) on the write path under retention configs, exactly the
  shape pillar 3 targets.
- **Suggested fix:** Keep an `AtomicU64` cached-min (recompute-on-eviction: if
  the removed version == cached min, rescan — the expensive rescan then only
  happens when the *oldest* snapshot closes), or mirror registrations in a
  `scc::TreeIndex<u64, ()>` whose `front`-style range probe gives the min in
  O(log S).

### 7. A10 in-flight barrier can starve GC (commit-write log / version cache growth) under sustained snapshot-open churn
- **File:line:** `crates/shamir-tx/src/repo_tx_gate.rs:110-127` (barrier field), `:636-657` (`min_alive` returns 0 while openers in flight)
- **Severity:** low
- **Issue:** While `active_snapshots_opening > 0`, `min_alive()` returns 0,
  making `gc_below`, `prune_version_cache`, `vacuum_key`'s fast path and
  `prune_commit_log_below` all no-ops. This is the deliberate A10 conservative
  choice and is *safe* (over-retention), but there is no bound on how long the
  counter can stay non-zero under continuous open-snapshot traffic: on a busy
  server where a snapshot is always mid-registration at tick time, the
  commit-write log (one entry per Serializable/footprint commit, only pruned
  via `prune_commit_log_below(min_alive())`) and the `cells` cache grow
  unboundedly between quiet windows. Each barrier window is short (one
  `entry_sync` + two atomic loads), so sustained starvation requires extreme
  churn — hence low / residual-risk rather than a live defect.
- **Failure scenario:** Continuous high-rate snapshot opens (e.g. one short
  tx per request) with Serializable commits: GC ticks repeatedly observe
  `min_alive() == 0`, `commit_write_log` never prunes, memory climbs with
  commit count until an occasional quiet tick lets GC catch up.
- **Suggested fix:** Make the barrier floor *pinned-floor-aware*: have each
  opener publish the `last_committed` value it captured *before* incrementing
  the barrier into a `min` over in-flight openers (an `AtomicU64` min-CAS),
  and let `min_alive()` return `min(registered_min, in_flight_captured_min)`
  instead of a blanket 0. Prune/GC can then run against a real floor while
  still never deleting a version an in-flight reader can need.

### 8. `history_of` resolves commit timestamps with N sequential awaited gets
- **File:line:** `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:208-219`
- **Severity:** nit
- **Issue:** Phase 3 loops `self.lookup_ts(version).await` per timeline entry —
  N sequential `history.get` round-trips where one `history.get_many` over the
  `ts_key(version)` set (the same batching `get_at_many`/`get_current_many`
  already use) would collapse them. Off-hot admin/diagnostic path (T4), so nit.
- **Suggested fix:** Collect the versions, issue a single
  `get_many(ts_keys)`, then assemble the `VersionEntry` vector.

### 9. Stale "cannot early-return" comment on `validate_read_set`'s `iter_sync`
- **File:line:** `crates/shamir-tx/src/tx_context.rs:770-776`
- **Severity:** nit
- **Issue:** The comment asserts scc's synchronous visitor "cannot
  early-return; capture the first conflict and report it after the scan", yet
  the closure immediately below returns `false` on the first conflict — which
  is precisely scc's early-stop protocol (and `append_ri_barrier_deps` at
  `:748-753` relies on the same protocol returning `true` to continue). The
  code is right and early-stops; the comment is wrong and will mislead future
  edits around this lock-free iteration.
- **Suggested fix:** Reword the comment to state that returning `false` stops
  the iteration (first conflict wins), removing the claim that a full scan is
  unavoidable.

---

**Positive observations (for balance, no action needed):** the sync/async scc
accessor discipline is exemplary — every mutating accessor on a map with any
`*_sync` reader is `entry_sync`/`upsert_sync`/`retain_sync`, each with an
inline #589-class rationale and H1/H2 regression tests; `PredicateSet` and
`VersionedOverlay` follow the pillar-3 `AtomicUsize`-mirror pattern for O(1)
`len`; the three `tokio::sync::Mutex` sites each document a real contention
model; `drain_exclusive`'s try-lock back-off keeps the drainer's loop immune
to a stuck admin drain (#1032); and all four `scc::*::len()` call sites in
`mvcc_store/mod.rs` carry the sanctioned `O(N) ack` annotations.
