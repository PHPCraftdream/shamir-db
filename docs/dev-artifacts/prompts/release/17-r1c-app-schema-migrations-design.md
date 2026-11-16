# Brief: design of application schema migrations and the client tool (release-1 R1c, design only)

## Why

Applications built on the database change their schema (tables, indexes, functions, validators) between their own
releases. Without a recorded, ordered, forward-only migration journal the application side cannot be upgraded safely;
this is a release blocker (`docs/dev-artifacts/release-1/README.md`; detail in `01-versioning-and-migrations.md`, sections
4, 5, 6, 7 and 8). It is separate from the format migration of the data directory (R1b,
`docs/dev-artifacts/design/migration-framework.md`); the stamp design (`format-stamp.md`) and the logical export (R3)
record the application-schema version in their headers. This task produces the DESIGN only.

## What

Write `docs/dev-artifacts/design/app-schema-migrations.md` (English, the style of `format-stamp.md`: numbered sections, a
file and line reference for every claim about the current code, OBSERVED and PROPOSED kept apart). It must:

1. **Map what exists (OBSERVED):** the DDL surface (create, drop, rename of tables and indexes, function and validator
   registration, their wire operations and the query builder calls), how DDL is made durable and replicated, the
   transactional guarantees DDL has today (inside a batch or not), the existing `MigrateRepo` / `StartMigration` (the
   repository move between engines: name the collision and keep the terms apart), and the clients (`shamir-client`,
   `shamir-client-ts`, the node binding) and their admin surfaces.
2. **Specify the model** of `01-versioning-and-migrations.md` 4.1: where the journal of applied migrations lives (per
   database or per repository; decide), what one record holds (id, ordinal, checksum of the migration body, applied at,
   applied by, the binary version), how ordering and gaps are enforced, and forward-only (4.5): no down migrations, the
   recovery path is restore or a new forward migration.
3. **Specify applying a migration** (4.2): atomicity of one migration (one batch, or the stated weaker guarantee for DDL
   that cannot join a batch, with the exact recovery rule), concurrency between two clients applying the same set, the
   lock or compare-and-set that makes it safe, idempotence on retry, behaviour on a follower or a read replica, and the
   interaction with functions and validators (4.4): a migration that changes a function or a validator and the data it
   validates.
4. **Specify the tool** (4.3): how the Rust and TypeScript clients expose it (a migration set defined in code with the query
   builder, never hand-written wire JSON), the status/plan/apply operations, the checksum check of already-applied
   migrations, and the server-side operations it needs (decide whether a new admin operation is needed or the existing
   batch and system store suffice; state the authorisation).
5. **Protocol** (section 6): what changes on the wire, if anything, and how an older client sees a newer journal.
6. **Tests and acceptance** (CI only): named tests in the server and in each client, concurrent appliers, crash between
   the DDL and the journal record, a changed checksum refused, the three OS where it matters.
7. **Implementation plan:** at most four PR-sized steps with the crate and layer of each change (nothing added to the
   `repo`, `table`, `tx` or `query` layers of `shamir-engine` without listing it; the engine is being cut into crates),
   and the guide sections to add (operations guide and the client guides).
8. **Open questions** that need the owner, each with a default (answer the questions of `01-versioning-and-migrations.md`
   section 8 that belong to the application side).

## Do not

Change any code, test or CI file; redesign the format migration (R1b) or the replication handshake (R1d) beyond boundary
notes; add a dependency (if an option needs one, write it as an owner decision with the alternative that needs none);
use the bare word "migration" where the repository move (`MigrateRepo`) could be meant.

## Observable acceptance

One new file `docs/dev-artifacts/design/app-schema-migrations.md`; every statement about the current code carries a path
(and a line where it matters); the open questions have defaults. `python -I scripts/ci/check_layout.py` OK. The final
report lists the sections, the decisions taken and the open questions.
