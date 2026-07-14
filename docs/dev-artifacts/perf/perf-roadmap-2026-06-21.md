# S.H.A.M.I.R. Performance Roadmap — 2026-06-21

בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

Synthesis of 8 dimension-researcher reports into a ranked, ROI-driven attack plan.
ROI score = `(target_speedup × likelihood) / effort`, normalized 0–100.

---

## 1. Executive Summary

Eight dimensions were surveyed (read planner/filter eval, group-by/order-by/distinct,
WASM validators/UDFs, tx-commit/SSI/WAL/MVCC drainer, wire codec/auth/session, subscriptions/
changefeed, vector-HNSW/FTS, storage scan/membuffer/interner). Across them, **39 concrete
opportunities** (hot paths + hypotheses) were identified.

The dominant signal is not a shortage of micro-optimizations — it is a cluster of **asymptotic
cliffs** that masquerade as linear work and a pervasive pattern of **redundant per-record /
per-request re-derivation** of data that could be computed once. Two findings stand out as
true `O(N²)` (or worse) traps on hot, always-on paths:

1. **`FjallStore::scan_prefix_stream` does a full keyspace iter + linear cursor re-seek per
   batch** — `O((N + M²)/batch)` where the sibling `iter_stream` already shows the correct
   `O(log N + M)` range-seek. Fjall is the *sole durable backend*; every MVCC version-key walk
   and index-posting scan pays this. Small effort, low risk, unbounded payoff. **This is the
   single highest-ROI item in the entire survey.**
2. **The MVCC drainer re-reads + re-decodes + re-sorts the ENTIRE WAL on every commit-wake**
   (`drain_step` → `wal.recover()`), a latent `O(N²)` over WAL depth between truncations, and
   THE dominant cost on the durable write path under load.

The recommended first campaign attacks the two cheapest structural cliffs plus one zero-cost
correctness fix that rides along (§5).

---

## 2. Cross-cutting themes

These patterns recur across ≥2 dimensions and should shape a *shared* fix vocabulary rather
than N independent point-patches:

- **T1 — "Re-derive once, not per-row/per-request."** The same compiled artifact is rebuilt in
  the innermost loop across at least five dimensions:
  - Read planner: `FilterNode::matches()` rebuilds a `SmallVec<[InternerKey;4]>` from a `u64`
    path on *every node, every record*.
  - Group-by: `build_aggregate_object` re-parses + re-interns the select plan *once per group*.
  - WASM filter: `resolve_filter_query` re-interns field paths and re-hashes the *function name
    string* per row.
  - Subscriptions: `indexed_targets` allocates 2 `String`s per change per subscriber.
  - Auth: per-request `tickets_invalid_before_ns` re-reads + re-decodes the full `PersistedUser`
    blob to extract one `u64`.
  *Shared fix:* hoist resolution to compile/subscribe/session-creation time; store the resolved
  form (interned ids, fn-pointers, cached atomics).

- **T2 — "Single-pass field access (`O(F+K)` not `O(K·F)`)."** `RecordView::get/get_path` does an
  independent linear scan per field reference. A `FieldIndex` (one-pass `O(F)` offset map) already
  exists at `lens.rs:1062` but the filter/projection/aggregate paths never reuse it. This one lens
  primitive, threaded through filter eval **and** projection **and** group-key resolution,
  collapses the dominant full-scan per-record cost across the read dimension.

- **T3 — "Double-walk / double-decode of the same bytes."** Recurs everywhere: bytes-prefilter
  then full re-eval (read path); envelope-view then full `DbRequest` decode (wire); `snapshot_ops`
  Vec then `project_event` Vec (changefeed); bucketing decode then aggregate re-decode (group-by);
  WAL entry encode then drainer re-decode (tx). *Shared fix:* carry a "pre-confirmed/already-decoded"
  token forward instead of re-walking.

- **T4 — "Top-K / LIMIT pushdown."** The S2 ORDER-BY+LIMIT top-K heap is already landed for the
  ordered scan, but the same cliff is *unfixed* in FTS (`FtsRankedBackend::lookup` scores + fully
  sorts every matched doc, then `read_exec.rs:328` slices LIMIT *after*) and in non-grouped
  projection (materializes all N rows before LIMIT). Same `BinaryHeap<k>` pattern applies.

