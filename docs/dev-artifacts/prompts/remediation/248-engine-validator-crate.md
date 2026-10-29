# 248 — engine-validator-crate (engine crate split #1548, stage S2: S0b + extract the portable validator into the new workspace crate `shamir-engine-validator`)

Context (@oh ruling 2026-10-29, E_PLAN §6 "S2" + §10). PR-S2 = S0b (`05c01c88`, b245) + this stage; it merges strictly AFTER PR-S1 (b246+b247). Two parts:
- **Part A — parallel-safe with S1** (worktree on a branch from `ci/engine-validator-engine-bound` = f9dc5788 + S0b): code only, files disjoint from S1 except `crates/shamir-engine/Cargo.toml` and `Cargo.lock`.
- **Part B — after PR-S1 is merged**: the orchestrator rebases the branch onto origin/master (resolving the engine `Cargo.toml` hunk, regenerating `Cargo.lock` with `cargo check`); then the campaign, deltas, CI and doc lines (they reuse C0 and S1's edits of the same shared files, so they cannot be written before).

Re-verified on S0b (05c01c88), excluding `validator/engine_bound/**`:
- **Portable validator** = `crates/shamir-engine/src/validator/{codecs,execution,registry,schema,tests}/**`: 47 `.rs` (27 prod, 20 test), 154 tests; plus `validator/mod.rs` (facade, see Part A 3/7). After S0b, its only non-`crate::validator` path is `use crate::query::TableRef;` (`schema/schema_validator.rs:11`) — a re-export of `shamir_query_types::TableRef`. The `crate::query::write` refs E_PLAN §6 cited all live in `engine_bound/tests` (stay). **So S2 does not depend on S1's code at all.**
- Its only engine link: `execution/record_validator.rs` — `ValidatorCtx.db: Option<&'a ValidatorDb<'a>>` (+ `with_db(…, db: &'a ValidatorDb<'a>)`, `db() -> Option<&ValidatorDb<'_>>`, 2 doc links `[`ValidatorDb`]`); consumers call only `db.exists_in(&TableRef, &str, &QueryValue)` and `db.exists_in_self(&str, &QueryValue, Option<&RecordId>)` (both `async`, `-> DbResult<bool>`): `schema/schema_validator.rs` (FK / unique) and engine_bound tests. Constructed only in `table/table_manager/accessors/table_manager_validators.rs:224,312` (`ValidatorCtx::with_db(actor, interner, scalars, &vdb)`).
- No validator `pub(crate)`/`pub(in …)` item is used outside `validator/` (grep-verified), so no widening is needed except the new seam trait. Engine reaches the validator only through public facade paths (`crate::validator::{ValidatorRegistry, ValidatorBinding, schema::…, record_fields::RecordFields, …}`), which a glob re-export preserves.
- `FieldRule::check_for_test` is `cfg(any(test, feature = "test-util"))`; its sole consumer is `crates/shamir-db/tests/schema_ddl/basics/declarative_schema_e2e.rs` via shamir-db's dev-dep on `shamir-engine/test-util` → forward the feature.
- Dependencies of the portable part (prod): shamir-types, shamir-collections, shamir-query-types, shamir-funclib, shamir-wasm-host (`NativeValidatorAdapter`, `WasmRecordValidator`), async-trait, arc-swap, thiserror, serde, smallvec, tokio, log, regex; + shamir-storage (default-features = false) for `DbResult` in the seam. Tests also: rust_decimal, num-bigint, bincode.
- **Seam decision (dyn-safe, boxed futures).** New trait in the validator crate, implemented by engine's `ValidatorDb`:
  ```rust
  /// Tx-scoped read-only relational probes for declarative FK / unique rules.
  #[async_trait]
  pub trait ValidatorDbProbe: Send + Sync {
      async fn exists_in(&self, table: &TableRef, field: &str, value: &QueryValue) -> DbResult<bool>;
      async fn exists_in_self(&self, field: &str, value: &QueryValue, exclude_rid: Option<&RecordId>) -> DbResult<bool>;
  }
  ```
  `ValidatorCtx.db: Option<&'a dyn ValidatorDbProbe>`. Rejected: a generic `ValidatorCtx<D>` (infects `RecordValidator`, which is stored as `Arc<dyn RecordValidator>`); moving `ValidatorDb` (needs `TxContext`, `TableManager`, `TableResolver`). Cost: one `Box` per FK/unique probe (same `async_trait` mechanism `RecordValidator::validate` already pays), negligible next to the index/storage read the probe performs. `TableResolver: Send + Sync`, and `&ValidatorDb` already crosses `Send` futures today, so `ValidatorDb: Send + Sync` holds.

Read first: `src/validator/mod.rs`, `execution/record_validator.rs`, `registry/mod.rs`, `schema/schema_validator.rs` (FK/unique arms), `engine_bound/{mod.rs,validator_db.rs}`, `table/table_manager/accessors/table_manager_validators.rs` (≈200–320), b245, b247 (crate skeleton, facade, discovery script, orchestrator flow — S2 mirrors them), `CLAUDE.md` (layout, imports).

## Part A changes (parallel with S1)
1. New crate `crates/shamir-engine-validator/` (`Cargo.toml`, `src/lib.rs`, `src/validator/`): `Cargo.toml` like b247's (name `shamir-engine-validator`, `publish = false`, `license = "MIT OR Apache-2.0"`, version `0.1.0-alpha.1`, edition 2021, description "ShamirDB portable validators (schema rules, codecs, registry), re-exported as shamir_engine::validator::*.", every version spec copied verbatim from `crates/shamir-engine/Cargo.toml`, `[features] test-util = []` with a one-line comment, `[lib] doctest = false`, `[lints] workspace = true`); no dependency on `shamir-engine`. `src/lib.rs`: `#![deny(clippy::unwrap_used, clippy::expect_used)]`, `//!` one-liner, `pub mod validator;`.
2. Move (plain filesystem moves) `crates/shamir-engine/src/validator/{codecs,execution,registry,schema,tests}/` → `crates/shamir-engine-validator/src/validator/…` — 47 files, byte-identical except changes 4–6. The new crate's `validator/` = `mod.rs` + those 5 folders (6 entries).
3. New `crates/shamir-engine-validator/src/validator/mod.rs` = engine's current `validator/mod.rs` minus `mod engine_bound;`, `pub use engine_bound::{persistence, validator_db};`, `pub use engine_bound::validator_db::ValidatorDb;`; `validator_db_probe` added to the `pub use registry::{…}` facade line; `pub use registry::validator_db_probe::ValidatorDbProbe;` added; `#[cfg(test)] mod tests;` kept; doc otherwise unchanged.
4. New `registry/validator_db_probe.rs` (registry 4 → 5 entries) with the trait above (imports at top: `async_trait`, `DbResult` from `shamir_storage::error`, `TableRef`/`QueryValue`/`RecordId` from their shamir crates); `registry/mod.rs` + `pub mod validator_db_probe;`.
5. `execution/record_validator.rs`: import `validator_db_probe::ValidatorDbProbe` instead of `validator_db::ValidatorDb`; field `db: Option<&'a dyn ValidatorDbProbe>`; `with_db(…, db: &'a dyn ValidatorDbProbe)`; `db(&self) -> Option<&dyn ValidatorDbProbe>`; the 2 doc links → `[`ValidatorDbProbe`]` (an engine type cannot be linked from this crate). Bodies unchanged.
6. `schema/schema_validator.rs`: `use crate::query::TableRef;` → `use shamir_query_types::TableRef;` — the only edit (an import line; item fingerprints unchanged). `db.exists_in(…)` now resolves to the trait method of the `dyn` object without an import.
7. Engine `src/validator/mod.rs` becomes the facade:
   ```rust
   //! Validator facade (#1548 S2): portable validators live in
   //! `shamir-engine-validator`; storage-bound pieces stay in `engine_bound`.

   mod engine_bound;

   #[doc(no_inline)]
   pub use shamir_engine_validator::validator::*;

   pub use engine_bound::validator_db::ValidatorDb;
   pub use engine_bound::{persistence, validator_db};
   ```
   (the glob reproduces today's public surface exactly; `crate::validator::{…}` paths in engine, engine_bound and shamir-db keep resolving). Engine `validator/` = `mod.rs` + `engine_bound/`.
8. `engine_bound/validator_db.rs`: after the inherent impl, `#[async_trait] impl<'a> ValidatorDbProbe for ValidatorDb<'a>` whose two methods delegate with `ValidatorDb::exists_in(self, table, field, value).await` / `ValidatorDb::exists_in_self(self, field, value, exclude_rid).await` (type-relative paths resolve to the inherent methods; a mistaken self-call would be infinite async recursion, which `validator_db_tests.rs`'s ctx.db() validators would surface as a TIMEOUT in CI — do not rely on that, check by reading). Imports at file top. Nothing else in engine_bound changes; `table_manager_validators.rs` stays unchanged (`&vdb` coerces).
9. Engine `Cargo.toml`: `shamir-engine-validator = { path = "../shamir-engine-validator" }`; `test-util` gains `"shamir-engine-validator/test-util"` (Part A on S0b: `test-util = ["shamir-engine-validator/test-util"]`; after the rebase the orchestrator merges it with S1's `"shamir-engine-query/test-util"`). No engine dev-dependency (engine's own tests use no validator test-only item). `Cargo.lock`: only the new package + the engine dependency line.

## Part B changes (after the rebase onto post-S1 master)
10. `.rust-guard/stages.toml`: remove the `engine-split-query` entry and delete its two files under `.rust-guard/stages/` (inert since PR-S1 landed — C0 lifecycle rule); add
    ```toml
    [[campaigns]]
    name = "engine-split-validator"
    packages = ["shamir-engine", "shamir-engine-validator"]
    delta = ".rust-guard/stages/engine-split-validator.toml"
    discovery_delta = ".rust-guard/stages/engine-split-validator.discovery.toml"
    ```
11. `.rust-guard/stages/engine-split-validator.toml`: header comment, `base = "<git merge-base origin/master HEAD>"`, exactly ONE `[[allow_widen]]` (`path = "crates/shamir-engine-validator/src/validator/registry/validator_db_probe.rs"`, `item = "ValidatorDbProbe"`, `owner = "@engine"`). Conservation entries come from the orchestrator (b247 flow).
12. `.rust-guard/stages/engine-split-validator.discovery.toml`: the b247 script with these inputs — auto pairs: every `.rs` under `crates/shamir-engine-validator/src/validator/` (old binary `shamir-engine`, new `shamir-engine-validator`, same src-relative path); explicit S0b pairs (binary `shamir-engine` on both sides, head file under `crates/shamir-engine/src/`): `validator/tests/execution/run_validators_tests.rs` → `validator/engine_bound/tests/run_validators_tests.rs`, `validator/tests/execution/validator_actor_tests.rs` → `…/engine_bound/tests/validator_actor_tests.rs`, `validator/tests/registry/staged_probe_tests.rs` → `…/staged_probe_tests.rs`, `validator/tests/registry/validator_db_tests.rs` → `…/validator_db_tests.rs`, `validator/tests/codecs/persistence_tests.rs` → `…/persistence_tests.rs`, `validator/schema/tests/schema_rules/schema_validator_tests.rs` → `validator/engine_bound/tests/computed_default_transform_tests.rs` (only the 3 tests now in the head file). Reasons `"S2 #1548: lib test moved with its module into shamir-engine-validator"` / `"S0b #1548: storage-bound validator test moved under validator::engine_bound"`; output file and header renamed accordingly. Expected count: **181** (154 + 27).
13. CI / docs (same places S1 edited): ci.yml `rustdoc` + fast "rustdoc gate": `cargo doc -p shamir-engine-validator --no-deps --document-private-items --locked` and `… --features test-util`; `engine-features`: the two isolated clippy steps for `-p shamir-engine-validator` (cells `no-default` / `test-util`); `features_matrix_check.py` `FORWARDED["shamir-engine-validator"]` (same expectations as the query crate) + fixture lines in its test; `scripts/test.sh` `@engine` adds `-p shamir-engine-validator`; CLAUDE.md and AGENTS.md: "29 crates", `shamir-engine-validator` after `shamir-engine-query`, the `@engine`, `rustdoc`, `engine-features` lines.

## Tests (CI runs them — never locally)
No new tests. Acceptance by reading:
- Renames: 47 `.rs` `crates/shamir-engine/src/validator/{codecs,execution,registry,schema,tests}/…` → `crates/shamir-engine-validator/src/validator/…`, all R100 except `execution/record_validator.rs`, `schema/schema_validator.rs` (one import line), `registry/mod.rs` (one `pub mod` line).
- New files: the crate's `Cargo.toml`, `src/lib.rs`, `src/validator/mod.rs`, `src/validator/registry/validator_db_probe.rs`; the two `.rust-guard/stages/engine-split-validator*.toml`. Deleted: the two `engine-split-query*` delta files.
- `grep -rnE 'crate::[a-z_0-9]+' crates/shamir-engine-validator/src | grep -v 'crate::validator\b'` → no code hit; `grep -rn 'ValidatorDb\b\|shamir_engine\b' crates/shamir-engine-validator/src` → none.
- `grep -rE '^\s*#\[(tokio::)?test' crates/shamir-engine-validator/src | wc -l` → 154; under `crates/shamir-engine/src/validator` → 27.
- Trait method signatures == the inherent `ValidatorDb::exists_in` / `exists_in_self` signatures; the impl delegates via `ValidatorDb::` paths.
- Engine diff outside `validator/`: only `Cargo.toml` (+ Part B CI/doc files); `table_manager_validators.rs` untouched.
- `[[allow_widen]]` count 1; discovery `[[moved]]` count 181; `stages.toml` holds exactly one campaign.
- Folder entries: engine `validator/` 2, new `src/validator/` 6, `registry/` 5.

## Orchestrator
Same flow as b247 "Orchestrator" (draft push → source-integrity suggest-delta → declare residue → draft test-discovery green → `gh pr ready`). Expected conservation residue: the `ValidatorDbProbe` trait (+ its 2 methods) and the 2 impl members in `validator_db.rs` → `[[added]]`; `ValidatorCtx` (field type), `with_db`, `db` → `[[moved]]` naming both sides + `[[changed]]`; from S0b: the 3 computed-default tests (hoisted local `use`s changed their bodies) → `[[moved]]` + `[[changed]]`. S0b's in-place `schema_validator_tests.rs` debt (4 local `use`s) is excused here because the file is a NEW path at head (moved into the new crate) — that is why S0b rides PR-S2, not PR-S1.

## Rules
- Touch only the files named above. No widening beyond `ValidatorDbProbe`, no `#[allow]`, no `#[cfg(test)]` changes, no hook gating; if a restricted validator item turns out to be needed across the boundary: STOP and report.
- Imports at file top; `mod.rs` = declarations/re-exports only; one file = one primary export; ≤ 7 entries/folder; ≤ 1000 lines/file. Comments laconic.
- Compile / build / test policy (CLAUDE.md, verbatim allowed list): banned unless the user explicitly asks in the current conversation — `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t`/`cargo tl`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc/vitest, WASM builds, e2e/integration harnesses, loom, any command whose purpose is to build the project or execute its tests (incl. `python -m unittest`). Allowed locally, scoped: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` / `-- --check`, `python -I scripts/ci/check_layout.py`. If you believe a run is needed, say so in the final message.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm` / `mv`, or any git command that mutates the working tree or index. Only edit files (moves are plain filesystem moves); the orchestrator commits and rebases.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-engine-validator -p shamir-engine -- --check`
- `cargo clippy -p shamir-engine-validator --all-targets -- -D warnings` and `… --lib -- -D warnings`
- `cargo clippy -p shamir-engine --all-targets -- -D warnings` and `… --features test-util -- -D warnings`
- `cargo check -p shamir-db --all-targets` (`shamir_engine::validator::*` paths, `check_for_test` via `test-util`)
- `python -I scripts/ci/check_layout.py`
Report every NEW/MOVED/EDITED/DELETED file, the 3 non-identical moves, the discovery script count (Part B), the Cargo.lock diff summary and the real exit codes.
