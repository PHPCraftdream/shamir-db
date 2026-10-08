<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-collections — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Repeated shift-removal has a concrete superlinear witness in the MVCC consumer. The alias documentation and unsupported performance percentage remain unchanged.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — TMap/TSet docs omit the O(N) order-preserving-removal asymmetry; consumers hit it on hot paths via the alias invisibly

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Pinned IndexMap 2.14.0 decrements later indices and removes from its dense Vec. MVCC snapshot_le returns key-sorted winners, collected into TMap; matching key-major history groups can repeatedly remove the first remaining winner, yielding N(N−1)/2 entry shifts. This proves structural worst-case work, not measured latency. Replacing shift_remove blindly changes the remaining overlay drain order. Source: https://docs.rs/crate/indexmap/2.14.0/source/src/inner.rs.

Evidence: [Cargo.lock:1782](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1782); [crates/shamir-collections/src/lib.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L19); [crates/shamir-tx/src/versioned_overlay.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/versioned_overlay.rs#L221); [crates/shamir-tx/src/mvcc_store/mod.rs:1403](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/mod.rs#L1403); [crates/shamir-tx/src/mvcc_store/version_entry.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/version_entry.rs#L124); [crates/shamir-tx/src/mvcc_store/version_entry.rs:289](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/mvcc_store/version_entry.rs#L289).

<a id="review-2"></a>

### Claim 2 — "~15–20% faster than TMap/TSet" claim on TFxMap/TFxSet has no benchmark anywhere in the workspace

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The percentage persists without supporting comparative evidence in inspected benchmark sources/history. Existing collection-using planner/filter/vector benchmarks do not isolate this alias-family difference.

Evidence: [crates/shamir-collections/src/lib.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L41); [crates/shamir-collections/src/lib.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L45); [crates/shamir-query-types/benches/batch_planner.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/benches/batch_planner.rs#L22).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

## Evidence and recipe corrections

- The sorted overlay construction strengthens the shifting witness: early removals are reachable without inventing arbitrary insertion order.
- The overlay is a committed-but-undrained window; actual size, scheduling and latency were not measured.
- swap_remove's average-case cost trades away order. The current leftover Vec is popped from the end, so a replacement must preserve or deliberately revise that observable sequence.
- Documentation alone does not remove the consumer's quadratic shifting.
- Do not replace the percentage with measurably faster without measurements, or describe preallocation as one constant-time allocation.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-collections -- Performance & O(x->0)

## Summary
The crate is a 64-line leaf: `THasher` plus five type aliases (`TMap`/`TSet` over IndexMap, `TFxMap`/`TFxSet` over std) and eight O(1) constructors. It is structurally clean against the five pillars — every hash-keyed surface is Fx-hashed (pillar 4), the constructors allocate once or not at all, there are no loops, no locks, no buffering in the crate itself. The crate-level `#![allow(clippy::disallowed_types)]` is the one allow-site explicitly sanctioned by the workspace `clippy.toml`, so it is compliant, not drift. The only substantive gap is interface documentation: the aliases that define the workspace's default ordered collections say nothing about IndexMap's order-preserving removal being O(N), and 100+ consumer sites pick a removal strategy through these aliases blind.

## Findings

### 1. TMap/TSet docs omit the O(N) order-preserving-removal asymmetry; consumers hit it on hot paths via the alias invisibly
- **File:line:** `crates/shamir-collections/src/lib.rs:19-23` (alias doc comments); impact surfaces at e.g. `crates/shamir-tx/src/mvcc_store/version_entry.rs:124`
- **Severity:** medium
- **Issue:** `TMap<K,V>` / `TSet<T>` are documented only as "Ordered map/set that maintains insertion order for predictable iteration." In IndexMap, the order-preserving removals (`shift_remove` / `shift_take`) are **O(N)** — they memmove every subsequent entry down *and* decrement its stored index — while `swap_remove` / `swap_take` are O(1) but scramble order. This asymptotic asymmetry is invisible at every one of the 100+ `use shamir_collections::...` sites; this leaf crate's doc comments are the single canonical definition point where that cost could be documented once for all consumers. This is exactly pillar 3's "avoid hidden O(N)/O(N²) in helpers" trap: the alias looks like a drop-in map, so an author reaches for `.shift_remove()` expecting map-like O(1).
- **Failure scenario (real call site, this workspace):** `OverlayWinners = TMap<Bytes, (u64, Bytes)>` (`version_entry.rs:42`) backs the streaming CURRENT-scan group-by; `flush_group` calls `overlay.shift_remove(&key)` once per history group matched (`version_entry.rs:124`, comment acknowledges "shift_remove keeps remaining keys"). With N overlay winners and K matched groups, cost is Σ O(N−i) ≈ O(N·K); removing entries near the front of the insertion order (worst case for shift) on a large pending-write window turns the merge super-linear. The caller bears the code fix (owning reviewer: shamir-tx), but the root interface knowledge belongs here.
- **Suggested fix:** Add two sentences to the `TMap`/`TSet` doc comments at lib.rs:19–23, e.g.: "Order-preserving removal (`shift_remove`/`shift_take`) is O(n): it shifts all later entries. On hot paths prefer `swap_remove`/`swap_take` (O(1), changes iteration order) or drain/bulk-build instead of per-element shifts." Cost: zero runtime change, closes the visibility gap at the alias definition site.

### 2. "~15–20% faster than TMap/TSet" claim on TFxMap/TFxSet has no benchmark anywhere in the workspace
- **File:line:** `crates/shamir-collections/src/lib.rs:41-47`
- **Severity:** nit
- **Issue:** The doc comments justify `TFxMap`/`TFxSet` with a quantified perf delta ("~15-20% faster … for hot-path lookups"), but the crate ships no benches (no `benches/` directory — verified by glob) and no other bench in the repo compares IndexMap-with-Fx vs std-HashMap-with-Fx under a named scenario. An unverifiable number steers hot-path authors by authority rather than measurement, contrary to the project's bench-first culture (`bench_scale_tool::Harness`). Directionally the claim is plausible (IndexMap pays a double-structure lookup + index indirection vs std's flat table), so this is a credibility nit, not a correctness issue.
- **Failure scenario:** none in code; risk is misplaced tuning effort based on a stale/unbenchmarked figure.
- **Suggested fix:** Either soften to qualitative wording ("avoids IndexMap's index indirection; measurably faster lookups") or add a small `benches/tmap_vs_tfx_lookup.rs` (via `bench_scale_tool::Harness`, isolated target dir per CLAUDE.md) and cite it.

## Verified non-findings (checked, compliant)
- All hash structures default to `THasher` (FxHasher) — pillar 4 fully honored (lib.rs:17,20,23,43,47).
- Constructors are O(1) one-shots; `_wc` variants exist for pre-reservation; no allocation-in-loop possible in this file (no loops).
- Crate-wide `#![allow(clippy::disallowed_types)]` (lib.rs:9) is required for `TFxMap`/`TFxSet` and is sanctioned verbatim by workspace `clippy.toml` ("The ONE sanctioned allow-site") — documented design, not lint drift.
- No locks, no async surfaces, no unbounded buffering owned by this crate.
- Test-coverage note (for the record): the crate contains no `tests/` directory at all; given it exports pure type aliases + thin constructors, behavioral-test surface is near-zero, though finding #2 shows the perf claim would benefit from a bench rather than a unit test.

</details>
