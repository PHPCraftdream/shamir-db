# 245 — engine-validator-engine-bound (engine crate split #1548, stage S0b: segregate the storage-bound part of `validator`, engine only)

Context (@oh ruling 2026-10-29). Plan #1548 later extracts `shamir-engine/src/validator` into its own crate (S2, not this stage). The earlier plan said "move ~19 test files and gate ~15 cfg(test) hooks". Re-verified at origin/master f9dc5788:
- 19 non-manifest test files under `src/validator/**/tests/` (181 tests) and 15 test-hook cfg attributes in validator production code: 4 `#[cfg(test)]` in `registry/validator_db.rs` (`STAGED_RECHECKS`, `reset_staged_rechecks`, `staged_rechecks`, the increment site); 10 `#[cfg(test)]` in `registry/validator_registry.rs` (mutation-hook type/field/init/setter/`run_hook` + call sites); 1 `#[cfg(any(test, feature = "test-util"))]` in `schema/rules/field_rule.rs` (`pub fn check_for_test`, already feature-gated, sole consumer `crates/shamir-db/tests/schema_ddl/basics/declarative_schema_e2e.rs`).
- Storage-bound PRODUCTION files (cannot leave the engine): `registry/validator_db.rs` (uses `crate::table::TableManager`, `table::record_cow::RecordCow`, `crate::query::TableResolver` = batch) and `registry/persistence.rs` (uses `crate::meta::{MetaEnvelope, MetaKey}` = repo).
- Storage-bound TESTS (need engine fixtures or the two files above): `tests/execution/run_validators_tests.rs` (9 tests), `tests/execution/validator_actor_tests.rs` (2), `tests/registry/staged_probe_tests.rs` (1, the only consumer of the 4 `validator_db` hooks), `tests/registry/validator_db_tests.rs` (9), `tests/codecs/persistence_tests.rs` (3), and 3 tests at the end of `schema/tests/schema_rules/schema_validator_tests.rs` (lines ~355–465: `computed_default_fn_call_stamps_absent_field`, `computed_default_does_not_overwrite_present_value`, `computed_default_does_not_overwrite_explicit_null`, which call `crate::table::write_helpers::apply_transforms` via function-local `use`s). The other 14 test files are pure.
- Consequence: if the storage-bound code and its tests stay together on the engine side, NO hook needs feature-gating — the `validator_db` hooks travel with their only consumer, the registry hook stays with `registry_atomic_tests`, `check_for_test` is already gated (S2 only has to forward engine `test-util` to the new crate). So this stage moves ~6 files, not 19, and gates nothing. Do not touch any `#[cfg(test)]` item.
- Not in scope (S2): `execution/record_validator.rs` `ValidatorCtx` holds `Option<&ValidatorDb>` concretely (async `exists_in` / `exists_in_self`); the dyn-safe seam for that is S2's design.

Read first: `src/validator/mod.rs`, `registry/mod.rs`, the 7 files above, the three test manifests (`tests/codecs/mod.rs`, `tests/execution/mod.rs`, `tests/registry/mod.rs`), `CLAUDE.md` (layout, test organisation, imports).

## Changes (all under `crates/shamir-engine/src/validator/`)
1. New folder `engine_bound/` (validator: 6 → 7 entries):
   - `engine_bound/mod.rs`: `//!` one-liner ("validator pieces bound to engine storage; stay in shamir-engine at the crate split") + `pub mod persistence; pub mod validator_db;` + `#[cfg(test)] mod tests;`.
   - `engine_bound/validator_db.rs` ← `registry/validator_db.rs`, `engine_bound/persistence.rs` ← `registry/persistence.rs` (plain filesystem moves, content byte-identical — both use only absolute `crate::` paths; verify no `super::`).
