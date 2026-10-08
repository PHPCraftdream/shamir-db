<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-collections — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The removal-cost documentation gap and unsupported percentage remain open. The cited MVCC merge still performs repeated shift_remove calls, proving possible superlinear shifting work; no latency measurement was performed.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — TMap/TSet docs omit the O(N) order-preserving-removal asymmetry; consumers hit it on hot paths via the alias invisibly

Status: `confirmed-open`. Current risk: `medium`.

Alias rustdoc still omits removal costs. OverlayWinners remains a TMap and flush_group still calls shift_remove once per matched history group; repeated early-position removals can incur quadratic aggregate shifting. The consumer mechanism remains, not merely a stale citation.

Evidence: [crates/shamir-collections/src/lib.rs:19](../../../../../crates/shamir-collections/src/lib.rs#L19); [crates/shamir-collections/src/lib.rs:22](../../../../../crates/shamir-collections/src/lib.rs#L22); [Cargo.lock:1783](../../../../../Cargo.lock#L1783); [crates/shamir-tx/src/mvcc_store/version_entry.rs:42](../../../../../crates/shamir-tx/src/mvcc_store/version_entry.rs#L42); [crates/shamir-tx/src/mvcc_store/version_entry.rs:124](../../../../../crates/shamir-tx/src/mvcc_store/version_entry.rs#L124); [crates/shamir-tx/src/mvcc_store/version_entry.rs:197](../../../../../crates/shamir-tx/src/mvcc_store/version_entry.rs#L197).

<a id="review-2"></a>

### Claim 2 — "~15–20% faster than TMap/TSet" claim on TFxMap/TFxSet has no benchmark anywhere in the workspace

Status: `confirmed-open`. Current risk: `nit`.

The numeric claim remains. Collection-using workspace benches were searched, but no comparative measurement supporting it was found; existing planner/filter benches do not establish this alias-family delta.

Evidence: [crates/shamir-collections/src/lib.rs:41](../../../../../crates/shamir-collections/src/lib.rs#L41); [crates/shamir-collections/src/lib.rs:45](../../../../../crates/shamir-collections/src/lib.rs#L45); [crates/shamir-query-types/benches/batch_planner.rs:22](../../../../../crates/shamir-query-types/benches/batch_planner.rs#L22); [docs/dev-artifacts/audits/shamir-collections.md:38](../../../../../docs/dev-artifacts/audits/shamir-collections.md#L38).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

## Corrections and qualified non-findings

- Constructor non-findings need correction: nonzero IndexMap/IndexSet capacity construction is O(capacity) and allocates an indices table plus dense entries, not one allocation and constant work.
- The crate remains free of local loops, locks and async surfaces; delegated allocation costs still count.
- Removal costs are average-case hash-table bounds: shift_remove shifts later entries, whereas swap_remove changes order. Deprecated remove is swap-removal.
- The MVCC claim establishes structural worst-case work, not measured production slowdown or proof that every consumer selects removals blindly.
- Changing the MVCC caller to swap_remove requires checking overlay-only output ordering; the documentation fix alone does not remove its shifting cost.
- Suggested replacement wording must not say measurably faster without evidence. Use an explicitly unmeasured qualitative description or cite an actual comparative run.

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
