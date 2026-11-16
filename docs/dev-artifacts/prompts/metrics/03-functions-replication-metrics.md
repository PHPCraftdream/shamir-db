# Brief: function and replication metrics (release-1 R4a, step PR-5 of the metrics design)

Design: `docs/dev-artifacts/design/engine-metrics.md` — read sections 1 (inventory), 2.7 (functions) and 2.8 (replication), 3
(the histogram primitive, already landed as the crate `crates/foundation/shamir-metrics`), 4 (the export path), 6 (tests) and 7
(step PR-5), and the "Review" section. Release plan: `docs/dev-artifacts/release-1/04-observability.md` section 2 (the engine
keeps state in atomics, the server polls; no metrics library in the engine).

## Why

User functions (WASM) and replication are the two subsystems whose failures operators see last: a slow or fuel-exhausting
function, an admission refusal, a lagging follower or a reconnect loop. This step adds the metrics of catalogue sections 2.7
and 2.8 and exports them on `/metrics`.

## What

1. **Record** exactly the metrics of catalogue sections 2.7 and 2.8 (names, types, units, labels as written there): call
   duration and fuel per call into `shamir_metrics::Histogram`, compile-cache hits and misses, admission rejections by
   reason (bounded label set), replication lag gauges, the reconnect counter and the stream state gauge. Record at the
   owners: `crates/wasm/shamir-wasm-host` for functions, `crates/server/shamir-sync` (replication) for the follower and
   leader stream. Hot paths get only relaxed atomic operations; no lock, no allocation per call.
2. **Export path.** The folder `crates/server/shamir-server-security/src/security/observability/` already has 6 entries, so
   the per-area exporters go into a new subfolder `observability/export/` (its 7th entry): `mod.rs` (re-exports only),
   `functions.rs` and `replication.rs`. A parallel step (storage and WAL metrics) creates the same folder with `storage.rs`;
   if your base lacks the folder, create it — the orchestrator reconciles `mod.rs` and the scrape-handler call when both
   land. Call the exporters from the scrape handler next to the existing rows, reading snapshots only. Follow the house
   naming of section 1.1 and the cardinality rules of section 4 (no per-function label unless the catalogue says so, and
   then capped).
3. **Tests** (CI only): unit tests for each counter at its owner, and an export test that scrapes and checks every new series
   name and type.
4. **Docs**: one paragraph per new metric in `docs/guide-docs/guide/07-operations.md` section 3 (meaning, unit, normal range).

## Rules

* No change in the `repo`, `table`, `tx` or `query` layers of `shamir-engine` (being cut into crates in parallel). If a
  replication metric's natural home is there, record it on the `shamir-sync` side instead, or stop and say so in the report.
* `crates/server/shamir-sync/src/format_stamp/` is being changed by another step: do not touch it.
* No new dependency except `shamir-metrics` where a histogram is recorded.
* At most 7 entries per folder and 1000 lines per file; count from listings.

## Acceptance (observed in CI)

fmt, clippy (`-p shamir-wasm-host -p shamir-sync -p shamir-server-security --all-targets`) and layout clean locally; the tests of
the touched crates green in CI.
