# shamir-query-builder — Style & CLAUDE.md structural conformance

## Summary

The crate is largely conformant with CLAUDE.md's structural rules: every module wires tests
through a `tests/` directory whose `mod.rs` is a manifest only, tests are split by topic,
there is not a single inline `#[cfg(test)] mod tests { ... }` block anywhere in `src/`,
implementation files keep `use` statements in the header, JSON/`mpack!` literals in tests are
multi-line and indented, and `src/` contains no raw `json!`/`serde_json` (the
"Query construction — builder only" rule is trivially satisfied — this crate *is* the
builder). The deviations concentrate in three places: two of the eleven `mod.rs` files
(`wire`, `macros`) carry definitions instead of being re-export-only; the `ddl` module
applies "one file = one primary export" inconsistently (op-per-file for ~15 ops vs. family
files bundling 3–9 public builders); and the imports-at-top rule is breached once in
production code plus ~37 times inside test functions.

## Findings

### 1. `ToWire` trait + blanket impl live directly in `wire/mod.rs`
- **File:line:** `src/wire/mod.rs:24-48`
- **Severity:** medium
- **Issue:** CLAUDE.md: "mod.rs files contain re-exports only. Types and logic live in
  sibling files." `wire/mod.rs` instead defines the public `ToWire` trait (two provided
  methods with real logic — the msgpack round-trip in `to_query_value`) plus the blanket
  `impl<T: Serialize + ?Sized> ToWire for T {}`. The module has no sibling implementation
  file at all; it is the only mod.rs in the crate that carries runtime logic.
- **Failure scenario:** none functional. Structural debt: grep/`git blame` for the trait
  points at a manifest file; anyone extending `wire` (the module already has a `tests/`
  dir) has no sibling file to extend and the documented layout stops matching reality.
- **Suggested fix:** move the trait + blanket impl verbatim to `src/wire/to_wire.rs`
  (keeping the module doc on the new file) and reduce `wire/mod.rs` to module docs +
  `mod to_wire;` + `pub use to_wire::*;` + `#[cfg(test)] mod tests;`. Zero public-API
  change.

### 2. All four `macro_rules!` definitions live inline in `macros/mod.rs`
- **File:line:** `src/macros/mod.rs:24-32` (`doc!`), `:46-51` (`vals!`), `:62-69` (`bind!`),
  `:85-189` (`subscribe!`)
- **Severity:** medium
- **Issue:** same "mod.rs = re-exports only" rule; `macros/mod.rs` is ~190 lines of
  definitions and zero re-exports. `subscribe!` alone spans five match arms. Under
  one-file-one-export this would be four sibling files (or at least `subscribe.rs`
  separate from the small ones).
- **Failure scenario:** none functional (`#[macro_export]` macros are crate-root-visible
  regardless of file). Diff-atomicity suffers: a tweak to `subscribe!`'s `deliver:`
  grammar and a tweak to `doc!` land in the same file's blame.
- **Suggested fix:** split into `macros/doc.rs`, `macros/vals.rs`, `macros/bind.rs`,
  `macros/subscribe.rs`; keep `macros/mod.rs` as `mod`/`pub(crate) use` wiring (macros
  need `#[macro_use]`/`pub use` at the mod level to keep current scoping — verify the
  `#[macro_use] pub mod macros;` ordering in `lib.rs:53-54` still compiles identically).
  Mitigating factor acknowledged: declarative macros are the most conventional mod.rs
  tenant, so if the team prefers, document a macro exception in CLAUDE.md instead of
  migrating — but today the rule as written is violated.

### 3. `ddl/` applies one-file-one-export inconsistently — family files bundle many unrelated public builders
- **File:line:** `src/ddl/access_control.rs` (9 public builder types + 9 ctors);
  `src/ddl/schema.rs` (5 builders + the `field()` DSL, 595 lines, 5 distinct wire ops);
  `src/ddl/validator.rs` (5 builders); `src/ddl/auth.rs` (4 builders);
  `src/ddl/replication.rs` (3 builders + 6 free fns); `src/ddl/list.rs` (4 builders + 3
  free fns); `src/ddl/migration.rs` (3 builders + 1 fn); `src/ddl/buffer_config.rs` (3
  builders); `src/ddl/retention.rs` (3 builders); `src/ddl/function.rs` (2 builders + 3
  free fns) — versus ~15 one-op-per-file siblings (`create_db.rs`, `drop_db.rs`,
  `rename_db.rs`, `create_repo.rs`, `create_table.rs`, `drop_table.rs`, `rename_table.rs`,
  `create_index.rs`, `drop_index.rs`, `rename_index.rs`, `describe_table.rs`,
  `create_index_build_error.rs`, `tokenizer.rs`, `metric.rs`, `quantization.rs`, …)