- **T5 — "Unconditional work for the empty case."** Changefeed `project_event` + journal run on
  every commit even with zero subscribers; `CachedStore` eager-mirrors the whole inner store at
  open; validators create a fresh WASM Store per row. *Shared fix:* gate on demand (atomic flag),
  amortize across a batch.

- **T6 — "String allocation in the hot loop."** Per-key `String` allocs in de-intern
  (`record_view_to_query_value`), per-row `String` clone in ORDER BY sort keys, per-ngram `String`
  in the tokenizer, per-probe `table.to_owned()` in subscriptions, per-row table `String` clone in
  `project_event`. *Shared fix:* `Arc<str>`/interned-id keys, borrowed `&str` sort keys.

- **T7 — "Interner spine clone-the-world."** `Interner::touch_ind` deep-copies the entire
  `Vec<Option<UserKey>>` (owned Strings) on every first-touch → `O(N²)` cold schema growth and WAL
  recovery replay. Touches both the storage and tx-recovery dimensions.

---

## 3. Top 10 ranked opportunities

| # | Title | Dimension | Target | Effort | Risk | ROI | Rationale |
|---|-------|-----------|--------|--------|------|-----|-----------|
| 1 | `scan_prefix_stream` range-seek rewrite | Storage | 10–100×+ (unbounded) | small | low | **98** | Sole durable backend; every prefix scan pays an `O(N+M²)` cliff with the correct `O(log N+M)` seek already present in the sibling `iter_stream`. Cheapest possible fix for the largest asymptotic win. |
| 2 | Incremental drainer cursor (drop per-pass `wal.recover()`) | Tx/WAL | 3–10× drain tput | large | medium | **82** | Removes latent `O(N²)` over WAL depth — THE dominant durable-write cost under load. Commit path already holds the `WalEntryV2`; push to a lock-free queue, fall back to `recover()` only on cold start. Unlocks #5, #8. |
| 3 | Single-pass `FieldIndex` for filter eval + projection | Read | 1.5–3× (grows w/ F,K) | large | medium | **80** | Highest-leverage *read* change: `O(K·F)→O(F+K)` per record, reusable across filter + projection + group-key. The `FieldIndex` primitive already exists, only un-threaded. |
| 4 | Per-request `tickets_invalid_before_ns` atomic cache | Wire/Auth | 2–4× ping/small-read RPS | medium | medium | **78** | 30–55% of small-request server CPU is 2 fjall LSM gets + a full `PersistedUser` decode to extract one `u64`, **with no bench coverage**. Write-rarely/read-every: an `scc::HashMap<[u8;16],AtomicU64>` collapses it to one atomic load. |
| 5 | FTS top-K / LIMIT pushdown into `FtsRankedBackend::lookup` | Vector/FTS | 3–20× on LIMIT queries | medium | medium | **76** | Exact analog of the already-shipped S2 ORDER-BY+LIMIT cliff, never applied to FTS. Replace full `TFxMap`-collect + full sort with a bounded `BinaryHeap<k>`. |
| 6 | `FilterNode` stores `InternerKey` path (kill per-record rebuild) | Read | 1.15–1.4× scalar filters | small | low | **74** | Representation-only change in ~14 `matches()` arms; removes a `SmallVec` rebuild (+ possible heap spill) per predicate per record. Zero semantic risk. |
| 7 | No-subscriber changefeed gate | Subscriptions | eliminates 100% of changefeed cost + ~2× store-write amp (no-sub repos) | medium | medium | **72** | `project_event` + always-on journal run unconditionally *before* the subscriber check at 4 call sites. Hoist a `changefeed_demand` atomic check. Huge for the common write-heavy/no-sub workload. |
| 8 | Interner reverse-spine: `Arc<str>` slots / segmented vec | Storage/Tx | `O(N²)→O(N)` cold growth | medium | medium | **70** | `touch_ind` clones the whole reverse `Vec` (owned Strings) per first-touch. Kills `O(N²)` schema warm-up + WAL-recovery interner replay. Read path untouched (single `ArcSwap` load). |
| 9 | Streaming hash aggregation in `apply_group_by` | Group-by | 2–4× + `O(N)→O(G)` mem | large | medium | **66** | Collapses a 2/3-pass, N-Bytes-cloning, per-group-plan-rebuilding design into one pass with a once-built plan. Memory-class change. |
| 10 | Batched WASM validator Store reuse | WASM | 3–8× validated bulk insert | large | medium | **64** | Validated bulk inserts pay one fresh `Store` + `instantiate_async` + record clone + msgpack encode **per row per validator**. Reuse one warm Store across a row-batch. |

