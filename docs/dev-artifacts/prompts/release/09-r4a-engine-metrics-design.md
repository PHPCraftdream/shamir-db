# Brief: design of the engine metrics (release-1 R4a, design only)

## Why

An operator of the first release must see query latencies, data and WAL size, replication lag and function costs in
`/metrics` (release plan `docs/dev-artifacts/release-1/README.md` section 5, direction R4a; detail in
`docs/dev-artifacts/release-1/04-observability.md` sections 1-3, 6 and 7). Today only the server crates publish metrics;
the engine, storage, WAL, indexes and the function host expose nothing. This task produces the DESIGN only.

## What

Write `docs/dev-artifacts/design/engine-metrics.md` (English, the style of the other documents in that folder: numbered
sections, file and line references for every claim about the current code, OBSERVED vs PROPOSED kept apart). It must:

1. **Inventory (OBSERVED).** What `/metrics` exports today and where each family is defined and collected (the export
   modules under `shamir-server-security`, `shamir-sync`, `shamir-server`, the polling pattern of
   `visibility_export.rs` and `durability_export.rs`); what counters already exist inside the engine, storage, WAL, index
   and function-host crates but are not exported (for example the `TxMetrics` cell, the WAL and group-commit counters,
   the persist counter, the compile-cache counters); where the hot paths are (query execution, commit, WAL append, index
   probe, function call) with the exact functions.
2. **The metric catalogue (PROPOSED).** For every area of section 3 of the plan table: metric name, type, unit, labels,
   the source in the code (existing counter or the exact place to add one), the cost on the hot path (an atomic increment,
   a histogram bucket search, a clock read) and the normal range for the operations guide. Names follow the existing
   Prometheus naming of the server (prefix, `_seconds`, `_bytes`, `_total`). Mark which signals R4a-1, R4a-2 and R4a-3
   of the plan deliver.
3. **The primitive.** The fixed-bucket histogram on atomics named in section 2 of the plan: API, bucket layout for
   latencies (and a second layout for sizes), merge and snapshot semantics (a reader must never see a torn histogram),
   memory per instance, `loom`-testability, and WHERE it lives. The engine crates are being cut into crates right now
   (`docs/dev-artifacts/design/engine-layering.md`): the primitive must sit in a crate that every engine layer can depend
   on without a cycle and without a metrics-library dependency (principle of section 2: no metrics library in the
   engine). Decide between an existing foundation crate and a new tiny one, with the dependency graph.
4. **The export path.** How engine state reaches the exporter without locks on the hot path (state kept by the engine,
   polled by the server module), how the exporter finds every repository and table, and the cardinality policy of open
   question 1 (a limit on per-table and per-index labels, how the remainder is aggregated, how an operator sees which
   tables are included); also the time cost of one scrape on a server with thousands of tables.
5. **Cost proof plan.** Which existing benches (`crates/*/benches`, `bench_scale_tool::Harness`) cover each hot path,
   the acceptance threshold for "no significant slowdown", and how the before/after comparison is taken in CI only.
6. **Tests and acceptance** (CI only): per stage R4a-1..3 the named tests, crate and fixture; how values are checked
   against real state; which tests need all three OS.
7. **Implementation plan:** files and crates to create and change, grouped into PR-sized steps, with the layer of each
   change; nothing may be added to the `repo`, `table`, `tx`, `query` layers of `shamir-engine` in a way that creates a
   backward edge (`.rust-guard/layers.toml`); the Grafana panel and alert rules (file locations) and the operations guide
   section (`docs/guide-docs/guide/07-operations.md`).
8. **Open questions** for the owner, each with a default.

## Do not

Change any code, test, CI or dashboard file; design R4b (activity views, cancellation) or R4c (tracing) beyond a
boundary note; propose a metrics-library dependency inside engine crates.

## Observable acceptance

One new file `docs/dev-artifacts/design/engine-metrics.md`; every statement about the current code carries a file path
(and a line where it matters) that the reviewer can open; every area of the plan table is covered by catalogue rows;
the open questions have defaults. `python -I scripts/ci/check_layout.py` OK. The final report lists the sections, the
decisions taken and the open questions.
