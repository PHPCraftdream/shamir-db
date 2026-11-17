# Brief: R1b step 3a — format-migration proofs (release-1 R1b, implementation)

Design: `docs/dev-artifacts/design/migration-framework.md` — read sections 4 (journal, crash table 4.3, resume rule 4.4),
5.5, 6 (test-only modules) and 9 (tests and acceptance) closely, plus the "Review" section. Steps 1 and 2 landed: the
framework is `crates/server/shamir-sync/src/format_stamp/migration/`, its tests are
`crates/server/shamir-sync/src/format_stamp/tests/migration/` (journal, registry, resume, runner) with helpers in
`format_stamp/tests/support/`; the command is `crates/server/shamir-server/src/migrate/` with tests in `migrate/tests/`.
Read the existing tests first: many design-9 cases are already covered. This step adds only what is missing.

Step 3b (the reference data directory, its generator and `reference_compat.rs`) is a separate task — not in this step.

## Why

Release blocker R1b acceptance: "crash at every journal point resumes or refuses safely; repeat completes; a plain start
during a migration refuses; plan mode changes nothing; wrong chain refused" — each proved by a test that runs on Linux,
macOS and Windows.

## What

1. **Gap map first.** List every test named in design section 9 and say, for each, which existing test already proves it
   (file and name) or that it is missing. Put this map in the final report.
2. **Test-only failing module** (design 6): `failing_step` writes a marker file and returns `Err`. Lives with the existing
   test-only empty step in `format_stamp/tests/support/`; never reachable from the production registry.
3. **Missing runner/journal tests** in `format_stamp/tests/migration/`, at least:
   * `journal_crash_at_every_point` — parameterised over every row of the crash table 4.3: build the journal and directory
     state up to that point on a real temp directory, then resume through the runner and assert that row's resulting state
     (complete, retry the step, or refuse), with the stamp and `DirSnapshot` checked;
   * `repeat_completes_the_migration` — the failing step crashes the run; a rerun with it replaced by the empty step
     completes and lands the stamp at the test policy's current epoch (4.4c);
   * `plain_start_during_migration_refuses` — partial journal + old stamp: `check_data_dir(Writer)` returns `OlderEpoch`,
     data unchanged except the journal and the step's own artefact;
   * `backup_taken_mid_migration_is_safe` — a snapshot captured mid-run, restored: plain start refuses, resume completes
     (4.4d);
   * `journal_append_is_atomic_and_fsynced_per_record` if not already proved;
   * `apply_twice_is_noop` for the failing module's successful half where it applies (design 2.2 helper).
4. **Missing command tests** in `shamir-server/src/migrate/tests/`: `format_migrate_resolves_rows_9_12_5b_and_leaves_8_11_18_20_refused`
   (design 5.5 against the stamp decision table of `docs/dev-artifacts/design/format-stamp.md`), and any other design-9
   server case the gap map shows missing.
5. **Windows**: a held maintenance lock is byte-range locked on Windows (reading `.shamir.lock` while it is held fails
   with OS error 33). Never read the lock file's bytes while a lock is held; take backups before acquiring a lock.

## Rules

* Tests only, plus test-only support code. No production change; if a test exposes a production defect, stop and describe
  it in the report instead of fixing it here.
* No new dependency. No engine crate touched. Existing tests are never weakened, renamed away or deleted.
* At most 7 entries per folder (count `format_stamp/tests/migration/`, `format_stamp/tests/support/` and
  `migrate/tests/` first; split into topical subfolders if a folder would pass 7) and 1000 lines per file; `mod.rs`
  re-exports only; no local `use`; no inline test modules; no new `#[allow]`.

## Acceptance (observed in CI)

fmt, `cargo clippy -p shamir-sync -p shamir-server --all-targets -- -D warnings` and layout clean locally; the new tests
green on Linux, macOS and Windows in CI. The final report gives the gap map, every file created and changed, and every
design-9 case with the test that now proves it.
