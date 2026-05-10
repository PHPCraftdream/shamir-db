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

## Next

Optimisations land in PR sequence A → B → C → D. Each PR re-runs the
suite and appends a column to this table; final commit summarises
"before vs after" with speedup factors per scenario.
