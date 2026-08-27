# shamir-engine -- Style & CLAUDE.md structural conformance
## Summary
Structural conformance is strong but not clean: 15 of the crate's 16 `mod.rs` files are re-export-only and the per-module `tests/` layout is otherwise exemplary, yet three explicit CLAUDE.md rules are violated. `repo/group_commit/mod.rs` carries a full 125-line implementation; `#[cfg(test)] mod tests` blocks are embedded inline in two implementation files; and mid-function `use` imports appear systemically (~25 sites across ~15 production files, none fitting the documented exceptions). Remaining findings are naming/manifest-form drift in the test trees.
## Findings

### 1. `mod.rs` contains a full implementation, not re-exports
**File:** `crates/shamir-engine/src/repo/group_commit/mod.rs:1-128`
**Severity:** high
**Issue:** CLAUDE.md: "mod.rs files contain re-exports only. Types and logic live in sibling files." This file implements `GroupCommit`, `GcState`, `run()`, `leader_loop()`, and `recv()` (125 lines of logic plus the `mod tests;` decl). It is the only `mod.rs` in the crate with logic — every other one (`table/`, `tx/`, `repo/`, `validator/`, `meta/`, `migration/`, `db_instance/`, `index/`, `query/*`) complies.
**Failure scenario:** The precedent invites the next module to grow logic in its `mod.rs`; diffs against this file mix structural moves with behavioural edits, diluting `git blame` exactly as the rule intends to prevent.
**Suggested fix:** Move the implementation to a sibling `group_commit.rs` (module dir keeps `mod.rs` with `mod group_commit; pub use group_commit::GroupCommit;`, adding `#[allow(clippy::module_inception)]` — same precedent as `table/table.rs` and `db_instance/db_instance.rs`). Style-only commit per the CLAUDE.md sweep rule.

### 2. Systemic mid-function `use` imports (~25 sites, ~15 production files)
**File:** representative sites listed below
**Severity:** high
**Issue:** CLAUDE.md "Imports at the top" bans `use` inside function/block bodies outside three documented exceptions (test-mod `use super::*`, trait-name collision with a comment, cfg-gated bodies). None of these sites fit an exception. Confirmed sites:
- `migration/shadow_log.rs:50,109,129` — the *same* `use futures::StreamExt;` repeated inside three different fns
- `table/table_manager.rs:16` (`table_token_for`), `:935`, `:975` (`use std::sync::atomic::Ordering`), `:1582`
- `table/read_exec.rs:1717-1719, 1774-1778`; `table/read_planner.rs:38-39, 78`; `table/read_index_scan.rs:412, 508`
- `table/table_manager_index_mgmt.rs:37-40`; `table/table_manager_sorted_index.rs:48`; `table/table_manager_tx_ops.rs:94`; `table/table_manager_validators.rs:316`
- `tx/commit.rs:832-833`; `tx/commit_phases.rs:889` (`:566` is inside a `#[cfg(test)]` block — borderline under the cfg exception)
- `query/batch/fk_actions.rs:1284`; `query/batch/fk_on_update.rs:1092`
- `query/filter/compile.rs:196`; `query/filter/eval_bytes.rs:656`; `query/read/parser.rs:178` (import nested inside a match arm)
- `validator/validator_db.rs:117`; `validator/schema/cross_field.rs:37` (`use CompareOp::*`), `:118`
- `repo/repo_instance.rs:1702`
**Failure scenario:** Hidden per-function dependencies: a reader scanning the file header misses that `shadow_log.rs` methods need `StreamExt`, and the same import is already duplicated three times in that one file — the pattern propagates by copy-paste.
**Suggested fix:** Hoist all listed imports to file headers in one `style:` commit (some may need no other change; `use CompareOp::*` at top of `cross_field.rs` is safe since `CompareOp` is defined there). Flag `parser.rs:178` (match-arm-nested import) as the most misleading of the set.

### 3. Inline `#[cfg(test)] mod tests` embedded in implementation files
**File:** `crates/shamir-engine/src/query/read/hashable_query_value.rs:250-379`; `crates/shamir-engine/src/table/writer_drain_barrier.rs:410-534`
**Severity:** medium
**Issue:** CLAUDE.md test-organisation rule 5: "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." `hashable_query_value.rs` is ~34% inline tests (129 of 379 lines) even though `query/read/tests/` already exists; `writer_drain_barrier.rs` carries ~124 lines of inline tests (a sibling `#[cfg(loom)] mod loom_model` at :535 also exists, but that one is a deliberate, `build.rs`-coupled model-checker module with documented rationale — a defensible cfg-gated exception; the plain `#[cfg(test)] mod tests` is not).
**Failure scenario:** Test and impl edits collide in one file's history; the inline block grows unbounded because the `tests/` split discipline is invisible at the point of editing.
**Suggested fix:** Move to `query/read/tests/hashable_query_value_tests.rs` and `table/tests/writer_drain_barrier_tests.rs` (manifest entries added to the respective `tests/mod.rs`), converting `use super::*` to explicit `crate::…` paths. Keep the loom module where it is.

