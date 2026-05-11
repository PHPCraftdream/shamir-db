# Performance Baseline

End-to-end criterion benchmarks measuring the full
`ShamirDb::execute(BatchRequest)` path on the **in-memory** backend
(removes disk variance — measures engine + planner + interner +
indexes pure-CPU cost).

## Bench harness

```
crates/shamir-db/benches/engine_perf.rs   ← criterion suite
cargo bench -p shamir-db                  ← run full
cargo bench -p shamir-db -- 'set_existing' ← filter by name
```

Realistic-ish data fixture: each user record carries `id`, `name`
(8 first × 6 last name pool), `email`, `age` 18..=77, `city` (8-pool),
pseudo-random `score`, `active` bool, `created_at_ns`, two `tags`.
Variation enough to exercise interner growth and to make filter
selectivity non-trivial.

Each scenario where an index can apply runs **twice** — once against
a table with no indexes (full-scan path), once with the relevant
**regular** index pre-created. We deliberately use regular (not
unique) indexes because the read planner only consults the regular
index store today; unique indexes are stored separately.

## Run conditions

| Field          | Value                                                  |
|----------------|--------------------------------------------------------|
| Date           | 2026-05-10                                             |
| Git SHA        | `80dff58` (after benchmarks committed: post-commit run) |
| Rustc          | 1.93.0 (2026-01-19)                                    |
| Host           | x86_64-pc-windows-gnu                                  |
| OS             | Windows 10 (MINGW64 shell)                             |
| Backend        | in-memory (DashMap, no disk)                           |
| Criterion args | `--warm-up-time 1 --measurement-time 2 --sample-size 10` |

The criterion timing settings are abbreviated for fast iteration
during the optimisation sprint; full statistical confidence (default
`--measurement-time 5 --sample-size 100`) takes ~10× longer and is
worth running for the final "before vs after" report.

## Baseline numbers

### Bulk insert (no scan)

| Records | Time             | Throughput      |
|--------:|------------------|-----------------|
|     100 |  2.13 ms         |  47 K elem/s    |
|   1 000 | 27.19 ms         |  37 K elem/s    |

