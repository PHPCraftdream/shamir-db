# Brief: design of the data-directory format stamp and the startup rules (release-1 R1a, design only)

## Why

The first release must promise that a binary never misreads or corrupts data written by another version: today there is
no stamp on the data directory and a binary simply tries to open whatever it finds. Direction R1a of the release plan
(`docs/dev-artifacts/release-1/README.md`, wave 1; detail in `01-versioning-and-migrations.md`, sections 2, 3.1-3.3,
3.5, 3.6, 7 and 8) is the first blocker of the release: the backup (R2) and logical-export (R3) manifests will record the
format epoch, and the migration framework (R1b) builds on the stamp. This task produces the DESIGN only.

## What

Write `docs/dev-artifacts/design/format-stamp.md` (English, the style of the other documents in that folder: numbered
sections, file and line references for every claim about the current code, OBSERVED vs PROPOSED kept apart). It must:

1. **Map the current data layout (OBSERVED, from the code).** What a server data directory contains and who creates each
   part: the system store, repository directories (disk and in-memory), WAL segments and their sidecars, the data-dir
   lock, seed and backup artefacts, the compile cache, the audit log. Where the open path starts (`shamir-server` launch,
   `ShamirDb::init`, the repository registry) and the order in which those parts are opened. Which of them can live
   outside the data directory (configured paths).
2. **Specify the stamp.** File name and location (one stamp per data directory, or also per repository directory; decide
   for configurations with several repository directories and for in-memory repositories, open question 2 of section 8);
   fields (`format_epoch`, `components`, `created_by`, `last_opened_by`, `last_migration`, checksum) with types and an
   encoding that a future binary can extend without bumping the epoch; the atomic write (temporary file, `fsync`, rename,
   directory `fsync`, and what each OS needs: Windows has no directory `fsync`); how a torn or corrupt stamp is detected and
   what the binary does then.
3. **Specify the startup rules** of section 3.2 as a decision table over (stamp absent / present / corrupt) x (directory
   empty / has data) x (epoch equal / newer / older), with the exact point in the open path where the check runs (before
   anything is written: the lock, a WAL segment, the system store), the error type and message, and the exit behaviour of
   the server. "No stamp, data present" adopts the base epoch: define what counts as "data present" and which pre-release
   builds are covered (default from the README section 6: only the format at the first release; earlier data moves through
   logical export).
4. **Define the base epoch and the change log of the format.** Where the list of format-relevant changes lives
   (next to the future migration modules), what one entry contains, and the rule of section 3.3 (an epoch bump for any
   change after which the previous binary could read wrongly or lose data on write), with the current `call_signature`
   example worked through.
5. **Replication and seeds (R1d boundary).** State only what R1a must already record so that R1d can refuse mixed epochs
   later; do not design the handshake.
6. **Tests and acceptance** (CI only): the cases of section 7 for R1a (newer epoch refused and data untouched, older epoch
   not modified without a command, stampless directory stamped, partially written stamp), each as a named test with the
   crate and the fixture it uses; which of them must run on all three OS (rename and `fsync` semantics differ).
7. **Implementation plan:** the files to create and change, grouped into at most two PR-sized steps, with the layer and
   crate of each change (the stamp check belongs where the data directory is opened, not inside `shamir-engine`'s `tx`,
   `repo` or `table` layers, which are being split into crates right now); the operations guide section to add
   (`docs/guide-docs/guide/07-operations.md`).
8. **Open questions** that need the owner, each with a default.

## Do not

Change any code, test or CI file; touch `crates/engine/shamir-engine/src/{repo,table,tx,query}` beyond reading; design
R1b (migration modules), R1c (application schema migrations) or R1d (replication handshake) beyond the boundary notes
above; use the word "migration" for the format change without the qualifier: `MigrateRepo` / `StartMigration` already
name the repository move between engines.

## Observable acceptance

One new file `docs/dev-artifacts/design/format-stamp.md`; every statement about the current code carries a file path
(and a line where it matters) that the reviewer can open; the decision table covers every combination; the open questions
have defaults. `python -I scripts/ci/check_layout.py` OK. The final report lists the sections, the decisions taken and the
open questions.