- **Severity:** medium
- **Issue:** CLAUDE.md: "One file = one primary export … If a file defines multiple
  unrelated public types, split them into separate files. This keeps diffs atomic and
  `git blame` meaningful." The module's own dominant pattern is one op per file, which
  makes the family files the anomaly rather than an alternative convention. The rule's
  "closely-coupled group" carve-out plausibly covers the smallest cases (`list.rs`
  builders all feed the single `ListOp` enum; `buffer_config.rs` is one DTO family), but
  it does not cover `schema.rs` (Set/Add/Remove/Get schema ops + a field-rule DSL are four
  independent op families) or `access_control.rs` (chmod/chown/chgrp/groups are unrelated
  op families).
- **Failure scenario:** `schema.rs` shows repeated phase-marked accretion (Phase B / C2 /
  C3 / ②.2a / ③.2c comments) — exactly the churn pattern the rule exists to prevent;
  blame for `FieldBuilder` and `GetTableSchemaBuilder` interleaves in one file, and
  unrelated-schema-task diffs are not atomic.
- **Suggested fix:** at minimum split `schema.rs` (`field.rs`, `set_table_schema.rs`,
  `add_schema_rule.rs`, `remove_schema_rule.rs`, `get_table_schema.rs`) and
  `access_control.rs` (`chmod.rs`, `chown.rs`, `chgrp.rs`, `group_*.rs`, `access_tree.rs`);
  secondarily `auth.rs` and `validator.rs`. Pure mechanical moves — `ddl/mod.rs` already
  glob-re-exports each sibling (`pub use <file>::*;`), so the public API is unchanged.
  Keep the small single-DTO family files (`list.rs`, `buffer_config.rs`, `retention.rs`)
  as-is, or note the "closely-coupled op family" carve-out in CLAUDE.md so the layout is
  a documented decision rather than drift.

### 4. Imports not at top: one production-code site + a pervasive function-local `use` pattern in tests
- **File:line:** `src/batch/batch.rs:1123` (`use shamir_types::types::value::QueryValue;`
  inside `fn collect_query_refs` — redundant: `QueryValue` is already imported at
  `batch.rs:10`). Test files: `batch/tests/batch_tests.rs:448,460,532,565,576,591,606`;
  `batch/tests/after_tests.rs:24,25,140,193,194`; `batch/tests/when_tests.rs:35,36,59`;
  `batch/tests/call_tests.rs:139`; `batch/tests/sub_batch_tests.rs:142`;
  `macros/tests/q_macro_tests.rs:298,474,576,607,608,620,629,638,654,655,669,670`;
  `select/tests/select_tests.rs:123,157,191`; `write/tests/write_tests.rs:386`;
  `query/tests/query_tests.rs:1036`; `filter/tests/filter_tests.rs:197`;
  `ddl/tests/schema_ddl_tests.rs:551`; `ddl/tests/replication_ddl_tests.rs:268`
- **Severity:** low
- **Issue:** CLAUDE.md: "All `use` statements live in the file header … never inside a
  function or block body," with three narrow exceptions (`use super::*;` in a test mod;
  collision-documented single-method trait imports; macro/cfg-gated bodies). None of these
  sites qualifies: the `batch.rs` site duplicates the file-header import; the test sites
  are per-`#[test]`-function imports (several, e.g. `use crate::wire::ToWire;`, are the
  "trait imported solely to call one method" shape but lack the required naming-collision
  justification comment, and hoisting would collide with nothing).
- **Failure scenario:** none functional. Cost is consistency: the rule as written is
  absolute, so every new test written in the local style deepens the drift, and a future
  mechanical enforcement (or a contributor following CLAUDE.md literally) will flag the
  whole body of tests at once.
