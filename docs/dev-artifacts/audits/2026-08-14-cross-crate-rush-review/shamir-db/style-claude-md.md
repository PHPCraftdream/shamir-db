# shamir-db -- Style & CLAUDE.md structural conformance

## Summary

Structural conformance is strong: every `mod.rs` is a re-export manifest, all four test trees follow the documented `tests/` layout (manifest-only `mod.rs`, topic-split files, `#[cfg(test)] mod tests;` wiring, zero inline test blocks), and comment discipline (task-numbered rationales, fail-closed notes) is exemplary. The main deviation is the "imports at the top" rule: ten function-local `use` statements survive in three production files, one of which re-imports an item already hoisted in the same file. Secondary items are the papered-over `shamir_db::shamir_db` module inception and a handful of one-line hygiene nits.

## Findings

### 1. Function-local `use` statements in production code violate the imports-at-top rule
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/core.rs:759-761`; `crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:109-110, 277, 302, 375, 418`; `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:498`
- **Severity:** medium
- **Issue:** CLAUDE.md ("Imports at the top") mandates all `use` statements live in the file header, with three narrow documented exceptions (test-local `use super::*;`, one-method trait import with a collision comment, cfg-gated bodies). None applies here, and none of the ten sites carries the required collision/justification comment. Sites: `core.rs::get_ddl_op_status` hoists `ddl_op_log`, `RecordId`, and `FromStr` mid-function — where the `RecordId` import (line 760) **duplicates** the file-header import at `core.rs:13`; `schema_management.rs` has local imports in five free helpers (`parse_one_rule`, `parse_type_tag`, `parse_num_constraint`, `parse_fk_action`, `parse_cross_field_compare`); `admin_replication.rs::delete_repl` imports `crate::access::Actor` locally while the sibling header import (line 16) already pulls from `crate::access`.
- **Failure scenario:** the rule exists so a file's dependency surface is greppable from its header; mid-body imports hide dependencies from audit and drift — the redundant `RecordId` re-import in `core.rs` is exactly that drift already happening. A future reader auditing `schema_management.rs`'s schema-validator dependency sees only `FieldRule, SchemaValidator` at the top, not the six schema types actually used.
- **Suggested fix:** hoist all ten imports to the file headers (`use std::str::FromStr;`, `use shamir_engine::table::ddl_op_log;`, the six `shamir_engine::validator::schema::*` items in `schema_management.rs`, `Actor` in `admin_replication.rs`) and delete the duplicate `RecordId` line in `core.rs`. Keep them in one style sweep commit per the CLAUDE.md "style-only sweeps live in their own commits" rule.

### 2. `shamir_db::shamir_db` module inception suppressed with an unannotated allow
- **File:line:** `crates/shamir-db/src/shamir_db/mod.rs:7-8` (`#[allow(clippy::module_inception)] pub mod shamir_db;`)
- **Severity:** medium
- **Issue:** the crate ships the path `shamir_db::shamir_db::ShamirDb` (crate → module → nested module of the same name). `lib.rs`'s doc records that the old `db/` wrapper was "lifted" precisely to remove a redundant level, but the inner nesting was left in place and the resulting clippy lint is suppressed rather than the structure flattened. Unlike the project's other allow sites (e.g. `scc::*::len()` allowances carry an `// O(N) ack: <why>` comment per CLAUDE.md pillar 3), this allow carries no comment naming why the inception is kept, so nothing distinguishes "temporary" from "permanent".
- **Failure scenario:** every new file added to the facade must be placed at the correct one of two identically-named levels (`shamir_db/ports.rs` vs `shamir_db/shamir_db/core.rs`); the ambiguity already produced the split convention where sibling files live at both depths, and callers face the doubly-qualified `shamir_db::shamir_db::*` paths in docs and stack traces.
- **Suggested fix:** flatten `src/shamir_db/shamir_db/*` into `src/shamir_db/*` (rename `core.rs` to e.g. `shamir_db.rs` or keep `core.rs`), preserving the existing re-export surface in `src/shamir_db/mod.rs`; both `mod.rs` files are already re-export-only, so the move is mechanical. If the nested level is intentionally load-bearing (e.g. matching an upstream layout), add the inline justification comment to the allow.

