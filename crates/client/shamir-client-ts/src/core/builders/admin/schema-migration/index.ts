/**
 * Public barrel for the application-schema migration module
 * (`src/core/builders/admin/schema-migration/`). Re-exports only —
 * no logic lives here.
 *
 * Explicit named re-exports (not `export *`) to avoid collisions with
 * sibling admin builders; `U64` and internal helpers stay private.
 *
 * Files under active delegation (`canonical/set.ts`, `canonical/body.ts`)
 * are re-exported from here as the public surface; their implementation
 * lands in a sibling task.
 */

// ── Values ──────────────────────────────────────────────────────────

// Migration client (composed over a DbHandle / ShamirClient).
export { ApplicationMigrationClient } from './client/client.js';

// Typed errors.
export { MigrationError } from './client/error.js';

// Typed migration-set authoring (model builders).
export { schemaMigration, migrationDdl, migrationFloat } from './model/index.js';

// Canonical bytes and checksum (SHA-256 over the canonical v1 encoding).
export { canonicalBody, checksum } from './canonical/body.js';

// The typed migration set (value export: construct with `new`).
export { ApplicationMigrationSet } from './canonical/set.js';

// Admin wire builders (exact Rust serde wire shape).
export {
  schemaMigrationSubmission,
  applicationSchemaStatus,
  applicationSchemaPlan,
  applyApplicationSchemaMigration,
} from './wire.js';

// ── Types ───────────────────────────────────────────────────────────

// The typed migration set and status-comparison types.
export type { MigrationStatus, PrefixCoverage } from './client/status.js';
export type {
  MigrationErrorCode,
  UnknownApply,
} from './client/error.js';
export type { UnknownApplyResolution } from './client/client.js';

// Body and protocol (wire DTO) type declarations.
export type {
  MigrationNumBound,
  MigrationFieldRuleDto,
  MigrationCreateTablePayload,
  MigrationSetTableSchemaPayload,
  MigrationAddSchemaRulePayload,
  MigrationCreateIndexPayload,
  MigrationLiteral,
  SchemaMigrationOp,
  SchemaMigrationMetadata,
  SchemaMigrationFunctionDependencies,
  SchemaMigrationBody,
  MigrationTableSchemaOptions,
} from '../../../types/schema/migration-body.js';
export type {
  ChecksumRequest,
  ChecksumResult,
  SchemaMigrationErrorCode,
  SchemaMigrationChecksumValidation,
  SchemaMigrationWarning,
  SchemaMigrationTarget,
  SchemaMigrationPredecessor,
  SchemaMigrationMetadataWire,
  SchemaMigrationSubmission,
  SchemaMigrationStatusRequest,
  SchemaMigrationPlanRequest,
  SchemaMigrationApplyRequest,
  ApplicationSchemaOperation,
  ApplicationSchemaEntry,
  SchemaMigrationAdminOp,
  SchemaMigrationAppliedEntryResult,
  SchemaMigrationInFlightResult,
  SchemaMigrationStatusResult,
  SchemaMigrationPlannedOperationResult,
  SchemaMigrationMetadataResult,
  SchemaMigrationPredecessorResult,
  SchemaMigrationPlanResult,
  SchemaMigrationApplyResult,
} from '../../../types/schema/migration-protocol.js';