- **Suggested fix:** hoist the `batch.rs:1123` import away (delete it — the header import
  already covers it) and hoist the test-file `use`s into each file's header import block
  (they are few and disjoint per file; the crate already has clean header blocks to merge
  into). A dedicated `style:` commit per the CLAUDE.md style-sweep rule.

### 5. `cursor` module's tests use a bare `tests.rs` instead of the documented `tests/` directory + manifest
- **File:line:** `src/cursor.rs:81-82` (`#[cfg(test)] mod tests;`) + `src/cursor/tests.rs`
  (single file)
- **Severity:** low
- **Issue:** CLAUDE.md's test-organisation layout is "one `tests/` directory per module"
  with a manifest-only `tests/mod.rs`. Every other module in the crate follows it
  (`query/tests/mod.rs`, `wire/tests/mod.rs`, `batch/tests/mod.rs`, `ddl/tests/mod.rs`,
  …); `cursor` is the sole module using the degenerate `cursor.rs` + `cursor/tests.rs`
  form. Wiring itself (`#[cfg(test)] mod tests;`) is fine.
- **Failure scenario:** none. Discoverability/consistency cost only: tooling or habits
  tuned to `tests/mod.rs` manifests miss cursor's tests.
- **Suggested fix:** either migrate to `src/cursor/tests/mod.rs`
  (`pub mod cursor_tests;`) + `src/cursor/tests/cursor_tests.rs` (move the file
  verbatim), or — since it is a single-topic file — add a one-line note to CLAUDE.md's
  test-organisation section blessing the single-file degenerate case so the layout is
  intentional.

### 6. Duplicate re-exports: the same items re-exported in both a sibling file and its `mod.rs`
- **File:line:** `src/select/select_item.rs:8` + `src/select/mod.rs:12`
  (`AggFunc`, `AggregateField`); `src/write/update.rs:10` + `src/write/mod.rs:69`
  (`UpdateReturnMode`)
- **Severity:** nit
- **Issue:** the sibling-file `pub use` is already scooped up by the mod.rs glob
  (`pub use select_item::*;`), making the explicit duplicate in `mod.rs` redundant — and
  re-exports per the convention belong in exactly one place (the mod.rs manifest), not in
  implementation files. Contrast the correct pattern in the same crate:
  `val/filter_value.rs:7` (`FnCall`) is re-exported only via the sibling.
- **Failure scenario:** none (both paths resolve to the same item, so no ambiguity
  error). Minor reader confusion about where the re-export is authored.
- **Suggested fix:** delete the sibling-file `pub use` in `select_item.rs:8` and
  `update.rs:10`, keeping the commented re-exports in `select/mod.rs:12` and
  `write/mod.rs:69`. Public API unchanged.

### 7. Doc-comment drift: crate-level module list and one stale macro name
- **File:line:** `src/lib.rs:33-42` (module list omits the `wire` module entirely, and
  describes `macros` as only "`doc!` / `vals!` declarative macros; `filter!` / `q!`
  proc-macro re-exports" while `macros/mod.rs` also defines `bind!` and `subscribe!`);
  `src/write/insert.rs:44` (doc comment references a non-existent `` `mpak!` `` macro —
  the actual macro is `mpack!`, as used correctly in `src/write/mod.rs:28,44` and
  `src/write/upsert.rs:49`)
- **Severity:** nit
- **Issue:** comment-discipline drift: the lib.rs "Modules are wired in here phase by
  phase" inventory predates the `wire` module and the newer macros; the `mpak!` typo is a
  stale reference to the `shamir_types` mpack macro that a reader cannot resolve.
- **Failure scenario:** a newcomer consulting the crate doc misses `wire::ToWire`,
  `bind!`, and `subscribe!`; the `mpak!` reference sends readers searching for a macro
  that does not exist.
- **Suggested fix:** add `wire` to the lib.rs module list and extend the macros line to
  "`doc!` / `vals!` / `bind!` / `subscribe!` declarative macros; `filter!` / `q!`
  proc-macro re-exports"; fix `mpak!` → `mpack!` in `insert.rs:44`. Comments-only change.