### 3. `SYSTEM_DB_NAME` constant declared inside `mod.rs` (re-exports only)
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/mod.rs:15`
- **Severity:** low
- **Issue:** CLAUDE.md: "mod.rs files contain re-exports only. Types and logic live in sibling files." This `mod.rs` additionally declares `pub(super) const SYSTEM_DB_NAME: &str = "__system__";` — a definition, not a re-export. It is the only definition in any of the crate's six `mod.rs` files.
- **Failure scenario:** minor — breaks the "open any mod.rs, see only the surface" convention; a future editor looking for the constant's owner finds it in a manifest.
- **Suggested fix:** move the constant into `core.rs` (its only sibling consumer besides `db_management.rs`, both reachable via `super::SYSTEM_DB_NAME` unchanged) or its own `system_db_name.rs` sibling if it grows companions.

### 4. Blanket `#![allow(deprecated)]` with no named reason
- **File:line:** `crates/shamir-db/src/main.rs:1`; `crates/shamir-db/src/api/types.rs:1`; `crates/shamir-db/src/api/tests/api_tests.rs:1`
- **Severity:** low
- **Issue:** three files open with a file-wide `#![allow(deprecated)]` and no comment identifying which deprecated API requires it or until when. The project's comment discipline elsewhere is to name the why inline (the lint-allow convention used throughout the workspace). In `main.rs` it silences deprecation warnings for the whole binary; in `api/types.rs` for the whole module.
- **Failure scenario:** a genuinely new deprecation (e.g. in `shamir-types` value/codec APIs these files use) will also be silenced, so the crate stops learning that its demo binary / shim needs migration.
- **Suggested fix:** replace the file-wide allow with item-scoped `#[allow(deprecated)] /* reason: <which API, tracked by which task> */` on the specific call sites, or add a one-line comment naming the deprecated item and removal condition.

### 5. Stray empty statement and dead comment in `curl_gateway.rs`
- **File:line:** `crates/shamir-db/src/shamir_db/curl_gateway.rs:132-133`
- **Severity:** nit
- **Issue:** `// Cleanup happens when tmp_dir drops at the end of this scope.` is followed by a bare `;` — the leftover of a removed statement. The same comment is repeated (accurately, without the stray semicolon) at line 172.
- **Failure scenario:** none behavioral; it reads as a statement was intended there.
- **Suggested fix:** delete line 133 (and the now-redundant first copy of the comment at 132, keeping the one at 172).

### 6. `tests/mod.rs` manifests use private `mod` instead of the documented `pub mod` form
- **File:line:** `crates/shamir-db/src/api/tests/mod.rs:1`; `crates/shamir-db/src/shamir_db/tests/mod.rs:1-26`; `crates/shamir-db/src/shamir_db/shamir_db/tests/mod.rs:1-3`; `crates/shamir-db/src/shamir_db/execute/tests/mod.rs:1`
- **Severity:** nit
- **Issue:** CLAUDE.md's test-organisation section shows the manifest form as `pub mod value_tests;`. All four manifests use `mod <name>_tests;` (private). Content-wise they are manifest-only and conform; only the visibility spelling differs. (Mixed topic-file naming also varies — `*_tests.rs` vs e.g. `replication_ddl_tests.rs` — which is consistent with the rule's spirit.)
- **Failure scenario:** none; purely a fidelity gap to the documented snippet, so greps/templates copied from CLAUDE.md won't match.
- **Suggested fix:** either switch the manifests to `pub mod` or update the CLAUDE.md snippet to the private form actually used; pick one and keep them aligned.

### 7. Inconsistent qualified vs imported spelling of `new_map`/`QueryValue` throughout the facade
- **File:line:** e.g. `crates/shamir-db/src/shamir_db/system_store.rs:9` vs `148, 229, 320, 452, 487, 586, ...`; `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:2` vs `314, 359, 372`; `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:4` vs `48-57`; `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:12` vs `206-243, 423-436`; `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs` vs `110-136, 376-380, 578-589`
- **Severity:** nit
- **Issue:** these files import `new_map` / `QueryValue` at the top yet also spell them fully-qualified inline (`shamir_types::types::common::new_map()`, `shamir_types::types::value::QueryValue::Str(...)`) in the same functions — sometimes both forms within ten lines. Not a CLAUDE.md violation (no mid-body `use`), but the mixed style is the same header-vs-body ambiguity the imports rule exists to prevent, and it is the dominant source of visual noise in the crate's largest files.
- **Failure scenario:** none functional; reviewers cannot tell whether the qualified form signals a deliberately different `new_map` (it does not).
- **Suggested fix:** normalize to the top-level imports within touched files (can ride along with the finding-1 sweep).

