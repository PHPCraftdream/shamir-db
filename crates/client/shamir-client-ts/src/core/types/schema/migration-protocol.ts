/**
 * Application-schema migration protocol DTOs — type-only mirror of
 * `crates/query/shamir-query-types/src/admin/types/schema_migration/protocol/`
 * (`request.rs`, `status.rs`, `plan.rs`, `apply.rs`, `admin_op.rs`) plus the
 * server's manual `QueryValue` result encodings in
 * `crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/`.
 *
 * REQUEST side (`SchemaMigrationStatusRequest`, `SchemaMigrationPlanRequest`,
 * `SchemaMigrationApplyRequest`, the submission and the tagged admin op)
 * follows serde exactly:
 *   - `u64` ordinals/fence generations accept `number | bigint` (the
 *     package's u64 convention; msgpack round-trips both).
 *   - 32-byte checksums are a FIXED wire ARRAY of 32 numbers (serde's
 *     fixed-array encoding) — NOT a binary blob. Binary appears only on the
 *     RESPONSE side (see below).
 *   - `migration_id` / `function_dependencies` / result-optionals are
 *     `skip_serializing_if` → omitted when unset or empty; the builders in
 *     `../../builders/admin/schema-migration/wire.ts` enforce that.
 *   - `SchemaMigrationOp` is `#[serde(tag = "operation",
 *     content = "payload", rename_all = "snake_case")]` → every operation is
 *     `{ operation, payload }` (types come from `./migration-body.js`).
 *   - These operations carry NO session-HMAC tag in Rust (the
 *     `BatchOp::ApplicationSchema` arm adds none) — no outer HMAC here.
 *
 * RESPONSE side: the server builds results manually as `QueryValue` maps
 * (NOT serde), so results differ from the serde DTOs in exactly these ways:
 *   - every checksum is `QueryValue::Bin` → msgpack binary → `Uint8Array`;
 *   - every integer is `QueryValue::Int` → `number | bigint` per u64
 *     convention;
 *   - the additive `in_flight.in_doubt_reason` key (R1c 2b-3b) is present
 *     only when the unresolved event carries evidence;
 *   - `applied` (the whole applied prefix) and `fence_generation` are
 *     additive (R1c 3a): absent from older servers.
 *
 * The batch-entry wire shape is produced by `BatchOp::ApplicationSchema`'s
 * custom Serialize: a three-key map
 * `{ application_schema: true, operation, payload }` where `operation` is
 * the snake_case arm name (`application_schema_status`,
 * `application_schema_plan`, `apply_application_schema_migration`) — the
 * `application_schema: true` key is routing data the server strips before
 * typed decoding.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  SchemaMigrationFunctionDependencies,
  SchemaMigrationMetadata,
  SchemaMigrationOp,
} from './migration-body.js';

// ── Shared scalars ──────────────────────────────────────────────────

/** u64 on the wire: small values decode as `number`, larger as `bigint`. */
export type U64 = number | bigint;

/**
 * A 32-byte SHA-256 body checksum as it travels in a REQUEST: serde's
 * fixed-array encoding — a plain array of 32 byte values. (Responses carry
 * binary instead; see `SchemaMigrationChecksumResult`.)
 */
export type ChecksumRequest = readonly number[];

/** A 32-byte checksum as it comes back in a RESULT: `QueryValue::Bin`. */
export type ChecksumResult = Uint8Array;

/**
 * Stable wire error identifiers from design §5.3
 * (`SchemaMigrationErrorCode`, `rename_all = "snake_case"`).
 */
export type SchemaMigrationErrorCode =
  | 'schema_migration_in_progress'
  | 'schema_migration_in_doubt'
  | 'schema_migration_checksum_mismatch'
  | 'schema_migration_gap'
  | 'schema_migration_predecessor_mismatch'
  | 'schema_migration_follower_read_only'
  | 'schema_migration_durability_unavailable'
  | 'schema_migration_postcondition_failed'
  | 'schema_migration_identity_mismatch';

/** `SchemaMigrationChecksumValidation` (`rename_all = "snake_case"`). */
export type SchemaMigrationChecksumValidation =
  | 'valid'
  | 'missing_body'
  | 'mismatch'
  | 'unknown_version'
  | 'not_checked';

/** `SchemaMigrationWarning` (`rename_all = "snake_case"`). */
export type SchemaMigrationWarning =
  | 'server_global_registration_blast_radius'
  | 'maintenance_fence_required'
  | 'recovery_requires_postcondition'
  | 'dependency_list_must_be_reviewed'
  | 'data_dml_not_admitted';

// ── Request-side DTOs (serde-exact) ─────────────────────────────────

/**
 * Database-scoped application-schema address, keyed by stable identity
 * (`request.rs::SchemaMigrationTarget`; `database_identity` travels as the
 * canonical base58 RecordId string).
 */
export interface SchemaMigrationTarget {
  database_identity: string;
}

/**
 * Identity of the applied predecessor expected by a plan/apply request
 * (`request.rs::SchemaMigrationPredecessor`; `migration_id` is
 * `skip_serializing_if = "Option::is_none"` → omitted when unset).
 */
export interface SchemaMigrationPredecessor {
  migration_id?: string;
  ordinal: U64;
  checksum: ChecksumRequest;
}

/** Metadata as authored in a body (serde names; see `./migration-body.js`). */
export type SchemaMigrationMetadataWire = SchemaMigrationMetadata;

