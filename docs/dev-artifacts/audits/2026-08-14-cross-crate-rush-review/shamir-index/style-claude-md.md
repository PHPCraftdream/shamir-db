<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-index — style-claude-md independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Concrete style drift is limited to local imports, the ordinary inline test, and stale wording/references. Coupled type groups and provenance-prefixed topical names are allowed. The loom placement is an exception/documentation decision.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 5 | 0 | 0 | 1 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `use` statements inside function/block bodies across six production files

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The cited expression, tokenizer, write-op, manager, and HNSW imports remain local without the specified collision/cfg exceptions. write_ops redundantly imports a name already re-exported. This is header-import conformance, not runtime risk.

Evidence: [crates/shamir-index/src/expr.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/expr.rs#L85); [crates/shamir-index/src/tokenizer.rs:306](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tokenizer.rs#L306); [crates/shamir-index/src/write_ops.rs:166](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/write_ops.rs#L166); [crates/shamir-index/src/base_index/index_manager_unique.rs:578](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager_unique.rs#L578); [crates/shamir-index/src/vector/hnsw_adapter.rs:2327](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/hnsw_adapter.rs#L2327).

<a id="review-2"></a>

### Claim 2 — Inline `#[cfg(test)] mod tests` inside an implementation file

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The cfg(test) mod remains inside implementation code, contrary to the literal layout rule. It is registered via vector::quant_meta; relocation must keep that test selected through the existing tests manifest.

Evidence: [crates/shamir-index/src/vector/quant_meta.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/quant_meta.rs#L83); [crates/shamir-index/src/vector/mod.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/mod.rs#L7); [crates/shamir-index/src/vector/tests/mod.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/tests/mod.rs#L9).

<a id="review-3"></a>

### Claim 3 — `kind.rs` defines eight public types — "one file = one primary export" deviation

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

IndexKind and tokenizer/vector configuration types form a coupled vocabulary. CLAUDE explicitly allows such groups; eight exports alone is not a violation. Public re-export preservation would be required if reorganized.

Evidence: [crates/shamir-index/src/kind.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/kind.rs#L11); [crates/shamir-index/src/kind.rs:190](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/kind.rs#L190); [CLAUDE.md:504](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L504).

<a id="review-4"></a>

### Claim 4 — Stale crate-root invariant: "NO `std::sync::Mutex` / `RwLock` / `parking_lot`"

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The crate-root absolute prohibition contradicts sanctioned DDL guard sets and the BruteForce teardown slot. It should describe scoped hot-path policy rather than universally forbidden primitives or formal lock freedom.

Evidence: [crates/shamir-index/src/lib.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/lib.rs#L11); [crates/shamir-index/src/base_index/index_manager.rs:260](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager.rs#L260); [crates/shamir-index/src/base_index/sorted_index_manager.rs:193](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/sorted_index_manager.rs#L193); [crates/shamir-index/src/vector/brute_force.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/brute_force.rs#L64).

<a id="review-5"></a>

### Claim 5 — Feature-gated inline loom test module in an implementation file

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The cfg(loom) model is inline and build.rs explicitly enables it for this crate's feature. It differs from the ordinary cfg(test) prohibition; documenting or relocating the exception is optional structural cleanup, not runtime repair.

Evidence: [crates/shamir-index/src/reader_drain_gate.rs:306](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/reader_drain_gate.rs#L306); [crates/shamir-index/Cargo.toml:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L98); [crates/shamir-index/build.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/build.rs#L13).

<a id="review-6"></a>

### Claim 6 — Task-ID-prefixed test file names drift from topic-based naming

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The files still describe drop durability, legacy compatibility, generation ordering, and other topics. The rule requires topical splitting, not omission of provenance prefixes. Renaming is optional.

Evidence: [AGENTS.md:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/AGENTS.md#L131); [crates/shamir-index/src/base_index/tests/mod.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/tests/mod.rs#L8); [crates/shamir-index/src/base_index/tests/index_manager_tests/mod.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/tests/index_manager_tests/mod.rs#L10).

<a id="review-7"></a>

### Claim 7 — Comment nits: typo and a stale cross-file doc reference

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

suffient remains misspelled, and index_write_op.rs::Provenance is not a local source path. The type lives in shamir-tx and is re-exported by write_ops. No runtime mechanism follows.

Evidence: [crates/shamir-index/src/reader_drain_gate.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/reader_drain_gate.rs#L113); [crates/shamir-index/src/base_index/index_definition.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_definition.rs#L48); [crates/shamir-index/src/write_ops.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/write_ops.rs#L10).

<a id="review-8"></a>

### Claim 8 — Multi-type bundles in `backend.rs` and `bm25.rs` (borderline one-file-one-export)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Trait query/result/error types and BM25 scoring/statistics types satisfy the permitted coupled-group exception. No mandatory file split or behavioral change is justified.

Evidence: [crates/shamir-index/src/backend.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/backend.rs#L20); [crates/shamir-index/src/backend.rs:69](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/backend.rs#L69); [crates/shamir-index/src/bm25.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/bm25.rs#L9); [CLAUDE.md:504](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L504).

## Evidence and recipe corrections

- Do not label all sanctioned mutexes locally justified: BruteForce's teardown slot is the documented exception.
- Pure re-export/registration manifests can contain module declarations and documentation; their existence does not establish every architectural doc is current.
- Loom's opt-in command comments conflict with the repository's current raw-cargo-test perimeter guidance; any later documentation cleanup should route through the established runner without executing it here.
- Type splitting, prefix renaming, and a style commit are optional decisions, not authorized actions in this review.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-index -- Style & CLAUDE.md structural conformance

## Summary

The crate is broadly conformant to CLAUDE.md's structural rules: every `mod.rs` (crate root, `base_index/`, `vector/`, and all three `tests/` manifests) is a pure re-export manifest, tests live in dedicated per-module `tests/` directories split per topic, sanctioned `std::sync::Mutex` sites all carry the required inline contention-model comments, and several files (`sq8.rs`, `quant_meta.rs`) even cite the "one primary export per file" rule in their own docs. The two genuine deviations are systemic: `use` statements inside function/block bodies at 15+ sites across six production files, and one inline `#[cfg(test)] mod tests` in `vector/quant_meta.rs`. Remaining items are minor — `kind.rs` as an eight-type grab-bag, a stale crate-root "NO `std::sync::Mutex`" invariant claim, a feature-gated inline loom test module, and naming/comment nits.

## Findings

### 1. `use` statements inside function/block bodies across six production files

- **File:line:** `src/expr.rs:85-86`; `src/tokenizer.rs:306,321,464-465`; `src/write_ops.rs:165-166`; `src/base_index/index_manager.rs:1249,1443,1834,2794,2907`; `src/base_index/index_manager_unique.rs:578,656`; `src/vector/hnsw_adapter.rs:754,2327,2524`
- **Severity:** medium
- **Issue:** CLAUDE.md ("📦 Imports at the top") mandates that all `use` statements live in the file header, "never inside a function or block body," with three documented exceptions (test-mod `use super::*;`, a commented trait-name collision, and imports only valid under a specific `cfg`). None of the 15 sites above qualifies: every import (`futures::StreamExt` ×7, `scc::hash_map::Entry::{Occupied, Vacant}` ×3, `std::sync::OnceLock` ×2, `FxHasher`/`Hasher`, `ScalarError`, codec fns, `TFxMap`, `new_map`/`TMap`) hoists cleanly with no collision. `write_ops.rs:166` even re-imports `shamir_tx::IndexWriteOp` while the identical name is already `pub use`-exported at the top of the very same file (`write_ops.rs:9-10`) — a concrete demonstration of the shadowing/confusion the rule guards against. (`vector/simd.rs`'s `std::arch::*` imports are the legitimate cfg-gated exception; its `use std::sync::OnceLock` at lines 27/35/43/540 merely rides along inside cfg-gated fns and would hoist harmlessly. Test files repeat the pattern in test fns, e.g. `src/tests/write_ops_tests.rs:143,230,268`.)
- **Failure scenario:** None at runtime. The drift normalizes the pattern for every new contributor (each new fn copies the local-`use` habit), and the redundant `write_ops.rs:166` import shows the maintainability cost already materializing.
- **Suggested fix:** Hoist all listed imports to their file headers and delete the redundant `write_ops.rs:166` line. Land as a dedicated `style:` commit (per CLAUDE.md's style-sweep rule), `cargo fmt -p shamir-index` scoped to this crate.

### 2. Inline `#[cfg(test)] mod tests` inside an implementation file

- **File:line:** `src/vector/quant_meta.rs:83-111`
- **Severity:** medium
- **Issue:** Verbatim violation of Test-organisation rule 5: "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." `quant_meta.rs` ends with an inline `mod tests` containing `quant_meta_round_trips_sq8_params`. This is the only instance in the crate — `vector/tests/` already exists with a manifest (`vector/tests/mod.rs`) and sibling topic files (`quantization_snapshot_tests.rs`, `sq8_tests.rs`) that could host it. Ironic detail: the module's own doc (line 3) advertises "One primary export: [`QuantMeta`]" — awareness of the layout rules is present, the test just wasn't migrated.
- **Failure scenario:** None at runtime; it undermines an otherwise perfectly uniform layout and is the natural precedent cited by the next inline test module.
- **Suggested fix:** Move the test to `vector/tests/quant_meta_tests.rs` (or fold it into `quantization_snapshot_tests.rs`) and add `pub mod quant_meta_tests;` to `vector/tests/mod.rs`.

### 3. `kind.rs` defines eight public types — "one file = one primary export" deviation

- **File:line:** `src/kind.rs:11-200` (`IndexKind`, `TokenizerKind`, `StemLanguage`, `FunctionalConfig`, `VectorMetric`, `VectorQuantization`, `VectorConfig`, `VectorBackendRef`)
- **Severity:** low
- **Issue:** CLAUDE.md: "Each `.rs` file (except `mod.rs`) owns one struct, enum, trait, or closely-coupled group... If a file defines multiple unrelated public types, split them into separate files." Eight public types is well past a "closely-coupled group," and several members have natural existing homes: `TokenizerKind`/`StemLanguage` belong beside `tokenizer.rs` (which currently imports them back — `tokenizer.rs:14`), and the four vector types belong under `vector/` (which imports `VectorMetric`/`VectorQuantization` back — `hnsw_adapter.rs:14`). Contrast `sq8.rs:50` ("One primary export per file: this struct and its inherent methods") and `quant_meta.rs:3` — the crate demonstrably holds itself to this rule elsewhere.
- **Failure scenario:** None at runtime; diffs touching tokenizer config and vector config collide in one file, weakening the atomic-diff / meaningful-`git blame` goal the rule states.
- **Suggested fix:** Split (e.g. tokenizer kinds into `tokenizer.rs` or a `tokenizer_kind.rs`; the vector config family into `vector/config.rs`), keeping `kind.rs` re-exports for compatibility — a `style:`-scoped commit per CLAUDE.md.

### 4. Stale crate-root invariant: "NO `std::sync::Mutex` / `RwLock` / `parking_lot`"

- **File:line:** `src/lib.rs:8-11`
- **Severity:** low
- **Issue:** The crate-root doc lists as an architectural invariant: "**Lock-free**: ... NO `std::sync::Mutex` / `RwLock` / `parking_lot`," while this same crate contains seven sanctioned `std::sync::Mutex` fields — `base_index/index_manager.rs:260,262,319,367,372` (`dropping_regular`, `dropping_unique`, `dirty_sets`, `renaming_regular`, `renaming_unique`) and `sorted_index_manager.rs:193,224` (`dropping_sorted`, `renaming_sorted`) — all of which CLAUDE.md's F-9/#1076 revision explicitly sanctions as DDL-only guard sets, each with its required inline justification comment (verified present). The invariant is arguably scoped to the index2 subsystem in context ("must hold across all impls" of `IndexBackend`), but as written on the crate root it contradicts both the code beneath it and the workspace's own documented exception policy.
- **Failure scenario:** A reviewer treating the crate doc as authoritative either flags the sanctioned base_index Mutexes as violations (re-litigating closed decisions) or, worse, cites the doc to reject/permit a *new* Mutex by the wrong rule instead of the sanctioned-exception categories.
- **Suggested fix:** Reword to reference the policy, e.g. "lock-free on all read/write hot paths; `std::sync::Mutex` appears only under CLAUDE.md's sanctioned DDL-only/low-frequency exception categories, each justified inline."

### 5. Feature-gated inline loom test module in an implementation file

- **File:line:** `src/reader_drain_gate.rs:306-422` (`#[cfg(loom)] mod loom_model`, `#[test]` at 389)
- **Severity:** low
- **Issue:** Embeds a test module (with a `#[test]` fn) inside an implementation file. The *letter* of Test-org rule 5 targets `#[cfg(test)] mod tests`, and this module is a deliberately opt-in (cargo feature `loom`, compiled away from every normal build) model-checker harness with an unusually honest scope doc — so this is a spirit-of-the-layout deviation, not a bright-line breach. Notably the type's ordinary unit tests correctly live in `src/tests/reader_drain_gate_tests.rs`.
- **Failure scenario:** None at runtime; the risk is precedent — the next cfg-gated test module cites this one to justify staying inline.
- **Suggested fix:** Either add one sentence to the module doc acknowledging the tests/-layout deviation and why the model must sit beside the atomics it models, or relocate to a cfg(loom)-gated sibling under `tests/` if the feature plumbing permits.

### 6. Task-ID-prefixed test file names drift from topic-based naming

- **File:line:** `src/base_index/tests/`: `p03_drop_durability_tests.rs`, `p03b_sorted_drop_durability_tests.rs`, `p05b_sorted_rename_durability_tests.rs`, `p12_ddl_partial_error_tests.rs`, `f72_legacy_state_compat_tests.rs`, `p1068_ddl_op_log_retention_tests.rs`; also `index_manager_tests/{f72_,f78_,p1058_,p1098_,p1102_}*` and `sorted_index_manager_tests/{f71_,p1007_}*`
- **Severity:** nit
- **Issue:** CLAUDE.md's test-organisation rule prescribes topic-named files ("`value_tests.rs`, `record_id_tests.rs`, `config_tests.rs`"); sibling files here follow it (`index_definition_tests.rs`, `write_barrier_flags_tests.rs`, `index_status_tests.rs`). The `pNN`/`fNN` prefixes encode task provenance, not topic, so the directory's topic grouping degrades as tasks accumulate (already 12+ such files).
- **Failure scenario:** None.
- **Suggested fix:** Prefer topic names for new test files; opportunistically fold task-named files into their topic homes when next touched (a `chore:`/`style:` commit, never riding a feature diff).

### 7. Comment nits: typo and a stale cross-file doc reference

- **File:line:** `src/reader_drain_gate.rs:113`; `src/base_index/index_definition.rs:48`
- **Severity:** nit
- **Issue:** (a) `reader_drain_gate.rs:113` — "is not suffient proof" → "sufficient". (b) `index_definition.rs:48` cites "`index_write_op.rs::Provenance`" — no such file exists in this crate; `Provenance` lives in `shamir-tx` and is re-exported by `write_ops.rs:9-10` ("Re-export from shamir-tx where the pure-data enum now lives"). The stale path sends a reader grepping this crate for a file that isn't there.
- **Failure scenario:** None.
- **Suggested fix:** Fix the typo; repoint the doc reference to `crate::write_ops::Provenance` (or `shamir_tx::Provenance`).

### 8. Multi-type bundles in `backend.rs` and `bm25.rs` (borderline one-file-one-export)

- **File:line:** `src/backend.rs:20-66`; `src/bm25.rs:9-93`
- **Severity:** nit
- **Issue:** `backend.rs` defines four public enums (`IndexQuery`, `FtsMode`, `IndexResult`, `IndexError`) alongside the `IndexBackend` trait; `bm25.rs` defines `Bm25Params`, `FtsPostingValue`, and `FtsStats` plus two free fns. Both are defensible as closely-coupled groups (the trait's I/O vocabulary; the BM25 scoring family) and are much smaller outliers than `kind.rs` (finding 3) — flagged only so the split decision is made consciously if finding 3 is acted on.
- **Failure scenario:** None.
- **Suggested fix:** Optional: if `kind.rs` is split, consider giving `IndexQuery`/`IndexResult`/`IndexError` their own sibling files at the same time; otherwise leave as documented coupled groups.

</details>