**Runners-up (just outside top 10):** DISTINCT single-set collapse (1.5–2×, small/low — actually
an excellent quick win, ROI ~73 but capped here for variety); fixed-width FTS posting codec
(1.3–2×, small/low); borrowed/interned ORDER BY sort keys (2–3×, medium); double msgpack-decode
elimination on the wire (1.3–1.8×); multi-index AND intersection (2–10× on a query class the
planner leaves on the table, large).

---

## 4. Per-dimension findings (condensed)

### D1 — Read planner / filter eval / index dispatch
- **Cliffs:** `FilterNode::matches()` rebuilds `SmallVec<InternerKey>` per node/record; `RecordView::get/get_path` is `O(K·F)` (independent scans, `FieldIndex` un-reused); bytes-prefilter double-walks surviving rows (re-eval); `InSet`/`Contains` materialize a full owned subtree for a scalar probe.
- **Top hypotheses:** single-pass `FieldIndex` (1.5–3×, large) = biggest read lever; `InternerKey`-typed path (1.15–1.4×, small); pre-filter de-dup confirmed-flag (1.2–1.5×); borrowed-scalar `IN` fast path (1.3–2×); multi-index AND intersection (2–10×, asymptotic).
- **Note / blind spot:** `try_plan_order_limit_fast_path` reportedly BAILS even when a sorted index exists on the ORDER BY field (N=1M → 14.8s vs sub-ms) — *orders of magnitude*, possibly already fixed post-#128/#130. **Re-measure on HEAD before any micro-work** — it dwarfs everything else here.

### D2 — Group-by / order-by / distinct / projection
- **Cliffs:** `apply_group_by` 2/3-pass, clones N Bytes into per-group Vecs, rebuilds the agg plan per group, re-decodes rows 2–3×; ORDER BY clones full `String` per row for sort keys; DISTINCT does a redundant 2nd `FxHashSet` + 3rd pass over an already-ordered `IndexSet`.
- **Top hypotheses:** streaming hash aggregation (2–4×, `O(N)→O(G)` mem, large); DISTINCT single-set collapse (1.5–2×, small/low — quick win); borrowed/interned sort keys (2–3×, medium); late materialization for ORDER BY+LIMIT (projection `O(N)→O(K)`, 2–5×, large). Accumulator inner loop already optimal — leave it.

### D3 — WASM validators / UDFs / funclib
- **Cliffs:** per-call WASM `call` builds a fresh Store + `instantiate_async` + 2× memory copy + 2× msgpack with zero reuse; validators run per-row per-validator with a deep `record.clone()` each; scalar-fn filter dispatch re-interns paths + re-hashes the fn *name string* per row.
- **Top hypotheses:** batched validator Store reuse (3–8×, large); per-validator clone elimination via shared encoded bytes (1.5–2.5×); compile-time fn-pointer binding (1.3–2×); extend zero-alloc bytes-prefilter to `lower/upper/trim/length` computed compares (2–4×). Registries + wasmtime engine config already optimal.

### D4 — Tx commit / SSI / WAL / MVCC drainer
- **Cliffs:** drainer `wal.recover()` re-reads/decodes/sorts the ENTIRE WAL per wake (latent `O(N²)`); `gc_upto` full-iterates the overlay B+ tree per drain pass; values materialized twice (WAL body + overlay); per-append `Arc<Waiter>` alloc in group-commit; redundant warm-path `publish_cell`.
- **Top hypotheses:** incremental drainer cursor (3–10×, large) = dominant; version-major overlay GC index (2–4×, medium); skip in-process WAL encode/decode (1.3–1.6×); drop redundant `publish_cell` (1.1–1.3×, small); waiter-pool in group commit (1.2–1.5×). Ack path already well-optimized (Buffered = no hot fsync). **Do NOT pursue single-writer-task WAL rewrite — already prototyped & reverted (+22% mem latency).**

