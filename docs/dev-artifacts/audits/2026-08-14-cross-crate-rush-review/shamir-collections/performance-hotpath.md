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