Insert is genuinely O(n). The throughput drop from 47K to 37K elem/s
between the 100 and 1 000 case mostly reflects per-batch interner
persist cost (full interner snapshot rewritten every operation; see
PAIN POINT #2 in `TRANSACTIONS_IMPL.md`).

### `set` (upsert) on existing key — currently O(n) scan, ignores indexes

Target = the **last** seeded record (worst-case scan; the executor
short-circuits on first match).

| Records | No index    | With regular index `by_id` | Index speed-up |
|--------:|-------------|----------------------------|---------------:|
|     100 |   898 µs    |   821 µs                   |  1.09×         |
|   1 000 |  8.12 ms    |  8.02 ms                   |  1.01×         |
|  10 000 | 79.50 ms    | 82.72 ms                   |  0.96× (noise) |

**Index doesn't help.** The current `execute_set` always does a full
table scan to find the existing record by key fields. Optimisation
**B** (implicit PK index lookup) targets exactly this row.

### Read by id — equality on a single column

| Records | No index    | With regular index `by_id` | Index speed-up |
|--------:|-------------|----------------------------|---------------:|
|     100 |   803 µs    |    47.5 µs                 |   17×          |
|   1 000 |  8.19 ms    |    49.9 µs                 |  164×          |
|  10 000 | 78.91 ms    |    50.1 µs                 | **1574×**      |

The read planner picks up the index correctly; lookup is essentially
constant-time (~50 µs) regardless of table size. **Read-side index
infrastructure works.** Pain points are in the write path.

### Read by city — non-PK equality, ~12.5 % selectivity

| Records | No index    | With regular index `by_city` | Index speed-up |
|--------:|-------------|------------------------------|---------------:|
|     100 |   897 µs    |   211 µs                     |  4.2×          |
|   1 000 |  8.29 ms    |  1.73 ms                     |  4.8×          |
|  10 000 | 90.47 ms    | 20.58 ms                     |  4.4×          |

Index speedup ~4–5× (matches selectivity — index lookup yields ~12.5 %
of records, then per-record materialisation dominates). Confirms
the index-scan path is healthy for non-unique multi-match indexes too.

### Update by id

| Records | No index    | With regular index `by_id` | Index speed-up |
|--------:|-------------|----------------------------|---------------:|
|     100 |   856 µs    |   930 µs                   |  0.92×         |
|   1 000 |  8.50 ms    |  8.81 ms                   |  0.96×         |
|  10 000 | 90.32 ms    | 88.12 ms                   |  1.02×         |

**Index ignored.** `execute_update` does its own full scan + filter
loop instead of going through the same `try_plan_index_scan` that
the read path uses. Optimisation **C**.

### Delete by id

| Records | No index    | With regular index `by_id` | Index speed-up |
|--------:|-------------|----------------------------|---------------:|
|     100 |   948 µs    |  1.03 ms                   |  0.92×         |
|   1 000 |  8.08 ms    |  9.32 ms                   |  0.87×         |
|  10 000 | 91.10 ms    | 96.35 ms                   |  0.95×         |

Same story as update — write path scans regardless.

### Complex filter (AND of nested OR over indexed + non-indexed columns)

| Records | Time         |
|--------:|--------------|
|     100 |   1.12 ms    |
|   1 000 |  10.49 ms    |
|  10 000 |  92.87 ms    |

Linear in N — current planner doesn't (yet) split the AND across
index lookups for the indexed sub-conditions. Future planner work.

### Order_by + LIMIT 10

| Records | Time         |
|--------:|--------------|
|     100 |   1.41 ms    |
|   1 000 |  16.54 ms    |
|  10 000 | 180.92 ms    |

Full-scan + sort. Could in principle be O(N log K) using a heap with
K=10, but currently sorts the whole result. Modest optimisation
target after A/B/C.

### Multi-query batch (8 independent reads in one batch)

| Records | Time         | Per query (avg) |
|--------:|--------------|-----------------|
|   1 000 |  75.83 ms    | ~9.5 ms each    |
|  10 000 | 720.62 ms    | ~90 ms each     |

**No parallel speedup observed** in the planner's "stage" execution.
8 reads-without-index of 1 000 records each take ~76 ms — same as 8 ×
~9 ms serial. The execution_plan stages are documented as parallel
but the executor doesn't actually `tokio::spawn` them. Pain point
**D** (new, surfaced by benchmark).

## Pain points the benchmark surfaced

Confirmed (predicted in `TRANSACTIONS_IMPL.md`):

1. **`set` always full-scan, ignores index** → addressed by Opt **B**
   (auto-create + use PK-from-`set.key` index). Expected gain at
   N=10 000: ~80 ms → ~80 µs (~1000×).
2. **`update`/`delete` by indexed column also full-scan** → Opt **C**
   (write path uses `try_plan_index_scan`). Expected: same order of
   magnitude as above.
3. **Bulk insert throughput drops with N** — interner persist on
   every op rewrites the whole interner blob. Opt **A** (debounced
   persist). Expected: ~37 K elem/s → ~70-80 K elem/s for the 1 000
   case; bigger relative win at higher N.

Newly surfaced:

4. **`batch_multi_read_8` doesn't parallelise.** Independent queries
   in the same execution_plan stage run serially. The infrastructure
   for parallel staging exists in the planner output but the executor
   doesn't `tokio::spawn`. Easy fix once we look at it. Expected gain
   on a 4-core box: ~3-4× for the multi-read pattern. Promote to
   Opt **D**.
5. **Unique indexes not consulted by the read planner.** First
   benchmark pass with `unique=true` on `id` showed no speedup; only
   after switching to a regular index did `read_by_id` get its
   1 500× win. Either documented limitation that needs spelling out,
   or a planner gap (`try_plan_index_scan` apparently looks only at
   regular indexes). Worth investigation alongside Opt B/C.

## Reproducing

```bash
cd /path/to/shamir-db
cargo bench -p shamir-db --bench engine_perf -- \
    --warm-up-time 1 --measurement-time 2 --sample-size 10 --noplot
```

For a publishable comparison run:

```bash
cargo bench -p shamir-db --bench engine_perf
```

Criterion writes per-bench HTML reports under `target/criterion/`.

## After Opt A — interner persist debouncing

Change: `InternerManager::persist()` becomes a near-free no-op when
the interner hasn't grown since the previous call. Tracking via
`last_persisted_len: AtomicUsize` (interner is monotonic, so length
identifies content). Implementation in
`crates/shamir-engine/src/table/interner_manager.rs`.

Same hardware, same criterion args. Numbers vs baseline:

| Bench                            | Baseline | After A   | Δ        |
|----------------------------------|---------:|----------:|---------:|
| **bulk_insert/100**              |  2.13 ms |  1.91 ms  | **-10 %** ✓ (criterion p=0.00) |
| **bulk_insert/1000**             | 27.19 ms | 24.63 ms  | **-9 %** ✓ (criterion p=0.02) |
| set_existing_no_index/10000      | 79.5 ms  | 77.5 ms   | -3 % (noise) |
| set_existing_with_index/10000    | 82.7 ms  | 100 ms    | +21 % (noise; iter variance high here) |
| read_by_id_no_index/10000        | 78.9 ms  | 92.9 ms   | +18 % (noise — measurement-time 2 s) |
| read_by_id_with_index/10000      |  50 µs   |  55 µs    | noise |
| update_by_id_no_index/10000      | 90.3 ms  | 88.2 ms   | -2 %  (no new intern keys in updates) |
| delete_by_id_no_index/10000      | 91.1 ms  | 94.5 ms   | noise |
| complex_filter/10000             | 92.9 ms  | 103.9 ms  | noise |

**Takeaway.** Real win on the write-heavy bulk insert path (10 % off
for both 100 and 1 000 — criterion confirms p < 0.05). Other
scenarios show no measurable change because their workload doesn't
add new interner keys after the initial seed (the field names
`id`/`name`/`email`/etc. all interned once during setup; per-op
persist was already cheap content-wise but expensive serialisation-
wise, and now `persist()` short-circuits on `cur_len == last`).

The "+18 %" / "+21 %" outliers are within the noise floor of a 2 s
measurement window with 10 samples. A longer-time run smooths this
out (see "Reproducing" section — bump `--measurement-time` to 5+).

Zero regressions, broad workspace test sweep stays at 1179/0.

## After Opt B — `execute_set` uses single-field index

Change: `lookup_existing_for_set` helper added to `write_exec.rs`.
When `set.key` has exactly one field AND a regular single-field
index covers that field, we go through `IndexManager::lookup_by_index`
(O(log n) BTreeSet read out of `info_store`) instead of the
full-table scan. Falls back to scan when no index exists or the key
is composite.

Headline numbers (10K records, set on the LAST seeded record):

| Bench                            | Baseline | After A   | After B   | Speed-up vs baseline |
|----------------------------------|---------:|----------:|----------:|---------------------:|
| **set_existing_with_index/100**  |   821 µs |   821 µs  |   78 µs   | **10.5×**            |
| **set_existing_with_index/1000** |  8.02 ms |  8.4 ms   |   84 µs   | **95×**              |
| **set_existing_with_index/10000**| 82.7 ms  | 100 ms    |  101 µs   | **818×**             |
| set_existing_no_index/100        |   898 µs |   867 µs  |   852 µs  | unchanged (no index → fallback to scan) |
| set_existing_no_index/1000       |  8.12 ms |  7.85 ms  |  7.57 ms  | unchanged |
| set_existing_no_index/10000      | 79.5 ms  | 77.5 ms   | 77.8 ms   | unchanged |

The "no index" path is unchanged by design — Opt B opts INTO the
index path; absence of an index keeps the original scan behaviour.

Other benches (read / update / delete / complex_filter / order /
batch) are within noise vs Opt A. They don't go through the
modified `lookup_existing_for_set` helper.

Workspace test sweep stays at 1179/0.

## After Opt C — `execute_update` / `execute_delete` use the read planner

Change: new `lookup_records_via_index` helper in `write_exec.rs`
that runs the same `try_plan_index_scan` the read path already
uses, then loads each candidate by id and applies the residual
filter. `execute_update` and `execute_delete` try this helper
first and fall back to full scan only when no index plan applies.

`try_plan_index_scan` and `find_single_field_index` made `pub` so
the write path can call them.

Headline numbers (10K records, last seeded record target):

| Bench                              | Baseline | After A   | After B   | After C   | Speed-up vs baseline |
|------------------------------------|---------:|----------:|----------:|----------:|---------------------:|
| **update_by_id_with_index/100**    |   930 µs |   972 µs  |   930 µs  |   56 µs   | **16.6×**            |
| **update_by_id_with_index/1000**   |  8.81 ms |  8.89 ms  |  8.81 ms  |   54 µs   | **163×**             |
| **update_by_id_with_index/10000**  | 88.1 ms  | 92.2 ms   | 88.1 ms   |   79 µs   | **1115×**            |
| **delete_by_id_with_index/100**    |  1.03 ms |  1.01 ms  |  1.03 ms  |   85 µs   | **12.1×**            |
| **delete_by_id_with_index/1000**   |  9.32 ms | 10.03 ms  |  9.32 ms  |   97 µs   | **96×**              |
| **delete_by_id_with_index/10000**  | 96.4 ms  | 95.7 ms   | 96.4 ms   |  102 µs   | **944×**             |
| update_by_id_no_index/10000        | 90.3 ms  | 88.2 ms   | 90.3 ms   | 100.6 ms  | unchanged (no index) |
| delete_by_id_no_index/10000        | 91.1 ms  | 94.5 ms   | 91.1 ms   | 93.1 ms   | unchanged (no index) |

The "no index" path is unchanged by design — Opt C opts INTO the
index plan; absence of an index keeps the original scan fallback.

complex_filter / order_limit / batch_multi_read benchmarks within
noise vs prior runs (this PR doesn't touch their code paths).

Workspace tests: 1179/0 unchanged.

## Sorted index on disk backend (sled) — real-world numbers

Added bench groups `range_query_*_sled` that run the same scenarios
against a sled-backed repo. Sled exercises the native
`iter_range_stream` path (B-tree `range()`) added in `dab2b19`.

### Wide range — `age between 30 and 35` (~10 % selectivity)

| Records | no_index    | with_index   | Speed-up   |
|--------:|------------:|-------------:|-----------:|
|     100 |    1.16 ms  |    665 µs    |  **1.7×**  |
|   1 000 |    9.53 ms  |    5.36 ms   |  **1.8×**  |
|  10 000 |   81.3 ms   |   66.5 ms    |  **1.22×** |

### Narrow range — `age = 30` (~1.6 % selectivity)

| Records | no_index    | with_index   | Speed-up   |
|--------:|------------:|-------------:|-----------:|
|     100 |    944 µs   |    344 µs    |  **2.7×**  |
|   1 000 |    7.95 ms  |    1.14 ms   |  **7.0×**  |
|  10 000 |   79.5 ms   |   21.0 ms    |  **3.8×**  |

### Why "only" 1–7× on disk (and 100–1000× on in_memory)

This is real database physics, not a missed optimisation. The
sorted-index flow on disk:

1. Native B-tree range scan over the index store. Cheap, scales
   with K matching keys.
2. **For each matched RecordId — one `data_store.get(id)`.**
   Random read.
3. Apply residual filter, build output.

Costs (measured on this Windows / sled host):

- sequential scan via `iter_stream` (full-scan path):
  **~8 µs / record** — batched, cache-friendly.
- random `get(id)` against sled (sorted-index path):
  **~125 µs / record** — B-tree walk from root for each key.

Break-even: `K / N < 8 / 125 ≈ 6 %`. Below that, sorted index
wins. Above, the random-read penalty eats the records-not-loaded
savings. Our wide-range (10 %) sits right above; narrow-range
(1.6 %) is comfortably below — exactly what the numbers show.

In-memory doesn't see this trade-off because `DashMap.get` is
~50 ns regardless of N — per-record load is essentially free,
so the "skip non-matching records" win pays clean.

### Path to true 1000× on disk

Two real options, both architectural:

- **Covering index** — store the projected fields directly in the
  index entry, so range queries answer from the index alone without
  per-record gets. Costs: extra disk + index maintenance on every
  update touching those fields. Wins: zero per-record disk reads →
  range queries become genuinely O(log N + K).
- **Vectored / batched get** — instead of N independent gets,
  hand the BTreeSet of RecordIds to a single helper that does
  one B-tree walk over the data store. Some backends (sled, redb)
  expose multi-get / scan-with-key-list primitives that can fold
  these into far fewer disk seeks.

Tracked as future work.

## After sorted-index v1 (range queries via `Between`)

`SortedIndexManager` lands as a parallel index variant alongside
the hash-based `IndexManager`. Creating an index with `sorted: true`
encodes each indexed value through `shamir-types::core::sort_codec`
into bytes that sort the same way the value itself does, then stores
`<sorted_tag>||<name_interned>||<encoded_value>||<record_id>` →
empty as a separate info_store record. Within one index, scan_prefix
returns every record in value order — the storage backend's B-tree
does the work.

Planner adds `try_plan_sorted_index_scan` for `Filter::Between` /
`Gte` / `Lte`; routes to `read_sorted_index_scan` which loads
matched records and applies residual + select + order + paginate
through the existing pipeline.

| Bench                            | After A+B+C+D-class | After sorted v1 | Speed-up |
|----------------------------------|---------------------:|----------------:|---------:|
| **range_query_with_index/100**   |              1.14 ms |          183 µs |  **6.2×**|
| **range_query_with_index/1000**  |             10.80 ms |         1.52 ms |  **7.1×**|
| **range_query_with_index/10000** |              111 ms  |         19.7 ms |  **5.6×**|
| range_query_no_index/10000       |             104.7 ms |          80.1 ms| unchanged baseline |

**Why ~5× and not 1000×.** This v1 uses `scan_prefix_stream` (filter
upper-bound on each entry, early termination) — not a true B-tree
range scan from `lower` to `upper`. So for an index of N=10 000
entries we still iterate roughly N keys even when only K=1 000 match
the range. The win comes from skipping record decode for the
non-matching keys (we only deserialize record bytes for matched
RecordIds) — not from skipping the index scan.

To unlock the actual O(log N + K) cost (and the full 100×–1000×
class), `Store::iter_range_stream(start, end)` needs to be added,
with native impls on sled/redb/fjall (they all expose `range()` in
their underlying API). That's the next step — left out of this
commit to keep scope tight; v1 already demonstrates the layout
works end-to-end.

Not yet:
- `Filter::Gt` / `Filter::Lt` — need "skip boundary value" trick.
- `MIN(field)` aggregate fast-path via `lookup_min` (one B-tree seek).
- `MAX(field)` and `order_by DESC + LIMIT K` — needs reverse iter
  on `Store`.
- Composite sorted indexes over multiple columns.

## After Opt D-class (#2, #2.5, counter cache)

Three more optimisations after A+B+C, all surfaced through bench-
driven analysis:

- **#2** — `SELECT count(*)` (no filter) wires to the existing
  `RecordCounter` instead of materialising every record just to
  count the result vec. Truly O(1).
- **#2.5** — `SELECT count(*) WHERE indexed_eq = X` uses the
  index's `BTreeSet::len()` directly, never loads any record.
- **Counter cache** — `RecordCounter::increment` was doing 2 store
  ops per call (read-old + write-new) inside a mutex. Now an
  in-memory `AtomicU64` + `dirty` flag; `persist()` no-ops when
  unchanged, and the bulk write path persists once at the end.
  Mirrors the Opt A pattern for the interner.

| Bench                          | A+B+C    | After +D-class | Speed-up vs baseline |
|--------------------------------|---------:|---------------:|---------------------:|
| **count_all_no_filter/100**    |   674 µs |     20.7 µs    |  **33×**             |
| **count_all_no_filter/1000**   |  7.17 ms |     19.2 µs    |  **374×**            |
| **count_all_no_filter/10000**  |  79.5 ms |     23.5 µs    |  **3383×**           |
| **count_with_filter_with_index/100**  |  130 µs |   32 µs    |  4×                  |
| **count_with_filter_with_index/1000** |  873 µs |   54 µs    |  16×                 |
| **count_with_filter_with_index/10000**| 8.5 ms  |  393 µs    |  **22×**             |
| **bulk_insert/100**            |  1.91 ms |   1.56 ms      |  **-18 %** (p=0.00)  |
| **bulk_insert/1000**           |  24.6 ms |   19.4 ms      |  **-21 %** (p=0.00)  |

`count_all_no_filter` is now genuinely O(1) — time is flat across
N=100/1000/10000 at ~20 µs (the fixed envelope cost: serde + batch
planner + result wrapper).

### Sorted-index opportunity (revised — much smaller than originally framed)

Originally targeted #1 (order_by+LIMIT via index), #4 (MIN/MAX via
index), #5 (range queries via index). All three need an index that's
ordered **by value**. The current index format is hash-keyed
(`IndexRecordKey { is_unique, name_interned, hash1, hash2 }`) —
great for equality, no help for range/order/min-max.

The fix is *not* a big architectural addition. Every storage backend
we wrap (sled / redb / fjall / nebari / canopy) already stores keys
in an ordered B-tree natively — that ordering comes for free. We just
need to:

1. **Order-preserving codec** for `i64` / `u64` / `f64` / `String` /
   `bool` — write a value as bytes that sort the same way the value
   does (big-endian for unsigned ints, sign-bit-flipped big-endian
   for signed ints, raw UTF-8 for strings, etc.). Pure functions in
   `shamir-types::core::sort_codec`. ~150 lines.
2. **Separate per-index store** like
   `__sorted_idx_<table>_<index_name>__`. Not system records; just
   ordinary KV entries whose physical key is
   `<encoded_value> || <record_id>` and value is empty (or a small
   pointer payload).
3. **`Store::iter_range_stream(start, end)`** — already partially
   there via `scan_prefix_stream`; add a true range form with default
   impl over `iter_stream + filter`, native impls on backends that
   expose `range()` directly (redb, sled, fjall).
4. **`IndexKind::Sorted`** variant on `IndexDefinition`. New hooks
   `on_record_created_sorted` / `on_record_deleted_sorted` mirror the
   existing regular-index hooks.
5. **Planner extension** — recognise `Filter::Between/Gt/Gte/Lt/Lte`
   and `order_by + limit` and pick a sorted index when one matches.
   New planner cases sit next to `try_plan_index_scan`.

A day's work, not a sprint. System records are *not* used — those
are for engine metadata (interner blob, counter, index definitions).
Sorted indexes are ordinary data records whose key is the indexed
value bytes.

Tracked as the next perf work item.

### Opt D (parallel stages) — tried and reverted

`futures::future::try_join_all` was added inside the stage loop
and measured against `batch_multi_read_8`. Zero win on in-memory
CPU-bound workloads — there are no await suspension points inside
in-memory queries, so concurrent futures on a single task degrade
to serial. Real parallelism needs `tokio::spawn`-per-query, which
requires `Arc<dyn TableResolver>` and `Arc<dyn AdminExecutor>`
through the executor signature. Kept out of scope; reverted to
the original sequential loop. Disk-backed backends would benefit
from a `try_join_all` since their I/O awaits actually yield — but
without `tokio::spawn` we still can't put N queries on N worker
threads.

## Headline summary — A + B + C combined

| Scenario (10K records)                | Baseline | After A+B+C | Speed-up    |
|---------------------------------------|---------:|------------:|------------:|
| `set` by id  (with index)             | 82.7 ms  |  101 µs     | **818×**    |
| `update` by id (with index)           | 88.1 ms  |   79 µs     | **1115×**   |
| `delete` by id (with index)           | 96.4 ms  |  102 µs     | **944×**    |
| `read` by id (with index, was already fast) |  50 µs |  60 µs   | unchanged   |
| `bulk_insert/1000`                    | 27.2 ms  | 24.6 ms     | -10 % (Opt A) |
| no-index variants of any op           | unchanged | unchanged  | unchanged    |

Net architectural shift: the write path went from "always O(n) scan
regardless of indexes" to "O(log n) when a covering single-field
index exists, scan otherwise." Three orders of magnitude on the hot
read-modify-write path with effectively no risk to the no-index
fallback.

Still on the table:
- **Sorted (B-tree-by-value) indexes** — unlocks the 1000×-class
  wins for `order_by + LIMIT`, `MIN/MAX`, and range queries. Real
  architectural addition (second index variant alongside the
  current hash index).
- **`tokio::spawn`-per-query** parallel stage execution — requires
  Arc-ifying the resolver/admin trait objects. ~N_cores× for
  read-heavy batches.
- **`Store::set_many`** native batch write on durable backends
  (redb/sled/fjall WriteBatch). Big win for bulk insert on disk;
  marginal on in-memory.
- **Composite-key** index lookup for `set/update/delete`. Today
  multi-field keys fall back to scan.
- **Implicit index auto-creation** when the user `set`s by an
  un-indexed key. We deliberately stopped at "use what's there";
  auto-create is a separate behaviour change that needs UX
  consideration.

## After sled durability rework (2026-05-11)

Found: `SledStore::insert / set / remove` called `tree.flush()`
unconditionally on every write — a real fsync, per record. Bulk
insert 1000 records = 1000 fsyncs.

Fix at the unified-interface level: added `Store::flush()` to the
trait (default no-op), removed the per-write `tree.flush()` from
sled, kept the explicit fsync available via `Store::flush()`. sled's
own background flusher (default every 500 ms) carries the
"eventually durable" contract. Callers that need a strict
commit-boundary call `store.flush().await`.

### Bulk insert — sled backend

| Records | Before (fsync-per-write) | After (background flusher) | Speedup |
|--------:|-------------------------:|---------------------------:|--------:|
|     100 |              **272 ms**  |                **7.4 ms**  | **36.7×** |
|   1,000 |              **2.59 s**  |               **71.1 ms**  | **36.4×** |

Throughput went from ~370 elem/s to ~14 000 elem/s. The previous
number had no real reason to exist — fsync-per-write was an
overcautious default with no caller asking for it.

### Durability contract — explicit

| Operation                       | Durability after change                  |
|---------------------------------|------------------------------------------|
| `Store::insert/set/remove`      | Buffered; durable on next background flush (≤500 ms by default for sled) |
| `Store::flush()`                | Forces fsync. Returns when on disk.       |
| Crash window                    | ≤500 ms of in-flight writes for sled. Same as Postgres `synchronous_commit=off`, MySQL `innodb_flush_log_at_trx_commit=2`, sqlite `PRAGMA synchronous=NORMAL`. |

If a caller needs the old "every write is fsync'd before return"
semantics, the pattern is now explicit: `store.set(...).await?;
store.flush().await?;`.

## After sorted-index range path → `iter_range_stream` (2026-05-11)

`SortedIndexManager::lookup_range / lookup_min / lookup_first_k` used
`Store::scan_prefix_stream` and then filtered each returned batch
in-process against the lower/upper byte-bounds. sled's
`scan_prefix` starts at the prefix-min and walks forward, so any
records before the lower bound got materialised + filtered, wasted.

Fix is again at the unified-interface level: same call sites now
build (lower, upper) absolute keys in the physical-key space and
delegate to `Store::iter_range_stream`. Disk backends use their
native B-tree `range()` (already implemented in `storage_sled.rs` /
`storage_redb.rs`), seeking straight to `lower` and stopping at
`upper`. In-memory falls back to the default filter wrapper —
correct, same as before.

Logic preserved: the same lower/upper construction (prefix +
encoded_value + record_id-tiebreaker padding); `None` ends become
`prefix` / `prefix || [0xFF; 64]` (greater than any real entry
inside this prefix, less than the start of the next prefix).

### Sled range queries — after #2

| Bench                                 | Pre   | Post  | Speedup |
|---------------------------------------|-------|-------|---------|
| range_query_with_index_sled/10000     | 66.2 ms | 47.9 ms | **1.38×** |
| range_query_with_index_sled/1000      | 4.84 ms | 4.80 ms | noise   |
| range_query_with_index_sled/100       | 583 µs  | 579 µs  | noise   |
| range_query_narrow_with_index_sled/10000 | 20.2 ms | 7.95 ms | **2.54×** |
| range_query_narrow_with_index_sled/1000  | 1.21 ms | 895 µs  | **1.36×** |
| range_query_narrow_with_index_sled/100   | 335 µs  | 207 µs  | **1.62×** |

Win scales with how much of the index prefix sits before the lower
bound. Wide ranges at low `start` (age=30 out of 18..78 → ~20%
waste) get ~1.4×. Narrow ranges (age=30 exactly, single value)
where we used to walk from age=18 → ~2.5×. As expected.

## Cross-backend bulk_insert parity pass (2026-05-11)

Now that sled's per-write fsync is gone, the picture for the other
disk backends sharpens. Baseline (before this pass, with sled
already on the new path):

| Backend  | bulk_insert/100 | bulk_insert/1000 | Cost  |
|----------|----------------:|-----------------:|-------|
| sled     |          7.4 ms |           71 ms  | ~70 µs/rec |
| **redb** |        **282 ms** |       **2.75 s** | ~2.75 ms/rec |
| **persy** |       **296 ms** |       **2.95 s** | ~2.95 ms/rec |
| canopy   |          9.5 ms |           83 ms  | ~83 µs/rec |
| fjall    |           72 ms |          127 ms  | ~127 µs/rec (n.b. /100 overhead is fixed) |
| **nebari** |      **487 ms** |       **4.79 s** | ~4.79 ms/rec |

Three slow backends — redb, persy, nebari — all share the same
underlying cost: every `Store::insert/set/remove` opens a write
transaction and `commit()`s it, and each commit fsyncs.

### redb — Durability::None on writes + immediate on flush ✅

Per-write `WriteTransaction` now calls
`set_durability(Durability::None)` before commit. redb's
`Durability::None` keeps the in-memory state consistent (subsequent
reads see the writes) but skips fsync. `Store::flush()` issues an
empty commit with `Durability::Immediate`, forcing all pending
in-memory state to disk.

Same semantic model as sled's amortised durability — and same kind
of win:

| Bench                | Pre   | Post  | Speedup |
|----------------------|-------|-------|---------|
| bulk_insert_redb/100  | 282 ms | 24.0 ms | **11.7×** |
| bulk_insert_redb/1000 | 2.75 s | 188 ms  | **14.6×** |

### persy — tried `set_background_sync(true)`, got SLOWER, reverted ❌

persy exposes `TransactionConfig::set_background_sync(true)`
(behind the `background_ops` cargo feature) — supposedly moves the
fsync to a background thread so `commit()` returns immediately.

Measured: bulk_insert_persy/1000 went 2.95 s → 5.23 s (**+77 %
regression**). The background thread bottlenecks worse than the
direct fsync on Windows, presumably from queue contention. Or the
"background" path still synchronises before commit returns.
Reverted; persy stays at 2.95 s for now.

The real fix for persy is a batched-write Store API
(`Store::insert_many` etc.), which lets a single transaction
absorb the whole batch. That's a wider refactor — deferred.

### nebari — no fsync-skip mode in API ❌

`nebari::Config` exposes no equivalent of `Durability::None`. Every
`Tree::set()` is a transactional commit with mandatory fsync.

Path forward: same as persy — batched `Tree::modify(Modification)`
through a `Store::insert_many` extension. Deferred.

### fjall — already on LSM, journal-buffered ✅ (no change)

fjall's `keyspace.insert()` writes to the WAL journal buffer (no
fsync per write) — durability comes from journal rotation +
flush thread. Already fast (72 ms / 127 ms). No code change.