2. `registry/mod.rs`: remove `pub mod persistence;` and `pub mod validator_db;`.
3. `validator/mod.rs`: add `mod engine_bound;`; `pub use registry::{persisted_validators, persistence, validator_binding, validator_db};` → `pub use engine_bound::{persistence, validator_db};` + `pub use registry::{persisted_validators, validator_binding};`; `pub use registry::validator_db::ValidatorDb;` → `pub use engine_bound::validator_db::ValidatorDb;`. Public paths `crate::validator::{persistence, validator_db, ValidatorDb}` (and `shamir_db`'s re-exports of them) stay identical.
4. New `engine_bound/tests/` (7 entries): `mod.rs` manifest (`pub mod …;` only, alphabetical) + moved byte-identical `persistence_tests.rs`, `run_validators_tests.rs`, `staged_probe_tests.rs`, `validator_actor_tests.rs`, `validator_db_tests.rs`; the only allowed edit inside them: `staged_probe_tests.rs` `use crate::validator::registry::validator_db::{…}` → `use crate::validator::validator_db::{…}`. Remove their `pub mod` lines from the old manifests.
5. New `engine_bound/tests/computed_default_transform_tests.rs`: the 3 tests cut from `schema_validator_tests.rs` with bodies byte-identical; their function-local `use` lines hoisted (deduplicated) to the file top — this removes 3 pre-existing local-`use` sites, nothing else. `schema_validator_tests.rs` keeps the other 19 tests untouched; drop now-unused imports there only if clippy says so.
6. Intra-doc links: grep the edited/moved files for links naming `registry::validator_db` / `registry::persistence`; at f9dc5788 there are none outside `validator/mod.rs` — keep it that way (CI `rustdoc` gate, broken private links = error, cannot run locally).

## Tests (CI runs them — never locally)
No new tests. Moved identities (report the 27 names): `validator::tests::<topic>::<file>::<name>` and `validator::schema::tests::schema_rules::schema_validator_tests::<name>` → `validator::engine_bound::tests::<file>::<name>`. Acceptance by reading:
- `grep -rE '^\s*#\[(tokio::)?test' crates/shamir-engine/src/validator | wc -l` → `181` (unchanged); under `engine_bound/tests` → `27`.
- outside `validator/engine_bound/`: `git ls-files crates/shamir-engine/src/validator | grep -v engine_bound | xargs grep -nE 'crate::(table|repo|meta|tx|db_instance)\b|crate::query::(batch|TableResolver)'` → no code hits (comments only). `execution/record_validator.rs` still imports `crate::validator::validator_db::ValidatorDb` (via the facade) — expected, that link is S2's seam.
- the diff leaves `registry/validator_registry.rs` and `schema/rules/field_rule.rs` untouched; the 4 `#[cfg(test)]` hook sites appear unchanged in `engine_bound/validator_db.rs` (move shown as a pure rename).
- folder entries: validator 7, engine_bound 4, engine_bound/tests 7, registry 4.

## Rules
- Touch only `crates/shamir-engine/src/validator/**`. No campaign/delta, no `.rust-guard/**`, no Cargo.toml (decided in E_PLAN: test-discovery currently ignores deltas, so a campaign would fail on the 27 moved test names).
- No visibility change, no `#[allow]`, no hook gating, no `pub(in crate::validator)` widening (all moved code stays under `crate::validator`, so restricted visibilities keep working — if one breaks, STOP and report).
- Layout: ≤ 7 entries/folder, ≤ 1000 lines/file, `mod.rs`/`tests/mod.rs` manifests only, imports at file top, one file = one primary export. Comments laconic.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm` / `mv`, or any git command that mutates the working tree or index. Only edit files (moves are plain filesystem moves); the orchestrator commits.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-engine -- --check`
- `cargo clippy -p shamir-engine --all-targets -- -D warnings`
- `cargo check -p shamir-db --all-targets` (public re-export paths `validator::{persistence, validator_db}` are consumed there)
- `python -I scripts/ci/check_layout.py`
Report every NEW/MOVED file path, the counts above and the real exit codes.
