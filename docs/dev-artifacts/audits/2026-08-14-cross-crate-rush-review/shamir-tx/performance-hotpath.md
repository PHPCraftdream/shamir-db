<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-tx — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

All reported storage-call, allocation and traversal shapes remain. The later borrowed-staging change removed a different intermediate vector, not these findings. Timing and scale-dependent harm remain unmeasured.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 14 | 13 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — vacuum_key: unbatched and duplicated per-version I/O on the write hot path

Status: `confirmed-open`. Current risk: `medium`.

Fast reclaim performs a timestamp get and two removals after the write; scan reclaim adds a duplicate timestamp get only with an age cap. Backend round-trips and timing vary.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:105](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L105); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:109](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L109); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:229](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L229); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:245](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L245); [crates/shamir-tx/src/mvcc_store/mod.rs:830](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L830).

<a id="review-2"></a>

### Claim 2 — gc_below and purge_below_ts materialise history before deleting

Status: `confirmed-open`. Current risk: `high`.

Both accumulate global per-key maps before deletion. gc_below stores below-threshold entries; purge stores all decoded versions. Memory scales with buffered history, although no RSS magnitude was measured.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:309](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L309); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:314](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L314); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:405](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L405); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:410](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L410).

<a id="review-3"></a>

### Claim 3 — min_alive: full-map iteration on the write tail and GC paths

Status: `confirmed-open`. Current risk: `medium`.

Every scan-path vacuum computes the minimum by traversing distinct snapshot-version entries. No minimum cache or ordered replacement exists.