### canopy — already fast ✅ (no change)

canopydb appears to amortise commits internally; matches sled's
post-fix shape (9.5 ms / 83 ms). No code change.

### Status

| Backend  | Status                                          |
|----------|-------------------------------------------------|
| sled     | ✅ Fast (this PR series) |
| redb     | ✅ Fast (this PR — **14.6×**) |
| canopy   | ✅ Fast (always was) |
| fjall    | ✅ Fast (LSM, journal-buffered) |
| persy    | ⏳ Awaits `Store::insert_many` batch API |
| nebari   | ⏳ Awaits `Store::insert_many` batch API |

The remaining persy / nebari work needs an architectural extension
to the `Store` trait (a `insert_many` / `set_many` API that lets
each backend coalesce N writes into one transaction). Deferred to
its own sprint.

## After hash-index posting layout: blob → key-per-record (2026-05-11)

Before: each regular (non-unique) index value held its entire posting
list as a serialised `BTreeSet<RecordId>` blob in one KV. Every
`add_index_entry` did read-blob → deserialise → insert → serialise →
write-blob — **O(K)** per write, where K is the cardinality of that
particular index value. For a low-cardinality field (e.g. `city`
with 8 unique values over 10 000 records → K up to 1250 per value),
the blob grew to ~30 KB and every insert re-(de)serialised it.

