# Brief: independent review of the logical export and import design (release-1 R3)

## Why

`docs/dev-artifacts/design/logical-dump.md` was written by one executor from
`docs/dev-artifacts/prompts/release/16-r3-logical-dump-design.md`. The project cycle is design, independent review, brief,
implementation. You are the independent review: do not trust the author's claims, check them against the code, the release
plan (`docs/dev-artifacts/release-1/README.md`, `03-logical-dump-restore.md` all sections, `01-versioning-and-migrations.md`
3.8) and the related designs (`format-stamp.md`, `migration-framework.md` section 8, `online-backup.md`).

## What

1. **Verify every OBSERVED claim.** Open each cited `path:line`; mark claims that are wrong, stale or unsupported. Pay special
   attention to the inventory of state kinds (every index kind, every function kind with sources and compiled modules,
   validators, users, groups, rights, tokens, TTL, the interner, the system store, the replication state) and to the read
   paths claimed to give one consistent point across repositories and the system store.
2. **Attack the format and the protocol.** Writes during the export, a repository created, dropped or renamed during the
   export, a schema change during the export, a crash of the exporter, a truncated or corrupted file, an import interrupted
   halfway and retried, an import into a non-empty target, an export from an older binary imported by a newer one, very
   large records and tables (bounded memory), secrets (what is exported for users and tokens, and what never is). For each:
   is the result a complete equal copy or a stated refusal? Write the missing case down.
3. **Check the round-trip property**: what "equal" means, whether rebuilt indexes and recompiled modules are equal by the
   stated rule, and whether the test proposed can prove it.
4. **Check the boundaries** with R1c (application-schema version in the header), R2c (encryption extension point) and R2a
   (no conflicting manifest or layout decisions).
5. **Check the tests**: deterministic in CI, sizes and times on the runners, the three OS where it matters.
6. **Check the implementation plan** against the layout rules (7 entries per folder, 1000 lines per file; count folders from
   listings, never from another document), the crate and layer of each change (nothing in the `repo`, `table`, `tx` or `query`
   layers of `shamir-engine` without listing it; the engine is being cut into crates), and the PR-sized steps; check the open
   questions and their defaults against `03-logical-dump-restore.md` section 6.

## Output

Edit `docs/dev-artifacts/design/logical-dump.md` in place: fix every defect you can fix with certainty, and append a final
section "Review" (English, numbered findings: severity, what was wrong, evidence as `path:line`, what you changed or what
stays open for the owner). Findings you could not settle go to the open questions with a recommended default. Do not change
sound decisions or the structure; do not touch any other file. If the design is sound, say so and list what you checked.

## Do not

Change any code, test or CI file; redesign the feature; edit other design documents (list conflicts as findings).
