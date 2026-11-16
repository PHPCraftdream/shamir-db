# Application Schema Migrations (Release-1 R1c)

- Status: design only; R1c application-schema migrations, not the data-directory format-migration framework (R1b).
- Authority: `docs/dev-artifacts/prompts/release/17-r1c-app-schema-migrations-design.md`.
- Plan: `docs/dev-artifacts/release-1/README.md` (R1c release blocker); `docs/dev-artifacts/release-1/01-versioning-and-migrations.md`, sections 4–8.
- Boundary: `docs/dev-artifacts/design/format-stamp.md` (R1a) and `docs/dev-artifacts/design/migration-framework.md` (R1b). R1c records a database application's schema level; it does not change either format design or the R1d replication handshake.
- Evidence convention: **OBSERVED** statements describe source inspection and carry full repository-relative source citations. They are not claims that runtime tests were executed. **PROPOSED** statements are this design. No build, test, benchmark, or layout command was run for this design task.
- Terminology: “application-schema migration” means a forward-only, ordered change to an application's tables, indexes, functions, validators, and related data. The pre-existing `StartMigration` API is an online repository/table storage-engine move, not this feature. No new R1c type, command, or client method should use the ambiguous bare name `migration` when referring to that storage move.

## 1. OBSERVED: current DDL, wire, client, durability, and replication surface

### 1.1 DDL operations and typed wire surface

The typed `BatchOp` enum contains table and index create/drop/rename, the four storage-move operations, function and validator operations, and declarative schema operations. It places `StartMigration`, `CommitMigration`, `RollbackMigration`, and `MigrationStatus` in the online engine-change group, separate from table/index/function/validator DDL (`crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs:68-92,113-138`). The batch query builder puts these operations into a typed batch through methods such as `create_table`, `drop_table`, `rename_table`, `create_index`, `drop_index`, `rename_index`, and `create_function` (`crates/query/shamir-query-builder/src/batch/batch.rs:389-445`), plus validator methods (`crates/query/shamir-query-builder/src/batch/batch.rs:510-540`).

| Surface | Typed wire DTO and current fields/contract | Rust builder evidence |
|---|---|---|
| Create table | `CreateTableOp`: name, repo, `if_not_exists`, optional retention and declarative schema (`crates/query/shamir-query-types/src/admin/types/db/table_ops.rs:13-33`). | `crates/query/shamir-query-builder/src/ddl/db/create_table.rs:7`; batch insertion at `crates/query/shamir-query-builder/src/batch/batch.rs:391-394`. |
| Drop and rename table | Drop includes repo, optional HMAC, `if_exists`, and cascade cleanup; rename has old/new names and repo (`crates/query/shamir-query-types/src/admin/types/db/table_ops.rs:35-57,73-94`). | `crates/query/shamir-query-builder/src/ddl/db/drop_table.rs:8`; `crates/query/shamir-query-builder/src/ddl/db/rename_table.rs:9`; batch insertion at `crates/query/shamir-query-builder/src/batch/batch.rs:396-413`. |
| Create index | Index name, table, field paths, unique/sorted flags, repo, family, FTS/function/vector options, covering fields, and `if_not_exists` (`crates/query/shamir-query-types/src/admin/types/ddl/index_ops.rs:13-95`). | `crates/query/shamir-query-builder/src/ddl/index/create_index.rs:17`; typed index builder surface and validation live in that file. |
| Drop and rename index | Drop includes HMAC, `if_exists`, and optional retry `request_id` (`crates/query/shamir-query-types/src/admin/types/ddl/index_ops.rs:245-275`). Rename includes source/destination, table/repo, `if_exists`, and optional `request_id` (`crates/query/shamir-query-types/src/admin/types/ddl/index_ops.rs:197-243`). | `crates/query/shamir-query-builder/src/ddl/index/drop_index.rs:8-80`; `crates/query/shamir-query-builder/src/ddl/index/rename_index.rs:7-80`; batch insertion at `crates/query/shamir-query-builder/src/batch/batch.rs:417-438`. |
| Function registration and maintenance | `CreateFunctionOp` accepts source or base64 WASM, replace, language, optimization, visibility/security, grants, and HMAC (`crates/query/shamir-query-types/src/admin/types/ddl/function_ops.rs:8-73`). DTOs also define drop, rename, describe, export, check, alter, folder operations (`crates/query/shamir-query-types/src/admin/types/ddl/function_ops.rs:75-180`). | `crates/query/shamir-query-builder/src/ddl/schema/function.rs:10-148` builds create and its options; batch methods include create/drop/rename/describe/export/check/alter (`crates/query/shamir-query-builder/src/batch/batch.rs:441-489`). |
| Validator registration and binding | Create accepts source/WASM, replace, HMAC; drop/rename and bind/unbind/list DTOs define the remaining wire surface (`crates/query/shamir-query-types/src/admin/types/ddl/validator_ops.rs:11-110`). | `crates/query/shamir-query-builder/src/ddl/schema/validator.rs:11-77` builds create/drop; rename/bind/unbind/list builders are in `crates/query/shamir-query-builder/src/ddl/schema/validator.rs:129-270`; batch methods are in `crates/query/shamir-query-builder/src/batch/batch.rs:510-540,738-745`. |
| Declarative table schema | `FieldRuleDto` carries path, type, constraints, server-computed `keyset_safe`; the wire DTO includes defaults, foreign keys, uniqueness and transformations (`crates/query/shamir-query-types/src/admin/types/db/schema_ops.rs:30-97,129-160`). Set/add/remove/get schema and describe-table operations are `BatchOp` variants (`crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs:130-138`). | Builders: `ddl/schema/config/schema_builders/set_table_schema.rs:6`, `add_schema_rule.rs:6`, `remove_schema_rule.rs:6`, `get_table_schema.rs:6`; describe builder `ddl/schema/describe_table.rs:10`. |

The Rust query-builder crate deliberately constructs a `BatchRequest` and its `BatchOp`s, not the general session/transport envelope; SDKs wrap that batch (`crates/query/shamir-query-builder/src/lib.rs:11-23`). R1c client APIs should therefore compose existing typed builder output rather than serialize handwritten operations.

### 1.2 Server dispatch, authorization, and per-operation behavior

`ShamirDb::execute_as` routes authenticated operations through authorization and `execute_batch`; its `ShamirAdminExecutor` is separately passed to the batch executor (`crates/engine/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:28-46,71-75,116-131`). The admin dispatcher routes table/index operations, online storage move operations, function operations, validator operations, and schema operations to different handlers (`crates/engine/shamir-db/src/shamir_db/execute/core/admin_dispatch.rs:46-52,68-72,85-108`). This is the current code path; it does not itself define an application migration set or an enclosing transaction for arbitrary DDL.

Observed authorization is operation-specific, not a single schema-migration capability: table create checks `Action::Create` on a repo resource before probing existence (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:32-46`); table drop checks `Action::Delete` before its existence path (`:133-146`); index create uses `Action::Write` on the table (`:342-349`); index rename uses `Action::Write` on the table (`:798-812`); validator create/drop use `Create`/`Delete` on the function namespace (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_validator.rs:36-43,105-112`); binding/unbinding uses `Write` on the target table (`:195-220,248-271`). These code paths provide examples for R1c authorization design; they do not prove runtime authorization outcomes here.

Table schema mutation has a per-table RMW lock, an optional expected `schema_version` check, and a schema repository permit; a mismatch is returned as `version_conflict` (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_schema.rs:377-413`). `schema_version` is the table catalogue's monotonic optimistic-concurrency counter, not an application-wide applied-migration journal (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/schema_management.rs:32-46`; `crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_schema.rs:21-22`).

The handler behavior also exposes real retry distinctions: index drop supports client-supplied request IDs, and short-circuits if a prior operation status exists (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:640-659`); index rename has the same correlation/status pattern (`:853-875`). These are per-index-operation protections, not a transaction or idempotency guarantee for a multi-operation application migration. Table create offers `if_not_exists` (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:61-79`), but that flag alone does not establish that an existing table has the intended definition.

### 1.3 Storage move naming collision

