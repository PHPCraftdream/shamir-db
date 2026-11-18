/**
 * Application-schema migration wire builders — mirror of the Rust wire
 * constructors in
 * `crates/query/shamir-query-builder/src/wire/schema_migration/admin_op.rs`
 * plus the request constructors
 * (`SchemaMigrationStatusRequest`, `SchemaMigrationPlanRequest::from_bodies`,
 * `SchemaMigrationApplyRequest::from_body`,
 * `SchemaMigrationSubmission::new`).
 *
 * Each function returns the exact three-key wire map that Rust's
 * `BatchOp::ApplicationSchema` custom Serialize emits
 * (`crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs::`
 * `serialize_application_schema`):
 *
 *     { application_schema: true, operation: <snake_case arm>, payload: {…} }
 *
 * Serde layout mirrored precisely:
 *   - `protocol_version` is always `1`; no session-HMAC tag exists on this
 *     arm in Rust, so none is attached here.
 *   - `SchemaMigrationSubmission::new` copies `metadata`,
 *     `function_dependencies` and `operations` from the body; the
 *     `skip_serializing_if = "Vec::is_empty"` attribute means an EMPTY
 *     dependency list is OMITTED from the wire (not `[]`).
 *   - `migration_id: Option<String>` is `skip_serializing_if` — omitted when
 *     unset (a fresh set with no applied predecessor uses
 *     `migrationId: undefined`, ordinal `0` and 32 zero bytes only when the
 *     server's status reported exactly that predecessor).
 *   - 32-byte checksums travel as a fixed wire ARRAY of 32 numbers (serde's
 *     fixed-array encoding — NOT msgpack binary). A `Uint8Array` input is
 *     accepted and normalized to the array form.
 *   - Operation order inside a body is authored order; nothing is sorted.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  ApplicationSchemaEntry,
  ChecksumRequest,
  SchemaMigrationAdminOp,
  SchemaMigrationApplyRequest,
  SchemaMigrationPlanRequest,
  SchemaMigrationPredecessor,
  SchemaMigrationStatusRequest,
  SchemaMigrationSubmission,
  SchemaMigrationTarget,
  U64,
} from '../../../types/schema/migration-protocol.js';
import type { SchemaMigrationBody } from '../../../types/schema/migration-body.js';

// ── Normalization helpers ───────────────────────────────────────────

/**
 * Normalize a checksum to the serde fixed-array wire form. `Uint8Array`
 * (the natural SHA-256 output) is copied element-wise into a plain number
 * array; an already-array checksum is validated byte-by-byte. A checksum
 * is always 32 bytes (SHA-256), each a value in 0..255.
 */
function checksumWire(checksum: ChecksumRequest | Uint8Array): number[] {
  const bytes = Array.from(checksum);
  if (bytes.length !== 32) {
    throw new TypeError(
      `checksum must be exactly 32 bytes (SHA-256), got ${bytes.length}`,
    );
  }
  for (const byte of bytes) {
    if (!Number.isInteger(byte) || byte < 0 || byte > 255) {
      throw new TypeError(`checksum byte out of range 0..255: ${String(byte)}`);
    }
  }
  return bytes;
}

/**
 * `SchemaMigrationSubmission::new`: body fields copied under serde names;
 * empty `function_dependencies` omitted (`skip_serializing_if`).
 */
export function schemaMigrationSubmission(
  body: SchemaMigrationBody,
  checksum: ChecksumRequest | Uint8Array,
): SchemaMigrationSubmission {
  const { metadata, function_dependencies, operations } = body;
  const wire: SchemaMigrationSubmission = {
    metadata,
    operations,
    checksum: checksumWire(checksum),
  };
  if (function_dependencies !== undefined && function_dependencies.length > 0) {
    wire.function_dependencies = function_dependencies;
  }
  return wire;
}

/** `SchemaMigrationPredecessor` wire form; `migration_id` omitted when unset. */
function predecessorWire(predecessor: SchemaMigrationPredecessor): SchemaMigrationPredecessor {
  const wire: SchemaMigrationPredecessor = {
    ordinal: predecessor.ordinal,
    checksum: checksumWire(predecessor.checksum),
  };
  if (predecessor.migration_id !== undefined) {
    wire.migration_id = predecessor.migration_id;
  }
  return wire;
}

/**
 * Wrap a typed tagged operation into the batch-entry wire map: the
 * `application_schema: true` discriminator is added on top of the exact
 * `{ operation, payload }` pair, so each operation keeps its own payload
 * type through the intersection.
 */
function entry(op: SchemaMigrationAdminOp): ApplicationSchemaEntry {
  return { application_schema: true, ...op };
}

// ── Builders (one per Rust wire constructor) ────────────────────────

/**
 * Read-only application-schema status for the database identified by
 * `database_identity` (design §5.2: requires `Read` on the database).
 * Mirrors `application_schema_status`.
 */
export function applicationSchemaStatus(target: SchemaMigrationTarget): ApplicationSchemaEntry {
  const payload: SchemaMigrationStatusRequest = {
    protocol_version: 1,
    target: { database_identity: target.database_identity },
  };
  return entry({ operation: 'application_schema_status', payload });
}

/**
 * Application-schema plan for the next ordinal: the server's cost/impact
 * estimate, never a write. `migrations` are the planned bodies with their
 * canonical SHA-256 checksums (contiguous ordinals from 1 — the server
 * re-validates the set). Mirrors `application_schema_plan` and
 * `SchemaMigrationPlanRequest::from_bodies`.
 */
export function applicationSchemaPlan(
  target: SchemaMigrationTarget,
  expectedPredecessor: SchemaMigrationPredecessor,
  migrations: readonly SchemaMigrationSubmission[],
): ApplicationSchemaEntry {
  const payload: SchemaMigrationPlanRequest = {
    protocol_version: 1,
    target: { database_identity: target.database_identity },
    expected_predecessor: predecessorWire(expectedPredecessor),
    migrations,
  };
  return entry({ operation: 'application_schema_plan', payload });
}

/**
 * Apply exactly ONE next migration: typed body + checksum, the expected
 * applied predecessor and the fence generation — both read from `status`
 * immediately before sending. Mirrors `apply_application_schema_migration`
 * and `SchemaMigrationApplyRequest::from_body`.
 */
export function applyApplicationSchemaMigration(
  target: SchemaMigrationTarget,
  expectedPredecessor: SchemaMigrationPredecessor,
  expectedFenceGeneration: U64,
  migration: SchemaMigrationSubmission,
): ApplicationSchemaEntry {
  const payload: SchemaMigrationApplyRequest = {
    protocol_version: 1,
    target: { database_identity: target.database_identity },
    expected_predecessor: predecessorWire(expectedPredecessor),
    expected_fence_generation: expectedFenceGeneration,
    migration,
  };
  return entry({ operation: 'apply_application_schema_migration', payload });
}
