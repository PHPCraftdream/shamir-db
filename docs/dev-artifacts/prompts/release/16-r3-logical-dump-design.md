# Brief: design of the logical export and import (release-1 R3, design only)

## Why

A physical backup (R2a) restores only onto a binary that reads the same format; a logical export is the fallback path when
an in-place format migration is impossible (`migration-framework.md` section 8), the way to move data between machines and
versions, and a release blocker (`docs/dev-artifacts/release-1/README.md`; detail in `03-logical-dump-restore.md`, all
sections, and `01-versioning-and-migrations.md` 3.8). R1a landed the format stamp; R1b and R2a are designed
(`docs/dev-artifacts/design/migration-framework.md`, `online-backup.md`). This task produces the DESIGN only.

## What

Write `docs/dev-artifacts/design/logical-dump.md` (English, the style of `format-stamp.md`: numbered sections, a file and
line reference for every claim about the current code, OBSERVED and PROPOSED kept apart). It must:

1. **Map what exists (OBSERVED):** every kind of state a database holds and where it lives (repositories, tables and their
   schemas, every index kind including vector and full-text, records, the interner, functions of every kind with their
   sources and compiled modules, validators, users, groups, rights, tokens, TTL settings, the system store, the replication
   state), the read paths that can enumerate each consistently (snapshot reads, MVCC versions, the query builder), the
   existing seed exporter and importer (`sync/replication_seed/`) and what it already serialises, and the admin and
   authentication surfaces an export would run through.
2. **State the requirements as testable properties** from `03-logical-dump-restore.md` section 3: a consistent point
   (which snapshot, across repositories and the system store), round trip equality (export, import into an empty
   directory, export again: byte- or content-equal by a stated rule), independence from the storage format (readable by a
   newer binary), streaming with bounded memory, verifiability offline, secrets policy (what is exported for users and
   tokens, and what is never exported).
3. **Specify the format:** container and encoding (self-describing, versioned header with the format epoch and the
   application-schema version once R1c exists, per-section records, checksums), ordering so that import can stream
   (schemas before records, indexes rebuilt or loaded), and the extension point for the backup encryption of R2c (do not
   design the encryption).
4. **Specify export and import:** the command line and/or admin operation (decide against the observed surfaces), the
   consistent read point, progress and cancellation, import into an empty directory only or into an existing database
   (decide, with the refusal rules), conflict and partial-failure behaviour, idempotent retry, what is rebuilt on import
   (indexes, compiled modules) versus loaded, and how a newer binary imports an older export.
5. **Tests and acceptance** (CI only): round-trip property tests over every state kind, an export from the reference data
   directory of the R1b compatibility test, corruption and truncation of the file, sizes and times on the runners, the three
   OS.
6. **Implementation plan:** at most four PR-sized steps with the crate and layer of each change (nothing added to the
   `repo`, `table`, `tx` or `query` layers of `shamir-engine` without listing it; the engine is being cut into crates), and
   the operations guide section to add (`docs/guide-docs/guide/07-operations.md`).
7. **Open questions** that need the owner, each with a default (answer the questions of `03-logical-dump-restore.md`
   section 6).

## Do not

Change any code, test or CI file; design R1c, R2c or R2b beyond boundary notes; add a dependency (if an option needs one,
write it as an owner decision with the alternative that needs none).

## Observable acceptance

One new file `docs/dev-artifacts/design/logical-dump.md`; every statement about the current code carries a path (and a
line where it matters); the open questions have defaults. `python -I scripts/ci/check_layout.py` OK. The final report lists
the sections, the decisions taken and the open questions.
