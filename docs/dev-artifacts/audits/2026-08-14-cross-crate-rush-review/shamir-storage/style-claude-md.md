<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-storage — style-claude-md revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The listed layout/import/documentation issues remain. They are low/nit maintenance findings, not runtime medium defects; test-tree wiring is valid.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 7 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Function-local imports violate the mandatory Imports at the top rule

Status: `confirmed-open`. Current risk: `nit`.

All thirteen identified function-local imports remain; module-header imports in nested fixture modules are separate and valid. This is structural policy nonconformance without demonstrated runtime impact.

Evidence: [crates/shamir-storage/src/types.rs:395](../../../../../crates/shamir-storage/src/types.rs#L395); [crates/shamir-storage/src/types.rs:424](../../../../../crates/shamir-storage/src/types.rs#L424); [crates/shamir-storage/src/types.rs:489](../../../../../crates/shamir-storage/src/types.rs#L489); [crates/shamir-storage/src/storage_cached.rs:218](../../../../../crates/shamir-storage/src/storage_cached.rs#L218); [crates/shamir-storage/src/storage_cached.rs:307](../../../../../crates/shamir-storage/src/storage_cached.rs#L307); [crates/shamir-storage/src/storage_fjall.rs:451](../../../../../crates/shamir-storage/src/storage_fjall.rs#L451); [crates/shamir-storage/src/storage_fjall.rs:610](../../../../../crates/shamir-storage/src/storage_fjall.rs#L610); [crates/shamir-storage/src/storage_fjall.rs:673](../../../../../crates/shamir-storage/src/storage_fjall.rs#L673); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:426](../../../../../crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L426); [crates/shamir-storage/src/tests/storage_cached_tests.rs:321](../../../../../crates/shamir-storage/src/tests/storage_cached_tests.rs#L321); [crates/shamir-storage/src/tests/storage_in_memory_tests.rs:264](../../../../../crates/shamir-storage/src/tests/storage_in_memory_tests.rs#L264); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:1289](../../../../../crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L1289); [crates/shamir-storage/src/key_bytes/tests/hash_consistency_tests.rs:52](../../../../../crates/shamir-storage/src/key_bytes/tests/hash_consistency_tests.rs#L52).

<a id="review-2"></a>

### Claim 2 — Stale module doc in key_bytes.rs claims the type is unused and that RecordKey = Bytes

Status: `confirmed-open`. Current risk: `low`.

The header still describes an unflipped alias despite RecordKey = KeyBytes. This is misleading production-type documentation, not itself a runtime medium.

Evidence: [crates/shamir-storage/src/key_bytes.rs:7](../../../../../crates/shamir-storage/src/key_bytes.rs#L7); [crates/shamir-storage/src/types.rs:9](../../../../../crates/shamir-storage/src/types.rs#L9).

<a id="review-3"></a>

### Claim 3 — Orphaned Tests banner comments left behind after tests moved out

Status: `confirmed-open`. Current risk: `nit`.

All three empty ending banners remain, while external test modules are registered.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:260](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L260); [crates/shamir-storage/src/storage_cached.rs:719](../../../../../crates/shamir-storage/src/storage_cached.rs#L719); [crates/shamir-storage/src/storage_fjall.rs:726](../../../../../crates/shamir-storage/src/storage_fjall.rs#L726); [crates/shamir-storage/src/tests/mod.rs:3](../../../../../crates/shamir-storage/src/tests/mod.rs#L3).

<a id="review-4"></a>

### Claim 4 — Duplicate private RecordStream alias re-declared instead of importing the canonical one

Status: `confirmed-open`. Current risk: `nit`.

Both local aliases still duplicate the crate-visible canonical alias. Internal code could already import it; public alias exposure is an optional external naming improvement, not required to implement the trait.

Evidence: [crates/shamir-storage/src/types.rs:11](../../../../../crates/shamir-storage/src/types.rs#L11); [crates/shamir-storage/src/storage_membuffer.rs:628](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L628); [crates/shamir-storage/src/tests/types_tests.rs:12](../../../../../crates/shamir-storage/src/tests/types_tests.rs#L12); [crates/shamir-storage/src/storage_mirrored.rs:33](../../../../../crates/shamir-storage/src/storage_mirrored.rs#L33).

<a id="review-5"></a>

### Claim 5 — Shared batch/conformance suite never runs against CachedStore or MirroredStore

Status: `confirmed-open`. Current risk: `low`.

Only the three original backends invoke the registered helper. Dedicated Mirrored batch tests exist but do not replace all helper assertions.

Evidence: [crates/shamir-storage/src/tests/types_tests.rs:38](../../../../../crates/shamir-storage/src/tests/types_tests.rs#L38); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:168](../../../../../crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L168); [crates/shamir-storage/src/tests/mod.rs:3](../../../../../crates/shamir-storage/src/tests/mod.rs#L3).

Grouping/duplicate: `correctness-tdd.md#5`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — storage_membuffer_tests.rs packs three unrelated fixture topics into nested inline mods instead of topic files

Status: `confirmed-open`. Current risk: `nit`.

The three fixture submodules remain in the same registered test file. This is topic-file granularity debt, not forbidden inline tests inside implementation files.

Evidence: [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:728](../../../../../crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L728); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:867](../../../../../crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L867); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:974](../../../../../crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L974); [crates/shamir-storage/src/tests/mod.rs:5](../../../../../crates/shamir-storage/src/tests/mod.rs#L5).

<a id="review-7"></a>

### Claim 7 — Drifted line-number reference in a comment

Status: `confirmed-open`. Current risk: `nit`.

scan_prefix_stream still references iter_stream at approximately line 323, although that method starts at 596.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:655](../../../../../crates/shamir-storage/src/storage_fjall.rs#L655); [crates/shamir-storage/src/storage_fjall.rs:596](../../../../../crates/shamir-storage/src/storage_fjall.rs#L596).

<a id="review-nf-structure"></a>

### Claim NF-structure — Manifest-only mod.rs files, external test wiring, closely coupled exports, and thiserror

Status: `not-applicable`. Current risk: —.

The manifests contain declarations only; lib/key_bytes wire external test trees, Fjall tests are feature-gated, and implementation files do not embed test bodies. Repo/Store/helper groupings are closely coupled, and DbError uses thiserror.

Evidence: [crates/shamir-storage/src/tests/mod.rs:1](../../../../../crates/shamir-storage/src/tests/mod.rs#L1); [crates/shamir-storage/src/tests/mod.rs:8](../../../../../crates/shamir-storage/src/tests/mod.rs#L8); [crates/shamir-storage/src/key_bytes/tests/mod.rs:5](../../../../../crates/shamir-storage/src/key_bytes/tests/mod.rs#L5); [crates/shamir-storage/src/lib.rs:29](../../../../../crates/shamir-storage/src/lib.rs#L29); [crates/shamir-storage/src/key_bytes.rs:315](../../../../../crates/shamir-storage/src/key_bytes.rs#L315); [crates/shamir-storage/src/error.rs:6](../../../../../crates/shamir-storage/src/error.rs#L6).

## Corrections and qualified non-findings

- Downgrade import placement, ending banners, and alias duplication to nit; stale production-type documentation is low.
- Do not imply private-alias naming prevents external Store implementations.
- Nested modules inside an external test file are registered and reachable; splitting them is organizational, not a coverage fix.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-storage -- Style & CLAUDE.md structural conformance

## Summary

Structurally this crate conforms well to the documented layout rules: `src/tests/mod.rs` and `src/key_bytes/tests/mod.rs` are re-export manifests only, implementation files contain no inline `#[cfg(test)] mod tests` (both test trees are wired via `#[cfg(test)] mod tests;` pointers per the documented pattern), and the one-file-one-primary-export rule holds (each `storage_*.rs` pairs a `Repo` with its `Store` as a closely-coupled group; `WriteMode`/`CacheAction`/`CacheWriteJob` are private helpers of their owning store). The two real clusters of non-conformance are (1) function-local `use` statements, which CLAUDE.md's "Imports at the top" section explicitly bans outside its three exceptions -- 8 instances in production code plus 5 in tests, none qualifying for an exception -- and (2) stale residual comments left behind by refactors: a module doc in `key_bytes.rs` that now states the opposite of what `types.rs` does, three orphaned "Tests" banner comments at file ends whose contents were moved to `src/tests/`, and one drifted line-number reference.

## Findings

Ranked most severe first.

### 1. Function-local imports violate the mandatory "Imports at the top" rule
- **File:line:** types.rs:395, types.rs:424, types.rs:489, storage_cached.rs:218, storage_cached.rs:307, storage_fjall.rs:451, storage_fjall.rs:610, storage_fjall.rs:673; also tests: storage_membuffer_tests.rs:426, storage_cached_tests.rs:321, storage_in_memory_tests.rs:264, storage_mirrored_tests.rs:1289, key_bytes/tests/hash_consistency_tests.rs:52
- **Severity:** medium
- **Issue:** CLAUDE.md ("Imports at the top") requires all `use` statements in the file (or enclosing module) header, allowing only three documented exceptions (`use super::*` inside cfg(test) test modules; collision-justified single-method trait imports; cfg-gated/macro bodies). None of these apply here:
  - `use futures::StreamExt;` inside fn bodies/closures: types.rs (in `default_reverse`, `default_range_filter`, `Repo::copy_store`), storage_cached.rs (in `new_with_mode`, `reload`). No name is in collision scope, and sibling files already hoist exactly this import at top level (types_tests.rs:7, storage_cached_tests.rs:11, storage_mirrored.rs:36), so even the repo's own precedent contradicts the local placement.
  - `use std::ops::Bound;` inside the three `spawn_blocking` closures of storage_fjall.rs (`iter_range_stream_reverse`, `iter_stream`, `scan_prefix_stream`) -- not cfg-gated, hoistable.
  - Test files: `use tokio::task::JoinSet;` mid-`#[tokio::test]` (x2), `use crate::storage_cached::CachedStore;` mid-test at storage_mirrored_tests.rs:1289 (top-level imports there already include other storage modules, so no collision), `use futures::StreamExt;` inside the `collect_stream` helper, `use std::hash::DefaultHasher;` inside a test body where the sibling header import line (`std::hash::{BuildHasher, Hash, Hasher}`) could simply be extended.
- **Failure scenario:** none behavioral; it defeats the rule's purpose (single-glance dependency inventory per file, diff hygiene), and because enforcement is manual, each new stream/range method tends to copy the nearest local `use` rather than the header.
- **Suggested fix:** hoist all thirteen imports to their file headers (module headers for the nested test mods). Mechanical, zero-risk diff.

### 2. Stale module doc in key_bytes.rs claims the type is unused and that `RecordKey = Bytes`
- **File:line:** src/key_bytes.rs:4-9 (module doc)
- **Severity:** medium
- **Issue:** The doc says step 1 landed "with zero call-site changes anywhere else", that `types.rs`'s "`pub type RecordKey = Bytes;` alias is left untouched", and that KeyBytes is "currently unused by production code". Since then the alias flip happened: types.rs:9 reads `pub type RecordKey = KeyBytes;`, making `KeyBytes` *the* production record key across every backend.
- **Failure scenario:** A maintainer reading only the module doc would believe `RecordKey` is still `bytes::Bytes` and that inline-vs-heap behavior is dormant/unexercised in production -- e.g. reasoning wrongly about allocation costs, or "deferring" work that has actually shipped, or trusting serialization properties of `Bytes` that no longer apply on these paths.
- **Suggested fix:** update the doc's framing to describe state-after-step-2 (alias flipped; representation-invariance guarantees now load-bearing everywhere `RecordKey` flows), keeping the history references.

### 3. Orphaned "// ===== Tests =====" banner comments left behind after tests moved out
- **File:line:** storage_in_memory.rs:260-262, storage_cached.rs:719-721, storage_fjall.rs:726-728
- **Severity:** low
- **Issue:** All three files end with the empty banner block marking where inline tests used to live. The tests now follow the documented layout in `src/tests/*.rs`; these residues imply inline tests should follow and invite re-adding them there (the exact anti-pattern CLAUDE.md's test-organisation section bans).
- **Failure scenario:** a contributor appends a new test under the banner, recreating an inline test block in an impl file.
- **Suggested fix:** delete the three dangling banners.

### 4. Duplicate private `RecordStream` alias re-declared instead of importing the canonical one
- **File:line:** storage_membuffer.rs:628 (vs. canonical `pub(crate) use` target at types.rs:11-12); same duplication in tests/types_tests.rs:12-13
- **Severity:** low
- **Issue:** `type RecordStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;` is re-declared locally in `storage_membuffer.rs` although `crate::types::RecordStream` is `pub(crate)` and already imported cross-module by storage_mirrored.rs:33. Two copies can drift independently (e.g. if the item type ever grows a third field or the error type narrows).
- **Failure scenario:** a future signature change to the canonical alias silently leaves MemBuffer's copy inconsistent, surfacing as a compile break or -- worse if both happen to still unify -- unnoticed divergence.
- **Suggested fix:** `use crate::types::RecordStream;` in storage_membuffer.rs (and in tests/types_tests.rs); delete the local aliases.

### 5. Shared batch/conformance suite never runs against CachedStore or MirroredStore
- **File:line:** tests/types_tests.rs:38 (`run_batch_store_tests` helper); consumed only by storage_in_memory_tests.rs:77, storage_membuffer_tests.rs:33, storage_fjall_tests.rs:125
- **Severity:** low
- **Issue:** The backend-agnostic suite covering `insert_many`/`set_many`/`remove_many`/`get_many`/`flush`/`iter_range_stream_reverse` semantics is exercised over InMemoryStore, FjallStore, and MemBuffer-wrapped stacks, but not over `CachedStore` or `MirroredStore` -- precisely the two wrappers whose correctness depends on faithfully preserving/inheriting default batch semantics through delegation (`supports_atomic_transact` is tested separately in storage_mirrored_tests.rs, but not the flag-free fast paths' end-to-end data effects).
- **Failure scenario:** a regression in wrapper batch plumbing (e.g. cache-populate/dirty-accounting interacting with `set_many`) passes CI because no shared-suite case runs against those backends.
- **Suggested fix:** add two cheap cases running `run_batch_store_tests` over `CachedStore::new_sync(InMemoryStore)` and `MirroredStore::new(InMemoryStore, is_durable_table_config)` respectively.

### 6. storage_membuffer_tests.rs packs three unrelated fixture topics into nested inline mods instead of topic files
- **File:line:** tests/storage_membuffer_tests.rs:728 (`mod audit_2_3`), :867 (`mod clear_race_535`), :974 (`mod batch_insert_republish_535`)
- **Severity:** nit
- **Issue:** The test-organisation section prescribes splitting by topic into one file per related group within `tests/` (the pattern `key_bytes/tests/` follows properly). Here three self-contained fixture groups (~380 lines including mock `Store` impls) sit as inline submodules of one 1075-line file. Their imports are correctly placed at each submodule's header (the documented exception pattern), so this is purely about file granularity.
- **Failure scenario:** continued accretion; the next audit fixture gets a fourth inline mod instead of a file.
- **Suggested fix:** promote each `mod` to its own `audit_2_3_tests.rs` / `clear_race_tests.rs` / `batch_pause_tests.rs` under `src/tests/`, registered in `tests/mod.rs`.

### 7. Drifted line-number reference in a comment
- **File:line:** storage_fjall.rs:655
- **Severity:** nit
- **Issue:** `scan_prefix_stream`'s doc says the resume pattern matches "iter_stream above (lines ~323)" -- `iter_stream` now sits around line 596; hard-coded line numbers rot on every edit above them.
- **Failure scenario:** reader chases a stale pointer, wastes time, distrusts nearby comments.
- **Suggested fix:** drop the parenthetical line reference; name the method only.

## Verified-conformant notes (no findings)

- `mod.rs` manifests (`src/tests/mod.rs`, `src/key_bytes/tests/mod.rs`) are pure re-export lists, feature-gating `storage_fjall_tests` correctly.
- No inline `#[cfg(test)] mod tests { ... }` in any implementation file; `key_bytes.rs:315-316` and `lib.rs:32-33` wire external test trees via the prescribed pointer form; `membuffer_clear_race_hook` keeps its logic in a sibling file, root-declared and `cfg(test)`-gated.
- `lib.rs` contains declarations + docs only (crate roots have no `mod.rs` requirement).
- One-file-one-primary-export holds: Repo+Store pairings are closely-coupled groups; private helper enums (`CacheWriteJob`, `CacheAction`, `WriteJob`) serve exactly their owning store.
- Error enum uses `thiserror`; `DbError` + `code()` co-location is appropriate.

</details>