### 4. Test manifests deviate from the documented `pub mod` form and duplicate cfg gating
**File:** `crates/shamir-engine/src/repo/tests/mod.rs:1-16`; `crates/shamir-engine/src/repo/group_commit/tests/mod.rs:1-4`; `crates/shamir-engine/src/query/*/tests/mod.rs` (private `mod` decls); mixed forms in `query/read/tests/mod.rs:3-7` and `query/batch/tests/executor_tests/mod.rs`
**Severity:** low
**Issue:** CLAUDE.md prescribes the manifest form `pub mod value_tests;`. The `query/**` trees use private `mod x_tests;` instead, two manifests mix both forms in the same file, and `repo/tests/mod.rs` + `group_commit/tests/mod.rs` add a redundant `#[cfg(test)]` to every line although the parent (`repo/mod.rs:9-10`) already gates the whole `tests` module. Spirit (manifest-only, no test code) is honored everywhere; the form is inconsistent across siblings (`table/`, `tx/`, `validator/`, `meta/`, `migration/`, `db_instance/` all use the documented `pub mod` form).
**Failure scenario:** None functional; drift makes the "which tests exist" grep different per subtree (`pub mod` vs `mod` changes what `cargo doc`/IDE exposes and what a `pub`-visibility grep finds).
**Suggested fix:** One `style:` commit normalising manifests to `pub mod x_tests;` and dropping the redundant per-line `#[cfg(test)]`.

### 5. Test files missing the `_tests` suffix
**File:** `crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check.rs`, `p1097_remove_posting_owner.rs`, `p1100_stale_snapshot_delete_posting.rs`, `p1101_released_skip_durable_check.rs`; `crates/shamir-engine/src/table/tests/f53b_step3_cursor_after_spike.rs`
**Severity:** nit
**Issue:** CLAUDE.md's test layout prescribes one `*_tests.rs` file per topic; ~95% of the crate's test files follow it, these five don't (helper files like `test_helpers.rs` / `stream_utils.rs` / `helpers.rs` are correctly exempt — they aren't test files).
**Suggested fix:** Rename in a `style:` commit (git-mv to preserve history); add the `_tests` suffix to the manifest entries.

### 6. Test file nests a redundant `#[cfg(test)] mod tests` and splits helpers from tests
**File:** `crates/shamir-engine/src/query/batch/tests/watchdog_tests.rs:146-163`
**Severity:** low
**Issue:** The file defines `TestResolver` + `setup_resolver()` at top level, then wraps the actual `#[test]` fns in a nested `#[cfg(test)] mod tests { use super::*; … }`. The whole file is already test-gated by the parent manifest chain (`query/batch/mod.rs:187-188` → `tests/mod.rs`), so the inner `cfg` is dead and the helper/test split is arbitrary — unlike sibling test files, which put helpers and tests at one level.
**Failure scenario:** Copy-paste template risk: new test files imitate the nested form, spreading a second layout convention.
**Suggested fix:** Drop the inner `mod tests` wrapper (hoist its contents to file level) or move `TestResolver`/`setup_resolver` next to the tests they serve.

### 7. Tail-of-file `pub use` re-exports outside `mod.rs`
**File:** `crates/shamir-engine/src/query/batch/query_runner.rs:1856-1861`
**Severity:** nit
**Issue:** A comment "Re-export public items used outside this module" introduces `pub use crate::query::batch::batch_execute::execute_batch;` (plus a `#[cfg(test)]` sibling and the interactive-tx trio) at the bottom of an impl file, creating a second valid path (`…batch::query_runner::execute_batch`) alongside the canonical `query/batch/mod.rs:159-161` re-export of the same names. The crate's own convention (and CLAUDE.md) keeps re-exports in `mod.rs`.
**Suggested fix:** Fold into the existing `pub use query_runner::{…}` block in `query/batch/mod.rs` and delete the tail block.

### 8. `repo_types.rs` stretches "one file = one primary export" to 11 public types
**File:** `crates/shamir-engine/src/repo_types.rs:28-377`
**Severity:** nit
**Issue:** `BoxRepo` + 3 composites + `RepoFactory` trait + 5 factory types/enums live in one file. They form one conceptual family (repo backend + its factory variants), so this is defensible under the "closely-coupled group" clause — but it is the largest export surface in a single non-mod file in the crate, and the composites vs. factories split is a natural seam.
**Suggested fix:** Optional: split composites (`BoxRepo` + `*RepoComposite`) from factories (`RepoFactory` + `*RepoFactory`) when the file is next touched for substance; not worth a dedicated churn commit.