/**
 * Typed body plus its server-recomputed digest, as it travels in a
 * plan/apply REQUEST (`request.rs::SchemaMigrationSubmission`).
 * `function_dependencies` is `skip_serializing_if = "Vec::is_empty"` —
 * the builders omit it when the body has none.
 */
export interface SchemaMigrationSubmission {
  metadata: SchemaMigrationMetadataWire;
  function_dependencies?: readonly SchemaMigrationFunctionDependencies[];
  operations: readonly SchemaMigrationOp[];
  checksum: ChecksumRequest;
}

/** `status.rs::SchemaMigrationStatusRequest`. */
export interface SchemaMigrationStatusRequest {
  protocol_version: number;
  target: SchemaMigrationTarget;
}

/** `plan.rs::SchemaMigrationPlanRequest`. */
export interface SchemaMigrationPlanRequest {
  protocol_version: number;
  target: SchemaMigrationTarget;
  expected_predecessor: SchemaMigrationPredecessor;
  migrations: readonly SchemaMigrationSubmission[];
}

/** `apply.rs::SchemaMigrationApplyRequest`. */
export interface SchemaMigrationApplyRequest {
  protocol_version: number;
  target: SchemaMigrationTarget;
  expected_predecessor: SchemaMigrationPredecessor;
  expected_fence_generation: U64;
  migration: SchemaMigrationSubmission;
}

// ── Batch-entry wire shape (`BatchOp::ApplicationSchema`) ───────────

/** Discriminator the server routes on and strips before typed decoding. */
export type ApplicationSchemaOperation =
  | 'application_schema_status'
  | 'application_schema_plan'
  | 'apply_application_schema_migration';

/**
 * Typed single-operation union (arm names, snake_case per `admin_op.rs`).
 */
export type SchemaMigrationAdminOp =
  | {
      operation: 'application_schema_status';
      payload: SchemaMigrationStatusRequest;
    }
  | {
      operation: 'application_schema_plan';
      payload: SchemaMigrationPlanRequest;
    }
  | {
      operation: 'apply_application_schema_migration';
      payload: SchemaMigrationApplyRequest;
    };

/** The exact wire map `BatchOp::ApplicationSchema` serializes to. */
export type ApplicationSchemaEntry = { application_schema: true } & SchemaMigrationAdminOp;

// ── Response-side DTOs (server manual `QueryValue` encoding) ────────

/** `status.rs::SchemaMigrationAppliedEntry` as the server emits it. */
export interface SchemaMigrationAppliedEntryResult {
  id: string;
  ordinal: U64;
  checksum: ChecksumResult;
}

/**
 * In-flight owner as the server emits it (`status.rs::
 * SchemaMigrationInFlight` plus the additive `in_doubt_reason` evidence key
 * of R1c 2b-3b — present only for in-doubt/failed events that carry
 * evidence).
 */
export interface SchemaMigrationInFlightResult {
  migration_id: string;
  ordinal: U64;
  operation_index?: U64;
  fence_generation: U64;
  status: string;
  in_doubt_reason?: string;
}

/**
 * `status.rs::SchemaMigrationStatusResult` as the server emits it.
 * `fence_generation` is the next fresh apply's
 * `expected_fence_generation` (`0` for an empty journal); both it and
 * `applied` are absent from servers older than R1c 3a.
 */
export interface SchemaMigrationStatusResult {
  protocol_version: U64;
  target: SchemaMigrationTarget;
  last_applied_id?: string;
  last_applied_ordinal: U64;
  last_applied_checksum?: ChecksumResult;
  checksum_validation: SchemaMigrationChecksumValidation;
  in_flight?: SchemaMigrationInFlightResult;
  error?: SchemaMigrationErrorCode;
  fence_generation: U64;
  applied?: readonly SchemaMigrationAppliedEntryResult[];
}

/** `plan.rs::SchemaMigrationPlannedOperation` as the server emits it. */
export interface SchemaMigrationPlannedOperationResult {
  operation_index: U64;
  operation_kind: string;
  target: string;
  work_bound?: U64;
  dependent_functions?: readonly string[];
  affected_database_registrations?: readonly string[];
}

/** Metadata fields of a plan result (`plan.rs`, manual encoding). */
export interface SchemaMigrationMetadataResult {
  id: string;
  ordinal: U64;
  display_name: string;
  canonical_version: U64;
}

/** Predecessor echoed back in a plan result (binary checksum). */
export interface SchemaMigrationPredecessorResult {
  migration_id?: string;
  ordinal: U64;
  checksum: ChecksumResult;
}

/** `plan.rs::SchemaMigrationPlanResult` as the server emits it. */
export interface SchemaMigrationPlanResult {
  protocol_version: U64;
  migration: SchemaMigrationMetadataResult;
  checksum: ChecksumResult;
  predecessor: SchemaMigrationPredecessorResult;
  operations: readonly SchemaMigrationPlannedOperationResult[];
  total_work_bound?: U64;
  warnings?: readonly SchemaMigrationWarning[];
  recovery_summary?: string;
  requires_server_wide_fence?: boolean;
}

/** `apply.rs::SchemaMigrationApplyResult` as the server emits it. */
export interface SchemaMigrationApplyResult {
  protocol_version: U64;
  migration_id: string;
  ordinal: U64;
  checksum: ChecksumResult;
  status: string;
  applied_at_unix_micros?: U64;
  fence_generation?: U64;
  error?: SchemaMigrationErrorCode;
}