### D5 — Wire codec / auth / session / permissions
- **Cliffs:** per-request `tickets_invalid_before_ns` = 2 fjall gets + full `PersistedUser` decode for one `u64`, no cache, **unbenched**; double msgpack decode (envelope-view then full `DbRequest`/`BatchRequest`); `record_view_to_query_value` allocates a `String` per key per row on read responses; 3× `batch.queries` walks (admin/destructive/lifecycle); `to_vec_named` emits field-name strings per frame.
- **Top hypotheses:** atomic `tickets_invalid` cache (2–4×, medium) = top; borrowing/lazy `DbRequest` decode (1.3–1.8×); endgame S-read passthrough → client de-intern (2–3×, xlarge, overlaps read campaign); fuse the 3 batch passes (1.05–1.1×, small); positional msgpack (1.1–1.3×, wire-break). Argon2/SCRAM/SessionStore already optimal — do not touch.

### D6 — Subscriptions / changefeed / filter lens
- **Cliffs:** `project_event` runs unconditionally on every commit before the subscriber check (2 Vec allocs + per-row table `String` clone + `SystemTime::now()` syscall); `indexed_targets` 2 `String` allocs per change per subscriber; `PushEnvelope` re-encoded + data-copied per subscriber despite Arc-shared data; per-event full-`retain()` cache eviction across all bridges (`O(cache_size)`); always-on journal = ~2× write amp on no-sub repos.
- **Top hypotheses:** no-subscriber demand gate (100% elimination, medium) = top; intern table→id (1.3–1.8× fan-out); shared envelope body / seq-patch (1.2–1.5×, large); watermark eviction via `TreeIndex` range-drain (1.2–1.4×); fuse `project_event`/`snapshot_ops` (1.2–1.4×, small).

### D7 — Vector (HNSW) + FTS
- **Cliffs:** `FtsRankedBackend::lookup` scans full posting lists, builds `Vec<BTreeSet>`, folds `&acc & &s` (fresh alloc/fold), scores + fully sorts every matched doc with NO top-k pushdown; bincode per 8-byte posting; small-index brute path clones every `Vec<f32>` + 2N awaits; one `spawn_blocking` per vector on build; tokenizer allocs `String` per ngram.
- **Top hypotheses:** FTS top-K pushdown (3–20×, medium) = top; fixed-width LE posting codec (1.3–2×, small); smallest-first galloping AND intersection (2–5×, medium); int8/PQ vector quantization (2–4× + 4× mem, xlarge — needs a recall harness first); brute-path async/clone cleanup (1.5–3×, small); parallel HNSW build (2–4×). SIMD distance kernels already optimal — do NOT touch. Recommended FTS campaign order: top-K → codec → intersection.

### D8 — Storage scan / MemBuffer / Cached / Interner
- **Cliffs:** `scan_prefix_stream` full keyspace iter + linear `position()` re-seek per batch (`O(N+M²)`) — worst cliff in the area, sole durable backend; `Interner::touch_ind` clones entire reverse `Vec<String>` per first-touch (`O(N²)`); MemBuffer `drain_once` double-clones + per-key relock + unsorted drain (poor LSM locality); fjall `insert/set` redundant `contains_key` pre-check; `CachedStore` eager full-mirror at open + per-write `tokio::spawn`.
- **Top hypotheses:** `scan_prefix_stream` range-seek (10–100×+, small/low) = #1 overall; interner reverse `Arc<str>`/segmented vec (`O(N²)→O(N)`, medium); sorted-batch MemBuffer drain (1.5–2×); drop fjall `contains_key` pre-check (15–30%, small); batched CachedStore async drainer (2×). **Correctness rider:** `insert_many` omits the `dirty_nonempty` sentinel set → fast-path `get()` stale-miss window.

---

## 5. Recommended first phase — "Asymptotic cliffs + a free correctness fix"

The first campaign should bank the **two cheapest structural cliffs** plus the **zero-cost
correctness rider** that lives in the same file. Rationale: maximize realized payoff per unit of
risk, stay inside crate boundaries that don't require wire/format breaks, and establish the
benchmark scaffolding the larger campaigns (drainer cursor, single-pass FieldIndex) will reuse.