The current wire enum explicitly labels the group “Migration (online engine change)” and includes `StartMigration`, `CommitMigration`, `RollbackMigration`, `MigrationStatus` (`crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs:88-92`). The corresponding DTO describes an online table migration to another storage engine (`crates/query/shamir-query-types/src/admin/types/ddl/migration_ops.rs:9-53`), and the Rust builder calls are `start_migration`, `commit_migration`, `rollback_migration`, `migration_status` (`crates/query/shamir-query-builder/src/ddl/policy/migration.rs:9-155`). The release plan says that `MigrateRepo` / `StartMigration` refer to moving a repository between engines and must stay distinct from application-schema migration (`docs/dev-artifacts/release-1/01-versioning-and-migrations.md:55-63`).

The online storage move is experimental and disabled by default. Its handler states the source is frozen for writes, destination engine is `in_memory` only, coordinator/freeze state is partly process-local and not crash-safe for uncommitted restart, and followers are unsupported because replicated apply bypasses the freeze (`crates/engine/shamir-db/src/shamir_db/execute/admin_db/admin_migration.rs:41-63`). The `ShamirDb` API documentation repeats those limitations and says the active map/freeze guard are process-local (`crates/engine/shamir-db/src/shamir_db/shamir_db/core.rs:697-730`). R1c must not reuse that API or imply its recovery guarantees.

### 1.4 Current persistence and durability boundaries

The system store has a `system` repository and catalogues including databases, repositories, tables, functions, validators and replication metadata (`crates/engine/shamir-db/src/shamir_db/system_store/store.rs:22-58`). It can be in-memory or persistent Fjall (`:60-67`); its initialization opens the repo with the supplied durability policy and recovers/certifies before exposing it (`:147-187`). Therefore the phrase “system store” alone is not a promise of disk durability in every embedding configuration.

A shared catalogue durability tail first syncs the system repo WAL, then flushes the particular table data store (`crates/engine/shamir-db/src/shamir_db/system_store/store.rs:431-440`). Table catalogue `save_table_meta` uses an implicit transaction, persists interner state, then calls this tail (`crates/engine/shamir-db/src/shamir_db/system_store/catalogue/ops.rs:646-683`). Function record replacement uses an implicit transaction for delete/insert, persists interner state, then calls the same tail (`crates/engine/shamir-db/src/shamir_db/system_store/identities.rs:25-68`); validator save/remove similarly calls the tail after its implicit write (`crates/engine/shamir-db/src/shamir_db/system_store/validators.rs:12-50`). These are observed catalogue paths, not proof that every index backend operation or every user DDL has one atomic durable unit with the relevant catalogue change.