### 8. `schema_management` alone breaks the sibling export convention
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/mod.rs:8`; consumers at `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:42-44`, `admin_table_index.rs:6`, `admin_describe.rs:7`
- **Severity:** nit
- **Issue:** every sibling module in `shamir_db/shamir_db/` is a private mod re-exported at the parent (`pub use artifact_kind::{...}; pub use core::{...};`), but `schema_management` is `pub(crate) mod` and its items are consumed through the deep path `crate::shamir_db::shamir_db::schema_management::{SCHEMA_FIELD, SCHEMA_VALIDATOR_ID_FIELD, SCHEMA_VERSION_FIELD, parse_schema}` in three execute files.
- **Failure scenario:** none; two surface conventions coexist, so a reader looking for where `SCHEMA_FIELD` is publicly rooted must check both.
- **Suggested fix:** add `pub use schema_management::{SCHEMA_FIELD, SCHEMA_VALIDATOR_ID_FIELD, SCHEMA_VERSION_FIELD};` (or `pub(crate) use ...`) to the parent `mod.rs`, revert the module to private `mod`, and update the three import sites.

### 9. `ports.rs` carries four public exports in one file (borderline cohesion)
- **File:line:** `crates/shamir-db/src/shamir_db/ports.rs:32, 39, 64, 99`
- **Severity:** nit
- **Issue:** "one file = one primary export" would split `PortError` (type alias), `PrincipalInfo`, `PrincipalResolver`, and `UserAdminPort`. They are re-exported as one group (`shamir_db/mod.rs:12`) and the module doc explicitly frames them as a single identity seam with a shared dependency-direction rationale, so this reads as the sanctioned "closely-coupled group" case — flagging only because the group is four items, not the rule's trait-plus-impl example. (`api/types.rs`'s `Command`/`Request`/`Response` trio is the same shape and clearly fine.)
- **Failure scenario:** none today; if a third port trait is added the cohesion argument weakens.
- **Suggested fix:** no action needed; if the seam grows, split read-side (`PrincipalResolver` + `PrincipalInfo`) from write-side (`UserAdminPort` + `PortError`).

## Positive conformance notes (for the record)

- **mod.rs re-export-only:** all six `mod.rs` files are declaration/re-export manifests (sole exception: finding 3's one constant). `execute/mod.rs` doubles as an accurate module map.
- **tests/ layout:** zero inline `#[cfg(test)] mod tests { ... }` blocks anywhere in the crate (grep-verified); four `tests/` trees match the documented pattern (manifest-only `mod.rs`, `#[cfg(test)] mod tests;` wired from the parent, topic-split files such as `access_meta_tests.rs` / `replication_ddl_tests.rs` / `keyset_safe_write_barrier_tests.rs`); `tests/ddl_wire_e2e/` is a proper multi-file integration harness with its own `main.rs` and shared `helpers.rs`. Coverage claims hold up: ~30 unit-test files (100+ `#[test]`/`#[tokio::test]` units counted before truncation) plus ~35 integration files.
- **Imports:** aside from finding 1, every production file hoists imports; the two blanket `#![allow(deprecated)]` sites are the only file-wide attributes without rationale.
- **Doctests / benches:** `Cargo.toml` sets `doctest = false` with the rationale comment; all five `benches/*.rs` use `bench_scale_tool::Harness` (`bench`/`bench_async`/`bench_batched_async`) with `harness = false` bench targets — no Criterion APIs, no raw `serde_json::json!` query assembly.
- **Comment discipline:** the crate is a model for the project's rationale-comment culture (task-numbered guards in `access_control.rs`/`admin_*`, drop-order derivations in `admin_schema.rs`, wire-reachability SAFETY notes on `#[doc(hidden)]` wrappers in `db_management.rs`/`table_management.rs`).
- **Cargo.toml:** feature cascade and bench-allocator switches are documented inline; `shamir-funclib` appears in both `[dependencies]` and `[dev-dependencies]` (line 43 vs 106) — redundant but harmless and commented, so not raised as a finding.