### Op A — `FjallStore::scan_prefix_stream` range-seek rewrite *(ROI 98 — lead)*
- **What:** Replace the full-keyspace `iter()` + `iter().position()` cursor re-seek with
  `keyspace.range((Excluded(cursor)|Included(prefix), Unbounded)).take_while(|k| k.starts_with(prefix)).take(batch_size)`,
  mirroring the already-correct `iter_stream` (`storage_fjall.rs:323-371`).
- **Why first:** Sole durable backend; *every* MVCC version-key walk and index-posting scan rides
  this path. Small effort, low risk, payoff unbounded in table size. Nothing else in the survey
  has this risk/reward ratio.
- **Sketch:** (1) Unit test asserting identical row set + order vs current impl over a seeded
  prefix. (2) Rewrite the cursor loop. (3) Confirm fjall `keyspace.range` yields lexicographic
  (LSM) order — resolve the `storage_fjall.rs:390` "order not guaranteed" comment (the comment
  refers to `iter`, range is sorted; verify). (4) Bench: add a 50k-row shared-prefix case to
  `store_raw.rs`; `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-storage --bench store_raw --features fjall`.
- **Gate:** `./scripts/test.sh -p shamir-storage --full`.

### Op B — Interner reverse-spine de-clone *(ROI 70)*
- **What:** Change `Interner.reverse` slots from owned `UserKey`/`String` to `Arc<str>` (lower-risk:
  clone bumps N refcounts instead of deep-copying N Strings), or a segmented append-only vec
  (full `O(1)`-append fix) if a `boxcar::Vec`-equivalent is already vendored.
- **Why bundled:** Same `O(N²)`-cliff class, independent crate (`shamir-types`), and it directly
  de-risks WAL-recovery latency (`touch_with_id` replay shares the pathology) — complementing Op A's
  storage-layer scan fix with the recovery-layer growth fix. Read path (`get_str`/`with_str`, single
  `ArcSwap` load) is untouched.
- **Sketch:** (1) Extend `interner_cold_growth.rs` with an in-memory-only `touch_ind` variant (no
  persist) at N=1k/5k/20k to isolate the spine clone. (2) Confirm current scales ~N², fixed ~N.
  (3) Run `interner_concurrent.rs` to prove read throughput unchanged.
- **Gate:** `./scripts/test.sh -p shamir-types -- interner`.
- **Decision gate (open Q):** prefer the `Arc<str>`-slot variant unless a vendored segmented vec is
  confirmed — avoid introducing a new dependency for the first campaign.

### Op C — `MemBufferStore::insert_many` dirty-sentinel fix *(correctness rider, ~free)*
- **What:** Set `dirty_nonempty` (Release) *before* the `dirty.insert` loop, matching
  `set_many`/`remove_many`. Closes the fast-path `get()` window where a concurrent reader sees
  `dirty_nonempty==false` (Acquire), skips the dirty probe, and stale-misses an `insert_many`'d key.
- **Why bundled:** Same file/crate as the MemBuffer drain work, trivially small, low risk, and it's
  a latent consistency bug — fix it while the storage crate is already open and under test.
- **Sketch:** Add a targeted test (`insert_many` a key, `get()` from another task before flush,
  assert hit), then the one-line sentinel set.
- **Gate:** `./scripts/test.sh -p shamir-storage -- membuffer`.

**Why these three together:** all three land in `shamir-storage` + `shamir-types` (no cross-crate
wire/format break, no client lockstep), the lead op (A) is the highest-ROI item in the whole survey,
and B + C ride the same crates and test infra. This campaign also produces the `store_raw` /
`interner_cold_growth` bench scaffolding that the **next** campaign — Op #2 (drainer cursor) and
Op #3 (single-pass FieldIndex) — will build on. Defer the larger #2/#3 to a dedicated second phase
once this risk-light foundation is banked and re-measured.

**Pre-flight before phase 2:** re-measure the `try_plan_order_limit_fast_path` ORDER-BY-on-sorted-index
bail (D1 blind spot) on current HEAD — if still failing it is an *orders-of-magnitude* win that
outranks every per-record micro-op and should be promoted ahead of #3.

---

## 6. Open questions / blind spots