Table creation routes through `add_table_as_locked` to persist the table catalogue (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:81-93`). Index creation delegates to the relevant table index API (`:491-529`). Index drop/rename use DDL operation status and index-registry behavior (`:640-659,853-875`); these differ from the system catalogue tail above. Function/validator registration persists records in the system catalogue, but registration, in-memory compilation/registry updates, validator binding, and changes to application records are not evidenced as one transaction spanning all those effects.

### 1.5 Batches, transactions, and non-transactional DDL boundary

The current `BatchRequest.transactional` field is documented as enabling MVCC transaction semantics with a consistent snapshot; isolation and durability have distinct fields (`crates/query/shamir-query-types/src/ops/batch/req/batch_request.rs:62-102`). This is not evidence that arbitrary DDL, system-catalogue writes, index builds, function/validator registration, or multiple DDL operations are enlisted atomically. Admin operations have a dedicated executor callback in `execute_batch` (`crates/engine/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:116-131`), while DDL handlers perform their own work and expose no shared transaction token in the inspected handler surface (`crates/engine/shamir-db/src/shamir_db/execute/core/admin_dispatch.rs:46-108`). The release plan explicitly records that transactional DDL does not exist and that application migration cannot be one atomic transaction (`docs/dev-artifacts/release-1/01-versioning-and-migrations.md:55-58`).

**Limit of observation:** these source inspections do not establish crash behavior for every DDL backend/OS, nor that all catalogue and physical index effects share the same durability boundary. R1c must not claim DDL replication, a global DDL transaction, or atomicity from `BatchRequest.transactional` alone. It must record durable progress around effects and define recovery per operation.

### 1.6 Replication and followers: verified limits

The engine's replicated event apply path describes a `ChangelogEvent` data path; it handles `ChangeOp::Put` and `ChangeOp::Delete` by making key/value-store operations (`crates/engine/shamir-engine/src/tx/replication/apply_replicated.rs:103-121,151-183`). It rejects an event if a referenced table is missing and uses commit watermark idempotency (`:122-138`). The inspected event preparation is not a typed application DDL or schema-migration journal protocol. In particular, that code does not prove that table DDL, index descriptors, function/validator catalogues, or the proposed journal are replicated together.

**Do not infer:** DDL replication from user-row replication, or system-store durability/replication from the data-repo changelog. No claim is made here about every server replication configuration or runtime follower role path. R1c's safe default is leader/writable-primary only; until the complete schema plus journal has a defined replication and replay contract, application-schema status/apply must refuse on a follower/read-only replica rather than make local-only schema edits.

### 1.7 Client and administrative entry points

| Client/surface | Observed current surface | R1c implication |
|---|---|---|
| Rust `shamir-client` | `Client::execute` wraps typed `BatchRequest` in `DbRequest::Execute` with current query-language version (`crates/client/shamir-client/src/client/api.rs:364-403`). No migration-set API was found in the inspected client surface. | Add an ergonomic Rust migration-set runner on top of builder-produced operations; do not hand-assemble transport payloads. |
| TypeScript `shamir-client-ts` | `DbHandle` provides `batch()` and bound `run()` and `ShamirClient.execute` sends the batch using the current query-language version (`crates/client/shamir-client-ts/src/core/client/db.ts:71-95`; `crates/client/shamir-client-ts/src/core/client/client.ts:448-495`). Typed table/index/function/validator operations are mirrored in `core/types/schema/ddl.ts` (`:296-405,456-504,595-635,656-704`) and built in `core/builders/ddl/ddl-schema.ts`, `ddl-index.ts`, and `ddl-func.ts`. | Add typed migration descriptions and status/plan/apply methods using these builders. Rust/TS canonicalization must agree. |
| Node binding `shamir-client-node` | `ShamirClient.execute` receives a MessagePack `Buffer`, snapshots it, deserializes it at the FFI boundary, and calls the Rust client (`crates/client/shamir-client-node/src/client.rs:330-360`). Its crate documentation says use the TypeScript builder's `.build()` and that this is a low-level object FFI; `Batch.execute`/DTO reexports are deferred (`crates/client/shamir-client-node/src/lib.rs:35-37`). | Do not make handwritten wire bytes the documented migration authoring path. Provide a supported typed Node/TS surface; retain low-level FFI only as transport. |
| Server admin surface | Existing admin DDL is batched through `ShamirDb::execute_as` and authenticated per operation (`crates/engine/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:28-46,71-75`). | New server operations must authorize journal access themselves; ordinary DDL authorization is not a migration authorization substitute. |

The Node FFI's MessagePack decoding is a request boundary, not justification to construct raw JSON in guides or code. No raw handwritten query or wire JSON should appear in this design's examples or in R1c migration guides.

## 2. PROPOSED: journal ownership, schema identity, and forward-only history

### 2.1 Journal decision: per database, in the durable system catalogue

Store the application-schema journal in the system catalogue, partitioned by a stable database identity. The journal is **per database**, not per repository: an application's schema version covers all its repositories and the function/validator registrations its schema uses, whereas a repo-local journal could report inconsistent versions for one application. Those registrations are **server-global**, not database-global: the function, validator, group and settings catalogues carry no `db_name` (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:175-181`), and validator create/drop authorize against the single db-less `ResourcePath::FunctionNamespace` (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_validator.rs:36-43,105-112`), so one server instance shares one registration namespace across all its databases. §4.3 states the fencing consequence and §5.1 the plan-output consequence. The release model itself assigns a journal to each database and calls it a system table (`docs/dev-artifacts/release-1/01-versioning-and-migrations.md:163-178`). The system tables, including this journal, are system-format state and must evolve through R1b rather than through an application's own migration set (`:216-223`).

Add a stable database identity field to the database catalogue as an R1c system-catalogue change; do not key durable history solely by a mutable database name. Existing databases receive an identity in the R1c backfill. Rename preserves it. This avoids a rename splitting history, and avoids a repository rename/move changing the application's migration namespace. The review settled the identity verification point: no stable database identity exists today — the catalogue record carries only `name` and `created_at` (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:96-105`) — so R1c adds a new identity field (default type: the existing `RecordId` primitive, no new dependency) and backfills it. Database rename is a pure catalogue re-key that rewrites every row carrying `db_name` (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:159-185`); journal rows must therefore be keyed by the identity, never by `db_name`, so a rename can neither re-key nor split them. Note the physical placement differs from the release plan's letter (a “system table in the database itself”, `docs/dev-artifacts/release-1/01-versioning-and-migrations.md:165-166`): the journal is database-scoped logically but lives in the shared system catalogue; a selective per-database export (R3) must carry the journal partition with the database.

The journal physically lives beside other system catalogue records, not in a user repository. This keeps it outside the application migration body and gives format migration a single owner. It does not by itself provide durability: server deployments must use a persistent system store with synced catalogue writes; in-memory stores remain explicitly ephemeral (observed configurations at `crates/engine/shamir-db/src/shamir_db/system_store/store.rs:60-67`). Status/apply must report/refuse when the system store cannot provide the required persistence policy, rather than acknowledge a durable application migration in memory.

### 2.2 Record model and history invariants

Use append-only journal events, with a computed current status, rather than rewriting a single row in place. A migration's durable identity includes:

- stable migration ID and a contiguous one-based ordinal; human-readable name;
- canonical body format version and SHA-256 checksum of the complete typed body;
- status transitions (`started`, per-operation intent/completion, `applied`, or `in_doubt`/`failed`), event sequence, and timestamps;
- applying actor, server binary version, and current format epoch;
- operation index, operation identity, and postcondition/result evidence for each body operation;
- a fencing generation for the database-level migration owner.

The `applied` record is immutable. A later repair is a new, higher-ordinal forward migration. Append-only history makes uncertain windows diagnosable; latest status is derived by replaying journal events in order. Persist timestamps as diagnostic metadata, not as migration identity or as checksum input. Duration is derived from start and finish times.

At every status/plan/apply boundary, validate that the server journal forms a contiguous applied prefix beginning at the release-defined baseline, has no duplicate ordinals or IDs, and contains no unknown mandatory record/body version. Do not infer “latest schema” from an ordinal maximum if there are gaps. The application schema version is the last fully applied migration ID (the release plan's definition, `docs/dev-artifacts/release-1/01-versioning-and-migrations.md:165-178`).

An already-applied ID must match both its recorded ordinal and checksum. A changed checksum, changed ordinal, duplicate ID, missing predecessor, unknown newer journal schema, or applied-after-gap state is a hard refusal with no DDL. Never rewrite or silently repair applied history. A failed/in-doubt record remains visible and blocks later ordinals until explicit recovery resolves it.

### 2.3 Database versus repo journal, exports, and backups

R1c chooses a database-scoped journal because table data and indexes are repo-addressed while one application spans repositories; one repo-specific version cannot safely describe the whole application. Function and validator registration lives in the server-global system catalogue (§2.1; fencing consequence in §4.3), distinct from individual table operations (`crates/engine/shamir-db/src/shamir_db/system_store/store.rs:34-48`). A migration may target one or more repositories while advancing one database ordinal.

Backup and logical export must include the journal and the stable database identity, and restore must restore the schema and its journal consistently. R1c does not implement R2 backup or R3 logical export. Until those designs explicitly carry the schema version and journal identity, document the boundary and block treating an export without journal metadata as an already-upgraded application. This is consistent with the release plan's requirement that backup/export manifests record both format epoch and application schema version (`docs/dev-artifacts/release-1/01-versioning-and-migrations.md:216-223`).

Explicit boundary cases (each must end fully applied-and-recorded or refused with a stated recovery):

- **Restore of an older backup under a newer journal.** Restoring rewinds the journal to the backup's applied prefix; ordinals the restored journal no longer records may be applied again verbatim afterwards, and the §2.2 immutability rule is not violated because the journal — the authority — no longer contains them. A state where data was rewound but the journal was not (or vice versa) is corruption: the §4.2 postcondition checks must detect the drift at the next status/plan/apply and refuse with a stated recovery (restore again from a consistent backup).
- **Backup or snapshot captured while a migration is in flight.** It carries the in-flight journal and the partial DDL together; restoring it reproduces the same in-flight state. The fence generation recorded in a restored journal is owner-less after the restore, so the next apply must acquire a new generation and reconcile per §4.2 — never resume the old owner.
- **Logical import (R3).** Importing journal metadata into a database that already has journal history is refused by default; merging two histories is not defined. An import whose journal's recorded database identity does not equal the target database's identity fails with a stable identity-mismatch outcome (§5.3). An export lacking journal metadata never marks the target upgraded. R3 owns the mechanics; this design owns the rule.

## 3. PROPOSED: deterministic typed migration sets and checksums

### 3.1 Migration body construction

A migration set is application code in Rust or TypeScript: an ordered list of stable migration metadata plus operations composed from the existing typed query/DDL builders. The body contains a stable ID, ordinal, display name, canonical-body version, and an ordered vector of supported typed operations. It is not an arbitrary callback, closure, SQL string, raw JSON object, or client-authored wire blob. The source body is immutable once released/applied.

The body may contain supported table/index/schema DDL, typed function/validator operations, and bounded data changes. Nested sub-batches, `ForEach`, arbitrary function invocation, subscriptions, replication control, user/ACL administration, online repository storage moves, and other effects not explicitly admitted by the migration executor are rejected. Migration bodies should use a strict subset with explicit recovery contracts, not the entire growing `BatchOp` enum.

All database query construction remains builder-only. The existing Rust query-builder produces the `BatchRequest`/`BatchOp` surface (`crates/query/shamir-query-builder/src/lib.rs:4-23,58-84`); TS DDL builders produce typed DTOs (`crates/client/shamir-client-ts/src/core/builders/ddl/ddl-schema.ts:52-71`; `ddl-index.ts:31-51`). No raw query/wire JSON example is included here.

### 3.2 Canonicalization and cross-client checksum semantics

Define one versioned canonicalization specification in `shamir-query-types`, shared by Rust and TS client packages through exported canonicalization vectors. An OBSERVED precedent exists in the same crate: the HMAC subsystem already canonicalizes typed operations (“canonical v2 operation encoding” — `crate::hmac::canonical_batch_op` / `canonical_db_request`, referenced by DTO docs such as `crates/query/shamir-query-types/src/admin/types/db/table_ops.rs:37` and `crates/query/shamir-query-types/src/admin/types/ddl/index_ops.rs:247`, golden bytes in `crates/query/shamir-query-types/src/security/tests/hmac_tests.rs:265-317`) with a TypeScript mirror (`crates/client/shamir-client-ts/src/core/protocol/hmac.ts:9-41`). The migration body canonicalization must state its relationship to it: the default is a new versioned body container that reuses the existing per-operation canonical encodings where a body operation has one and extends them to the admitted non-HMAC-gated operations; a wholly separate second encoding is rejected by default, because two cross-client canonical encodings over the same DTOs will drift. The server parses the typed operation list, validates the allowed subset, normalizes each operation by its DTO type (including explicit defaults, absent optional values, string/number rules, and ordered list semantics), then encodes a deterministic canonical byte sequence. Hash `domain_separator || canonical_version || canonical_body` with SHA-256. Exclude actor, timestamp, binary version, connection details, generated request IDs, and journal state. Include migration ID, ordinal, operation order, all semantic operation values, function/validator source or module bytes, and data payloads that affect the resulting schema/data.

Canonical map/object keys must have a specified order; numeric types must not pass through lossy JS `Number` conversions; list order must be preserved where it changes effects. Canonicalization must be independent of object insertion order, platform, locale, line endings, and serializer implementation. For UTF-8 source, specify whether line endings are preserved or normalized; default is preserve bytes exactly to ensure source edits change the checksum. Do not hash a client's reported checksum as proof: server recomputes it from the typed body.

Rust and TypeScript builders expose the same body semantics, and cross-client golden vectors assert identical canonical bytes and checksums for all operation families. The Node binding consumes the typed TS-built payload through its existing FFI and must not define a third canonicalization. A future canonicalization change uses a new body version; it never reinterprets a prior applied version.

Persist the canonical bytes (or their canonical typed representation sufficient to reproduce them) with the migration history. This permits diagnosis and checksum verification after application source changes or removal. The server stores the checksummed body before execution; checksum plus persisted body is the source of evidence, while the original client source remains outside the database.

### 3.3 Ordering, gaps, and status/plan

Migration IDs are stable, unique, and not inferred from timestamps. Ordinals are strictly contiguous; each migration's predecessor must already be `applied` with the expected checksum. A client cannot reorder already-published ordinals, insert a missing older ordinal after newer work, or redefine an applied body. Plan compares the local typed set against the server's full applied prefix, flags checksum drift, lists the next operations and recovery limitations, and makes no writes. Status returns the database identity, latest applied ID/ordinal, current in-flight state/fence, and checksum validation result without exposing stored source to unauthorized callers.

If the client has a local set with an ordinal gap or duplicate before reaching the server, fail locally. The server independently checks the prefix because clients are not trusted authorities. A newly deployed binary with a different body for an applied ordinal refuses; the remedy is restore a pre-migration backup or publish a new forward-only correction.

## 4. PROPOSED: applying, fencing, and recovering a migration

### 4.1 Atomicity contract and effects

Do not promise a whole-migration transaction. Current DDL is not globally transactional (`docs/dev-artifacts/release-1/01-versioning-and-migrations.md:55-58`), and the inspected batch MVCC contract does not enlist all admin effects (`crates/query/shamir-query-types/src/ops/batch/req/batch_request.rs:62-102`; `crates/engine/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs:116-131`). The guarantee is durable, ordered progress with operation-level recovery and fail-closed uncertainty.

The runner sequence is: validate database identity and full applied prefix; authenticate/authorize; acquire database migration admission/fence; persist and sync `started` plus the complete checksum-verified body; persist and sync operation intent; execute one bounded operation; verify its postcondition; persist and sync operation completion; repeat; verify end-state invariants; persist and sync `applied`; release the fence. Never record an operation complete before its postcondition is observed. Never mark the ordinal applied before all operation completions and final checks are durable.

Each operation has a stable ID derived from migration ID, ordinal and operation index. Server execution uses that ID to deduplicate retries where the underlying DDL supports it. The existing index drop/rename request ID behavior is a useful precedent but is not treated as a blanket guarantee (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:640-659,853-875`). For DDL with no existing request-ID support, add server-side migration operation receipts or postcondition reconciliation; do not invent client-supplied IDs that bypass authorization.

