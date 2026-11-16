# Sub-brief 03: WAL metrics recording + snapshot plumbing (PR-3 of engine-metrics, part 1)

Committed parent brief: `docs/dev-artifacts/prompts/metrics/02-storage-wal-metrics.md`.
Design: `docs/dev-artifacts/design/engine-metrics.md` §2.4 (WAL catalogue rows), §3 (histogram primitive), §7 step PR-3.
Scope: crates `shamir-wal` and `shamir-engine-repo` only. No other crate may change.

## Catalogue rows delivered (§2.4, exact names/types/units/labels)

| Series | Type | Unit | Labels |
|---|---|---|---|
| `shamir_wal_bytes_total` | counter | bytes | `db`,`repo` |
| `shamir_wal_size_bytes` | gauge | bytes | `db`,`repo` |
| `shamir_wal_segments` | gauge | segments | `db`,`repo` |
| `shamir_wal_fsync_seconds` | histogram | seconds | `db`,`repo` |
| `shamir_wal_truncation_lag_versions` | gauge | versions | `db`,`repo` |
| `shamir_group_commit_waiters` | gauge | waiters | `db`,`repo` |

Rendering happens in sub-brief 04 (server-security); this brief only records state and extends the snapshot.

## shamir-wal changes

1. `Cargo.toml`: add `shamir-metrics = { path = "../../foundation/shamir-metrics" }` (the one allowed new dependency).

2. `wal_group_commit/recoverable_through.rs` — `SyncCoordinator` gains one field
   `fsync: Histogram` (constructed with `shamir_metrics::LATENCY_NS_BOUNDS`).
   In `run()`, where the round duration is already computed (`let ns = ...`,
   next to the `metric_duration_ns` fetch_adds), observe the SAME `ns` value:
   `self.fsync.observe(ns)`. Zero additional clock reads; failed rounds count,
   exactly like the existing duration counter. Measured span = sync-worker
   round (snapshot replay + sink sync) — the exporter help text states this.
   Add a snapshot accessor `fn fsync_histogram_snapshot(&self) -> HistogramSnapshot`
   returning `self.fsync.snapshot()`.

3. `wal_group_commit/wal_group_commit.rs` — public pass-through accessors on
   `WalGroupCommit` (exact signatures, delegating to the sink / coordinator):

   ```rust
   pub fn segment_count(&self) -> u64
   pub fn wal_size_bytes(&self) -> u64
   pub fn bytes_written_total(&self) -> u64
   pub fn truncation_lag_versions(&self, durable_version: u64) -> u64
   pub fn fsync_histogram_snapshot(&self) -> shamir_metrics::HistogramSnapshot
   ```

   `group_commit_waiters` needs no new accessor: `sync_waiters()` already
   exists. The sync coordinator histogram accessor is `pub(crate)` (used by
   engine-repo only through `WalGroupCommit`).

4. `segment/wal_sink.rs` — sync (non-async) dispatch accessors on `WalSink`,
   `pub(crate)`: File variant delegates to the `SegmentSet` accessor,
   Mem variant returns 0 (the `shamir_wal_backend_mem` gauge already tells
   the operator the backend; the help text documents "Mem reports 0").
   Do NOT touch `sealed_retained_bytes()` (async, exact) — it stays for tests.

