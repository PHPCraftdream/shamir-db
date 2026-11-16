# Brief: independent review of the application schema migrations design (release-1 R1c)

## Why

`docs/dev-artifacts/design/app-schema-migrations.md` was written by one executor from
`docs/dev-artifacts/prompts/release/17-r1c-app-schema-migrations-design.md`. The project cycle is design, independent review,
brief, implementation. You are the independent review: do not trust the author's claims, check them against the code, the
release plan (`docs/dev-artifacts/release-1/README.md`, `01-versioning-and-migrations.md` sections 4 to 8) and the related
designs (`format-stamp.md`, `migration-framework.md`, `logical-dump.md` if present).

## What

1. **Verify every OBSERVED claim.** Open each cited `path:line`; mark claims that are wrong, stale or unsupported. Pay special
   attention to the DDL surface and its durability and replication, the transactional guarantees of DDL (inside a batch or
   not), the admin and authorisation surfaces, and the client APIs the tool would use.
2. **Attack the apply protocol.** Two clients applying the same set concurrently, a crash between a DDL and its journal
   record, a crash inside a multi-step migration, a retry after a timeout whose first attempt committed, a changed checksum
   of an applied migration, a follower or read replica, a migration that changes a function or validator together with the
   data it validates, a restore of an older backup under a newer journal, a logical import (R3). For each: is the result
   either fully applied and recorded or refused with a stated recovery? Write the missing case down.
3. **Check the forward-only rule and the recovery path** against the release plan and the backup and export designs.
4. **Check the tool** in both clients: built with the query builder (never hand-written wire JSON), status/plan/apply,
   checksum rules, and whether any new server operation or wire change is really needed (and its authorisation).
5. **Check the tests**: deterministic in CI, the three OS where it matters, every crash point covered.
6. **Check the implementation plan** against the layout rules (7 entries per folder, 1000 lines per file), the crate and
   layer of each change (nothing in the `repo`, `table`, `tx` or `query` layers of `shamir-engine` without listing it; the
   engine is being cut into crates), and the PR-sized steps; check the open questions and their defaults.

## Output

Edit `docs/dev-artifacts/design/app-schema-migrations.md` in place: fix every defect you can fix with certainty, and append a
final section "Review" (English, numbered findings: severity, what was wrong, evidence as `path:line`, what you changed or
what stays open for the owner). Findings you could not settle go to the open questions with a recommended default. Do not
change sound decisions or the structure; do not touch any other file. If the design is sound, say so and list what you
checked. Count folders from listings, never from another document.

## Do not

Change any code, test or CI file; redesign the feature; edit other design documents (list conflicts as findings).
