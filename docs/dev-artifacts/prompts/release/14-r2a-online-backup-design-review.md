# Brief: independent review of the online-backup design (release-1 R2a)

## Why

`docs/dev-artifacts/design/online-backup.md` was written by one executor from
`docs/dev-artifacts/prompts/release/12-r2a-online-backup-design.md`. The project cycle is design, independent review, brief,
implementation. You are the independent review: do not trust the author's claims, check them against the code, the landed
stamp wiring (`docs/dev-artifacts/design/format-stamp.md`, sections 3.5 and 5.2) and the release plan
(`docs/dev-artifacts/release-1/README.md`, `02-online-backup-and-encryption.md`).

## What

1. **Verify every OBSERVED claim.** Open each cited `path:line`; mark claims that are wrong, stale or unsupported. Pay special
   attention to the storage facts the whole design stands on: what the storage backends flush and when, what the WAL writes and
   rotates, what the retention gate protects, what the drainer guarantees, what a concurrent file copy can see torn. A wrong
   fact here invalidates the protocol.
2. **Attack the protocol.** Crash of the server during the copy, crash of the copier, a checkpoint or a segment rotation or a
   compaction during the copy, a second backup, a cancelled backup, a repository created or dropped or renamed during the copy,
   a table DDL during the copy, the replication state and the follower journal, in-memory repositories, a disk full on the
   destination, a destination inside the data directory, files that change size while hashed, Windows file locking and the
   absence of hard links on some file systems, a restore of an online copy on a different OS or a newer binary. For each: does
   the restored state equal a state committed at one point (or the exact weaker guarantee the design states)? Is any
   acknowledged write before the start of the copy lost? Write the missing case down.
3. **Check the replay claim** (the load-bearing one): the design relies on idempotent replay through the drainer. Re-derive from
   the code whether replaying WAL entries over files copied at a different moment is idempotent for every kind of change
   (records, indexes, DDL, interner, function and permission records, TTL, the system store). Name every kind for which the
   design's argument does not hold.
4. **Check the options comparison** is honest: each rejected option is rejected for a reason that holds in the code, and the
   changes required inside `shamir-storage`, `shamir-wal` and `shamir-engine` are stated completely (the engine is being cut
   into crates; nothing may be added to its `repo`, `table`, `tx` or `query` layers without being listed).
5. **Check the manifest, the layout and the extension point** for the later encryption (R2c) and the point-in-time restore
   (R2b) so that neither needs a format change; check the entry point (admin operation) against the authentication and
   authorisation surfaces; check the metrics names against `docs/dev-artifacts/design/engine-metrics.md`.
6. **Check the tests**: can the property test under write load be made deterministic enough for CI, what runs on all three OS,
   the sizes and times on the runners.
7. **Check the implementation plan** against the layout rules (7 entries per folder, 1000 lines per file), the crate and layer of
   each change, and the PR-sized steps; check the open questions and their defaults against the release plan section 6.

## Output

Edit `docs/dev-artifacts/design/online-backup.md` in place: fix every defect you can fix with certainty, and append a final
section "Review" (English, numbered findings: severity, what was wrong, evidence as `path:line`, what you changed or what stays
open for the owner). Findings you could not settle go to the open questions with a recommended default. Do not change sound
decisions or the structure; do not touch any other file. If the design is sound, say so in the Review section and list what
you checked.

## Do not

Change any code, test or CI file; redesign the backup; edit other design documents (list conflicts as findings).
