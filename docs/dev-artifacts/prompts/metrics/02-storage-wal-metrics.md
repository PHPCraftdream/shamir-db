# Brief: storage and WAL metrics (release-1 R4a, step PR-3 of the metrics design)

Design: `docs/dev-artifacts/design/engine-metrics.md` — read sections 1 (inventory), 2.3 (storage) and 2.4 (WAL), 3 (the
histogram primitive, already landed as the crate `crates/foundation/shamir-metrics`), 4 (the export path), 6 (tests) and 7
(step PR-3), and the "Review" section. Release plan: `docs/dev-artifacts/release-1/04-observability.md` section 2 (the engine
keeps state in atomics, the server polls; no metrics library in the engine).

## Why

Operators cannot see WAL growth, fsync latency or storage size today; these are the first signals of a disk or durability
problem. This step adds the storage and WAL metrics of the catalogue and exports them on `/metrics`.

## What

1. **Record** exactly the metrics of catalogue sections 2.3 and 2.4 (names, types, units and labels as written there): the
   segment-count atomic in `SegmentSet` (`crates/storage/shamir-wal`), fsync timing into a `shamir_metrics::Histogram`,
   size and lag gauges. WAL counters travel through the existing `DurabilityMetricsSnapshot` channel (now in the crate
   `crates/engine/shamir-engine-repo`, `journal/backend/durability_metrics.rs` area): new fields go at the END of the
   snapshot arrays and are read by index constants (the struct is versioned by position). Hot paths get only relaxed atomic
   operations; no lock, no allocation per operation.
2. **Export path.** The folder `crates/server/shamir-server-security/src/security/observability/` already has 6 entries, so
   the per-area exporters go into a new subfolder `observability/export/` (its 7th entry): `mod.rs` (re-exports only) and
   `storage.rs` (the rows of this step). Later steps add `functions.rs`, `replication.rs` and `engine.rs` there; this is a
   deliberate refinement of design section 4, which named a single `engine_export.rs`. Call the exporter from the scrape
   handler next to the existing `durability_export` rows, reading snapshots only. Follow the house naming of section 1.1
   (`_total` only for counters) and the cardinality rules of section 4.
3. **Tests** (CI only): unit tests for each new counter and the segment-count gauge, a test that the snapshot arrays keep the
   old indices stable, and an export test that scrapes and checks every new series name and type.
4. **Docs**: one paragraph per new metric in `docs/guide-docs/guide/07-operations.md` section 3 (meaning, unit, normal range).

## Rules

* No change in the `table`, `tx` or `query` layers of `shamir-engine` (they are being cut into crates in parallel); if a
  metric needs one, stop and say so in the report. `shamir-engine-repo` and `shamir-wal`/`shamir-storage` are allowed.
* No new dependency except `shamir-metrics` where a histogram is recorded.
* At most 7 entries per folder and 1000 lines per file; count from listings.

## Acceptance (observed in CI)

fmt, clippy (`-p shamir-wal -p shamir-storage -p shamir-engine-repo -p shamir-server-security --all-targets`) and layout clean
locally; the tests of the touched crates green in CI.
