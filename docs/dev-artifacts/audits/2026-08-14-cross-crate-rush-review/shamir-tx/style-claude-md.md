# shamir-tx -- Style & CLAUDE.md structural conformance

## Summary

The crate is mostly disciplined on the tested-grounds that matter most: no inline `#[cfg(test)] mod tests {}` blocks anywhere, all three `tests/mod.rs` files are pure manifests, tests are split by topic, and there is no TODO/FIXME noise. The one bright-line breach is `src/mvcc_store/mod.rs`, a 1,638-line implementation file (struct + ~1,400-line impl) that directly violates the "mod.rs files contain re-exports only" rule — ironically in the same crate that demonstrates the compliant `changefeed.rs` + `changefeed/` layout. Secondary issues: 8 mid-function `use` statements in production code, a crate-root test layout that diverges from the documented per-module `tests/` layout, and a handful of stale doc comments left behind by the #532 `version_cache`→`cells` rename and older refactors.

## Findings

### 1. `mvcc_store/mod.rs` is a full implementation file, not a re-export manifest
- **File:** `crates/shamir-tx/src/mvcc_store/mod.rs:1-1638`
- **Severity:** high
- **Issue:** CLAUDE.md (Discipline rules): "`mod.rs` files contain re-exports only. Types and logic live in sibling files." This mod.rs contains the module doc, `pub(super) const TS_TAG` (:68), `pub(crate) fn ts_key` (:71) and `decode_ts_key` (:82), `pub(crate) struct RecordCell` (:94), `pub struct MvccStore` (:125) with all its field docs, and roughly 1,400 lines of `impl MvccStore` (constructor, overlay-GC, ts-index, streaming group-by, batched reads). The sibling files (`mvcc_history.rs`, `mvcc_gc.rs`, `drain.rs`, `mvcc_locks.rs`, …) correctly hold extension `impl MvccStore` blocks — the pattern is right, but the anchor struct and its core impl live in the one file the rules reserve for wiring.
- **Failure scenario:** No runtime failure; this is the maintainability cost the rule exists to prevent — the crate's most-edited type has its `git blame` diluted in a file whose diffs should only ever be module wiring, and logic changes masquerade as module-structure changes in review.
- **Suggested fix:** Move the struct + core impl into `mvcc_store/store.rs` (or adopt the crate's own `changefeed.rs` + `changefeed/` sibling-file layout, i.e. `src/mvcc_store.rs` + `src/mvcc_store/`), leaving `mod.rs` as docs + `mod`/`pub use` only. Purely mechanical; no API change (paths stay `crate::mvcc_store::*`).

### 2. Mid-function `use` statements in production code (8 sites, 5 files)
- **Severity:** medium
- **Issue:** CLAUDE.md ("Imports at the top"): `use` statements live in the file header, never inside a function or block body; none of the three documented exceptions (test-mod `use super::*`, same-name trait collision, cfg/macro-gated bodies) apply to these:
  - `src/mvcc_store/mod.rs:399-400` — `use futures::StreamExt;` + `use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;` inside `ts_index_rebuild`.
  - `src/mvcc_store/mod.rs:1389` — `use futures::stream::unfold;` inside `snapshot_stream_impl`.
  - `src/mvcc_store/mvcc_history.rs:88` — `use std::sync::atomic::Ordering;` inside `version_at_or_before_ts` (the file header already imports other std items; no collision).
  - `src/mvcc_store/mvcc_history.rs:103,105` — `MAINT_SCAN_BATCH` + `use super::TS_TAG;` inside `version_at_or_before_ts_scan` (a `#[cfg(test)]` fn — nearest to the cfg-gated exception, but both imports hoist cleanly, so the local use is not required).
  - `src/mvcc_store/mvcc_gc.rs:301` and `:397` — the identical `use crate::version_codec::decode_version_key;` duplicated inside two separate fn bodies; one top-level import serves both.
  - `src/tx_context.rs:512` and `:670` — `scc::hash_map::Entry` (and its variants) inside fn bodies; no same-name `Entry` in scope at file top.
  - `src/layered_interner.rs:96` — `use scc::hash_map::Entry::{Occupied, Vacant};` inside `intern_ind`.
- **Failure scenario:** Style-only, but it compounds: because `StreamExt` is not at the top of `mvcc_store/mod.rs`, line 1436 is forced into a fully-qualified `futures::StreamExt::map(...)` call — the missing header import leaks into call sites.
- **Suggested fix:** Hoist each to the file header (test files — `retention_tests.rs`, `crud_tests.rs`, `stream_tests.rs`, etc. — carry the same pattern at lower stakes; fix opportunistically).

### 3. Test placement deviates from the documented per-module `tests/` layout
- **File:** `crates/shamir-tx/src/tests/` (wired from `src/lib.rs:33-34`), `crates/shamir-tx/src/tests/mvcc_store_tests/`
- **Severity:** medium
- **Issue:** CLAUDE.md ("Test organisation", "strict layout"): one `tests/` directory **per module** (examples: `crates/shamir-types/src/types/tests/`, `crates/shamir-engine/src/table/tests/`), wired via the parent module's `#[cfg(test)] mod tests;`. shamir-tx instead concentrates tests for ~14 modules into a single crate-root `src/tests/`, and nests MvccStore's 28-file suite at `src/tests/mvcc_store_tests/` rather than `src/mvcc_store/tests/`. Only `changefeed` follows the documented shape (`src/changefeed/tests/`). Mitigations that keep this from being worse: all `tests/mod.rs` files are re-export-only manifests, files are topical (`<module>_tests.rs`), there are zero inline test modules, and shared fixtures (`helpers.rs`, `test_stores.rs`) are properly factored.
- **Failure scenario:** Doc/code divergence: an engineer (or agent) following CLAUDE.md will look for `src/<module>/tests/` and not find it, and the mvcc suite is doubly-nested away from the module it tests.
- **Suggested fix:** Migrate incrementally (start with `mvcc_store_tests/` → `src/mvcc_store/tests/`; new tests go per-module), or amend CLAUDE.md to bless the crate-root layout — one of the two should move so the documented standard and the code agree.

### 4. Stale / self-contradictory doc comments (rename & refactor drift)
- **Severity:** low
- **Issue:** Several comments describe state that no longer exists:
  - `src/lib.rs:8-31` — the Status block says both "**Stage 2 (in progress).**" and "Stage 2 is now **complete**" in the same doc, and the landed-primitives list omits 10+ shipped modules (changefeed, completion_tracker, metrics, pending_commit, predicate_set, version_guard, version_provider, versioned_overlay, cell_reservation_guard, mvcc gc/drain/locks).
  - `src/repo_tx_gate.rs:33` — "same lifetime discipline as `MvccStore::version_cache` (mvcc_store.rs:446)": the field was renamed to `cells`/`RecordCell` (task #532), the file is now `mvcc_store/mod.rs` — both the identifier and the file:line reference are stale. Same vintage: `:643`, `:888` still say `version_cache`, and `mvcc_gc.rs:531` still names the method `prune_version_cache`.
  - `src/mvcc_store/version_entry.rs:27` — doc cites `[`MvccStore::record_ts`]`, removed per `mvcc_store/mod.rs:492` ("L2: `record_ts` and `record_ts_at` REMOVED"); broken intra-doc link.
  - `src/version_codec.rs:29-30` — "verified by [`decode_version_key`]'s round-trip property tests **below**": no tests live below; they are in `src/tests/version_codec_tests.rs` after the test-organisation move.
  - `src/pending_commit.rs:10-14` — doc describes the group-commit leader that "drains these from `RepoTxGate::pending_commits` … under a single WAL fsync"; that leader/follower path was removed in F-54 (#865) per CLAUDE.md's F-9 dead-scaffolding note — the doc describes dead machinery.
- **Failure scenario:** Misleads readers/agents about current structure (wrong field names, dead paths, wrong file pointers); broken rustdoc links ship silently since `doctest = false`.
- **Suggested fix:** Sweep the `version_cache` vocabulary to `cells`/`RecordCell` (or rename the method to `prune_cells`), refresh the lib.rs status block, and repoint the two stale file/method references.

### 5. One-file-one-export stretched in `changefeed.rs` and `repo_tx_gate.rs`
- **Severity:** low
- **Issue:** CLAUDE.md: "One file = one primary export… one struct, enum, trait, or closely-coupled group."
  - `src/changefeed.rs` (659 lines) hosts two distinguishable families: the event wire types (`RecordChange` :58, `ChangeOp` :73, `ChangelogEvent` :87 + `project_event`/`nontx_event`/`version_key`) and the runtime (`ChangelogStore` :152, `RepoChangefeed` :166, `JournalRead` :199 + the background writer loop). A natural split is `changefeed/event.rs` + runtime in `changefeed.rs`.
  - `src/repo_tx_gate.rs` (~1,100 lines) hosts the gate family (`RepoTxGate` :62, `SnapshotGuard` :209, `OpeningBarrier` :269) **and** the Phase-C conflict family (`TableWriteFootprint` :35, `CommitWriteRecord` :49, `record_conflicts` :1004, `build_footprint_from_tx` :1044). Coupled via `commit_write_log`, so defensible — but it is the largest non-mod.rs file and the two families change for different reasons.
- **Suggested fix:** Direction, not defect: split when either file is next substantively edited; do not churn them just for this.

### 6. `metrics.rs` has no test coverage in this crate
- **File:** `crates/shamir-tx/src/metrics.rs` (`TxMetrics` :7, `TxMetricsSnapshot` :92)
- **Severity:** nit
- **Issue:** `src/tests/` has no `metrics_tests.rs` and no test file references `TxMetrics`/`TxMetricsSnapshot`. The snapshot/diff arithmetic is pure and trivially testable. (`pending_commit.rs` is likewise unreferenced by tests, but that follows from finding 4 — it is documented-dead scaffolding.)
- **Suggested fix:** A small `metrics_tests.rs` covering `snapshot()`/delta math; fold into finding 4's dead-scaffolding decision for `PendingCommit`.
