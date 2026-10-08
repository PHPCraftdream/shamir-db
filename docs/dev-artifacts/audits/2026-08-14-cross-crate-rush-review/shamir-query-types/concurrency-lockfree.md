<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-types — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The no-concurrency-surface conclusion remains valid for this crate's own source and bench: no executable async, locks, atomics, channels, spawns, static mutable state, or unsafe. Planning is DTO-only, and the executor still runs stage aliases sequentially.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 0 | 0 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

No local defect was established for this lens. The scope and positive-assurance qualifications below still apply.

## Corrections and qualified non-findings

- The nesting-depth walker is bounded recursion, not an iterative worklist: crates/shamir-query-types/src/batch/planner.rs:770.
- The absence of direct concurrency dependencies does not establish their absence transitively through shamir-types.
- Sequential execution is source-proven at crates/shamir-engine/src/query/batch/batch_execute.rs:522; historical try_join_all performance claims are documentary evidence only.
- The planner's example still says stage 1 runs in parallel despite its explicit logical-grouping disclaimer.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-types -- Concurrency & lock-free invariants

## Summary
shamir-query-types is a pure-DTO crate, and it is fully clean against this theme: every `.rs` file under `src/` (plus `Cargo.toml` and `benches/batch_planner.rs`) was read, and the crate contains zero concurrency primitives of any kind — no `std::sync::Mutex`/`RwLock`, no `parking_lot`, no atomics, no `scc::*`/`dashmap`/`ArcSwap`, no tokio, no threads/channels, no `static` items, and no `unsafe` (verified by grep across code and tests). The crate defines no `async fn` and has no `.await`, so a lock held across `.await` is structurally impossible, and the `scc::*::len()` O(N) ban is inapplicable (no `scc` types exist here; every observed `.len()` is on `Vec`/`String`/IndexMap-backed `TMap`/`TSet`, all O(1)). The one concurrency-adjacent design surface, `BatchPlan::stages`, is honestly documented in `planner.rs` (lines 13–26) as a logical grouping that today's executor drives sequentially, citing the oql-01 stage-parallelism ADR and the measured no-op of `try_join_all` — the opposite of an overclaimed parallelism guarantee. Pillar compliance here needs no lock-free justification because no shared mutable state exists to guard.

## Findings
No findings for this theme.

### Verification evidence (for future audit diffs)
- Grep over all `crates/shamir-query-types/**/*.rs`: zero matches for `Mutex`, `RwLock`, `parking_lot`, `Atomic*`, `arc_swap`, `scc::`, `DashMap`, `std::sync`, `tokio`, `thread`, `mpsc`/channel primitives, `thread_local!`, `static` items, `unsafe`, `Box::leak`, `mem::forget`, spin/sleep/park/`block_on` — in implementation and test code alike. All matches for "concurrent"/"parallel"/"lock-free"/"atomic" are doc comments describing other crates' behavior (e.g. `admin/types/retention.rs:109` describes the engine's `ArcSwap` retention swap; `write/types.rs:132` describes the SSI race the `expected_version` field closes).
- No `async fn` and no `.await` exist in the crate; the only `.await`-looking text is a doc-comment example (`admin/types/schema_ops.rs:54`, `table.count().await?` describing the server-side handler). "Locks across await" is therefore structurally impossible.
- All hash-keyed structures come from `shamir_collections` (`TMap`/`TSet`/`TFxSet`/`new_map`/`new_set`, i.e. IndexMap/IndexSet + Fx per pillar 4); the only `std::collections` import is `VecDeque` (planner.rs:60), which is not hash-keyed. No `RandomState` maps anywhere.
- `BatchPlanner::plan` (the crate's only real logic, `batch/planner.rs`) is a pure function over borrowed DTOs with no interior mutability; its output stages are documented as "a logical grouping, not a parallelism guarantee" with the executor's sequential behavior and the deferred `tokio::spawn`-per-query design recorded (ADR link in-module).
- Cargo.toml confirms the boundary: dependencies are serde/serde_bytes/rmp-serde/indexmap/num-bigint/hmac/sha2 + the two in-workspace types crates — no concurrency dependency can even be pulled in.
- Considered and rejected as out-of-theme (documented for sibling reviewers, not counted here): `planner.rs:758-760`'s doc says "iterative worklist" while `max_nesting_depth_of_ops` actually recurses (safe — capped at `NESTING_WALK_LIMIT` = 64 frames); `planner.rs:663`'s doc says `HashSet<&str>` where the code uses `TFxSet<&str>` (Fx-compliant); `read/query_record.rs` and `write/inserted_record.rs` embed `#[cfg(test)] mod tests` inline (test-organization theme); `InsertedRecord::serialize` allocates a key-pairs `Vec` per row (performance theme).

</details>
