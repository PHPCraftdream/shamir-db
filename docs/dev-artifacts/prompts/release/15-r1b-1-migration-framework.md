# Brief: R1b step 1 — the format-migration framework, journal and registry (release-1 R1b, implementation)

Design: `docs/dev-artifacts/design/migration-framework.md` (reviewed; read it in full, the "Review" section included). This is
step 1 of its implementation plan (section 10). The stamp module it extends is `crates/server/shamir-sync/src/format_stamp/`
(design `docs/dev-artifacts/design/format-stamp.md`). Release plan: `docs/dev-artifacts/release-1/README.md` and
`01-versioning-and-migrations.md`, sections 3.4, 3.5, 3.7.

## Why

Release blocker R1b: without a format-migration framework the first format change after the release cannot ship. Step 1 adds
the library only — no command, no behaviour change; steps 2 (the `format-migrate` command) and 3 (crash-table tests and the
compatibility test) build on it.

## What

1. **Home** exactly as decision M1 of the design (section 2.1): `crates/server/shamir-sync/src/format_stamp/migration/` with
   `mod.rs` (re-exports only), `module.rs`, `registry.rs`, `error.rs`, `journal/`, `runner.rs`, `change_log.rs`. Move
   `format_stamp/change_log/` (`format_change.rs` + `format_changes.rs`) into `migration/change_log.rs` as one file (git
   rename of the larger file plus the other's content; no content change beyond the merge and paths); update
   `format_stamp/mod.rs` (drop the `change_log` lines, add `pub mod migration;`) and every user of the old path (grep the whole
   `crates/` tree). Count every folder you touch from a listing: `format_stamp/` must end at 7 entries or fewer.
2. **Types and registry** as sections 2.2 to 2.4: `ModuleDescriptor`, `FormatStep`, `StepContext`, `PlanLine`, the const
   `FORMAT_MODULES` registry (empty for the release: one epoch), the chain builder `chain_for(policy, modules)` returning
   `MigrationError::BrokenChain` on a gap, a duplicate or a short chain, and `MigrationError` (thiserror).
3. **Journal** as section 4 (4.1 name, location, records, encoding; 4.2 atomic append and `fsync` per OS; 4.4 the resume rule
   as a pure function over the parsed records; 4.5 the stamp is written only by the runner after the last step and the
   integrity check). Reuse the stamp module's atomic-write and directory-fsync helpers (`format_stamp/io/`), never a copy.
   A corrupt non-final line is a refusal, a torn final line is the only tolerated damage (section 4.3 rows).
4. **Runner** (`runner.rs`): plan mode that writes nothing (no lock, no journal, no stamp, no mtime change), and the execute
   loop: journal "chain started", per step "step started" → `apply` → "step finished", then the integrity check hook, then
   the stamp write, then "chain finished". The command (step 2) will call it; here it is exercised by unit tests only.
5. **Unit tests** under `format_stamp/tests/migration/` (declared in `format_stamp/tests/mod.rs`): registry invariants
   (ids unique, epochs gapless from `BASE_FORMAT_EPOCH` to `CURRENT_FORMAT_EPOCH`, every module id in exactly one
   `FORMAT_CHANGES` bump entry, `CURRENT == BASE + bumps`), chain builder refusals, journal encode/decode round trip, torn
   final line tolerated, corrupt middle line refused, the resume rule for every row of the crash table of section 4.3
   (pure-function tests over record lists), plan mode leaves a `DirSnapshot` unchanged, and `apply_twice_is_noop` in
   `format_stamp/tests/support/` with one empty test module. The crash-injection tests against a real directory and the
   compatibility test are step 3, not this step.
6. **No behaviour change**: nothing calls the runner yet; `launch()`, the token CLI, `doctor`, `backup` and `restore` are
   untouched except for the moved change-log import paths.

## Rules

* Nothing in `shamir-engine`, `shamir-engine-repo` or any engine crate; no new dependency (the design's no-new-dependency
  claim holds: serde, serde_json, thiserror, sha2 are already in `shamir-sync`).
* The word "migration" for this feature is always "format migration" in docs and identifiers where ambiguity is possible
  (`MigrateRepo` / `StartMigration` already name the repository move between engines).
* One file = one primary export; `mod.rs` re-exports only; at most 7 entries per folder and 1000 lines per file.

## Acceptance (observed in CI)

fmt, `cargo clippy -p shamir-sync --all-targets -- -D warnings` (and `-p shamir-server` if any import path there changed) and
layout clean locally; in CI the `shamir-sync` lib tests green on the three OS. The final report lists every file created,
moved and changed, every design point implemented, and any deviation from the design with its reason.