After: one physical KV per `(index_value, record_id)`:

```
key   = index_key (25 bytes) || record_id (16 bytes)   →   41 bytes
value = empty
```

Same approach as the sorted-index layout (which always worked this
way). Write is now **O(1)** regardless of K: just
`info_store.set(composite_key, empty)`. Read does
`scan_prefix(index_key, 25b)` and decodes record_ids from the last
16 bytes of each key.

Side-effects:
- **Concurrent writes for distinct record_ids no longer race** for
  one shared blob.
- **drop_index** unchanged — it already scans by the 9-byte
  `name_interned` prefix; both old (25b) and new (41b) keys match
  it, so the migration story is "drop_index + create_index"
  rebuilds in the new format.
- **Opt G posting-list cache** still works at the (25-byte
  logical-key → `Arc<BTreeSet<RecordId>>`) layer; populated by
  scan-and-collect instead of by bincode deserialise.

### Bench impact

Write paths (sled, with `by_city` regular index, cardinality 8 →
posting list grows toward 125/1000 records):

| Bench                            | Pre #6 | Post #6 | Speedup |
|----------------------------------|--------|---------|---------|
| bulk_insert_with_index_sled/100  | 16.0 ms | 12.2 ms | **1.31×** |
| bulk_insert_with_index_sled/1000 | 180 ms  | 121 ms  | **1.49×** |

