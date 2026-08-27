# shamir-query-types -- Style & CLAUDE.md structural conformance

## Summary

The crate's module/test skeleton is largely exemplary — every module has a `tests/` directory whose `mod.rs` is a re-export-only manifest, tests are split by topic, and the bench uses the mandated `bench_scale_tool::Harness`. However, two hard CLAUDE.md rules are breached: types are defined inside `validator/mod.rs` and `call/mod.rs` (violating "mod.rs files contain re-exports only"), and two implementation files embed inline `#[cfg(test)] mod tests { ... }` blocks despite their module's `tests/` directory already existing. The "imports at the top" rule is also violated repeatedly — ten mid-function `use` statements across four implementation files, plus twelve more in six standalone test files — none carrying a documented exception justification.

## Findings

### 1. Types defined inside mod.rs (re-export-only rule breach)
File: `crates/shamir-query-types/src/validator/mod.rs:5-30`, `crates/shamir-query-types/src/call/mod.rs:13-43`
Severity: high
Issue: CLAUDE.md (Discipline rules): "`mod.rs` files contain re-exports only. Types and logic live in sibling files." `validator/mod.rs` defines two public types inline — `WriteOp` (lines 9-16) and `ValidationError` (lines 23-30) — plus their `use` imports (lines 5-6). This also breaches the companion "one file = one primary export" rule: two distinct public types (a validator-trigger enum and an error DTO) in one file. `call/mod.rs` is the entire module in one `mod.rs`, defining `CallOp` (lines 31-43) and its `default_repo` helper (lines 13-15) instead of a `call/call_op.rs` sibling. Every other module in this crate (read/, batch/, wire/, write/, filter/, subscribe/, admin/) follows the sibling-file convention, so these two are outliers.
Failure scenario: contributors copying `validator/mod.rs` as a template propagate the mod.rs-with-types pattern; `git blame` on `WriteOp`/`ValidationError` conflates them with module-wiring churn, defeating the rule's stated goal of atomic diffs and meaningful blame.
Suggested fix: Move `WriteOp` to `validator/write_op.rs` and `ValidationError` to `validator/validation_error.rs` (their tests in `validator/tests/` already split exactly along these lines: `write_op_tests.rs`, `validation_error_tests.rs`); move `CallOp` to `call/call_op.rs`. Both `mod.rs` files become re-export-only (`pub use write_op::WriteOp; pub use validation_error::ValidationError;`). All external `use crate::validator::{...}` / `crate::call::CallOp` paths stay valid.

### 2. Inline `#[cfg(test)] mod tests { ... }` embedded in implementation files
File: `crates/shamir-query-types/src/read/query_record.rs:302-434`, `crates/shamir-query-types/src/write/inserted_record.rs:134-214`
Severity: high
Issue: Test-organisation rule 5: "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." Both modules already have compliant `tests/` directories wired via the parent `mod.rs` — and the coverage has drifted into overlap: `query_record.rs`'s inline block holds the msgpack round-trip tests while `read/tests/query_record_tests.rs` holds the accessor tests for the *same type*; `inserted_record.rs`'s inline `inserted_record_sorted_key_order` / `inserted_record_no_id_serialization` duplicate the sorted-key and no-id cases already pinned by `write/tests/inserted_record_tests.rs` (`set_insert_map_with_id_and_created`, `update_returning_base_only`, `no_id_non_map_value_direct_serialization`). These are the only two inline test modules in the crate (verified by grep).
Failure scenario: A wire-contract change (e.g. `_id` injection order) must be applied in two places for one type; updating only the external file leaves the stale inline assertion or vice-versa, and a future dev looking for "the tests for InsertedRecord" finds only one of the two halves.
Suggested fix: Move the inline blocks into the existing `tests/` directories as new topic files (e.g. `read/tests/query_record_serde_tests.rs`, `write/tests/inserted_record_roundtrip_tests.rs`), deduplicate the overlapping assertions against the existing files, and register them in the respective `tests/mod.rs` manifests.

