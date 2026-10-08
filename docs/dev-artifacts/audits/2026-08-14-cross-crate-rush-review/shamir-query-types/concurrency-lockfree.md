<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-types — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

No direct executable concurrency or unsafe surface was found. Sequential stage execution is independently source-supported; absence of transitive concurrency dependencies and blanket parallelism honesty are not.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 0 | 0 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

## Evidence and recipe corrections

- Cargo.lock:3813 positively includes arc-swap and dashmap through shamir-types. No direct concurrency source does not mean none can be pulled in.
- max_nesting_depth_of_ops is bounded recursion at planner.rs:770, not an iterative worklist.
- planner.rs:57 still says stage 1 runs in parallel; batch_execute.rs:522 runs aliases sequentially. The disclaimer is accurate, but the overview's universally honest-documentation assurance is too broad.
- Historical try_join_all timing is documentary evidence only, not a measurement performed or independently validated in this cycle.
- BatchPlanner is not the crate's only logic: custom codecs, reference parsing, canonicalization and validation helpers also execute code.

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