5. `segment_set/segment_set.rs` — the recording layer:
   - `SealedMeta` gains `pub(super) bytes: u64` (in-memory on-disk length of
     the sealed file; never persisted, the sidecar format is untouched).
   - `SegmentSet` gains two atomics:
     `segment_count: AtomicUsize` — mirror of retained segment FILES
     (sealed + the one active segment; design §2.4 prescribes the O(1)
     mirror). Seeded at `open` (fresh dir: 1 once the active segment exists;
     existing dir: sealed + 1), decremented at truncate by the number of
     entries that leave the sealed list (both the removed and the
     NotFound paths), unchanged by rotation (sealed +1, active stays 1).
     `bytes_written_total: AtomicU64` — cumulative WAL payload bytes.
     Seeded at `open` with (sum of sealed `bytes` + the active segment's
     `approx_len_bytes()`), and incremented with the batch payload length by
     ONE `fetch_add(Relaxed)` on each successful `append_batch` — including
     the retry-after-poison success path — so a batch is counted exactly once.
     All reads scrape-side with `Relaxed`; no lock, no allocation on the
     append path beyond this one relaxed RMW per window.
   - SealedMeta bytes are filled at `open` (one `std::fs::metadata` per
     sealed file inside the existing `spawn_blocking` probes; open is cold)
     and at seal time from the outgoing segment's `approx_len_bytes()`
     (both `seal_and_rotate` and `rotate_after_poison` push to the sealed
     list — cover both). A poison partial frame may leave a few on-disk
     bytes uncounted until reopen; the size gauge's help text says
     "approximate".
   - New sync scrape accessors on `SegmentSet` (crate-visible, called by
     `WalSink` only):
     `segment_count() -> u64` (relaxed load of the mirror),
     `bytes_written_total() -> u64` (relaxed load),
     `approx_size_bytes() -> u64` (= active `approx_len_bytes()` + sum of
     sealed `bytes` under the existing inner lock — scrape-only cold path,
     same sanctioned lock `has_truncatable` already takes),
     `truncation_lag_versions(durable_version: u64) -> u64` (under the same
     lock: the minimum `max_version` among sealed metas with
     `!pinned && max_version > 0 && max_version <= durable_version`;
     `durable_version.saturating_sub(that)`; 0 when none truncatable —
     "how far durability has advanced past the oldest reclaimable-but-
     retained segment boundary", matching the design's lag intent).

## shamir-engine-repo changes

1. `Cargo.toml`: add `shamir-metrics = { path = "../../foundation/shamir-metrics" }`.

2. `journal/backend/durability_policy/durability_metrics.rs`:
   - `DurabilityMetricsSnapshot` (versioned by position — extend ONLY at the
     end): `gauges: [u64; 11]` → `[u64; 15]`, `counters: [u64; 8]` → `[u64; 9]`,
     `journal` unchanged, plus ONE trailing struct field
     `pub fsync: Option<HistogramSnapshot>` (u64 arrays cannot carry
     histogram buckets; the trailing field keeps every positional reader
     stable; `#[derive(Default)]` keeps working — `None`).
   - Public index constants on the snapshot impl (single source of truth for
     positions): `GAUGE_WAL_SIZE_BYTES = 11`, `GAUGE_WAL_SEGMENTS = 12`,
     `GAUGE_WAL_TRUNCATION_LAG_VERSIONS = 13`,
     `GAUGE_GROUP_COMMIT_WAITERS = 14`, `COUNTER_WAL_BYTES_TOTAL = 8`.
   - `DurabilityMetrics::snapshot(policy, group)` fills the new slots from
     `group` (`WalGroupCommit` accessors; `map_or(0, ...)` when uninitialized):
     size, segments, lag (durable argument = the already-computed `w`,
     the sampled max-committed version — a diagnostic sample like W/R),
     `g.sync_waiters()`, `g.bytes_written_total()`, and
     `fsync: group.map(WalGroupCommit::fsync_histogram_snapshot)`.
   - Update the struct/module doc comment tersely (one line per new slot).

3. `journal/backend/durability_policy/mod.rs`: re-export `HistogramSnapshot`
   so `shamir_db::engine::repo::durability_policy::HistogramSnapshot` resolves
   (the chain `shamir-engine` → `pub use shamir_engine_repo::durability_policy;`
   → `shamir_db::engine` is wholesale — no other crate edits needed).

4. New tests `journal/backend/durability_policy/tests/` (`mod.rs` manifest
   only + one test file; the folder's entry count stays ≤ 7):
   index-stability test — old snapshot indices keep their documented meaning
   and the new slots land exactly at the constants. Build a `WalGroupCommit`
   over an in-memory sink (existing test patterns in shamir-wal show the
   constructor), take `durability_metrics_snapshot()` semantics via
   `DurabilityMetrics::snapshot` with a `Default` policy, assert legacy slots
   0..11 / 0..8 against their documented values and the new slots against the
   constants (0 for an uninitialized/mem WAL where applicable, `None` fsync
   when the group is absent — use `snapshot(…, None)` for that half).

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
  `use` inside bodies, no new `#[allow]`, no visibility widening of existing
  items, `THasher` for hash maps, comments terse. Count folder entries from
  listings before adding files.
* No new dependency except `shamir-metrics`. No change in any other crate.
* ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index. Only edit
  files; the orchestrator commits.

## Acceptance observed by the orchestrator

`cargo fmt -p shamir-wal -p shamir-engine-repo -- --check` clean;
`cargo clippy -p shamir-wal -p shamir-engine-repo --all-targets -- -D warnings`
clean; behavioural tests green in CI (nextest in the PR's CI).