### 3. Mid-function `use` statements in implementation files (imports-at-top breach)
File: `crates/shamir-query-types/src/hmac.rs:79,185,271,303,412-413,426-427`; `crates/shamir-query-types/src/batch/planner.rs:372,585,619`; `crates/shamir-query-types/src/batch/batch_op.rs:260`; `crates/shamir-query-types/src/table_ref.rs:52`
Severity: medium
Issue: CLAUDE.md ("📦 Imports at the top"): all `use` statements live in the file header, with only three documented exceptions (test-mod `use super::*`, commented trait collisions, cfg-gated bodies). None of these qualifies: `hmac.rs` has six functions opening with a local `use` (`sha2::{Digest, Sha256}`, `crate::admin::ResourceRef`/`PurgeScope`/`GroupRef`, twice `hmac::{Hmac, Mac}` + `sha2::Sha256`) — the whole file is already `#[cfg(feature = "crypto")]`-gated via `lib.rs`, so hoisting pulls nothing into a wrong scope; `planner.rs` repeats `use crate::filter::FilterValue;` inside three fn bodies while the header already imports `crate::filter::Filter` and even spells `crate::filter::FilterValue` fully-qualified elsewhere (line 148) — three styles for one import in one file; `batch_op.rs:260` imports `QueryValue`/`Value` inside `deserialize`; `table_ref.rs:52` imports `serde::de` inside `deserialize`.
Failure scenario: Rule erosion — the CLAUDE.md rule exists specifically because mid-body imports were a repeated violation; each unjustified instance normalizes the next. The planner.rs triple also misleads readers about what the file imports.
Suggested fix: Hoist all ten to the file headers (`hmac.rs`: one `use` block for `hmac::{Hmac, Mac}`, `sha2::{Digest, Sha256}`, `crate::admin::{GroupRef, PurgeScope, ResourceRef}`; `planner.rs`: add `FilterValue` to the existing `use crate::filter::Filter;` line and delete the three local copies plus the fully-qualified spellings).

### 4. Mid-function `use` statements in standalone test files
File: `crates/shamir-query-types/src/batch/tests/planner_tests.rs:456,531,583,1050`; `src/read/tests/query_record_tests.rs:78,102`; `src/filter/tests/filter_value_conv_tests.rs:113,123`; `src/wire/tests/repl_tests.rs:22,32`; `src/read/tests/pagination_after_tests.rs:120`; `src/write/tests/insert_op_tests.rs:25`
Severity: low
Issue: The imports-at-top rule's test exception covers only `use super::*`-style imports *inside an inline `#[cfg(test)] mod tests` block* — these are separate test files, whose imports belong in the file header. Worst case is `planner_tests.rs`: its header (line 8) already imports `crate::filter::{Cond, Filter, FilterValue}`, yet test functions at lines 456 and 531 locally re-import `Filter`/`Cond` — a shadowing re-import that misleads (exactly the "mislead" outcome the exception clause guards against, inverted). The others (`ByteBuf` imported in two tests of the same file, `QueryValue` in two helpers of `repl_tests.rs`, `mpack`/`RecordId`/`new_map`/`TSet` once each) are trivially hoistable with no collision.
Failure scenario: Reader scanning `planner_tests.rs`'s header concludes `Cond`/`Filter` are imported once; the duplicate local imports rot independently if the header import is later narrowed.
Suggested fix: Delete the local `use`s and extend the file-header imports; in `planner_tests.rs` lines 456/531 the imports are already present at the top and can simply be deleted.

### 5. `FieldPath` type alias defined in `filter/mod.rs`
File: `crates/shamir-query-types/src/filter/mod.rs:19-21`
Severity: low
Issue: `pub type FieldPath = Vec<String>;` is a type definition living in a `mod.rs` that otherwise correctly contains only `pub mod`/`pub use` declarations. The re-export-only rule says types live in sibling files; this alias is consumed crate-wide (`crate::filter::FieldPath` in validator, read, filter modules), so it is a real export with a real definition, not wiring.
Suggested fix: Move the alias (with its doc comment) to a sibling file (e.g. `filter/field_path.rs`) and re-export it: `pub use field_path::FieldPath;`. All existing `crate::filter::FieldPath` paths remain valid.

### 6. `is_false` helper defined four times with three visibilities and two referencing conventions
File: `crates/shamir-query-types/src/admin/types/db_ops.rs:6`; `src/admin/types/schema_ops.rs:160-164`; `src/admin/types/repl_ops.rs:36-45`; `src/read/read_query.rs:52-54`
Severity: low
Issue: The identical one-line serde helper exists as: `pub(crate) fn is_false` in `db_ops.rs` (the de-facto shared copy, imported by six sibling files plus `auth/types.rs`), `pub fn is_false` in `schema_ops.rs` (referenced via fully-qualified serde attribute strings at lines 64/144/155), a documented "declared locally to keep this module self-contained" `pub(crate)` copy in `repl_ops.rs`, and a private copy in `read_query.rs`. Three visibilities, two referencing styles, one trivial function. (The `default_repo()` helper repeated privately across eight files is the conventional serde-default-fn pattern and acceptable; `is_false` is not, because three of the four copies are explicitly shared/cross-referenced.)
Failure scenario: A behavioral tweak to one copy (e.g. also skipping `true` for a new sentinel mode) silently diverges the wire shape per module family.
Suggested fix: Keep exactly one `pub(crate) fn is_false` (a neutral home such as the crate root or a small `serde_helpers` sibling), import it everywhere, and delete the other three copies.