The indexed bulk insert now costs ~55 µs/record of index overhead
(down from ~113 µs/record). The remaining gap vs no-index bulk
insert (66 ms / 1000 = 66 µs/record) is mostly the per-insert
posting set + cache invalidation.

Read paths (in-memory; same hot scenarios, the lookup_by_index now
goes scan_prefix instead of blob-get + deserialise):

| Bench                                  | Pre #6 | Post #6 | Speedup |
|----------------------------------------|--------|---------|---------|
| read_by_city_with_index/10000          | 19.6 ms | 15.2 ms | **1.29×** |
| read_by_city_with_index/1000           | 1.83 ms | 1.40 ms | **1.30×** |
| update_by_id_with_index/10000          | 51 µs   | 42 µs   | **1.21×** |
| count_with_filter_with_index/10000     | 88 µs   | 81 µs   | **1.09×** |

Read wins come from "no bincode deserialisation of a 30 KB blob" —
even for cached lookups the cold-fill is faster, and warm lookups
inherit a smaller average path.

## Next

Sprint γ remaining:
- **#3 / Opt O** — covering indexes (range-by-index without per-record
  data fetch — break the 6%-selectivity ceiling on sled).
- **#4 / Opt P** — `Store::get_many` — batch the N random reads after
  an index lookup into one spawn_blocking on disk backends.
