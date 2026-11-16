# Sub-brief 04: storage/WAL metric export + ops guide (PR-3 of engine-metrics, part 2)

Committed parent brief: `docs/dev-artifacts/prompts/metrics/02-storage-wal-metrics.md`.
Design: `docs/dev-artifacts/design/engine-metrics.md` §1.1 (house naming),
§2.3/§2.4 (rows), §3.6 (histogram rendering), §4 (cardinality), §6 (tests).
Requires sub-brief 03 to have landed (extended `DurabilityMetricsSnapshot`).
Scope: crates `shamir-server-security` + `docs/guide-docs/guide/07-operations.md`.

## Exporter

New subfolder `crates/server/shamir-server-security/src/security/observability/export/`
(the parent folder has 6 entries; this is its 7th):
- `mod.rs` — re-exports only.
- `storage.rs` — `register_storage_metrics()` (describe all new families) and
  `record_storage_metrics(db: &str, repo: &str, snapshot: &DurabilityMetricsSnapshot)`
  (per-scrape rendering, snapshots only, same `db`/`repo` labels as
  `durability_export.rs`; owned label values like the neighbours).

Series rendered (catalogue §2.4; naming per §1.1 — `_total` counters only):
- gauges, `.set`: `shamir_wal_size_bytes` (Unit::Bytes, "approximate WAL bytes
  on disk: active segment + sealed retained; Mem backend reports 0"),
  `shamir_wal_segments` ("retained WAL segment files: sealed + one active;
  Mem backend reports 0"), `shamir_wal_truncation_lag_versions`
  ("max committed version minus the oldest truncatable sealed segment's max
  version; 0 when nothing is truncatable"), `shamir_group_commit_waiters`
  ("sync demanders parked on WAL fsync coverage") — values read by the
  snapshot's public index constants (slots 11..=14), never by position.
- counter, `.absolute`: `shamir_wal_bytes_total` (Unit::Bytes, slot
  `COUNTER_WAL_BYTES_TOTAL`; "WAL payload bytes appended since the WAL
  directory was created").
- histogram `shamir_wal_fsync_seconds`: only when `snapshot.fsync` is
  `Some(HistogramSnapshot)` — visibility-family pattern
  (`visibility_export.rs` cumulative `*_bucket` gauges with `le` labels +
  `_count`/`_sum` counters via `.absolute`). Internal cells are nanoseconds:
  render `le` labels and `_sum` in SECONDS (`ns as f64 / 1e9`), matching the
  family name. `torn == true` snapshots are DROPPED for that repo on that
  scrape (design §3.4). Help text: "sync-worker round spanning snapshot
  replay + sink fsync (same span as shamir_wal_sync_duration_ns)".

Wiring in `observability/mod.rs` (minimal): declare `mod export;` and call
`register_storage_metrics()` next to `register_durability_metrics()` (the
recorder-init block) and `record_storage_metrics(&db_name, &repo, &snapshot)`
in the scrape walk next to the existing `record_durability_metrics` call
(share the already-taken `durability_metrics_snapshot()` — one snapshot,
both exporters). No new background work.

`shamir_gc_pending_versions` (catalogue §2.3) is a cross-reference only — the
canonical series `shamir_gate_pending_versions` already exists; no new series.

## Export test

`observability/tests/storage_export_tests.rs` (wire into `tests/mod.rs`;
that folder stays ≤ 7 entries), pattern `durability_export_tests.rs`:
build a `PrometheusBuilder` recorder, `with_local_recorder`, register + record
a fully-populated snapshot (set every new slot; `fsync = Some(...)` with a
couple of known bucket counts and a known sum; include a `torn` snapshot case
dropping the histogram series for a second repo). Assert every new series
name renders with the expected Prometheus TYPE line (counter/gauge/
histogram), correct `db`/`repo` labels, `le` label ordering (cumulative,
`+Inf` last), `_sum` in seconds, and that a `None`-fsync repo exports no
`shamir_wal_fsync_seconds_*` series.

## Docs

`docs/guide-docs/guide/07-operations.md` §3 (`/metrics` section): add the new
series to the metric table (matching the existing Russian table style) AND
one short paragraph per new metric after the table (meaning, unit, normal
range — ranges from catalogue §2.4: segments single digits to tens, lag
small/growing = truncate backlog, waiters < commit concurrency, fsync sub-ms
on local disk, bytes grow with write rate, size retention-config bound).
One cross-reference line: versions pending GC are already exported as
`shamir_gate_pending_versions`. Do not document unimplemented metrics.

## House rules (binding)

* Compile/build/test policy: compilation, builds and tests run on GitHub CI,
  never on this machine. Banned: `cargo test`, `cargo nextest`,
  `./scripts/test.sh`, `cargo build/run/bench/doc`, npm/tsc/vitest, wasm
  builds, e2e/integration harnesses, loom, any release or full-workspace
  build. Allowed locally, scoped to the crates touched: `cargo fmt -p <crate>`
  (and `-- --check`), `cargo clippy -p <crate> --all-targets -- -D warnings`,
  `python -I scripts/ci/check_layout.py`, read-only git. Run cargo only under
  `wnice -p idle -c 50 -t 8 -m 50 --` with `-j 2`.
* Before writing Rust, read the skill file
  the `rust-intel` skill and follow it.
* Root CLAUDE.md is binding: ≤ 7 entries per folder, ≤ 1000 lines per file,
  `mod.rs` re-exports only, no inline `#[cfg(test)] mod tests`, no local
  `use` inside bodies, no new `#[allow]`, no visibility widening, comments
  terse. Count folder entries from listings before adding files.
* No new dependencies (the needed types resolve via
  `shamir_db::engine::repo::durability_policy::*`). No change in any other
  crate. Never write private machine paths, user names, or CI infrastructure
  names into repository files.
* ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index. Only edit
  files; the orchestrator commits.

## Acceptance observed by the orchestrator

`cargo fmt -p shamir-server-security -- --check` clean;
`cargo clippy -p shamir-server-security --all-targets -- -D warnings` clean;
`python -I scripts/ci/check_layout.py` clean; export tests green in CI.