### 7. Duplicated `fk_restrict` entry in `DbResponse::Error` doc vocabulary
File: `crates/shamir-query-types/src/wire/db_message.rs:330-332`
Severity: nit
Issue: The doc comment listing the `code` vocabulary for `DbResponse::Error` names `fk_restrict` twice — once on the first Foreign-keys line (line 330) and again on the third (line 332): "`fk_violation`, `fk_restrict`, `fk_cascade_depth`, `fk_requires_index`, `fk_actions`, `fk_on_update`, `fk_restrict`, `fk_update_unsupported_new_value`". Copy-paste drift in a list developers use as the authoritative error-code enumeration.
Suggested fix: Delete the duplicate token (keep one occurrence, presumably the first).

### 8. `hmac.rs` module doc: second half of the canonical-input table is an orphaned headerless block
File: `crates/shamir-query-types/src/hmac.rs:24-68`
Severity: nit
Issue: The "# Per-op canonical input" section opens a proper markdown table (header row + 13 op rows, lines 28-42), then interrupts it with three prose paragraphs explaining `<db_in_use>`/`<resource>`/`<retention>` (lines 44-59), then appends eight MORE pipe-delimited rows (create_group … create_scram_user, lines 61-68) with no header row. Markdown will not re-join these: the second block renders as literal pipe text, not a table — the documented canonical inputs for all group/function/superuser/SCRAM ops are effectively unformatted in rustdoc.
Failure scenario: A dev grepping the rendered docs for the `create_scram_user` canonical form finds broken formatting and may misread the null-byte layout of an HMAC-gated destructive op.
Suggested fix: Either restart the table (repeat the `| Op | Canonical input |` header after the prose) or move the eight late rows into the first table and the explanatory prose below it.

### 9. Inconsistent `//!` module-doc headers
File: `crates/shamir-query-types/src/subscribe/deliver_mode.rs:1`, `event_mask.rs:1`, `source.rs:1`, `subscribe_op.rs:1`, `unsubscribe_op.rs:1`; also `src/tests/hmac_tests.rs:1`, `src/validator/tests/write_op_tests.rs:1`, `src/wire/tests/db_message_tests.rs:1`
Severity: nit
Issue: Nearly every implementation file in the crate opens with a `//!` purpose header (read/, batch/, wire/, admin/, write/, filter/ all do, including one-liners like `fk_action.rs`); the entire `subscribe/` module and several test files start with a bare `use` instead. Within-subscribe mod.rs likewise has no module doc, unlike its peers.
Suggested fix: Add one-line `//!` headers (e.g. `//! [`DeliverMode`] — how matching events are delivered to the subscriber.`) matching the crate's established pattern.

### 10. Inconsistent per-file granularity: `types.rs` multi-type buckets vs. per-family splits
File: `crates/shamir-query-types/src/write/types.rs:17-172`; `crates/shamir-query-types/src/auth/types.rs:14-245`
Severity: nit
Issue: "One file = one primary export … closely-coupled group" is applied unevenly. The same crate that gives `admin/types/` fourteen per-family files (db_ops, table_ops, index_ops, …) and splits `write/` into single-type files (`inserted_record.rs`, `write_result.rs`) lumps eight public DML types plus three select-config types into `write/types.rs` and ten public auth types into `auth/types.rs`. The families are defensible as "closely coupled", but the generic `types.rs` bucket names hide the split points the admin layout makes explicit, and per-op diffs are less atomic than the sibling convention. (Related micro-inconsistency: `#[cfg(test)] mod tests;` sits before the re-exports in wire/batch/filter/write mod.rs but after them in read/subscribe/admin-types mod.rs, and `lib.rs` places its four `pub use` re-exports at the bottom, after `mod tests;`, unlike every mod.rs header.)
Suggested fix: Next time either file is materially touched, split along the family seams already proven in `admin/types/` (e.g. `write/insert_op.rs`, `write/update_op.rs`, `write/select_configs.rs`); no urgent action required.