### 4.2 Durable progress versus uncertain windows

Journal durability must use the persistent system-store policy and its durable catalogue write path (the current helper's sequence is WAL sync then table flush, `crates/engine/shamir-db/src/shamir_db/system_store/store.rs:431-440`). R1c implementation must verify the system-store backend and failure behavior for each journal event. If any journal write, sync, or read-back fails, stop before starting the next effect; report the migration in-doubt if an effect may already have occurred. Never return success based only on an in-memory journal cache.

A crash may occur after an effect commits but before its completion event is durable. On restart:

1. If no operation intent is durable, that operation did not start; proceed only after acquiring a new fence and validating the stored body/checksum.
2. If intent exists without completion, inspect the operation-specific postcondition and durable operation receipt. If the desired exact state is proven, append completion without replaying the effect. If the operation is proven not to have taken effect, execute it once under the new fence. If neither fact is proven, set/retain `in_doubt`, block later operations, and require explicit operator resolution, restore, or a new forward correction after safe reconciliation.
3. If completion exists, do not rerun the effect; verify its postcondition before continuing. If that postcondition has since changed, refuse rather than conceal drift.
4. If `applied` is durable, retries return the already-applied result only after verifying the same ID/ordinal/checksum. A mismatching body is refused.

No arbitrary migration callback is automatically replayed. For uncertain non-idempotent DML, default is refusal/restore, not retry. DML may be admitted only when its effect is provably idempotent or its deduplication marker and mutation commit atomically in the same data-repository transaction. A transaction flag alone is insufficient evidence. Where the system journal is a different repository from user data, the implementation must explicitly solve the cross-repo marker/uncertain window or reject that DML shape. Design default: permit only deterministic set/delete-style operations with exact postconditions and same-repo idempotency receipts; reject unsupported update expressions, dynamic loops, and effects whose result cannot be reconciled.

A DDL retry contract is operation-specific:

- Create: success only if the named object exists with the exact intended definition; an existing different definition is a conflict, not `if_not_exists` success.
- Drop: absence is success only if the recorded intended object identity/definition matches the operation's target; otherwise report drift.
- Rename: source absent plus destination carrying the exact expected object identity is completed; both names present, neither present, or destination conflict is in-doubt.
- Index build/drop/rename: use durable DDL status/request receipts where available and verify index family, fields/options, table, and terminal operation state before completing the journal event.
- Function/validator replacement: compare exact code/source hash and relevant registration/binding metadata; do not infer completion from name existence.

A partially completed migration does not roll back earlier operations. Recovery is resume of a proven incomplete idempotent operation, explicit reconciliation, restore of the pre-migration backup, or a later forward migration. There are no down migrations and no automatic reverse DDL.

### 4.3 Durable concurrency and fencing

Permit at most one application-schema migration per database — and, because function and validator registrations are server-global (§2.1), any migration body containing a function or validator operation serializes server-wide: at most one such migration in flight per server instance. The owner default until changed is stronger and simpler: at most one application-schema migration in flight server-wide, with the per-database scope a diagnostic label rather than a concurrency boundary. Use both an in-process per-database gate and a durable compare-and-set control record with monotonically increasing fencing generation in the system catalogue. The generation is attached to every begin, operation intent/completion, and applied event; a stale runner cannot append or execute after ownership changes. A lease timeout alone is not sufficient: a paused process could resume after its lease expires. Recovery must first fence the old owner, then acquire a new generation and reconcile its pending operation before progress resumes.

All DDL and writes that could conflict with migration effects must consult a database migration gate — including writes issued by stored procedures (`CallOp` executes guest code that performs database writes through the direct intrinsics), not only direct write operations. Safe initial release default: take a database-wide write/DDL maintenance fence while applying; reject conflicting writes and DDL with a stable `schema_migration_in_progress` error. Reads remain available only where the migration's intermediate state is safe; otherwise reject affected reads too. This is an explicit availability cost, but avoids silently interleaving ordinary writes with schema/data changes. If implementation cannot reliably gate every writer path (including nested batches and direct engine APIs), restrict execution to a stopped server/maintenance mode rather than advertise online concurrency safety.

Two clients attempting the same ordinal/body: one acquires the generation; the other reads the durable state. If already applied with same checksum, return success/no-op. If in progress under a live owner, return status/in-progress without running duplicate effects. If in doubt or crashed, only explicit resume/reconcile under a new generation is allowed. Same ID with a different checksum always refuses. Different concurrent ordinals do not queue or skip; they serialize and then revalidate predecessor/checksum.

### 4.4 Functions, validators, and data being validated

A migration that changes a function must include the replacement as a typed operation and must preflight the new body with `CheckFunction` before replacing the stored registration. `CheckFunctionOp` is present in the batch enum (`crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs:113-120`) and the builder batch supports a fallible check insertion (`crates/query/shamir-query-builder/src/batch/batch.rs:477-485`). The release plan notes that replacing a procedure can change `call_signature`, causing checked callers to fail, and identifies dependency checking via `check_function` as a second-queue enhancement (`docs/dev-artifacts/release-1/01-versioning-and-migrations.md:204-209`). R1c must not claim that a candidate `check_function` automatically discovers or validates every dependent function. Initial implementation requires the migration author to list dependent call sites/functions and the plan to show them; unresolved dependencies block apply. A server-side dependency graph check is a follow-up/owner decision.

A validator migration must define create/replace, rename/drop, and bind/unbind changes explicitly. Validator catalogue changes persist, but are separate operations (`crates/engine/shamir-db/src/shamir_db/system_store/validators.rs:12-50`; operation handlers at `crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_validator.rs:15-75,78-137,174-277`). Replacing validator code or adding a binding does not itself prove all existing rows satisfy the new rule. Before activating a stricter validator, migration data steps must scan/repair the affected rows, then perform a final validation pass, then bind/activate. If the scan is not supported or cannot run under a safe write fence, refuse the migration; never silently make future writes valid while existing data violates the advertised schema.

Data backfills are ordered explicit body operations with bounded work and a deterministic completion predicate. Record expected row-count/checksum or another content-specific postcondition as plan metadata; do not treat “operation returned no error” as proof of complete backfill. Every DML effect must satisfy §4.2 retry rules. Because R1c DDL is not all transactional, function, validator, schema and data changes can be visible in intermediate states; the maintenance fence, plan output, in-doubt status and recovery path are part of the contract, not optional diagnostics.

### 4.5 Followers and read replicas

Status may be served read-only from a follower only if it is explicitly identified as a consistent view of both database journal and all relevant schema/data effects. Otherwise refuse status as stale/unknown. Apply and plan-for-apply are primary-only; they must fail closed on follower/read-replica nodes. Do not rely on the current replicated row-event stream to propagate the journal, DDL, indexes, functions, or validators: inspected engine apply handles row `Put`/`Delete`, not this complete schema state (`crates/engine/shamir-engine/src/tx/replication/apply_replicated.rs:151-183`).

A future replication design must replicate database identity, journal sequence, application DDL/schema catalogues, function and validator registration/bindings, and dependent data changes with a consistent order and recovery protocol. That is outside R1c's first implementation unless separately designed; it must not modify R1d handshake as part of this task.

## 5. PROPOSED: client tool and server operations

### 5.1 Client authoring and operations

Rust and TypeScript packages expose a migration-set type that captures stable ID, ordinal, name, typed builder operations, and canonical checksum. The migration set is compiled into the application, not discovered from arbitrary filesystem SQL. Users author a new forward change for every later schema correction. Both clients expose:

- `status`: read the database journal and compare it with the locally compiled migration set; report applied prefix, drift, gaps, in-flight owner, and in-doubt operation.
- `plan`: compute the exact next migration(s), checksum checks, DDL/DML effects, estimated or bounded work, writer fence/outage, function/validator consequences (including the cross-database blast radius of server-global registrations, §2.1), and recovery warnings. It must not write a journal record or execute a side effect.
- `apply`: submit exactly one next ordinal at a time; server recomputes checksum, validates predecessor, enforces authorization/fencing, and returns durable status. Client never silently retries an uncertain migration body; it queries status then follows server reconciliation outcome.

Rust APIs should use `shamir-query-builder` typed constructors. TypeScript should use existing `core/builders/ddl` and batch builders, with migration-specific wrappers. Node's low-level `execute` remains transport only; public migration authoring is via TS migration API or a separately supported typed Node wrapper, not raw MessagePack construction (`crates/client/shamir-client-node/src/client.rs:330-360`; `crates/client/shamir-client-node/src/lib.rs:35-37`).

Each migration set carries a stable checksum checked at build/test time and by server at apply time. A development command may show local checksums and status; do not provide a down/rollback API.

### 5.2 New server-side typed operations, not direct journal writes

Add typed, authenticated admin `BatchOp` operations for:

1. `ApplicationSchemaStatus` — read-only current journal state and checksum verification.
2. `ApplicationSchemaPlan` — server-side validation of local ordered definitions against persisted prefix, returns allowed next operation plan without effects.
3. `ApplyApplicationSchemaMigration` — submits one checksum-verified body and expected predecessor/fence generation; server owns journal state transitions and executes the admitted typed subset.

Names deliberately include `ApplicationSchema`; do not use `StartMigration`, `MigrationStatus`, or bare `migration` for this feature. Clients cannot create/update journal rows through general table DML or supply trusted actor/time/binary version/format epoch/state. Server derives actor from authenticated session, binary version from build metadata, and the format epoch from the format stamp. The stamp module lives in `shamir-sync`, which sits above `shamir-db` in the dependency order (`shamir-server -> shamir-sync -> shamir-db`; stamp design S-2.2), so the handler inside `shamir-db` cannot read it: the server injects the observed epoch — and whether the stamp is absent or `adopted` — into `shamir-db` at initialization, the same direction the durability policy already travels (`crates/engine/shamir-db/src/shamir_db/system_store/store.rs:153-187`). An embedded `ShamirDb::init` without server wiring (stamp design D2) injects nothing; the journal records the explicit unknown/unsupported state and refuses claims of epoch compatibility. No upward dependency is added.

A new admin operation is required. Existing batch and system store provide typed operation transport and a place to persist internal state, but do not themselves supply atomic orchestration of journal+DDL, operation admission, or recovery. The server performs checksum and body validation, owns control records, and enforces each DDL operation's normal authorization in addition to the migration-level permission.

Authorization: status requires `Read` on the database; plan requires `Manage` on the database because it reveals the database's schema and admission state; apply requires `Manage` on the database plus authorization for every underlying DDL/data target. Applying migrations must not bypass HMAC-required destructive function/index/table operations. Default actor is never client-supplied. These are proposed R1c permissions; current per-DDL checks remain as observed in §1.2.

### 5.3 Status, plan, apply outcomes and safe retry

Return structured typed results with stable codes: `schema_migration_in_progress`, `schema_migration_in_doubt`, `schema_migration_checksum_mismatch`, `schema_migration_gap`, `schema_migration_predecessor_mismatch`, `schema_migration_follower_read_only`, `schema_migration_durability_unavailable`, `schema_migration_postcondition_failed`, and `schema_migration_identity_mismatch` (the journal's recorded database identity does not match the serving database — possible after an import under a different name or a restore that mixed catalogue states). Keep them distinguishable from the existing online storage-move errors.

On a lost response, client issues status for the same migration ID. If applied with same ordinal/checksum, report success. If an operation intent remains, client asks for plan/reconcile; it does not blindly resubmit. If the server cannot prove the effect, it returns in-doubt and documents restore or operator reconciliation. A server restart loses in-memory locks but must not lose the durable fencing generation or journal state. Resume/reconcile of an in-doubt migration uses the same `ApplyApplicationSchemaMigration` operation: the caller submits the identical stored body with the expected predecessor and a fresh fence generation, and the server fences the old owner and performs the §4.2 reconciliation. There is no separate reconcile operation in the first release.

## 6. PROPOSED: protocol and compatibility

Use existing `BatchRequest`/`BatchOp` transport and add typed operations; do not alter handshake negotiation or replication handshake (R1d boundary). Existing clients continue to send existing v4 operations unchanged. New migration operations are only understood by a server that implements R1c; an older server may reject the unknown typed operation. New clients must surface a clear unsupported-operation/server-version error and must not fall back to unjournaled DDL.

An old client does not gain a way to read or apply a newer schema journal. Its ordinary existing queries may continue to work if the application schema remains backward-compatible, but this is application policy rather than a journal guarantee. New status responses must have a versioned typed DTO with additive optional fields where possible; never require an old client to decode the new journal by reusing an existing operation's response shape.

Only introduce a query-language version change if the new request/response shape is incompatible with the currently supported version, or if its admission policy requires a protocol boundary. The server currently declares handshake protocol version 1 and query-language support `[4]` (`crates/server/shamir-sync/src/version.rs:25-48`); it rejects unsupported query-language versions (`:91-102`), and server request handling checks that version before execution (`crates/server/shamir-server/src/engine/db_handler/handler.rs:607-615`). Do not assert that adding a typed operation is automatically compatible merely because the outer batch schema is unchanged. Add explicit old-server/new-client and new-server/old-client compatibility tests; update the version matrix only with an owner-reviewed compatibility rule.

No changes to R1a format stamp, R1b format-migration command/journal, R1d handshake, or replication seed format are in scope. Application schema version and format epoch remain separate version axes; journal rows record the format epoch at apply time.

## 7. PROPOSED: CI-only tests and acceptance

All tests below run in GitHub CI, never locally for this design task. Run server/core acceptance on Linux, Windows, and macOS where filesystem persistence, crash recovery, and client behavior matter. The repository CI full tier uses selected affected targets across OS matrices; each PR touching these paths must be selected for the named tests. No test/build/cargo/npm command was run here.

### 7.1 `shamir-db` server/system-catalogue tests

Place in the `shamir-db` module's topic-split `tests/` directories — counted from listings, 2026-11-16: `src/shamir_db/tests/` and `src/shamir_db/shamir_db/tests/` both hold 7 entries, so a new topic folder goes inside a folder with room (e.g. `src/shamir_db/tests/ddl_contracts/` holds 6; see the §8 layout note):

- `schema_migration_journal_persistent_round_trip`: system store closes/reopens with identity, canonical body, checksum and event sequence intact.
- `schema_migration_journal_sync_failure_never_acknowledges`: injected durable-tail failure cannot return applied; pending effect becomes recoverable/in-doubt.
- `schema_migration_database_identity_survives_rename`: database rename preserves namespace and status.
- `schema_migration_status_requires_database_read`: denied actor cannot inspect status.
- `schema_migration_plan_requires_manage_and_is_read_only`: directory/catalogue snapshot proves no journal or data mutation.
- `schema_migration_apply_requires_manage_and_each_ddl_grant`: database management alone cannot bypass target DDL rights or HMAC gates.
- `schema_migration_applied_prefix_rejects_gap_duplicate_and_reorder`.
- `schema_migration_changed_applied_checksum_refused_without_write`.
- `schema_migration_same_id_same_checksum_retry_is_noop` and `schema_migration_same_id_different_checksum_refused`.
- `schema_migration_concurrent_appliers_one_fenced_owner`: two clients, one database, exactly one operation execution and one durable applied event.
- `schema_migration_stale_fence_cannot_append_or_execute`.
- `schema_migration_crash_after_intent_before_ddl_resumes_once`.
- `schema_migration_crash_after_ddl_before_journal_completion_reconciles_or_refuses`: test both provable postcondition and uncertain non-idempotent outcome.
- `schema_migration_crash_after_last_operation_before_applied_rechecks_final_state`.
- `schema_migration_dml_retry_requires_same_repo_atomic_receipt`; uncertain non-idempotent DML is refused, not replayed.
- `schema_migration_function_check_precedes_replace_and_checksum_covers_source`.
- `schema_migration_validator_backfill_precedes_binding_and_existing_rows_validate`.
- `schema_migration_follower_apply_refused_without_schema_journal_replication`.
- `schema_migration_status_and_plan_unknown_journal_version_fail_closed`.
- `schema_migration_restored_backup_rewinds_journal_and_reapplies`: the restored journal's applied prefix is authoritative; re-applying a previously applied ordinal verbatim succeeds.
- `schema_migration_import_with_conflicting_journal_or_identity_refused`.
- `schema_migration_global_registration_migration_serializes_server_wide`: two databases' migrations touching function/validator registrations cannot overlap.

Crash points are exercised deterministically through the existing system-store fault gates (`arm_durability_tail_faults`, `catalogue_write_fault_gate`; `crates/engine/shamir-db/src/shamir_db/system_store/store.rs:73-77,120-145`), not by timing races; a real server-restart window is additionally covered by the §7.3 e2e restart test.

### 7.2 Rust client `shamir-client` tests

- `rust_migration_set_uses_typed_ddl_builders_only`.
- `rust_migration_status_plan_apply_send_typed_admin_operations`.
- `rust_migration_lost_apply_response_queries_status_without_replay`.
- `rust_migration_changed_checksum_or_gap_fails_before_apply`.
- `rust_migration_body_canonical_vectors_match_typescript`.

### 7.3 TypeScript client `shamir-client-ts` tests

- `migrationSetBuildsOrderedTypedBody`.
- `migrationStatusAndPlanDoNotSubmitApply`.
- `migrationApplyUsesServerResultAndDoesNotBlindlyRetry`.
- `migrationChecksumDetectsChangedAppliedBody`.
- `migrationCanonicalVectorsMatchRust`.
- E2E `migrationConcurrentAppliersAndCrashRecovery` against the real server: verify same-set race, restart after DDL-before-journal window, and in-doubt refusal.

### 7.4 Node binding `shamir-client-node` tests

- `node_migration_execute_accepts_typed_typescript_built_batch`: exercise the existing Buffer FFI with a builder-generated, supported operation, not a manually authored wire object.
- `node_migration_error_and_status_round_trip`: preserve typed checksum/in-doubt/unsupported-operation codes through the binding.
- `node_migration_does_not_retry_uncertain_apply`: caller receives status-needed result without binding-level replay.

Run persistent server/system-store crash tests on Linux, Windows, and macOS. Run TypeScript/unit compatibility tests on supported Node CI OS; at minimum ensure the Node binding's supported Windows build job includes status/error and typed-body coverage, and run Rust/client/server migration suites on the three OS matrices. Runtime evidence is the CI result, not these source-inspection notes.

### 7.5 Acceptance gates

- A database has one stable identity and one durable per-database schema journal.
- Applied bodies are immutable and checksum-verified; order is contiguous and gaps/checksum changes refuse.
- Two concurrent appliers cannot execute one operation twice; stale fences are rejected.
- Crash tests cover the intent/effect/completion/applied windows; no uncertain DDL/DML is blindly replayed.
- Functions are preflighted; validators are not bound until backfilled data is checked.
- Followers/read replicas fail closed without a complete schema+journal replication design.
- Rust and TS canonical checksum vectors match; Node uses typed builder output.
- CI named tests above pass on required OS matrices; layout is checked by orchestrator after review. No local layout command is requested here.

## 8. PROPOSED: implementation plan (four steps; step 2 lands as two PRs)

| Step | Scope and crate/layer impact | Required proof/docs |
|---|---|---|
| 1. Typed model and canonical body | `shamir-query-types` (`crates/query` group): schema migration metadata, status/result DTOs, canonicalization version and typed admin operation variants. `shamir-query-builder` (`crates/query`): Rust operation/body builders and canonical test vectors. No new dependency. No change to `shamir-engine`. | Canonical bytes/checksum vectors, type-level validation, unknown-version rejection. Do not change existing `StartMigration` names. |
| 2. Durable journal and server executor | `shamir-db` (`crates/engine`, database facade/system-store layer): stable DB identity, append-only journal records, durable event append/read, CAS/fencing generation, operation postconditions and status/plan/apply handlers. `shamir-server` (`crates/server`): authenticated session wiring, primary/follower refusal, error mapping and policy. Keep journal schema inside the system catalogue and evolve it under R1b format compatibility. | `shamir-db` crash/fault/concurrency/auth tests. Existing system-store durable helper is not presumed to make separate-repo DML atomic. No additions to `shamir-engine` `repo`, `table`, `tx`, or `query` layers. |
| 3. Rust and TypeScript migration tools | `shamir-client` Rust API and `shamir-client-ts` typed migration-set API, operations, checksum verification, status/plan/apply and no-blind-retry behavior. Update Node binding only if a typed wrapper is needed; otherwise preserve its FFI and add Node-facing errors/tests through builder-built buffers. No new dependency. | Rust/TS cross-client vectors; client tests in §7.2–7.4. Add client guide sections. |
| 4. Integration, guides, compatibility CI | `shamir-server`/`shamir-db` integration tests, selected CI matrix wiring only where existing matrix does not select affected test targets; guide updates. No engine-layer additions. | Three-OS crash/concurrency acceptance. Operations guide and Rust/TS/Node migration guides. Coordinate any schema-version manifest fields with R2/R3; do not redesign those directions here. |

**Explicit `shamir-engine` impact:** none in `repo`, `table`, `tx`, or `query` layers. Keep orchestration in `shamir-db` and server. If implementation discovers a necessary engine primitive, stop and list the exact repo/table/tx/query additions, ownership rationale, and separate review rather than smuggling it into R1c.

**Layout constraint (counted from folder listings, 2026-11-16).** Most natural homes sit exactly at the 7-entry limit: `shamir-query-types` `src/admin/types/`, `.../types/db/`, `.../types/ddl/`; `shamir-db` `src/shamir_db/system_store/`, `.../execute/`, `.../execute/core/`, both test roots (`src/shamir_db/tests/`, `src/shamir_db/shamir_db/tests/`); `shamir-client` `src/` and `src/client/`; `shamir-client-ts` `src/core/`, `core/client/`, `core/builders/`. Room exists in `shamir-db` `src/shamir_db/system_store/catalogue/` (6), `.../execute/admin_ddl/` (5), and `src/shamir_db/tests/ddl_contracts/` (6). The implementation brief must name the concrete home of every new file; where the only right home is a full folder, the split/move is its own commit and must not ride along with a feature diff.

Step 2 of the table is larger than one PR; land it as two PRs — (2a) stable database identity, journal records, durable append/read, fencing generation; (2b) the three typed operations, handlers, postconditions, and server wiring. Step 4's guide updates note that the administrative shell (R5) gains the same apply/status/check commands later (release plan `docs/dev-artifacts/release-1/01-versioning-and-migrations.md:200-201`); R1c ships client APIs only.

**Guides:** add a migration runbook subsection to `docs/guide-docs/guide/07-operations.md` covering backup, status, plan, apply, maintenance fence, follower refusal, checksum drift, in-doubt recovery, restore-before-upgrade and forward-only correction. Add Rust migration-set guide to the `shamir-client` docs, TypeScript guide to `shamir-client-ts/docs`, and a short Node note that migration authoring uses the typed TS/client API rather than raw MessagePack. Confirm the exact client guide filenames against current package documentation during implementation; no other docs are changed in this design task.

## 9. PROPOSED: owner questions and defaults

Each default applies until the owner changes it. These cover the application side of `01-versioning-and-migrations.md` section 8; format-stamp-specific base epoch, data-directory stamp placement, and reference-catalogue size are R1a/R1b questions and are not reopened here.

| Question | Default |
|---|---|
| Journal scope: per database or per repository? | Per database; one application ordinal covers its repositories and database-global function/validator state. |
| Physical location? | Persistent system catalogue, partitioned by stable database identity. Do not put journal rows in an application-owned repo; system catalogue/journal format evolves under R1b. |
| Stable identity on pre-existing databases? | Add/backfill an immutable database identity in R1c; preserve it across database rename. Settled by review: no stable ID exists today (`db_management.rs:96-105`); add a new field, default `RecordId`; no new crate/dependency. |
| Append-only history or overwrite status row? | Append-only events and immutable applied records; derive current status from event history. Keep in-doubt evidence. |
| Ordering and baseline? | Release chooses an explicit baseline (default ordinal 0, no applied user migration); strictly contiguous one-based ordinals; IDs unique and immutable. No gaps or reorder. |
| Body/checksum authority and encoding? | Versioned canonical typed operation body, server-computed SHA-256, Rust/TS shared vectors, persist canonical body, preserve source bytes exactly. No arbitrary callbacks or handwritten JSON. |
| One atomic migration or weaker guarantee? | Weaker durable operation-level progress; no whole-migration rollback or transactional DDL claim. Each operation needs a postcondition/receipt. |
| Data DML retries? | Only deterministic idempotent operations with exact postconditions or an atomic same-repo dedupe receipt. If the system/data repository boundary leaves outcome uncertain, refuse and require restore or explicit forward repair. |
| Uncertain DDL after effect/before journal completion? | Reconcile by durable operation receipt and exact postcondition. If not provable, mark in-doubt and block later ordinals; never blindly repeat. |
| Concurrent clients and server crash? | Durable per-database CAS/fencing generation plus admission gate. Do not rely on in-memory lock or timeout-only lease. Stale owner is fenced before reconciliation. |
| Function/validator registrations are server-global — how do fences serialize them? | Default: at most one application-schema migration in flight server-wide; per-database concurrency only for bodies without global registrations, as a later refinement. |
| What does an R3 logical import do with a journal? | Refuse journal import into a database with non-empty journal history; require the recorded database identity to match the target; an export without journal metadata never marks the target upgraded. R3 owns the mechanics. |
| Online writes during migration? | Default maintenance fence: reject conflicting DDL and writes for the database for the duration. If every writer path cannot be gated, require a stopped server. Reads only when intermediate state is declared safe. |
| Followers/read replicas? | No plan/apply on followers; status only if journal and schema consistency can be proven. Otherwise all operations refuse. Do not imply DDL/journal replication exists. |
| Functions/dependent procedures? | Run `CheckFunction` on replacement code; migration author lists dependent procedures and they must be checked. Automatic dependency discovery is deferred unless owner makes it a release blocker. |
| Validator change and old data? | Backfill/repair and full validation before activating a stricter binding; if not provable, refuse. |
| Authorization? | `Read(database)` for status; `Manage(database)` for plan/apply, plus every underlying DDL grant/HMAC remains required. |
| Protocol version? | Keep existing handshake unchanged; additive typed operations, explicit unsupported-operation failure on older server, compatibility tests. Bump query-language version only if compatibility review shows a breaking shape/policy change. |
| What does a client do on lost response? | Query status/reconcile; never automatically resend an uncertain body. Same ID+checksum applied is a no-op; changed checksum refuses. |
| Rollback/down migration? | No down migrations. Recover from verified pre-migration backup or add a new forward migration; do not confuse this with the online storage-move `RollbackMigration`. |
| Backup/export of schema version? | R1c journals format epoch and database schema level. R2/R3 must include identity and journal/version metadata before claiming consistent backup/export. |
| Progress and long backfills? | Plan shows operation order, scope and work estimate; status shows current operation and elapsed time. Start with bounded steps and per-operation progress; no in-place cancellation that pretends to undo effects. |
| New dependency? | None. If an implementation option requires one, default is the existing typed serialization/hash capabilities or a small in-repo canonicalizer; any dependency becomes a separate owner decision. |
| Test platforms? | Linux, Windows and macOS for persistence/crash paths; clients on each supported CI platform, including the Windows Node binding lane. |
| Existing R1 plan section 8: format epoch boundary, stamp for external/in-memory repos, fixture size? | Not application-schema design questions. R1c records the observed format epoch but does not redefine R1a/R1b answers. |
| Existing R1 plan section 8: migration duration/progress on large data? | Applicable. Default plan estimates plus operation-level status; bounded backfill operations; detailed streaming progress can follow, but never silently drop in-doubt state. |

## 10. Evidence and review boundary

OBSERVED citations above are source locations inspected for this design; source inspection is not runtime proof. In particular, no conclusion here establishes that all DDL is replicated, that every system-store configuration is persistent, that DDL joins an MVCC batch transaction, or that Windows/Unix crash windows have been exercised. Those are explicit implementation/CI proof obligations in §7.

This artifact changes no code, test, CI, dependency, R1a stamp design, R1b format migration, or R1d replication handshake. The implementation must use focused follow-up review to verify any additional current-code claim or exact test placement before turning this proposal into an implementation brief.

## Review

Independent review, 2026-11-16 (branch `rel-r1c-design`). Every OBSERVED citation in sections 1–7 was re-opened and checked against the code; the release plan (`README.md`, `01-versioning-and-migrations.md` sections 4–8), the related designs (`format-stamp.md` R1a, `migration-framework.md` R1b) and the R3 plan (`docs/dev-artifacts/release-1/03-logical-dump-restore.md`; no separate `logical-dump.md` design exists) were read in full; folder counts were taken from listings, never from another document. All checks were read-only; only this document was edited.

### Verified as correct

- All section-1 `path:line` citations re-opened and accurate: the batch enum groups (`crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs:68-92,113-138`), builder methods (`crates/query/shamir-query-builder/src/batch/batch.rs:389-445,441-489,417-438,477-485`), the DTO field lists (`admin/types/db/table_ops.rs`, `admin/types/ddl/index_ops.rs`, `.../function_ops.rs`, `.../validator_ops.rs`, `admin/types/db/schema_ops.rs`), the auth checks (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:32-46,61-79,81-93,133-146,342-349,491-529,640-659,798-812,853-875`; `.../admin_validator.rs:36-43,105-112,174-277,195-220,248-271`), the schema RMW/version-conflict path (`.../admin_schema.rs:377-413`; `.../shamir_db/db/schema_management.rs:32-46`), the storage-move gate and limitations (`.../admin_db/admin_migration.rs:41-63`; `.../shamir_db/core.rs:697-730`), the system-store surface and durable tail (`.../system_store/store.rs:22-58,60-67,147-187,431-440`; `.../system_store/catalogue/ops.rs:646-683`; `.../system_store/identities.rs:25-68`; `.../system_store/validators.rs:12-50`), the replicated row-event apply (`crates/engine/shamir-engine/src/tx/replication/apply_replicated.rs:103-121,122-138,151-183`), all client surfaces (`crates/client/shamir-client/src/client/api.rs:364-403`; `crates/client/shamir-client-ts/src/core/client/db.ts:71-95`; `.../client/client.ts:448-495`; `.../types/schema/ddl.ts` ranges; `.../builders/ddl/ddl-schema.ts:52-71`, `ddl-index.ts:31-51`; node `crates/client/shamir-client-node/src/client.rs:330-360`, `lib.rs:35-37`), and the version gates (`crates/server/shamir-sync/src/version.rs:25-48,91-102`; `crates/server/shamir-server/src/engine/db_handler/handler.rs:607-615`). Three line ranges were off by 1–9 lines and were corrected (finding 11).
- The release-plan citations (`01-versioning-and-migrations.md:55-63,55-58,163-178,165-178,204-209,216-223`) match the plan text.
- “No migration-set API was found in the inspected client surface” re-verified by grep across all three clients: the only `migration` surface is the online storage-move API.
- The review brief's apply-protocol attack list was walked against §4: two concurrent appliers of the same set, crash between a DDL and its journal record, crash inside a multi-step migration, retry after a timeout whose first attempt committed, changed checksum of an applied migration, follower/read replica, and a function/validator change together with the data it validates — each already ends fully applied-and-recorded or refused with a stated recovery. The missing cases are findings 2 and 3.

### Findings and fixes

1. **MAJOR (fixed). §2.1/§2.3 called function/validator registrations “database-global”; they are server-global.** Evidence: the function, validator, group and settings catalogues carry no `db_name` (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:175-181`); validator create/drop authorize against the db-less `ResourcePath::FunctionNamespace` (`crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_validator.rs:36-43,105-112`); `CreateFunctionOp`/`CreateValidatorOp` have no database field (`crates/query/shamir-query-types/src/admin/types/ddl/function_ops.rs:29-73`). Consequence the design missed: a migration of database A that replaces a registration affects database B, and two per-database fences would not serialize concurrent migrations touching the same global registration. Fixed: wording corrected (§2.1, §2.3); §4.3 serializes bodies containing function/validator operations server-wide, owner default “one application-schema migration in flight server-wide”; §5.1 plan output names the cross-database blast radius; §9 gained the open question with that default.
2. **MODERATE (fixed). The restore/backup attack cases were missing.** “Restore of an older backup under a newer journal” and a snapshot taken mid-migration had no stated outcome. Fixed: §2.3 states the rewind-and-reapply rule (the restored journal is the authority; previously applied ordinals may be applied again verbatim), the mid-flight-snapshot rule (the restored in-flight journal is owner-less; the next apply takes a new generation and reconciles), and the drift-detection refusal for data/journal inconsistency; §7.1 gained `schema_migration_restored_backup_rewinds_journal_and_reapplies`.
3. **MODERATE (fixed). The R3 logical-import case was missing.** §2.3 blocked “export without journal metadata counts as upgraded” but not import INTO an existing journal, nor identity mismatch after import under another name. Fixed: §2.3 boundary rule (refuse import into non-empty history; recorded identity must equal the target's), §5.3 gained `schema_migration_identity_mismatch`, §7.1 gained the corresponding test, §9 gained the open question.
4. **MODERATE (fixed). The format-epoch plumbing was unspecified and as written would invert a dependency.** §5.2 said “format epoch from the server's format stamp interface when available”, but the stamp module lives in `shamir-sync`, which sits above `shamir-db` (`docs/dev-artifacts/design/format-stamp.md:117`), so a handler inside `shamir-db` cannot read it. Fixed: §5.2 specifies injection at initialization (the direction the durability policy already travels, `store.rs:153-187`), records the absent/`adopted` state, keeps embedded users on the explicit unknown state, and adds no upward dependency.
5. **MODERATE (fixed). §3.2 ignored the existing canonical v2 operation encoding.** `shamir-query-types` already canonicalizes typed operations for HMAC (`canonical_batch_op`/`canonical_db_request`; golden bytes `crates/query/shamir-query-types/src/security/tests/hmac_tests.rs:265-317`; DTO docs referencing “canonical v2 operation encoding” at `table_ops.rs:37` and `index_ops.rs:247`) with a TS mirror (`crates/client/shamir-client-ts/src/core/protocol/hmac.ts:9-41`). Fixed: §3.2 states the relationship — default is to reuse the existing per-operation encodings inside a new versioned body container; a second independent encoding is rejected by default.
6. **MODERATE (fixed). Reconcile/resume had no named server operation.** §4.2/§4.3 required explicit resume under a new generation, but §5.2 defined only status/plan/apply. Fixed: §5.3 states resume/reconcile goes through `ApplyApplicationSchemaMigration` (identical stored body + expected predecessor + fresh generation; the server fences and reconciles per §4.2); no fourth operation.
7. **MINOR (fixed). The implementation plan collided with the 7-entry layout rule at nearly every natural home.** Counted from listings: `shamir-query-types` `src/admin/types/`, `.../types/db/`, `.../types/ddl/`; `shamir-db` `src/shamir_db/system_store/`, `.../execute/`, `.../execute/core/`, both test roots (`src/shamir_db/tests/`, `src/shamir_db/shamir_db/tests/`); `shamir-client` `src/` and `src/client/`; `shamir-client-ts` `src/core/`, `core/client/`, `core/builders/` — all exactly 7 entries; room exists in `system_store/catalogue/` (6), `execute/admin_ddl/` (5), `tests/ddl_contracts/` (6). Fixed: §7.1 and §8 carry the counted constraint and the rule that the implementation brief must name a concrete home per file, with any split as its own commit.
8. **MINOR (fixed). Step 2 was not PR-sized.** Fixed: §8 lands it as two PRs (identity/journal/fencing, then the three typed operations + handlers + wiring); the heading says so. Also noted that R5 gains the shell commands later (`01-versioning-and-migrations.md:200-201`).
9. **MINOR (fixed). The database-identity “implementation verification point” was settled and the plan-letter divergence recorded.** No stable database ID exists (`db_management.rs:96-105` — `name` and `created_at` only), and rename is a pure re-key of every `db_name`-carrying row (`:159-185`), so journal rows must be identity-keyed. Fixed in §2.1; §9's default updated. The physical placement (shared system catalogue vs the plan's “system table in the database itself”, `01-versioning-and-migrations.md:165-166`) is stated as a deliberate refinement, with the R3 obligation to carry the journal partition on selective export.
10. **MINOR (fixed). The maintenance fence now names stored-procedure writes** (`CallOp`-issued writes must hit the migration gate, §4.3), and §7.1 gained the determinism note (crash points run through the existing fault gates, `store.rs:73-77,120-145`, not timing races) plus three missing tests (restore-rewind, import/identity, server-wide serialization).
11. **MINOR (fixed). Citation drift.** `batch.rs:510-538` → `510-540` (`list_validators` ends at 540), `batch.rs:738-744` → `738-745`, `validator.rs:129-261` → `129-270` (the `list_validators` builder spans 263-270).
12. **NOTE.** The R1b design (`migration-framework.md`) resolves the same “backup taken mid-flight” question for format migrations the same way (journal and stamp travel together); no conflict between the three designs was found. `logical-dump.md` does not exist; R3 lives at `docs/dev-artifacts/release-1/03-logical-dump-restore.md`, whose manifest requirement (format epoch + application schema version) matches §2.3.

### Left open for the owner

- Server-wide single-migration default (finding 1): relax to per-database for bodies without global registrations only with an owner ruling.
- R3 journal-import rule (finding 3): confirm refuse-by-default before the R3 brief is written.
- The existing §9 open questions are unchanged except the stable-identity default, which now records the settled verification point.
