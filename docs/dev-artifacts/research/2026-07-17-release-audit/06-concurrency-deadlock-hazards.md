# Concurrency / deadlock hazard sweep — release audit 06

Date: 2026-07-17. Scope: full workspace, read-only sweep for the #589 hazard
class (mixed scc `_async` / `_sync` lock acquisition on the same map) plus
other classic hazard shapes (guard across `.await`, lock-order cycles, bounded
channels, lost-wakeup `Notify`/`Barrier` patterns).

Reference mechanism (proven in #589, fixed in `crates/shamir-tx/src/mvcc_store/`):
scc's `_async` lock-wait is a **handoff** — on release, saa GRANTS the bucket
lock to the suspended waiter *task*, which then holds it while sitting in
tokio's run queue until next polled. scc's `_sync` accessors **park the calling
OS thread** while the bucket is locked. If, in the grant-to-poll window, every
runtime worker thread parks in a `_sync` accessor on the same bucket, the
lock-owning task can never be polled again → whole-runtime deadlock. The fix
convention: **every lock-acquiring op on a map that has synchronous accessors
must itself be synchronous** (`entry_sync`/`retain_sync`/…), so the exclusive
lock is only ever held by a *running* thread for a bounded few instructions.

Notation: worker count = tokio runtime worker threads. With `worker_threads = 1`
(tests, embedded) a SINGLE parked sync accessor suffices to deadlock.

---

## Executive summary

- The 3 already-known sites are **confirmed** (with refinements below):
  `repo_tx_gate.rs:415` and `mvcc_locks.rs:54` are real, high-confidence
  replicas of #589 with naturally hot single keys; `layered_interner.rs:61`
  is a structural violation but its map is per-`TxContext` and effectively
  single-task, so no plausible interleaving exists today (fix for hygiene).
- **One major NEW finding**: the vector index subsystem
  (`crates/shamir-index/src/vector/hnsw_adapter.rs` +
  `vector_backend.rs`) mixes `_async` and `_sync` lock acquisition on **five**
  shared maps (`deleted`, `vectors_u8`, `vectors`, `rid_to_internal`,
  `compaction_deleted_rids`), including sync iteration on the **live search
  path** over maps mutated via `insert_async`/`remove_async`, and a
  sync-park (`deleted.insert_sync`) executed **while holding** an
  async-acquired exclusive entry on `rid_to_internal` (two-bucket chain).
- **Two secondary NEW findings** in `shamir-engine`: the per-repo
  `per_table_mvcc` registry and the `token_names` reverse index are each
  read via `read_async` on the commit pipeline while also being accessed via
  `read_sync`/`get_sync`/`iter_sync` (hot) and mutated via
  `insert_sync`/`remove_sync` (DDL). These need an exclusive writer in the
  mix to deadlock, so they rank below the vector finding.
- Other hazard shapes (guards across `.await`, lock-order cycles, bounded
  channels, `Notify` lost-wakeups, backpressure parking): **no new candidates
  found** — several near-misses are already explicitly and correctly guarded
  (details in §4).

---

## 1. Confidence-ranked candidate hazards

### H1 — HIGH — `RepoTxGate::active_snapshots`: `entry_async` open vs `entry_sync` drop + `iter_sync` GC scan  *(known site 1, confirmed + sharpened)*

Files/lines (all `crates/shamir-tx/src/repo_tx_gate.rs`):
- `bump_refcount` — `active_snapshots.entry_async(version).await` at **:415**
  (called from `register_snapshot` :396 → `open_snapshot` /
  `open_snapshot_serializable`, i.e. **every tx/snapshot open**).
- `SnapshotGuard::drop` — `snapshots.entry_sync(v)` at **:216** (**every tx
  end**, on whatever runtime worker drops the guard).
- `min_alive` — `active_snapshots.iter_sync(...)` at **:557** (GC /
  vacuum / prune ticks).
- `active_snapshots_empty` — `active_snapshots.is_empty()` at **:580**
  (vacuum fast paths; scc `is_empty` also takes bucket read locks).

Why this is sharper than the generic description: the map is keyed by
**version**, and every concurrent `open_snapshot` targets the *same* key —
the current `last_committed` version. So exactly like the #589 `cells` hot
key, all contention funnels into ONE bucket by construction, not by unlucky
hashing.

Concrete interleaving (mirrors #589):
1. Tx A calls `open_snapshot` → `entry_async(v_cur)` finds the bucket locked
   (some other opener/dropper holds it) and suspends.
2. The holder releases; saa **hands the exclusive bucket lock to A's
   suspended task**. A now owns the lock while sitting in tokio's run queue.
3. Before A is polled, N committed txs finish on the N worker threads; each
   `SnapshotGuard::drop` runs `entry_sync(v_cur)` → **parks its OS worker
   thread** on the same bucket. (Guard drops run inline in whatever async fn
   drops the tx — i.e. on runtime workers.)
4. All workers parked → A is never polled → nobody ever releases →
   whole-runtime deadlock. A GC tick's `min_alive` `iter_sync` reaching that
   bucket parks additional threads the same way.

Load profile that triggers it: high tx open/close rate (every commit opens and
drops a snapshot) — no exotic conditions needed. On a small runtime
(1–2 workers) a single racing drop suffices.

Fix shape: make `bump_refcount` use `entry_sync` (the closure is a saturating
add — a few instructions; the fn can stay `async` with unchanged signature,
exactly like the `publish_cell` fix). Cancel-safety note at :303 already
describes the entry as effectively atomic, so nothing is lost.

### H2 — HIGH — `MvccStore::locks`: `lock_key` `entry_async` vs `release_locks` `get_sync`  *(known site 2, confirmed)*

Files/lines (`crates/shamir-tx/src/mvcc_store/mvcc_locks.rs`):
- `lock_key` — `self.locks.entry_async(key).await` at **:54** (every Level-3
  pessimistic lock acquisition).
- `release_locks` — `self.locks.get_sync(key)` at **:206** (every Level-3
  commit AND abort, via `release_pessimistic_locks`,
  `crates/shamir-engine/src/tx/commit.rs:825-826`).

The `locks` map is per-table, shared across all txs. The hot-key case is
precisely the interesting one for pessimistic locking: many txs contending
for the SAME record key → same bucket.

Interleaving:
1. Tx A's `lock_key(k)` suspends in `entry_async(k)`; a completing op on that
   bucket hands A the exclusive bucket lock while A sits unpolled. (Note: the
   entry guard in `lock_key` is short-lived once *running* — the hazard is
   purely the grant-to-poll window.)
2. Concurrent txs B..N finish (commit or abort) on the worker threads and
   call `release_locks` → `get_sync(k)` → each **parks its worker** (scc
   `get_sync` takes a shared bucket lock and parks while the bucket is
   exclusively owned).
3. All workers parked in `get_sync` → A never polled → deadlock. Extra
   nastiness: the parked releasers are exactly the txs whose release would
   have let waiters make progress — even a *near*-deadlock here inflates
   wound-wait latencies.

Fix shape: `entry_sync` at :54 (the entry body is `Arc::clone`-or-insert —
bounded); alternatively `get_sync`→async is the WRONG direction per the
project convention (sync accessors exist on this map and Drop-paths cannot
await). Also note `mod.rs:1276` `self.locks.len()` (O(N) scc `len` — should
carry the `#[allow]` ack if it doesn't already; telemetry only).

### H3 — MEDIUM-HIGH — **NEW**: vector index maps mix `_async` mutation with `_sync` hot-path readers (`shamir-index`)

This is the exact #589 structural class, replicated across five maps in
`crates/shamir-index/src/vector/hnsw_adapter.rs` (and one cross-file map in
`vector_backend.rs`). The `_sync` accessors run on runtime worker threads
(search / compaction / fit are async fns; only the CPU-bound graph insert
itself is `spawn_blocking`).

Per-map inventory (production code only; tests excluded):

1. **`deleted`** (tombstones, keyed by internal id)
   - Async exclusive: `insert_async` at **:2535** (`delete()` path);
     `contains_async` at **:1742** (search bruteforce loop — note the SAME
     probe is `contains_sync` elsewhere, the file uses both spellings).
   - Sync: `contains_sync` at :707, :864, :1265, :1347, :1509, **:1685**
     (`quantized_fastpath_publish` — upsert hot path), :2282, :2410, :2493;
     `insert_sync` at **:2182** and **:2369**; `iter_sync` at :505.
   - **Amplifier:** the `insert_sync` calls at :2182/:2369 execute **while the
     caller holds the exclusive `rid_to_internal` entry acquired via
     `entry_async`** (:2178/:2362 — guard explicitly dropped only at
     :2193/:2380). A worker parked in `deleted.insert_sync` therefore parks
     *while owning the rid bucket*, chaining two buckets: if `deleted`'s
     bucket is owned by an unpolled `insert_async`-granted delete task, the
     worker holding the rid bucket parks forever-until-poll, and every waiter
     on the rid bucket (other upserts of that rid via `entry_async` — those
     suspend, fine — but also `collect_live_vectors`' `iter_sync` :705 and
     snapshot serialisation `iter_sync` :491) parks sync waiters transitively.
2. **`vectors_u8`** (quantized codes)
   - Async: `insert_async`/`remove_async`/`read_async` (many sites: :878,
     :1745 area, :2209, :2433, :2557, …).
   - Sync: `iter_sync` at :1007 and **:1732** — :1732 is
     `search_quantized_bruteforce`, i.e. a **live query path** synchronously
     iterating (bucket-read-locking) a map concurrently mutated via
     `_async` calls; `read_sync` :711; `insert_sync` :1066, :1130;
     `contains_sync` :1140, :1528.
3. **`vectors`** (f32 buffer)
   - Async: `insert_async` :823, :2235, :2480; `remove_async` :878, :2240,
     :2298, :2503, :2522, :2540.
   - Sync: `iter_sync` :513, :1260, :1346, :1508, :1527 (fit
     snapshot/delta/catch-up scans — these run in async fns on the runtime);
     `read_sync` :714; `remove_sync` :1534.
4. **`rid_to_internal`**
   - Async exclusive: `entry_async` :754 (`backfill_if_absent`), :2178
     (`upsert`), :2362 (`upsert_batch`); `read_async` :1934, :2050, :2534.
   - Sync: `iter_sync` :491 (snapshot serialisation), :705
     (`collect_live_vectors`, compaction); `contains_sync` :500.
5. **`compaction_deleted_rids`** (cross-file)
   - Async: `insert_async` in `vector_backend.rs` **:339, :365, :527**
     (the delete double-write path — every user delete during a compaction).
   - Sync: `contains_sync` in `hnsw_adapter.rs` **:748, :759**
     (backfill check + under-entry-lock re-check); `iter_sync` in
     `vector_backend.rs` **:1074** (Step 4b reconcile).

Concrete interleaving (the cleanest single-map variant, `deleted`):
1. During or after a compaction burst, task D (`delete(rid)`) suspends in
   `deleted.insert_async(internal)` (:2535) because an upsert's
   `insert_sync` (:2182) briefly owns the bucket; on release, saa **grants D
   the exclusive bucket lock** while D sits in the run queue.
2. Concurrent search tasks (every candidate-filter probe is
   `deleted.contains_sync` — :1509 post-fit u8 graph search, :1265 brute
   force, :1685 upsert fast-path) **park their worker threads** on that
   bucket.
3. With search concurrency ≥ worker count (a plain query burst), all workers
   park → D never polled → runtime deadlock. On `worker_threads=1`, one
   search racing one delete suffices.

Confidence: HIGH on the structural violation (it is unambiguous and
pervasive); MEDIUM-HIGH on practical reproduction frequency — unlike
`cells`/`active_snapshots` there is no single naturally-hot key (internal ids
spread across buckets), so it needs either a hot-record delete/upsert churn
colliding with a search burst on the same shard, a compaction window
(`compaction_deleted_rids` IS single-map shared and hammered during
double-write), or a small runtime. The two-bucket chain via
:2182/:2369 (park while holding an async-granted exclusive entry) materially
widens the window versus plain #589.

Suggested repro direction: small runtime (`worker_threads=1..2`), quantized
adapter past FIT_THRESHOLD, loop {concurrent delete+re-upsert of one rid} ×
{concurrent `search_quantized_bruteforce`-sized searches}; and separately the
compaction stress test extended with concurrent deletes (the existing
`stress_concurrent_mutations_during_quantized_compaction` is the right
skeleton).

Fix shape: same as #589 — make every lock-acquiring op on these five maps
synchronous (`insert_sync`/`remove_sync`/`entry_sync`/`read_sync`/
`contains_sync`); none of the closures suspend, and the fns can stay `async`.
This also erases the :2182/:2369 park-while-holding chain (the rid entry
would then be sync-acquired by a running thread).

### H4 — MEDIUM — **NEW**: `RepoInstance::per_table_mvcc` — `read_async` commit pipeline vs `read_sync`/`get_sync` hot readers vs `insert_sync`/`remove_sync` DDL writers

Sites (map: `scc::HashMap<u64, Arc<MvccStore>>`, per repo, shared):
- Async (shared grants): `read_async` in
  `crates/shamir-engine/src/tx/pre_commit.rs:78` (every tx pre-commit),
  `commit_phases.rs:512`, `drainer.rs:490` (every drain step),
  `recovery.rs:587`, `apply_replicated.rs:217`; `iter_async` in
  `repo_instance.rs:1440` (`run_gc`).
- Sync shared: `read_sync` in
  `crates/shamir-engine/src/repo/version_provider.rs:14` (**SSI
  `validate_read_set` — commit hot path**); `get_sync` in
  `commit.rs:825` (pessimistic release) and `repo_instance.rs:491`;
  `iter_sync` in `repo_instance.rs:1347` (`flush_all_history`, drainer
  truncation gate) and `drainer.rs:637`.
- Sync exclusive: `insert_sync` `repo_instance.rs:376` (table attach),
  `remove_sync` `repo_instance.rs:456` (drop table).

Unlike H1–H3 the `_async` ops here are all SHARED acquisitions, so the
deadlock needs an exclusive writer in the mix — i.e. **DDL concurrent with
commit/drain load**:
1. `remove_sync`/`insert_sync` (DDL) owns a bucket exclusively for a moment;
   concurrent `pre_commit`/`drainer` `read_async` waiters suspend on it.
2. On release, saa hands SHARED grants to the suspended reader tasks — they
   hold read locks while unpolled.
3. A second DDL op (or repo-close teardown) parks a worker in
   `remove_sync` waiting for those unpolled readers. If saa applies
   writer-fairness (new shared acquirers queue behind a pending writer —
   this is the standard anti-starvation behavior; **needs verification
   against the vendored saa version**), every subsequent commit-path
   `read_sync`/`get_sync` parks behind the pending writer → workers drain
   into parks → the granted reader tasks are never polled → deadlock.

Confidence MEDIUM: requires two DDL ops (or DDL + writer-fair reader queue)
overlapping sustained commit traffic, plus the saa fairness property. The fix
is trivial and strictly convention-aligning: the `read_async` closures are all
`Arc::clone` — replace with `read_sync` (and `iter_async`:1440 with
`iter_sync`, matching :1347 which already is sync on the very same map).

### H5 — LOW-MEDIUM — **NEW**: `RepoInstance::token_names` — same class as H4, lower frequency

- Async: `read_async` `repo_instance.rs:1178` (`table_by_token` — commit
  pipeline Phases 1/2.6/5b–5d under `commit_lock`, and V2 WAL recovery) and
  `:1215` (`table_by_token_if_live` — every `pre_commit_prelock` barrier
  check).
- Sync: `insert_sync` :1507 + `read_sync` :1509 (`register_token`, DDL),
  `remove_if_sync` :441 (drop table).

Same mechanism and same trivial fix as H4 (`read_async`→`read_sync`; closure
is a `String` clone). Ranked lower because the exclusive writers are
DDL-only and the reader set is narrower.

### H6 — LOW (structural only) — `LayeredInterner` overlay: `entry_async` vs `entry_sync`/`iter_sync`  *(known site 3, confirmed but refined DOWN)*

- Async: `overlay.entry_async` `crates/shamir-tx/src/layered_interner.rs:61`
  (`touch`); `read_async` :121; `iter_async` :206
  (`commit_interner_overlay`) and `crates/shamir-engine/src/tx/commit.rs:174`.
- Sync: `entry_sync` :93 (`touch_sync`, called from
  `crates/shamir-engine/src/table/write_helpers.rs:348` and
  `write_exec.rs:903`); `iter_sync` :142 (`get_str` reverse scan).

Refinement the brief's description misses: the overlay map is **per-
`TxContext`** (`tx.interner_overlay`), and a tx's operations execute
sequentially on one logical task — writes, then Phase-1 merge, then commit.
No two tasks contend on the same overlay instance in any current call path,
so there is no plausible #589 interleaving *today*. It is still a convention
violation and a landmine for any future intra-tx parallelism (e.g. parallel
batch ops sharing one tx): fix by making `touch`/`get_id` use the sync
variants (the map is uncontended, so sync is also strictly faster —
`touch_sync` already exists and is identical).

---

## 2. Explicit note on the 3 already-known sites

| Site | Verdict | Refinement |
|---|---|---|
| `repo_tx_gate.rs:415` (`active_snapshots.entry_async` vs `entry_sync` :216 / `iter_sync` :557 / `is_empty` :580) | **Confirmed, HIGH** | Worse than described: the contended key is the *current* `last_committed` version, so ALL concurrent snapshot opens/drops collide on ONE bucket by construction — the same natural hot-key funnel that made #589 reproducible. Every tx open (`entry_async`) and every tx end (`entry_sync` in `Drop`, cannot be made async) hit it. Fix must therefore go async→sync at :415. |
| `mvcc_locks.rs:54` (`locks.entry_async` vs `get_sync` :206) | **Confirmed, HIGH** | The sync accessor is on BOTH commit and abort paths of every Level-3 tx (`release_pessimistic_locks`, engine `commit.rs:819-828`); pessimistic hot-key workloads maximize same-bucket collisions. `entry_async`'s guard is short-lived once running — the entire hazard is the grant-to-poll window, identical to #589. Also confirmed there are no OTHER accessors of `locks` (only :54, :206, and an O(N) `len()` at `mod.rs:1276`). |
| `layered_interner.rs:61` (`overlay.entry_async` vs `entry_sync` :93 / `iter_sync` :142) | **Confirmed as mixing, but risk refined DOWN to structural-only** | The map is per-`TxContext`, accessed by one task at a time in all current paths — no cross-task interleaving exists. Fix for convention/hygiene and future-proofing, not urgency. See H6. |

Also verified the #589 fix itself is complete and internally consistent:
`mvcc_store/mod.rs:494` (`publish_cell` → `entry_sync`), `mvcc_gc.rs:533`
(`prune_version_cache` → `retain_sync`), `mvcc_history.rs:263`
(`seed_version` → `entry_sync`), and `try_reserve`/`finalize_reservation`
were already sync. One stale comment: `mvcc_history.rs:460` still says
"CRIT-2: entry_async modify-or-insert" while the code below (:485) calls the
sync `finalize_reservation` — comment-only drift, worth cleaning when touched.

---

## 3. Sites swept and found CLEAN (all-sync or all-async, no mixing)

- `crates/shamir-engine/src/tx/drainer.rs` — `window`, `pending_unsafe`,
  interner-delta maps: all `_sync`. (Its `per_table_mvcc` uses are part of H4.)
- `crates/shamir-engine/src/validator/registry.rs` — all `_sync`.
- `crates/shamir-tx/src/tx_context.rs:370` `locked_keys.entry_sync` +
  engine `commit.rs:819` `iter_sync` — all-sync, correct.
- `crates/shamir-index/src/registry.rs` — all `_async`, no sync accessors →
  internally consistent (all-async is safe: async waiters suspend, never park).
- `crates/shamir-index/src/vector/snapshot.rs:1026-1043` `insert_sync` — boot
  loader populating private maps before publication; no concurrency.
- `shamir-server`, `shamir-storage`, `shamir-client`, `shamir-wasm-host`,
  `shamir-funclib`, `shamir-connect` — no scc `_async` locking calls at all
  (all-sync conventions or dashmap/parking_lot with self-audited short
  sections).

## 4. Other hazard shapes — checked, no new candidates

- **Sync guard across `.await`:** none found in production code. The two
  hnsw `entry_async` blocks explicitly scope and drop the guard before the
  `spawn_blocking().await` (`hnsw_adapter.rs:2193`, `:2380` — commented).
  `std::sync::Mutex` uses (`repo_tx_gate.rs:140` `pending_commits`,
  `predicate_set.rs`, `wal_sink.rs`, client `:301`/`:905`, connect/server
  `parking_lot` sites) are short-section, no-await, and mostly carry inline
  sanction comments. The one *park*-across-held-lock (not await) is the
  `deleted.insert_sync`-under-`rid_to_internal`-entry chain — folded into H3.
- **Lock-order cycles:** wound-wait (`lock_key`) waits only on strictly-older
  holders and wounds strictly-younger — acyclic by version order; the
  cross-key wound wakeup (`wound_notify` :144-145, selected at :185-188) is
  present, closing the parked-on-другой-key case. `commit_mutex` ordering vs
  `prune_commit_log_below` is documented lock-free (`repo_instance.rs:1466`).
  No AB/BA pair found.
- **Bounded channels:** `changefeed.rs:307` uses `try_send` with documented
  drop-on-full (data-loss-by-design, no блокировка); client sub channel is
  bounded but consumer-owned. No producer that can block forever on a dropped
  consumer.
- **`Notify` lost-wakeup / never-reached signal:**
  `lock_key` registers `notified()` + `enable()` BEFORE dropping the state
  lock (`mvcc_locks.rs:182-184`) — correct. `apply_backpressure`
  (engine `commit.rs:339-407`) registers `durable_notified()` before the gap
  re-read, caps each park with a 50 ms slice, AND bounds the whole loop with
  a 5 s wall-clock budget — a stuck drainer degrades to RAM growth, not a
  hang. `Barrier` appears only in benches.

## 5. Recommended fix order

1. **H1 + H2** (`active_snapshots.entry_async` → `entry_sync`;
   `locks.entry_async` → `entry_sync`): one-line-each, exactly the #589
   recipe, highest probability of being the NEXT production hang.
2. **H3** (vector maps): mechanical `_async`→`_sync` sweep across the five
   maps in `hnsw_adapter.rs`/`vector_backend.rs`; none of the closures
   suspend. Largest diff, biggest latent-risk retirement.
3. **H4 + H5** (`per_table_mvcc`, `token_names` `read_async`→`read_sync`,
   `iter_async`→`iter_sync`): trivial, closes the DDL-under-load window.
4. **H6** (layered interner): hygiene; collapse `touch` onto `touch_sync`.
5. Comment cleanup: `mvcc_history.rs:460` stale "entry_async" reference.
