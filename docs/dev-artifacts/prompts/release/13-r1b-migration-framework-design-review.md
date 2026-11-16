# Brief: independent review of the format-migration framework design (release-1 R1b)

## Why

`docs/dev-artifacts/design/migration-framework.md` was written by one executor from
`docs/dev-artifacts/prompts/release/11-r1b-migration-framework-design.md`. The project cycle is design, independent review,
brief, implementation. You are the independent review: do not trust the author's claims, check them against the code, the
landed stamp (`docs/dev-artifacts/design/format-stamp.md`, `crates/server/shamir-sync/src/format_stamp/`) and the release plan
(`docs/dev-artifacts/release-1/README.md`, `01-versioning-and-migrations.md`).

## What

1. **Verify every OBSERVED claim.** Open each cited `path:line`; mark claims that are wrong, stale or unsupported. The stamp
   module and its wiring landed after the stamp design was written: check the design against the CODE as it is now (the
   public API, the policy type, the change log location, the refusal messages, where the check runs in `launch()`, the token
   CLI, `doctor`, `access-tree`, the seed paths, `backup`, `restore`).
2. **Attack the journal and the resume rule.** Crash between every pair of records, inside a step, between the last step and
   the stamp write, between the stamp write and the journal removal, two runs racing, a stale journal from another binary
   version or another chain, a journal that is torn or corrupt, a journal copied with a backup, an interrupted run followed
   by a restore of an older backup, Windows rename and sharing failures, antivirus holding a file, a read-only volume, a disk
   full during an append. For each: is the behaviour safe (never loses data, never reports success for a half migrated
   directory, a plain start keeps refusing)? Write the missing case down.
3. **Check the crash table** is complete and every row has exactly one action, the same in the prose and in the tests of the
   test section.
4. **Check the command design** against the decision table of the stamp design: which rows it resolves, which it leaves
   refused, the exit codes, the backup-freshness gate (can it be satisfied by a backup that does not contain the data? by a
   backup of another directory?), the override and its safety, the interaction with a running server (the data-dir lock, the
   maintenance lock), and the plan mode (does it really change nothing, including mtimes and the journal?).
5. **Check the compatibility test**: what it proves with one epoch, whether the fixture design can rot silently, the size and
   the repository rules on binary fixtures, the three operating systems, whether the generator script is deterministic.
6. **Check the implementation plan** against the layout rules (7 entries per folder, 1000 lines per file), the dependency
   direction (nothing in the `repo`, `table`, `tx` or `query` layers of `shamir-engine`, which is being cut into crates), the
   PR-sized steps, and that the move of the change log (decision D12 of the stamp design) is correct and safe for the code that
   already uses it.
7. **Check the open questions** and their defaults against the release plan section 6; flag any unsafe default.

## Output

Edit `docs/dev-artifacts/design/migration-framework.md` in place: fix every defect you can fix with certainty, and append a
final section "Review" (English, numbered findings: severity, what was wrong, evidence as `path:line`, what you changed or
what stays open for the owner). Findings you could not settle go to the open questions with a recommended default. Do not
change sound decisions or the structure; do not touch any other file. If the design is sound, say so in the Review section
and list what you checked.

## Do not

Change any code, test or CI file; redesign the framework; edit the stamp design (list conflicts as findings).
