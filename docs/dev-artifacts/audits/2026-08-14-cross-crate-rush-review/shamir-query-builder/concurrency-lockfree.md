<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-builder — concurrency-lockfree revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The no-concurrency-finding conclusion is upheld for the builder itself: owned state, synchronous construction, no locks, atomics, mutable globals, runtime tasks, or unsafe code. Hash-keyed builder collections use the Fx-backed aliases. Complexity and codec correctness require the qualifications below.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 0 | 0 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

No local defect was established for this lens. The scope and positive-assurance qualifications below still apply.

## Corrections and qualified non-findings

- Ownership and collection guarantees are source-supported by crates/shamir-query-builder/src/batch/batch.rs:27, crates/shamir-query-builder/src/write/doc.rs:19, and crates/shamir-collections/src/lib.rs:18. Test-only BTreeMap fixture maps are not hash-keyed.
- Hash-map membership is expected/amortized constant-time, not an unconditional adversarial worst-case O(1) guarantee.
- The fallback is documented but not unconditionally correct: it violates nested scoping and exact marker recognition. See correctness-tdd.md#1 and correctness-tdd.md#3.
- switch construction and output are quadratic in case count, weighted by condition-tree sizes; this is inherent to the current complementary-guard encoding, not a concurrency defect.
- The crate has no benchmark harness. Its synchronous API is appropriate for in-memory construction; absence of concurrency tests is not a defect.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder -- Concurrency & lock-free invariants

## Summary

This crate is a pure, single-owner fluent builder over wire DTOs: it contains no locks, no atomics, no shared/global state, and no async surface at all (zero matches for `Mutex`/`RwLock`/`parking_lot`/`arc_swap`/`scc`/`dashmap`/`Atomic*`/`OnceLock`/`thread_local`/`Arc`/channels/`unsafe`/`async`/`.await`/`tokio` across `src/` and `tests/`), so the lock-free, async-I/O, and scc/dashmap pillars are satisfied vacuously — there is nothing shared to synchronize and no I/O-bound op to make async (the crate is deliberately WASM-lean, `lib.rs` doc: "no engine or runtime dependency"). Pillar 4 (Fx hash) is fully honored: every hash-keyed structure routes through `shamir_collections::{TMap, TSet, new_map}` (`Batch::queries` / `Batch::interner_epochs` in `batch/batch.rs`, `Doc::fields` in `write/doc.rs`, the `bind!` macro in `macros/mod.rs`); the only `std::collections` uses are `BTreeMap` in two integration-test fixtures (`tests/repl_ddl_msgpack.rs`, `tests/vector_filter_msgpack.rs`), which are comparison-ordered (not hash-keyed) and test-only, so outside pillar 4's scope. Pillar 3 (O(x→0)) holds on the paths that could have hidden costs: `try_build`'s alias validation uses O(1) `contains_key`/`get_mut` lookups, and the only O(payload) work — the msgpack round-trip fallback in `collect_op_query_refs` for the ~75 untyped `BatchOp` variants — is an explicit opt-in validation path whose typed fast path and conservative-fallback trade-off are already documented inline (the `#1093` section of `batch/batch.rs`); `Batch::switch`'s guard folding is O(K²) in output nodes, but that equals the size of the wire data its semantics require (each branch's `when` must exclude every prior condition), so it is not an algorithmic slip. Per-module `tests/` directories exist per the CLAUDE.md layout and cover builder semantics; the absence of concurrency-specific tests is appropriate since there is no concurrent or shared state to test.

## Findings

No findings for this theme.

</details>
