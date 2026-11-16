# Brief: R1c step 1 — typed schema-migration model and canonical body (release-1 R1c, implementation)

Design: `docs/dev-artifacts/design/app-schema-migrations.md` — read it in full, the "Review" section included. This is step 1
of its implementation plan (section 8). Read sections 3 (body, canonicalization, ordering), 5.2–5.3 (the typed operations and
their status/plan/apply results), 6 (protocol and compatibility) and 7 (tests) closely.

## Why

Release blocker R1c: applications need a forward-only, checksummed schema-migration journal. Step 1 lays the shared typed
model every later step builds on: the server journal (step 2), the client tools (step 3). Nothing executes yet.

## What

1. **Typed model in `shamir-query-types`**: schema-migration metadata (stable ID, ordinal, display name, canonical-body
   version), the migration body (ordered vector of admitted typed operations, design 3.1), status/plan/apply request and
   result DTOs (design 5.2–5.3; versioned, additive optional fields) and the typed admin operation variants that later
   steps wire to the server. Name the types "schema migration" so they never collide with `MigrateRepo` /
   `StartMigration` (do not rename those).
2. **Admitted subset**: a type-level or validation-level rule that rejects every operation design 3.1 excludes (nested
   sub-batches, `ForEach`, arbitrary function invocation, subscriptions, replication control, user/ACL administration,
   online storage moves, anything not explicitly admitted). Do not take the whole `BatchOp` enum.
3. **Canonicalization** (design 3.2): one versioned specification in `shamir-query-types`. Default: a new versioned body
   container that reuses the existing per-operation canonical encodings of the HMAC subsystem (`crate::hmac`) where an
   operation has one and extends them to admitted operations that lack one. SHA-256 over
   `domain_separator || canonical_version || canonical_body`. Exclude actor, timestamps, binary version, connection
   details, request IDs and journal state; include ID, ordinal, operation order and every semantic value (source and
   module bytes preserved exactly, no line-ending normalization). Unknown canonical versions are rejected.
4. **Rust builders in `shamir-query-builder`**: build a migration set (ordered migrations, each from typed DDL builders),
   fail locally on ordinal gaps, duplicates and non-admitted operations, and expose the canonical bytes and checksum.
5. **Golden vectors**: canonical bytes and checksums for every admitted operation family, stored as test data the
   TypeScript client (step 3) will assert against too. Put them where both can read them and say where in the report.
6. **Tests** (CI only): canonical byte/checksum vectors; determinism independent of construction order where the design
   says order does not matter; list order preserved where it changes effects; type-level validation; unknown-version
   rejection; gap/duplicate/excluded-operation rejection; serde round trips of the new DTOs.

## Rules

* Nothing in `shamir-engine`, `shamir-db`, `shamir-server` or the clients in this step. No new dependency (check what
  `shamir-query-types` already uses for SHA-256 and reuse it; if nothing, stop and report instead of adding one).
* Layout: `src/admin/types/`, `.../types/db/` and `.../types/ddl/` of `shamir-query-types` are at the 7-entry limit
  (counted 2026-11-16). Count them again from a listing. If a new file needs a full folder, the split/move into a topical
  subfolder is its own preparatory change, listed separately in the report (the orchestrator commits it separately). Name
  the concrete home of every new file in the report.
* Query construction stays builder-only; no raw JSON in code, tests or docs (serde round-trip tests excepted, with a
  one-line comment).
* At most 7 entries per folder and 1000 lines per file; one primary export per file; `mod.rs` re-exports only; no local
  `use`; no inline test modules.

## Acceptance (observed in CI)

fmt, `cargo clippy -p shamir-query-types -p shamir-query-builder --all-targets -- -D warnings` and layout clean locally;
the two crates' tests green in CI. The final report lists every file created and changed, every design point implemented,
the chosen canonicalization relationship to `crate::hmac`, and every deviation with its reason.
