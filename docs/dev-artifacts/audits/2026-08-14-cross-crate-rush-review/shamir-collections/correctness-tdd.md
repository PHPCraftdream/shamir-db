# shamir-collections -- Correctness & TDD-coverage

## Summary
The crate is a 64-line leaf (`src/lib.rs`: type aliases + capacity-aware constructors over
IndexMap/IndexSet/std collections with `THasher`). Implementation logic itself is sound -- no logic
bug, unsafe block, or invariant violation was found, and the crate-level
`#![allow(clippy::disallowed_types)]` is explicitly sanctioned by `clippy.toml` as "the ONE
sanctioned allow-site". The theme problem is discipline, not defects: the entire crate has **zero
tests** anywhere (no `src/tests/`, no integration `tests/`, `doctest = false`), despite CLAUDE.md's
normative Red/Green/Refactor protocol and despite this crate anchoring pillar-4 (`THasher`,
Fx-only hashing) for the whole workspace. One documentation-correctness issue: an unverifiable
performance figure is shipped in rustdoc as measured fact.

## Findings

### 1. Entirely untested crate -- every documented behavioral contract has zero regression protection
- **File:** `crates/shamir-collections/src/lib.rs:1-64` (whole crate); `Cargo.toml:16`
- **Severity:** high
- **Issue:** There is no `tests/` directory anywhere in the crate (neither `src/tests/` per the
  CLAUDE.md test-layout rule, nor a crate-root integration dir), and `doctest = false` disables
  even doc-level checks. CLAUDE.md makes TDD normative ("write a failing #[tokio::test] first") --
  none of that process left an artefact here. The following load-bearing contracts are pinned by
  nothing:
  - `THasher = BuildHasherDefault<FxHasher>` (lib.rs:17) is the ideological anchor cited by
    `clippy.toml` disallowed-methods/types and consumed workspace-wide (shamir-engine, shamir-tx,
    shamir-index, shamir-server, shamir-storage, shamir-db). No failing-if-swapped guard exists --
    e.g. a refactor back to `RandomState` compiles and passes the `@types` scope silently.
  - TMap/TSet insertion-order iteration (lib.rs:3,19-23 doc promise, relied upon by
    `shamir-query-types/src/batch/planner.rs` topological sort and `batch_execute.rs`'s
    order-preserving `TMap<String, QueryResult>` accumulation) -- including the sharp edges
    between order-preserving O(n) `remove` and order-scrambling O(1) `swap_remove`.
  - `_wc` constructors reserve >= requested capacity (lib.rs:29-38,53-63).
  - serde round-trip preserves order (indexmap `serde` feature enabled in `Cargo.toml:10`
    deliberately; wire DTOs serialize these maps).
- **Failure scenario:** an indexmap upgrade or an in-place refactor changes ordering semantics or
  hasher identity; batch-plan determinism and result/response ordering degrade silently across
  dependent crates. `./scripts/test.sh @types` still reports green for this crate -- it runs zero
  assertions -- so CI has no signal and the regression surfaces only as flaky downstream behavior.
- **Suggested fix:** add `src/tests/mod.rs` (manifest-only, per convention) with small pure-sync
  topic files: `hasher_tests.rs` (assert the builder type is Fx, assert two identically-keyed
  `TFxMap`s hash-insert consistently), `tmap_order_tests.rs` (insertion order after overwrite,
  after `remove`, after `shift_remove`; document `swap_remove` scrambling), `tset_tests.rs`
  (dedup keeps first-insert position), `capacity_tests.rs` (`_wc` len==0/capacity>=n), and
  `serde_roundtrip_tests.rs` (order preserved). Sub-second, fully inside the existing `@types`
  scope.

### 2. Unverifiable performance claim stated as fact in stable rustdoc
- **File:** `crates/shamir-collections/src/lib.rs:41-42, 45-46`
- **Severity:** low
- **Issue:** "~15-20% faster than TMap" / "than TSet" is presented as an established measurement,
  but it originates as an *expected* effect in a planning doc
  (`docs/dev-artifacts/audits/shamir-collections.md:16` -- "Ожидаемый эффект −15-20% lookup",
  probability rated Низкая). The crate has no `[[bench]]` target, no `benches/` directory, and no
  committed harness run substantiates the number anywhere; CLAUDE.md requires perf conclusions come
  from real runs through `bench_scale_tool`.
- **Failure scenario:** no runtime failure; future contributors treat the figure as benchmarked
  truth and churn hot-path call sites (or argue against fixing) on an unmeasured basis.
- **Suggested fix:** either (a) soften to "expected faster: see
  docs/dev-artifacts/audits/shamir-collections.md (#2)", or (b) actually measure once with a
  `benches/fx_vs_index_lookup.rs` using `bench_scale_tool::Harness` (isolated bench target dir per
  CLAUDE.md) and cite the run in the doc comment.

### 3. Redundant `use std::cmp::Eq` import
- **File:** `crates/shamir-collections/src/lib.rs:13`
- **Severity:** nit
- **Issue:** `Eq` (and `Hash`, imported separately) are both in the prelude; `std::cmp::Eq` adds
  noise with no scoping benefit. Imports are correctly top-of-file (no violation), just superfluous.
- **Suggested fix:** drop the import; keep the plain `Eq`/`Hash` names in the generic bounds.

## Verification notes
- `clippy.toml:37-44` confirms the crate-level `#![allow(clippy::disallowed_types)]` (lib.rs:9) is
  the single sanctioned escape hatch for the Fx-only type ban -- judged compliant, not a finding.
- All eight exported constructors (`new_map`, `new_map_wc`, `new_set`, `new_set_wc`, `new_fx_map`,
  `new_fx_map_wc`, `new_fx_set`, `new_fx_set_wc`) have live callers across the workspace -- no dead
  API found.
- Cross-checked consumers: `shamir-types/src/types/common.rs:5` re-exports the `new_*`/`TMap`/
  `TSet`/`THasher` surface (notably NOT `TFxMap`/`TFxSet`); downstream crates importing `TFx*` do so
  directly from `shamir_collections` -- consistent, though it means pinning tests must live in this
  crate, reinforcing finding 1.
