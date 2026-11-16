# Brief: design of the backup without stopping the server (release-1 R2a, design only)

## Why

Today `backup` copies a data directory with the server stopped (`crates/server/shamir-sync/src/sync/durability/backup.rs`,
`restore.rs`). A database that must be stopped to be backed up cannot be trusted with data for long. Direction R2a of the
release plan (`docs/dev-artifacts/release-1/README.md`, wave 2; detail in `02-online-backup-and-encryption.md`, sections 1, 2
and 6) is a release blocker: the gate requires a backup taken without stopping the server, restored onto a clean machine,
and proved by a test under write load. R1a landed the format stamp (`docs/dev-artifacts/design/format-stamp.md`, 3.5 and 5.2
say what `backup` and `restore` already do with it); the backup manifest must record the format epoch. This task produces the
DESIGN only.

## What

Write `docs/dev-artifacts/design/online-backup.md` (English, the style of `format-stamp.md`: numbered sections, a file and
line reference for every claim about the current code, OBSERVED and PROPOSED kept apart). It must:

1. **Map the current backup and restore (OBSERVED).** What is copied and what is skipped, the manifest and its hashes, the
   staging and atomic rename of `restore`, the probe, what the stamp observation and the snapshot gate do. Then the storage
   facts an online copy depends on: the storage backends in use (file layout, what each flushes and when), the WAL (segments,
   sidecars, checkpoints, retention), the repository registry and the system store, the data-dir lock, what a writer does
   that a concurrent file copy would see torn (compaction, segment rotation, checkpoint file replacement), and which
   existing quiesce points exist (flush, sync, drain, the clean-shutdown path).
2. **State the requirements as testable properties:** the copy restores to a state that was committed at one single point
   (all repositories and the system store consistent with each other, or the exact weaker guarantee stated and justified),
   no acknowledged write that precedes the start of the copy is lost, writers are not blocked longer than a stated bound,
   a failed or cancelled copy leaves the source untouched and removes its partial output, and the copy is verifiable offline.
3. **Compare the options** of the plan (consistent file copy under a short write barrier, a storage-level snapshot or hard
   links, copy plus WAL replay, replication to a file) against the code facts of section 1, with the cost, the failure modes
   and the OS differences (Windows file locking, hard links on each OS, `fsync` of directories), and recommend one with its
   reasoning. Name explicitly what each option needs from the storage backends and whether any change in `shamir-storage`,
   `shamir-wal` or `shamir-engine` would be required; the engine is being cut into crates now, so prefer the option that
   changes the least inside `repo`, `table` and `tx`.
4. **Specify the design of the recommended option:** the protocol step by step (barrier, capture of the consistency point,
   copy, verification, release), the interaction with checkpoints, WAL retention and compaction while the copy runs, the
   interaction with the data-dir lock and with a second backup, the manifest additions (format epoch from the stamp, schema
   or system-store versions, the WAL position or version of the consistency point, per-file hashes) and the on-disk layout
   of the copy so that the later encryption (R2c) wraps it without a format change (leave the extension point, do not design
   the encryption), the command and the server-side entry point (CLI against a running server, or an admin operation; decide
   with the observed authentication and administration surfaces), progress and cancellation, and the metrics to expose
   (names only; the metrics design is `engine-metrics.md`).
5. **Restore and verification:** what `restore` must do differently for an online copy (replay or none, the consistency
   check, refusal rules with the stamp), restore onto a clean machine, and an offline `verify` of a copy.
6. **Tests and acceptance** (CI only): the property tests under write load (a writer that never stops while the copy runs;
   the copy restored and compared with an oracle of acknowledged writes), failure injection at each protocol step, the
   Windows, Linux and macOS differences, and what is feasible on the CI runners (sizes and time).
7. **Implementation plan:** at most four PR-sized steps with the crate and layer of each change, and the operations guide
   section to add (`docs/guide-docs/guide/07-operations.md`, Backup).
8. **Open questions** that need the owner, each with a default. The point-in-time restore (R2b), the encryption (R2c) and the
   schedule (R2d) are boundaries only.

## Do not

Change any code, test or CI file; design R2b, R2c or R2d beyond the boundary notes; add a dependency (if an option needs one,
write it as an owner decision with the alternative that needs none); change a decision of the stamp design without listing
it as a finding with its reason.

## Observable acceptance

One new file `docs/dev-artifacts/design/online-backup.md`; every statement about the current code carries a path (and a line
where it matters); the options table covers every option of the plan with a verdict; the open questions have defaults.
`python -I scripts/ci/check_layout.py` OK. The final report lists the sections, the decisions taken and the open questions.
