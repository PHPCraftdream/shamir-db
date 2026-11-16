# Brief: R1b step 2 — the `format-migrate` command (release-1 R1b, implementation)

Design: `docs/dev-artifacts/design/migration-framework.md` — read it in full, especially section 5 (the command), section 9
(tests), section 10 step 2 and the "Review" section. Step 1 landed: the framework is
`crates/server/shamir-sync/src/format_stamp/migration/` (registry, chain builder, journal, resume rule, runner with
`plan_migration` / `run_migration`); read its code and tests before you start. Stamp design:
`docs/dev-artifacts/design/format-stamp.md` (decision table S-3.4). Operations guide:
`docs/guide-docs/guide/07-operations.md` (the format-epoch subsection).

## Why

Release blocker R1b: the refusal messages of the stamp already point the operator at "the format-migration command"; this
step makes it exist. It wires the library of step 1 into `shamir-server` as an offline subcommand for a stopped server.

## What

1. **Command** `shamir-server format-migrate` with exactly the arguments of design 5.1 (`--plan`, `--from-backup <dir>`,
   `--max-backup-age-hours <N>` default 24, `--i-have-a-backup`, `--yes`). One new `Subcmd` arm in `main.rs`; the
   implementation lives in a new folder `crates/server/shamir-server/src/migrate/` (design 2.1: `mod.rs` re-exports only,
   `cli.rs`, `plan.rs`, `backup_gate.rs`, `tests/`). Count `shamir-server/src/` from a listing first: it must stay at 7
   entries or fewer.
2. **Plan mode** (5.2): prints the stamp state and epoch, the resolved chain with each module's summary, touched surfaces and
   work estimate, the journal state and resume cursor, the backup-gate verdict, and what stays refused. It takes no lock and
   writes nothing (no file, no stamp, no journal, no mtime change). Without `--yes` the command behaves as plan mode.
3. **Preconditions in the order of 5.3**, each failing closed with exit code 1 and a message on stderr in the error style
   of the other offline arms: the stamp check (only `OlderEpoch` and `AbsentOlderBase` proceed; an `adopted` stamp needs
   `--yes` and is logged loudly; stamp equal to current is "nothing to do", exit 0), the maintenance lock
   (`maintenance_lock::acquire`; failure means "stop the server first"), the backup gate (5.4), the chain build, then the
   runner of step 1. Exit codes: 0 success or nothing to do, 1 refusal or failure, 2 usage error.
4. **Backup gate** (5.4): `--from-backup <dir>` must hold a manifest that passes `verify_manifest`, a copy of `format.stamp`
   whose epoch and `created_by` equal the live stamp's, a top-level name set that covers the live directory's, and a
   `created_at` within the freshness window. An empty-directory backup is rejected. `--i-have-a-backup` skips the gate with
   an explicit warning. The comparison is against the pre-migration stamp, not against the current epoch.
5. **Integrity hook**: pass the runner an integrity check that reuses the existing doctor check-only path (design 4.5,
   open question Q2 default); if it cannot be reused without a server, use the lightest existing offline check and state the
   choice in the report.
6. **Output**: one line per step on stdout (`[k/n] <module> <summary> … ok (<duration>)`) plus a final summary; the plan
   output is stable text.
7. **Tests** (CI only) under `migrate/tests/`: argument parsing; plan mode leaves a `DirSnapshot` unchanged and takes no
   lock; each refusal of 5.3 with its exit code (newer epoch, corrupt, torn tmp, lock held, gate missing, gate of another
   directory, gate too old, adopted stamp without `--yes`); nothing-to-do on a current stamp; a full run with the test
   policy `EpochPolicy::new(2, 1)` and the empty test module from step 1 advances the stamp and leaves a finished journal;
   `--i-have-a-backup` warns and proceeds.
8. **Guide**: extend the format-epoch subsection of `docs/guide-docs/guide/07-operations.md` with the upgrade procedure
   (stop, `backup --to`, `format-migrate --from-backup … --yes`, start), what each refusal means, resume after a crash, plan
   mode, the rollback rule (restore the pre-migration backup; in-place rollback is not supported) and the logical-export
   fallback (R3).

## Rules

* Nothing in `shamir-engine` or any engine crate. No new dependency. The test-only epoch policy reaches the command through
  a plain constructor or parameter, never a `cfg` hook in production code.
* Say "format migration" wherever "migration" could be confused with `MigrateRepo` / `StartMigration`.
* At most 7 entries per folder and 1000 lines per file; one primary export per file; `mod.rs` re-exports only; no local `use`.

## Acceptance (observed in CI)

fmt, `cargo clippy -p shamir-server -p shamir-sync --all-targets -- -D warnings` and layout clean locally; the
`shamir-server` tests green on the three OS. The final report lists every file created and changed, every design point
implemented and every deviation with its reason.
