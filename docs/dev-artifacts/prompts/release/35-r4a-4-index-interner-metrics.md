# Brief: R4a PR-4 — index and interner metrics

Design (settled): `docs/dev-artifacts/design/engine-metrics.md` — §2.5 (indexes), §2.6 (interner), §4 (the export path and the
cardinality policy), §5 (cost proof), §6 item "PR-4", the tests paragraph. The design's path:line citations predate the split of the
engine into `shamir-engine-repo` / `shamir-engine-table` crates and the landing of PR-2 (query metrics: `EngineMetrics`,
`shamir-server-security/.../observability/export/engine.rs`); re-verify every citation you rely on.

## Why

Operators need to see index size and build cost and the growth of the field-name interner before a large table or a runaway
schema becomes an incident. The state already exists (entry-count APIs, a build `Instant`, `Interner::len()`); this step
exports it without touching any hot path.

## What (this step only)

1. **Series** (names, types, labels, sources exactly as in design §2.5 and §2.6): `shamir_index_entries{db,repo,table,index}`
   (gauge, sampled at scrape from the existing entry-count APIs of the regular, sorted and full-text indexes),
   `shamir_index_build_seconds{db,repo,kind}` and `shamir_index_rebuild_seconds{db,repo,kind}` (histograms observed once per
   completed build or rebuild at the existing `Instant` site of index creation), `shamir_interner_names{db,repo}` and
   `shamir_interner_high_water{db,repo}` (gauges). Do NOT add `shamir_vector_snapshot_state` (not in this step) and do not
   re-export `shamir_degraded_indexes_total` (already exported).
2. **State.** Per-repository lock-free state for the two build histograms (reuse the histogram primitive and snapshot shape
   of PR-2's `EngineMetrics`, extending it or sitting next to it — your call, justify it in the report); the gauges read live
   state at scrape and need no new state.
3. **Export.** Add the series to the exporter next to the PR-2 export (`engine.rs` there or a sibling file; the folder has a
   7-entry limit — count), discovered through the existing repository walk. Cardinality policy of §4: at most 64 tables and
   32 indexes per repository carry per-table/per-index labels, chosen by current record count with the table name as
   tie-break; the remainder is aggregated into one `(other)` series per repository for gauges; the companion gauges
   `shamir_export_tables_truncated{db,repo}` and `shamir_export_tables_known{db,repo}` exist as designed. The limits are
   constants, not settings.
4. **Guide.** One entry per new series in `docs/guide-docs/guide/07-operations.md`, in the style of the PR-2 entries
   (meaning, labels, normal range, cardinality rule, the truncation gauges). Dashboard and alert rules are PR-6, not here.
5. **Tests** (CI-only; value versus real state, as in design §6): build an index and assert
   `shamir_index_build_seconds_count` == 1 (and the rebuild series for a rebuild); write a known number of rows and assert
   `shamir_index_entries`; scrape a repository with more than 64 tables and assert the cap, the `(other)` aggregation and the
   two companion gauges; the interner gauges against `Interner::len()` and the persisted high-water; all through the exporter
   path, not by reading the state structs directly. Builders only, no raw JSON.

## Do not

Add anything to a write, read or commit hot path (every new read happens at scrape; the only new write is one observe per
completed index build). Add the vector-snapshot, WAL, function or replication series (other PRs). Change index or interner
behavior. Add a dependency. Widen visibility beyond what the exporter needs and name each widening in the report.

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file (`python -I scripts/ci/check_layout.py`).
* rust-guard `structure`: no logic in `mod.rs`, no inline `#[cfg(test)]` modules (tests in `tests/` with a manifest `mod.rs`),
  no local `use`, no new `#[allow]`, no impl outside the type's module. Imports at the top. One primary type per file.
* Future size: do not add awaited helpers inline into large futures — `Box::pin` them at the call site (a Windows debug-build
  stack overflow came from exactly this on 2026-11-17). Prefer small synchronous observers on async paths.
* Machine memory: run at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; never pipe cargo output through `tail`/`head` (a pipe
  masks the exit code — redirect to a file and read the exit code).
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report
  what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for every crate you touched (plus dependents of a changed public
signature) and the layout gate are clean; the five series are exported with the designed labels and cardinality policy; the
guide has one entry per series; the tests above exist and name what they prove. The final report lists the touched files, the
visibility widenings, the decisions taken and what is not covered.
