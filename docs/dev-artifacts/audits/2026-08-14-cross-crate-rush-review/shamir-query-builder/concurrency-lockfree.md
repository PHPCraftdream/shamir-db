<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

No local concurrency mechanism was found: construction is synchronous and owned, and mutations require exclusive access. Fx-backed collections are used, but dependency implementations and complexity guarantees must not be conflated with lock-free production behavior.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 0 | 0 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

## Evidence and recipe corrections

- The no-finding conclusion applies to builder-owned synchronization, not every transitive dependency or engine caller.
- Exact indexmap 2.14.0 published src/map.rs documents insertion as amortized-average O(1), not unconditional worst-case O(1): https://docs.rs/crate/indexmap/2.14.0/source/src/map.rs . Its replacement behavior preserves key position and returns the old value.
- The fallback's documented conservatism does not make it semantically correct: nested scopes and literal markers contradict that assurance.
- Quadratic complementary switch guards are a property of this representation, not a necessary property of every possible conditional-execution design.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder -- Concurrency & lock-free invariants

## Summary

This crate is a pure, single-owner fluent builder over wire DTOs: it contains no locks, no atomics, no shared/global state, and no async surface at all (zero matches for `Mutex`/`RwLock`/`parking_lot`/`arc_swap`/`scc`/`dashmap`/`Atomic*`/`OnceLock`/`thread_local`/`Arc`/channels/`unsafe`/`async`/`.await`/`tokio` across `src/` and `tests/`), so the lock-free, async-I/O, and scc/dashmap pillars are satisfied vacuously — there is nothing shared to synchronize and no I/O-bound op to make async (the crate is deliberately WASM-lean, `lib.rs` doc: "no engine or runtime dependency"). Pillar 4 (Fx hash) is fully honored: every hash-keyed structure routes through `shamir_collections::{TMap, TSet, new_map}` (`Batch::queries` / `Batch::interner_epochs` in `batch/batch.rs`, `Doc::fields` in `write/doc.rs`, the `bind!` macro in `macros/mod.rs`); the only `std::collections` uses are `BTreeMap` in two integration-test fixtures (`tests/repl_ddl_msgpack.rs`, `tests/vector_filter_msgpack.rs`), which are comparison-ordered (not hash-keyed) and test-only, so outside pillar 4's scope. Pillar 3 (O(x→0)) holds on the paths that could have hidden costs: `try_build`'s alias validation uses O(1) `contains_key`/`get_mut` lookups, and the only O(payload) work — the msgpack round-trip fallback in `collect_op_query_refs` for the ~75 untyped `BatchOp` variants — is an explicit opt-in validation path whose typed fast path and conservative-fallback trade-off are already documented inline (the `#1093` section of `batch/batch.rs`); `Batch::switch`'s guard folding is O(K²) in output nodes, but that equals the size of the wire data its semantics require (each branch's `when` must exclude every prior condition), so it is not an algorithmic slip. Per-module `tests/` directories exist per the CLAUDE.md layout and cover builder semantics; the absence of concurrency-specific tests is appropriate since there is no concurrent or shared state to test.

## Findings

No findings for this theme.

</details>
