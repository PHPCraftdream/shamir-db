# Brief: query and load metrics (release-1 R4a, step PR-2 of the metrics design)

Design: `docs/dev-artifacts/design/engine-metrics.md` — read sections 1 (inventory, especially 1.2 and 1.3), 2.1 (queries),
2.2 (load), 3 (the histogram primitive, already landed as `crates/foundation/shamir-metrics`), 4 (export path and
cardinality), 5 (cost proof), 6 (tests, R4a-1) and 7 (step PR-2), and the "Review" section. Release plan:
`docs/dev-artifacts/release-1/04-observability.md` section 2 (the engine keeps state in atomics, the server polls; no
metrics library in the engine).

## Why

Release gate: "the operator sees query delays … in `/metrics`". Storage, WAL, functions and replication metrics have
landed; query latency, errors, rows and load are the missing half.

## What

1. **Record** exactly the series of catalogue sections 2.1 and 2.2 (names, types, units, labels as written there):
   `shamir_query_duration_seconds{db,repo,op}`, `shamir_query_errors_total{db,repo,code}`, rows viewed / returned, the
   four TxMetrics bridges, `shamir_sessions_active`, `shamir_connections_active`, `shamir_connections_rejected_total`,
   `shamir_admission_queue_depth`. Follow the design's notes (no duplicate series; `op=batch` instead of a separate batch
   histogram; rate-limit rejections are already covered).
2. **Placement** (design 7, PR-2): a per-repo `EngineMetrics` bundle in the instance layer, passed DOWN as a plain
   parameter to query/batch/write paths — never reached upward; zero new backward edges in `.rust-guard/layers.toml`.
   The engine was split since the design was written: table read/write paths now live in `shamir-engine-table`. Re-locate
   every §1.3 site from a fresh search and say where each hook lives now. If a table-crate path needs the bundle, it
   receives it as a parameter (or records into a value handed from above); the table crate must not depend upward.
3. **Hot-path cost**: one clock read + one histogram observe per op, `fetch_add` only on error/reject branches, row
   counters folded into the existing `ReadByteMeter` sites. No lock, no allocation per op.
4. **Export**: render the new series in the scrape handler through the existing exporter folder of
   `shamir-server-security/src/security/observability/` (count the entries; add files in the existing `export/` subfolder
   if it is there), with the cardinality rules of design 4.
5. **Tests** (CI only), per design 6 R4a-1 (b)–(d): engine query-metrics test (histogram count == ops, rows match the
   fixture), export test (every new series and its label set), TxMetrics bridge rows. One paragraph per new metric in
   `docs/guide-docs/guide/07-operations.md` section 3 (meaning, unit, normal range).

## Rules

* Other changes in flight touch the engine's tx layer (a WAL truncation pin in `tx/drain/`, `tx/commit/core/`): do not
  edit those files beyond adding a parameter where unavoidable, and list every tx-layer edit in the report.
* No new dependency. At most 7 entries per folder and 1000 lines per file; one primary export per file; `mod.rs`
  re-exports only; no local `use`; no inline test modules; no new `#[allow]`; visibility widening only where declared.
* Keep the change reviewable: if it passes roughly 2500 lines, stop after the query series (2.1) with their export and
  tests, and report what remains (load series become their own step).

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`. Run heavy commands with `-j 1`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Acceptance (observed in CI)

fmt, clippy (`-D warnings`, every touched crate, `--all-targets`) and layout clean locally; the touched crates' tests green
in CI. The final report lists every hook site with its current path, the bundle placement and how it is passed, every
file created and changed, and every deviation from the design with its reason.
