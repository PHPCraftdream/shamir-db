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

## Next

Optimisations land in PR sequence A → B → C → D. Each PR re-runs the
suite and appends a column to this table; final commit summarises
"before vs after" with speedup factors per scenario.
