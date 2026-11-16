# Brief: design of the format-migration framework, the execution journal and the compatibility test (release-1 R1b, design only)

## Why

R1a landed the format stamp: a data directory carries `format.stamp`, and a binary refuses newer data and refuses older data
without an explicit command (`docs/dev-artifacts/design/format-stamp.md`, code in `crates/server/shamir-sync/src/format_stamp/`).
The refusal messages already name "the format-migration command (R1b)", which does not exist. Direction R1b of the release
plan (`docs/dev-artifacts/release-1/README.md`, wave 2; detail in `01-versioning-and-migrations.md`, sections 3.4, 3.5, 3.7,
3.8, 7 and 8) is the second blocker of the release: without it the first format change after the release cannot be shipped.
This task produces the DESIGN only.

## What

Write `docs/dev-artifacts/design/migration-framework.md` (English, the style of `format-stamp.md`: numbered sections, a file
and line reference for every claim about the current code, OBSERVED and PROPOSED kept apart). It must:

1. **Map the starting point (OBSERVED).** What R1a exposes (public API of the stamp module, the change log, the policy type,
   the refusal rows 9, 12, 18 and their message texts); which subcommands open a data directory and where a new subcommand
   fits (`shamir-server`'s command line and boot code); the existing `backup`/`restore` and the data-dir and maintenance
   locks; what a migration may touch (system store, repository directories, WAL sidecars, the other format surfaces listed in
   section 4 of the stamp design) and who owns each.
2. **Specify the framework.** One module moves data exactly one epoch step (`N` to `N+1`); a chain is a sequence of steps
   without gaps. Define: the module description (name, what changes, work estimate, steps), idempotence rules a step must
   obey and how the framework helps to check them, the registry of modules and where it lives (crate, folder; the 7-entry and
   1000-line limits apply; nothing is added to the `repo`, `table`, `tx` or `query` layers of `shamir-engine`, which is being
   cut into crates), how a module reaches the data without opening the engine when the engine cannot open the old format.
3. **Specify the execution journal.** Name and location, record kinds ("step started", "step finished", the chain, the
   epochs, the binary version), encoding, atomic append and `fsync` per OS, behaviour after a crash at each point (between
   the records, inside a step, between the last step and the stamp), and the exact resume rule. State precisely when the
   stamp is written (after the last step and the integrity check, never before) and what the stamp holds while a migration is
   in flight (the old epoch, so that a plain start keeps refusing).
4. **Specify the command.** Name, arguments, the plan mode that changes nothing and prints the steps and what they touch,
   the requirement of a fresh backup before the first step (use the existing `backup` until the online backup exists; define
   how the command verifies "fresh" and what the operator can override, with the default safe), locking, exit codes, output,
   what happens to a running server (the first version only supports a stopped server), and the interaction with the decision
   table of the stamp design (which rows the command resolves, which it leaves refused).
5. **Define the first, empty migration** and how the chain is proved in CI before any real format change exists: a test-only
   policy with a higher current epoch (the stamp module already offers a constructor for tests), an empty step and a step
   that fails halfway on purpose.
6. **Specify the compatibility test** (plan 3.7): what a reference data directory contains (tables with schemas, every index
   kind, every function kind, users, groups, rights, tokens, an unfinished WAL), who creates it and when (a script run at
   release time), where it is stored and how its size stays small (the repository rules on binary files and fixtures), how CI
   opens it with the current binary, runs the chain and compares the content with the expected one, and which operating
   systems run it. Say what the test proves in the first release, when there is only one epoch.
7. **The fallback path** (plan 3.8): when in-place migration is impossible, the logical export and import (R3); state only the
   boundary, do not design R3.
8. **Tests and acceptance** (CI only): named tests with the crate and fixture of each (crash at every journal point, repeat
   completes the migration, a plain start during a migration refuses, plan mode changes nothing, wrong chain refused), and which
   must run on all three OS.
9. **Implementation plan:** the files to create and change, grouped into at most three PR-sized steps, with the crate and
   layer of each change, and the operations guide section to add (`docs/guide-docs/guide/07-operations.md`).
10. **Open questions** that need the owner, each with a default.

## Do not

Change any code, test or CI file; design the application schema migrations (R1c) or the replication handshake (R1d) beyond
boundary notes; change a decision of the stamp design without listing it as a finding with its reason; use the word
"migration" for the format change without the qualifier ("format migration"): `MigrateRepo` / `StartMigration` already name
the repository move between engines.

## Observable acceptance

One new file `docs/dev-artifacts/design/migration-framework.md`; every statement about the current code carries a path (and a
line where it matters); the crash table covers every journal point; the open questions have defaults.
`python -I scripts/ci/check_layout.py` OK. The final report lists the sections, the decisions taken and the open questions.