Evidence: [crates/shamir-tx/src/repo_tx_gate.rs:659](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L659); [crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:156](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_gc.rs#L156).

<a id="review-4"></a>

### Claim 4 — record_conflicts: linear interval scan over already-sorted keys inside commit critical section

Status: `confirmed-open`. Current risk: `medium`.

Sorted posting vectors are still scanned with iter().any inside the nested commit-window validation. Worst-case work is proportional to window × dependencies × postings.

Evidence: [crates/shamir-tx/src/repo_tx_gate.rs:1015](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L1015); [crates/shamir-tx/src/repo_tx_gate.rs:1089](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L1089); [crates/shamir-tx/src/repo_tx_gate.rs:876](../../../../../crates/shamir-tx/src/repo_tx_gate.rs#L876).

<a id="review-5"></a>

### Claim 5 — Stream group-by: per-version-row key allocation and shift_remove per group

Status: `confirmed-open`. Current risk: `medium`.

Each eligible decoded row copies its original key before detecting a group change, and flushing uses order-preserving shift_remove. Exact pinned IndexMap implementation costs were not independently inspected.

Evidence: [crates/shamir-tx/src/mvcc_store/version_entry.rs:193](../../../../../crates/shamir-tx/src/mvcc_store/version_entry.rs#L193); [crates/shamir-tx/src/mvcc_store/version_entry.rs:194](../../../../../crates/shamir-tx/src/mvcc_store/version_entry.rs#L194); [crates/shamir-tx/src/mvcc_store/version_entry.rs:124](../../../../../crates/shamir-tx/src/mvcc_store/version_entry.rs#L124); [Cargo.lock:1782](../../../../../Cargo.lock#L1782).

<a id="review-6"></a>

### Claim 6 — Pessimistic locks registry never evicts empty entries — unbounded growth

Status: `confirmed-open`. Current risk: `medium`.

Released locks retain map entries for all distinct acquired keys. The suggested unconditional remove would break lock identity for outstanding requesters.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:75](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_locks.rs#L75); [crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:223](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_locks.rs#L223).

Grouping/duplicate: `concurrency-lockfree.md#2`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — history_of: N sequential lookup_ts point-reads

Status: `confirmed-open`. Current risk: `low`.

Timestamp resolution remains an awaited point lookup per timeline entry; get_many is not used on this administrative path.

Evidence: [crates/shamir-tx/src/mvcc_store/mvcc_history.rs:211](../../../../../crates/shamir-tx/src/mvcc_store/mvcc_history.rs#L211); [crates/shamir-tx/src/mvcc_store/mod.rs:1625](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L1625).

<a id="review-8"></a>

### Claim 8 — Vectored reads: sequential fallback/cold awaits and redundant re-probe

Status: `confirmed-open`. Current risk: `low`.

Fallback slots resolve sequentially; get_at_many rereads current_version. Cold-heavy startup can exercise every fallback, but its prevalence and timings were not measured.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:1203](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L1203); [crates/shamir-tx/src/mvcc_store/mod.rs:1607](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L1607); [crates/shamir-tx/src/mvcc_store/mod.rs:1611](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L1611).

<a id="review-9"></a>

### Claim 9 — VersionedOverlay::gc_upto: full-tree collect-then-remove

Status: `confirmed-open`. Current risk: `low`.

The key-major tree is fully iterated, qualifying keys are cloned into a vector, then separately removed. A version-only upper bound cannot be applied to this existing ordering.

Evidence: [crates/shamir-tx/src/versioned_overlay.rs:40](../../../../../crates/shamir-tx/src/versioned_overlay.rs#L40); [crates/shamir-tx/src/versioned_overlay.rs:178](../../../../../crates/shamir-tx/src/versioned_overlay.rs#L178); [crates/shamir-tx/src/versioned_overlay.rs:192](../../../../../crates/shamir-tx/src/versioned_overlay.rs#L192).

<a id="review-10"></a>

### Claim 10 — project_event: per-record heap clone of the table-name String

Status: `confirmed-open`. Current risk: `low`.

Each RecordChange still owns a cloned String. Borrowed iter_ops removes the staging snapshot vector, not these clones; one clone per token cannot populate multiple owned String fields.

Evidence: [crates/shamir-tx/src/changefeed.rs:459](../../../../../crates/shamir-tx/src/changefeed.rs#L459); [crates/shamir-tx/src/changefeed.rs:462](../../../../../crates/shamir-tx/src/changefeed.rs#L462); [crates/shamir-tx/src/changefeed.rs:470](../../../../../crates/shamir-tx/src/changefeed.rs#L470); [crates/shamir-tx/src/changefeed.rs:60](../../../../../crates/shamir-tx/src/changefeed.rs#L60).

<a id="review-11"></a>

### Claim 11 — Changefeed journal writer: one sequential put await per event

Status: `confirmed-open`. Current risk: `low`.

WRITER_BATCH bounds a receive loop whose individual events each await put. The trait has no batch seam; overflow rate and resulting throughput impact were not measured.

Evidence: [crates/shamir-tx/src/changefeed.rs:154](../../../../../crates/shamir-tx/src/changefeed.rs#L154); [crates/shamir-tx/src/changefeed.rs:599](../../../../../crates/shamir-tx/src/changefeed.rs#L599); [crates/shamir-tx/src/changefeed.rs:602](../../../../../crates/shamir-tx/src/changefeed.rs#L602); [crates/shamir-tx/src/changefeed.rs:645](../../../../../crates/shamir-tx/src/changefeed.rs#L645).

<a id="review-12"></a>

### Claim 12 — set_versioned_many and append_only: duplicate key vector

Status: `confirmed-open`. Current risk: `nit`.

Both methods still clone keys into a separate vector used by trailing vacuum, while their items vectors remain available.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:880](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L880); [crates/shamir-tx/src/mvcc_store/mod.rs:934](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L934); [crates/shamir-tx/src/mvcc_store/mod.rs:982](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L982).

<a id="review-13"></a>

### Claim 13 — remap_inner_value_bytes re-encodes rows that changed nothing

Status: `confirmed-open`. Current risk: `low`.

A nonempty remap still transforms every staged Set using decode, recursive remap and encode without detecting unchanged rows. A8 captured-ID optimization does not change this path.

Evidence: [crates/shamir-tx/src/id_remap.rs:77](../../../../../crates/shamir-tx/src/id_remap.rs#L77); [crates/shamir-tx/src/id_remap.rs:80](../../../../../crates/shamir-tx/src/id_remap.rs#L80); [crates/shamir-tx/src/tx_context.rs:943](../../../../../crates/shamir-tx/src/tx_context.rs#L943); [crates/shamir-tx/src/staging_store.rs:333](../../../../../crates/shamir-tx/src/staging_store.rs#L333).

<a id="review-summary-positive-hot-path-guarantees"></a>

### Claim Summary: positive hot-path guarantees — Batched writes, vectored reads and atomic cardinality mirrors

Status: `not-applicable`. Current risk: —.

These mechanisms exist, but one transact call is not universally one atomic durable backend transaction: Store's default implementation is sequential and capabilities are separately documented.

Evidence: [crates/shamir-tx/src/mvcc_store/mod.rs:898](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L898); [crates/shamir-tx/src/mvcc_store/mod.rs:1189](../../../../../crates/shamir-tx/src/mvcc_store/mod.rs#L1189); [crates/shamir-tx/src/predicate_set.rs:117](../../../../../crates/shamir-tx/src/predicate_set.rs#L117); [crates/shamir-storage/src/types.rs:243](../../../../../crates/shamir-storage/src/types.rs#L243).

## Corrections and qualified non-findings

- Retain source-level cost findings without claiming measured write-latency, throughput or IOPS changes.
- gc_below scans all history but buffers only eligible below-threshold versions; its physical keys are moved/conversion-preserved, not universally newly allocated copies.
- The duplicate lookup_ts requires max_age_secs; keep_history with neither cap returns immediately.
- One-key streaming requires a keyspace grouping guarantee in addition to sorted physical iteration; arbitrary variable-length key namespaces can interleave.
- swap_remove changes leftover ordering. Existing reverse emission does not by itself prove the new order is unobservable.
- The September borrowed-staging/A8 commit does not fix table-name clones or unchanged-row remapping.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-tx -- Performance & O(x->0)

## Summary

The crate's core hot paths largely honour pillar 3: lock-free `scc` structures with
`THasher` everywhere, O(1) atomic len mirrors where the need was recognised
(`PredicateSet::len_mirror`, `VersionedOverlay::count`), single batched
`history.transact` per write/batch, and vectored `get_many` reads. The residual
risk concentrates in (1) the per-write `vacuum_key` tail, which issues unbatched and
partially *duplicated* per-version storage round-trips on every write; (2) the GC
family (`gc_below` / `purge_below_ts`), which materialises the entire history store
in memory before deleting anything; and (3) a handful of helper scans that are
O(S)/O(K) per operation (`min_alive` per scan-path write, linear interval checks
over already-sorted footprint keys inside the serialised commit window, IndexMap
`shift_remove` in the stream group-by). Findings below are ranked by severity;
several are half-acknowledged in the code's own comments as future work.

## Findings

### 1. `vacuum_key`: unbatched + duplicated per-version I/O on the write hot path
- **File:line:** `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:100-121` (fast path), `:229-250` (scan path); call sites `mvcc_store/mod.rs:830`, `:934-936`, `:1022-1024`, `:1084`
- **Severity:** high
- **Issue:** `vacuum_key` runs after EVERY non-tx write (`set_versioned`,
  `delete_versioned`, and per-key inside both `set_versioned_many` variants). Even
  the L6 "fast path" (the CurrentOnly *default*) performs up to 3 sequential storage
  round-trips per overwrite once a deferred anchor exists: `lookup_ts` (a
  `history.get`, :105) plus two `remove_no_flag` awaits (version-key + ts-key,
  :109-111) — none folded into the `history.transact` that just landed the new
  version. The scan path (any non-CurrentOnly retention, or any live snapshot) is
  worse per reclaimed version: `lookup_ts` is awaited **twice** per version — once
  in the age-cutoff check (:229) and again for reclaim bookkeeping (:245) — followed
  by two more sequential removes (:246-247), all inside the per-write tail.
- **Failure scenario:** Default retention + steady overwrites of a hot key: each
  write = 1 transact + 1 get + 2 removes (4 sequential I/O ops) where 2 would do.
  With `Retention::keep_history`/`max_count` set, each write pays a full prefix scan
  of the key's versions plus up to 4 sequential ops × reclaimed versions — write
  latency grows linearly with per-key version count, exactly the hidden O(N)-per-op
  cost pillar 3 bans. For `set_versioned_many` this repeats per key with no
  cross-key batching.
- **Suggested fix:** (a) reuse the ts already fetched in the age check instead of
  the second `lookup_ts`; (b) collect scan-path removals into ONE
  `history.transact(Vec<KvOp>)` (or a batched remove) per vacuum call instead of
  per-version awaits; (c) in the fast path, append the prev-anchor's two deletes to
  the SAME transact as the incoming data write (the anchor is knowable pre-transact);
  (d) consider moving scan-path vacuum into the GC tick rather than the write tail.

### 2. `gc_below` / `purge_below_ts` materialise the whole history store before deleting
- **File:line:** `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:305-322` (gc_below Phase 1), `:396-473` (purge_below_ts)
- **Severity:** high (memory), medium (CPU)
- **Issue:** Phase 1 streams the ENTIRE history store into
  `TFxMap<Vec<u8>, Vec<(u64, Bytes)>>`: one `orig.to_vec()` heap allocation plus one
  `phys_key` clone per visited row, all held simultaneously. `purge_below_ts`
  buffers ALL versions of every key — including current, snapshot-pinned, anchored,
  and ts-ineligible rows — before Phase 2 filters. Peak transient memory is
  O(total history entries), unbounded relative to store size.
- **Failure scenario:** A GC tick (or T4 purge) on a large table buffers every
  below-threshold key + physical key at once: a 100M-entry history causes a RSS
  spike of that magnitude plus heavy allocator churn from per-row `Vec<u8>` allocs.
  Long-lived servers under periodic GC pay it every tick.
- **Suggested fix:** The store iterates key-major — stream with a one-key
  lookahead: accumulate only the current key's entries (bounded by its version
  count), sort, delete, release, and never build the global `per_key` map. At
  minimum, filter eligibility during the scan (purge can skip current / >= min_alive
  / unknown-ts rows before buffering) and evict map entries chunk-wise.

### 3. `min_alive()`: full-map iteration on the write tail and GC paths
- **File:line:** `crates/shamir-tx/src/repo_tx_gate.rs:651-670`; hot caller `mvcc_gc.rs:156`
- **Severity:** medium
- **Issue:** `min_alive` iterates the whole `active_snapshots` map (`iter_sync`)
  per call. The vacuum scan path calls it on every write while any snapshot is live
  (a live snapshot sets `vacuum_needs_scan`, forcing the scan path), and `gc_below` /
  `prune_version_cache` call it per GC tick. Per-write cost is O(S) in concurrently
  open snapshots. This is the same shape pillar 3 / `clippy.toml` bans for scc
  cardinality, and this crate has already fixed that class twice with atomic mirrors
  (`predicate_set.rs:85-86`, `versioned_overlay.rs:58-59`).
- **Failure scenario:** N concurrent long-lived readers + a hot write key: every
  write walks N map entries only to learn nothing new (the min changes only on
  snapshot open/drop).
- **Suggested fix:** Back `active_snapshots` with an AtomicUsize occupancy mirror
  for the emptiness fast-path, and serve `min_alive` from an ordered structure
  (`scc::TreeIndex<u64, refcount>`) or cache the computed min, invalidated on
  snapshot open/drop — O(1)/O(log S) instead of O(S).

### 4. `record_conflicts`: linear interval scan over already-sorted keys, inside the commit critical section
- **File:line:** `crates/shamir-tx/src/repo_tx_gate.rs:1009-1018` (linear scan), `:1089-1091` (the sort that makes binary search "free"), `:876-883` (call site under `commit_lock`)
- **Severity:** medium
- **Issue:** The `IndexRange` arm does `inserted_index_keys.iter().any(key_in_interval)`
  — O(K) per (record, dep) pair. `predicate_conflicts_batch` runs for Serializable
  commits under `commit_lock` (per its own calling-contract doc), so total cost is
  O(W x P x K) inside the serialised commit window (W = commits since snapshot,
  P = predicate deps, K = postings per footprint). The vec is explicitly sorted
  ascending at build time.
- **Failure scenario:** A Serializable tx with several range predicates validating
  against a busy commit window with wide footprints stretches the lock-held critical
  section linearly, throttling ALL commits behind it.
- **Suggested fix:** Replace `.any()` with two `partition_point` calls over the
  sorted vec (lower/upper bound), preserving the inclusive/exclusive `Bound`
  semantics — O(log K) per pair.

### 5. Stream group-by: per-version-row key allocation + O(K) `shift_remove` per group
- **File:line:** `crates/shamir-tx/src/mvcc_store/version_entry.rs:193` (per-row copy), `:124` (`shift_remove`), `:295` (leftover pop — already reverse order)
- **Severity:** medium
- **Issue:** (a) `Bytes::copy_from_slice(orig)` executes for EVERY decoded version
  row of the history stream but is consumed only when the key run changes — a scan
  over R keys x V versions performs R x V heap allocs where R suffice; the
  group-change comparison itself only needs the borrowed slice. (b) `flush_group`
  uses `TMap::shift_remove` — IndexMap's order-preserving remove is an O(K) memmove
  — once per history key group, i.e. O(N_keys x K_overlay) on a full stream. The
  leftover drain already emits in reverse index order (`leftover.pop()`), so
  `swap_remove` (O(1)) would not make observable ordering worse.
- **Failure scenario:** `current_stream` (list / scan / replication / migration
  path) over a large table while a bursty overlay window holds many undrained keys:
  avoidable per-row allocations plus memmoves quadratic in the overlay key count.
- **Suggested fix:** (a) move the `orig_bytes` copy inside the key-change branch;
  (b) switch to `swap_remove` and document the (already reversed) leftover order.

### 6. Pessimistic `locks` registry never evicts empty entries — unbounded growth
- **File:line:** `crates/shamir-tx/src/mvcc_store/mod.rs:139-143` (field), `mvcc_locks.rs:219-238` (`release_locks`, "GC is intentionally not done here")
- **Severity:** medium (unbounded growth)
- **Issue:** `release_locks` deliberately retains emptied `KeyLock` entries. Every
  distinct key ever pessimistically locked leaves a permanent `Arc<KeyLock>`
  (tokio Mutex + Notify) in the per-store map for the process lifetime. No bound,
  sweeper, or eviction path exists anywhere in the crate.
- **Failure scenario:** A long-running server whose pessimistic txs churn a large
  keyspace (row-at-a-time locking) grows this map monotonically — steady memory
  growth and progressively worse probe locality.
- **Suggested fix:** Opportunistically remove the entry when `holders` empties
  (tolerate the re-insert race via the `entry` API — a racing `lock_key` simply
  re-creates the lock), or sweep idle empty entries on the existing GC tick using a
  last-release timestamp.

### 7. `history_of`: N sequential `lookup_ts` point-reads
- **File:line:** `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:209-218`
- **Severity:** low/medium
- **Issue:** Phase 3 resolves each archived version's commit ts with one awaited
  `history.get` per entry — V sequential round-trips for a key with V versions. The
  crate already built and uses the batched `Store::get_many` seam
  (`mvcc_store/mod.rs:1189`, `:1590`) that collapses exactly this pattern.
- **Suggested fix:** Collect the `ts_key(version)` list, resolve via one
  `get_many`, then assemble `VersionEntry`s in order.

### 8. Vectored reads: sequential per-key awaits on fallback/cold slots + redundant re-probe
- **File:line:** `crates/shamir-tx/src/mvcc_store/mod.rs:1202-1207` (`get_at_many` Phase 3), `:1595-1615` (`get_current_many` Phase 3)
- **Severity:** low
- **Issue:** Fallback / Cold / FloorExceeded slots are resolved one awaited call at
  a time. `get_at_many` also re-runs `current_version(&keys[i])` (:1203) already
  computed in Phase 1. The doc's "cold is the minority in steady state" assumption
  (:1529-1531) does not hold during post-restart warm-up or cache-pruned read-mostly
  workloads, where every key is `Cold` and each pays a sequential
  `seek_latest_version` range scan.
- **Suggested fix:** Store `cur_v` in the `Slot` enum (drops the re-probe); resolve
  the fallback/cold subset with bounded concurrency or chunking. The intentionally
  unbatched range-scan fallback itself is documented and fine.

### 9. `VersionedOverlay::gc_upto`: full-tree collect-then-remove
- **File:line:** `crates/shamir-tx/src/versioned_overlay.rs:170-203`
- **Severity:** low
- **Issue:** `gc_upto` iterates the ENTIRE tree (visiting entries above the
  threshold too) and materialises a `Vec` of every qualifying entry (cloning each
  `RecordKey`) before issuing `remove_sync` per entry — a transient O(K) allocation
  and a double walk per drainer tick. The overlay is window-bounded by design, but
  the window grows with drainer lag under write bursts; the doc itself defers a
  version-major index (:168-169, "P1e may optimise").
- **Suggested fix:** Derive a version-major upper bound for a range iteration so
  only qualifying entries are visited, or remove during iteration with a cursor
  (re-probing by last-removed key) instead of pre-collecting; chunk the collect if
  cursor removal is impractical.

### 10. `project_event`: per-record heap clone of the table-name String
- **File:line:** `crates/shamir-tx/src/changefeed.rs:453-477`
- **Severity:** low
- **Issue:** `table.clone()` executes per staged op although it is loop-invariant
  per `(token, staging)` group — a 10k-row single-table commit pays 10k identical
  `String` allocations on the commit path.
- **Suggested fix:** Clone once per token, or have `RecordChange` carry an index
  into a per-event table list (also shrinks the serialised event).

### 11. Changefeed journal writer: one sequential `put` await per event (no batching seam)
- **File:line:** `crates/shamir-tx/src/changefeed.rs:557-614` (`journal_writer_loop` / `persist_one`), `:151-158` (`ChangelogStore` — single-item `put` only)
- **Severity:** low
- **Issue:** The module doc says the background writer "batches pending events",
  but the drain is a loop of one-at-a-time `store.put` awaits (`WRITER_BATCH` caps
  the count per loop, not the I/O shape). Sustained commit bursts drain at one
  round-trip per event, the 4096-deep channel fills, and events are dropped
  (bounded by design, but the drop rate is a throughput artefact, not a policy
  choice).
- **Suggested fix:** Add a batched `put_many` to `ChangelogStore` and drain up to
  `WRITER_BATCH` events per call; keep the drop-on-overflow policy as the backstop.

### 12. `set_versioned_many` / `_append_only`: duplicate key vector
- **File:line:** `crates/shamir-tx/src/mvcc_store/mod.rs:880`, `:982`
- **Severity:** nit
- **Issue:** `keys: Vec<RecordKey> = items.iter().map(|(k,_)| k.clone()).collect()`
  — an N-key clone per batch used only by the trailing vacuum loop, while `items`
  is never consumed (later loops borrow it). `for (key, _) in &items` at the vacuum
  site suffices.
- **Suggested fix:** Drop the `keys` vec; iterate `&items` in the vacuum loop.

### 13. `remap_inner_value_bytes` re-encodes rows that changed nothing
- **File:line:** `crates/shamir-tx/src/id_remap.rs:73-81` (with `tx_context.rs:913-929`)
- **Severity:** low
- **Issue:** Whenever a tx created any new field name (remap non-empty), EVERY
  staged row is fully decoded, walked, and re-encoded — including rows referencing
  no overlay ids, whose re-encode output is byte-identical to the input. Commit-path
  CPU is O(N x row size) even for the unchanged majority.
- **Suggested fix:** Have `remap_value` report whether any key was rewritten and
  return the original `Bytes` untouched when not (or pre-scan the remap's id set
  against the row's u64 keys before committing to a re-encode).

</details>