- **B1 — Stale baseline for the biggest read win.** `try_plan_order_limit_fast_path` reportedly
  bails even when a sorted index covers the ORDER BY field (N=1M → 14.8s vs sub-ms). May already be
  fixed post-#128/#130. **Must re-measure on HEAD** before committing to per-record read micro-work —
  it dwarfs the entire D1 report.
- **B2 — No bench coverage on two top items.** The per-request `tickets_invalid_before_ns` §7.5 path
  (D5) and the scalar-fn-in-filter dispatch (D3) are *invisible* to the current bench suite
  (`db_handler_rps`/`wire_latencies` bypass `dispatch_request_view`; `filter_eval` may lack FnCall
  cells). New benches are a *prerequisite* to prove those wins — the cited speedups are structural
  estimates, not measurements.
- **B3 — Workload distribution unknown.** Several rankings hinge on production shape we don't have:
  group cardinality G/N (streaming aggregation), subscriber-count distribution (fan-out vs no-sub
  gate), record width F (FieldIndex amortization), validator multiplicity, WAL depth between
  truncations (drainer cliff magnitude), and whether FTS queries actually carry finite LIMITs
  (top-K win). A workload-signal pass would sharpen the §3 ordering.
- **B4 — Format/wire-break gating.** Three high-value items require version negotiation or a
  migration story: positional msgpack (D5, client lockstep), endgame S-read passthrough (D5,
  cross-crate xlarge), fixed-width FTS posting codec (D7, on-disk byte change). Kept out of phase 1
  deliberately.
- **B5 — Recovery/correctness contracts.** The drainer cursor (#2) must remain *byte-identical* for
  cold recovery (still `wal.recover()`-sourced); the WASM batched-Store (#10) must preserve per-row
  fuel-budget / DoS guarantees and needs an ABI `shamir_reset` decision; the changefeed gate (#7)
  needs product sign-off on whether the durable journal is contractually always-on. Each is a design
  decision, not just an implementation detail.
- **B6 — Missing recall harness for vector quantization.** Int8/PQ (#7-tier, D7) cannot be validated
  without a recall@k ground-truth harness that does not yet exist — building it is a prerequisite
  task, and quantization stays deferred until then.
- **B7 — fjall API affordances.** Does fjall 3.0 `Keyspace::insert` return the prior value (would make
  the `contains_key` pre-check removal strictly free)? Does `keyspace.range` guarantee lexicographic
  order (gates Op A correctness)? Confirm before the storage micro-ops past phase 1.

---

## 7. Outcomes — campaigns landed since this roadmap

- **Op A/A.2 — `scan_prefix_stream` range-seek** (fjall 164×, sled 50×). ✅ 98f256d / 7140f82
- **Op B — interner reverse-spine `Arc<str>`** (O(N²)→O(N) cold growth). ✅ 35ebd40
- **Op C — MemBuffer `*_many` dirty-sentinel** (correctness). ✅ d2d3504
- **Op #2 — incremental drainer cursor** (window + offer + gap-reseed +
  backpressure; removes the per-drain `wal.recover()` O(N²)). ✅ be0bc1f,
  92311d4 (offer O(1) depth + abort-aware reseed).
- **§4 D4 "version-major overlay GC index"** — INVESTIGATED, NOT pursued.
  Stage 0 measurement (`docs/dev-artifacts/perf/hidden-on-sweep-stage0.md`) proved
  `gc_upto`'s O(N) cliff is theoretical: Op #2 keeps the overlay window ≤3
  entries even on fjall under sustained burst. Restoring drainer health, not
  GC micro-opt, is the right lever if the overlay ever grows. ✅ measured 0a9571f
- **§6 D6 "per-event full-`retain()` cache eviction O(cache_size)"** —
  RESOLVED. decode/deliver caches migrated to `scc::TreeIndex` with a
  CV-first key; eviction is now `remove_range` (O(evicted+log N)). ✅ 3eb7601
- **Hidden-O(N) guard** — `scc::*::len()` (O(N) `iter().count()`) banned via
  `clippy.toml`; CLAUDE.md pillar #3 documents the atomic-mirror pattern. ✅ 0f5de6b
- **#128 pagination regression** — top-K + sorted-index `ORDER BY+LIMIT`
  fast paths dropped pagination metadata on the wire; fixed structurally via
  a shared `exec::fast_path_pagination` helper + a completeness-critic
  contract test. ✅ 604cc47, a37c950
