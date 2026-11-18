/**
 * Strict wire decoder for the application-schema migration RESULTS —
 * the TypeScript counterpart of
 * `crates/client/shamir-client/src/receipts/schema_migration/` result
 * constructors, decoding the server's manual `QueryValue` maps (see
 * `../../../../types/schema/migration-protocol.js` for the exact shapes
 * the encoders in
 * `crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/`
 * produce).
 *
 * STRICTNESS: every field is checked for presence and type; unknown
 * error strings are rejected (never guessed); every u64 is validated as
 * a safe non-negative integer (`number | bigint`), normalized to
 * `bigint` — including small values — and range-checked against the
 * full u64 span (`0..2^64-1`); every checksum must be EXACTLY 32
 * bytes, arriving as
 * msgpack binary (`Uint8Array`) or a fixed array of 32 numbers, and is
 * normalized to `Uint8Array`.
 *
 * Additive R1c 3a fields (`applied`, `fence_generation`) and the R1c
 * 2b-3b `in_flight.fence_generation`:
 *   - a NONEMPTY status (any head tuple or in-flight owner) missing
 *     `applied` OR `fence_generation` is `unsupported_server` — never
 *     guessed (§5.1: a non-empty journal reported without the additive
 *     fields is an unsupported server, never assumed);
 *   - a TRULY empty journal (ordinal 0, no head, no in-flight) may omit
 *     both;
 *   - a legacy unresolved `in_flight` MUST carry its explicit flight
 *     fence (`in_flight.fence_generation`) — its absence is
 *     `unsupported_server`.
 *
 * Identity: `decodeMigrationStatus` takes the expected target identity.
 * When the record reports `error === 'schema_migration_identity_mismatch'`,
 * the decoded `target` echoes the SERVING identity from the record (the
 * caller's migration runner must FAIL the mutation on it); any other
 * target/expected divergence is a `shape_mismatch` failure.
 *
 * `protocol_version` must be exactly 1 (`protocol1`); anything else is
 * `unsupported_server`.
 *
 * PLATFORM-AGNOSTIC.
 */

import {
  MigrationError,
  type MigrationErrorCode,
} from './error.js';
import type {
  SchemaMigrationAppliedEntryResult as AppliedEntry,
  SchemaMigrationApplyResult,
  SchemaMigrationChecksumValidation,
  SchemaMigrationErrorCode,
  SchemaMigrationInFlightResult,
  SchemaMigrationMetadataResult,
  SchemaMigrationPlanResult,
  SchemaMigrationPlannedOperationResult,
  SchemaMigrationPredecessorResult,
  SchemaMigrationStatusResult,
  SchemaMigrationTarget,
  SchemaMigrationWarning,
} from '../../../../types/schema/migration-protocol.js';

// ── Decoded types ───────────────────────────────────────────────────

/**
 * The decoded status: additive `fence_generation` (normalized to
 * `bigint`) is always present — defaulted per the R1c 3a rules;
 */
export type DecodedMigrationStatus = Omit<
  SchemaMigrationStatusResult,
  'fence_generation' | 'applied'
> & {
  fence_generation: bigint;
  applied?: readonly AppliedEntry[];
};

// ── Strict scalar helpers ───────────────────────────────────────────

/** The nine stable §5.3 error strings — the ONLY accepted `error` values. */
const KNOWN_ERROR_CODES: readonly SchemaMigrationErrorCode[] = [
  'schema_migration_in_progress',
  'schema_migration_in_doubt',
  'schema_migration_checksum_mismatch',
  'schema_migration_gap',
  'schema_migration_predecessor_mismatch',
  'schema_migration_follower_read_only',
  'schema_migration_durability_unavailable',
  'schema_migration_postcondition_failed',
  'schema_migration_identity_mismatch',
];

/** Accepted `checksum_validation` verdict strings. */
const CHECKSUM_VALIDATIONS: readonly SchemaMigrationChecksumValidation[] = [
  'valid',
  'missing_body',
  'mismatch',
  'unknown_version',
  'not_checked',
];

/** Accepted `warnings` strings. */
const KNOWN_WARNINGS: readonly SchemaMigrationWarning[] = [
  'server_global_registration_blast_radius',
  'maintenance_fence_required',
  'recovery_requires_postcondition',
  'dependency_list_must_be_reviewed',
  'data_dml_not_admitted',
];

function shapeError(where: string, expected: string): MigrationError {
  return new MigrationError(
    'shape_mismatch',
    `migration result decode: ${where}: expected ${expected}`,
  );
}

function unsupported(where: string, detail: string): MigrationError {
  return new MigrationError(
    'unsupported_server',
    `migration result decode: ${where}: ${detail}`,
  );
}

/** Strict object check: a non-null, non-array plain record. */
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function object(record: unknown, where: string): Record<string, unknown> {
  if (!isRecord(record)) {
    throw shapeError(where, 'an object');
  }
  return record;
}

/** Strict string: must be present and a `string`. */
function string(record: Record<string, unknown>, key: string, where: string): string {
  const value = record[key];
  if (typeof value !== 'string') {
    throw shapeError(`${where}.${key}`, 'a string');
  }
  return value;
}

/** Optional strict string. */
function optionalString(
  record: Record<string, unknown>,
  key: string,
  where: string,
): string | undefined {
  const value = record[key];
  if (value === undefined) return undefined;
  if (typeof value !== 'string') {
    throw shapeError(`${where}.${key}`, 'a string or absence');
  }
  return value;
}

/** The inclusive u64 span: every decoded u64 must satisfy `0..MAX_U64`. */
const MAX_U64 = 2n ** 64n - 1n;

/**
 * Strict u64: a non-negative integer given as `number` or `bigint`,
 * normalized to `bigint` (small values included) and range-checked
 * against the full u64 span.
 */
function u64(value: unknown, where: string): bigint {
  let raw: bigint;
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value)) {
      throw shapeError(where, 'a u64 (safe integer number | bigint)');
    }
    raw = BigInt(value);
  } else if (typeof value === 'bigint') {
    raw = value;
  } else {
    throw shapeError(where, 'a u64 (number | bigint)');
  }
  if (raw < 0n || raw > MAX_U64) {
    throw shapeError(where, 'an integer in the u64 range 0..2^64-1');
  }
  return raw;
}

function optionalU64(
  record: Record<string, unknown>,
  key: string,
  where: string,
): bigint | undefined {
  const value = record[key];
  return value === undefined ? undefined : u64(value, `${where}.${key}`);
}

function u64Field(record: Record<string, unknown>, key: string, where: string): bigint {
  const value = record[key];
  if (value === undefined) {
    throw shapeError(`${where}.${key}`, 'a u64 (present)');
  }
  return u64(value, `${where}.${key}`);
}

/** Strict `string[]` (optional field, ordered). */
function optionalStringArray(
  record: Record<string, unknown>,
  key: string,
  where: string,
): readonly string[] | undefined {
  const value = record[key];
  if (value === undefined) return undefined;
  if (!Array.isArray(value) || value.some((item) => typeof item !== 'string')) {
    throw shapeError(`${where}.${key}`, 'an array of strings or absence');
  }
  return value as readonly string[];
}

// ── Checksums ───────────────────────────────────────────────────────

/**
 * Decode one 32-byte checksum arriving as msgpack binary
 * (`Uint8Array`) or a fixed array of 32 numbers, normalized to
 * `Uint8Array`. Anything else — wrong container, wrong length, out of
 * byte range — is a `shape_mismatch` failure.
 */
function checksum32(value: unknown, where: string): Uint8Array {
  let bytes: Uint8Array;
  if (value instanceof Uint8Array) {
    bytes = value;
  } else if (Array.isArray(value)) {
    if (
      value.some(
        (byte) =>
          typeof byte !== 'number' ||
          !Number.isInteger(byte) ||
          byte < 0 ||
          byte > 255,
      )
    ) {
      throw shapeError(where, '32 byte values (0..255)');
    }
    bytes = Uint8Array.from(value as number[]);
  } else {
    throw shapeError(where, 'binary or a 32-byte array');
  }
  if (bytes.length !== 32) {
    throw shapeError(where, 'exactly 32 bytes');
  }
  return bytes;
}

function optionalChecksum32(
  record: Record<string, unknown>,
  key: string,
  where: string,
): Uint8Array | undefined {
  const value = record[key];
  return value === undefined ? undefined : checksum32(value, `${where}.${key}`);
}

/**
 * Byte-for-byte comparison of two 32-byte checksums, accepting either
 * container on each side (binary or byte array).
 */
export function sameChecksum(
  a: Uint8Array | readonly number[],
  b: Uint8Array | readonly number[],
): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

// ── Strict enum-ish fields ──────────────────────────────────────────

function checksumValidation(
  record: Record<string, unknown>,
  where: string,
): SchemaMigrationChecksumValidation {
  const value = string(record, 'checksum_validation', where);
  if (!(CHECKSUM_VALIDATIONS as readonly string[]).includes(value)) {
    throw shapeError(`${where}.checksum_validation`, 'a known checksum_validation');
  }
  return value as SchemaMigrationChecksumValidation;
}

/**
 * The typed server error: ONLY the nine stable §5.3 strings are
 * accepted; an unknown string is a `shape_mismatch` failure, never a
 * guess.
 */
function errorField(
  record: Record<string, unknown>,
  where: string,
): SchemaMigrationErrorCode | undefined {
  const value = optionalString(record, 'error', where);
  if (value === undefined) return undefined;
  if (!(KNOWN_ERROR_CODES as readonly string[]).includes(value)) {
    throw shapeError(`${where}.error`, 'a known §5.3 error string');
  }
  return value as SchemaMigrationErrorCode;
}

// ── Status ──────────────────────────────────────────────────────────

function appliedEntry(value: unknown, where: string): AppliedEntry {
  const entry = object(value, where);
  return {
    id: string(entry, 'id', where),
    ordinal: u64Field(entry, 'ordinal', where),
    checksum: checksum32(entry.checksum, `${where}.checksum`),
  };
}

function appliedArray(
  record: Record<string, unknown>,
  where: string,
): readonly AppliedEntry[] | undefined {
  const value = record.applied;
  if (value === undefined) return undefined;
  if (!Array.isArray(value)) {
    throw shapeError(`${where}.applied`, 'an array or absence');
  }
  return value.map((entry, index) => appliedEntry(entry, `${where}.applied[${index}]`));
}

function inFlight(
  record: Record<string, unknown>,
  where: string,
): SchemaMigrationInFlightResult | undefined {
  const value = record.in_flight;
  if (value === undefined) return undefined;
  const flight = object(value, `${where}.in_flight`);
  const fence = optionalU64(flight, 'fence_generation', `${where}.in_flight`);
  if (fence === undefined) {
    // A legacy unresolved event MUST carry its explicit flight fence.
    throw unsupported(
      `${where}.in_flight`,
      'unresolved in-flight event without an explicit fence_generation',
    );
  }
  const inDoubtReason = optionalString(
    flight,
    'in_doubt_reason',
    `${where}.in_flight`,
  );
  const operationIndex = optionalU64(
    flight,
    'operation_index',
    `${where}.in_flight`,
  );
  return {
    migration_id: string(flight, 'migration_id', `${where}.in_flight`),
    ordinal: u64Field(flight, 'ordinal', `${where}.in_flight`),
    ...(operationIndex !== undefined ? { operation_index: operationIndex } : {}),
    fence_generation: fence,
    status: string(flight, 'status', `${where}.in_flight`),
    ...(inDoubtReason !== undefined ? { in_doubt_reason: inDoubtReason } : {}),
  };
}

/**
 * The additive R1c 3a fields. `fence_generation` is REQUIRED on the
 * decoded result and normalized to `bigint`:
 *   - present on the record → used as-is;
 *   - absent on a truly empty journal (ordinal 0, no head tuple, no
 *     in-flight owner) → `0n`;
 *   - absent with only an ordinal-0 in-flight head → the in-flight
 *     event's explicit flight fence (Rust's `durable_fence_generation`
 *     is the max over ALL journal events, unresolved included);
 *   - absent on any nonempty applied prefix → `unsupported_server`.
 * `applied` stays optional: a modern server omits the (empty) applied
 * array when there is no applied prefix — it is required only when a
 * nonempty applied ordinal/head is reported, whose absence is
 * `unsupported_server`.
 */
function additiveStatusFields(
  record: Record<string, unknown>,
  where: string,
  flightFence: bigint | undefined,
): { fence_generation: bigint; applied?: readonly AppliedEntry[] } {
  const fence = optionalU64(record, 'fence_generation', where);
  const applied = appliedArray(record, where);
  const appliedOrdinal = u64Field(record, 'last_applied_ordinal', where);
  const hasHead =
    appliedOrdinal !== 0n ||
    record.last_applied_id !== undefined ||
    record.last_applied_checksum !== undefined;
  const hasFlight = record.in_flight !== undefined;
  const hasAppliedPrefix = applied !== undefined && applied.length > 0;
  const trulyEmpty = appliedOrdinal === 0n && !hasHead && !hasAppliedPrefix && !hasFlight;
  if (!trulyEmpty && hasHead && (fence === undefined || applied === undefined)) {
    throw unsupported(
      where,
      'nonempty applied journal reported without the additive applied/fence_generation fields',
    );
  }
  let resolved = fence;
  if (resolved === undefined) {
    if (trulyEmpty) {
      resolved = 0n;
    } else if (!hasHead && flightFence !== undefined && flightFence > 0n) {
      resolved = flightFence;
    } else {
      throw unsupported(
        where,
        'nonempty status reported without the additive fence_generation field',
      );
    }
  }
  return {
    fence_generation: resolved,
    ...(applied !== undefined ? { applied } : {}),
  };
}

function protocol1(record: Record<string, unknown>, where: string): bigint {
  const version = u64Field(record, 'protocol_version', where);
  if (version !== 1n) {
    throw unsupported(`${where}.protocol_version`, `protocol ${version} is not supported`);
  }
  return version;
}

function decodeTarget(record: Record<string, unknown>, where: string): SchemaMigrationTarget {
  const target = object(record.target, `${where}.target`);
  return { database_identity: string(target, 'database_identity', `${where}.target`) };
}

/**
 * Strictly decode a `status` result record. `expectedTarget` is the
 * identity the request named. On
 * `error === 'schema_migration_identity_mismatch'` the decoded `target`
 * echoes the SERVING identity from the record (the runner must fail the
 * mutation on it); any other divergence between the record's target and
 * the expected identity is a `shape_mismatch` failure. Throws
 * {@link MigrationError} on every refusal; never guesses a field.
 */
export function decodeMigrationStatus(
  record: unknown,
  expectedTarget: SchemaMigrationTarget,
): DecodedMigrationStatus {
  const where = 'status';
  const value = object(record, where);
  const version = protocol1(value, where);
  const target = decodeTarget(value, where);
  const error = errorField(value, where);
  if (
    error !== 'schema_migration_identity_mismatch' &&
    target.database_identity !== expectedTarget.database_identity
  ) {
    throw shapeError(
      `${where}.target`,
      `the expected identity ${expectedTarget.database_identity}`,
    );
  }
  const lastAppliedId = optionalString(value, 'last_applied_id', where);
  const lastAppliedChecksum = optionalChecksum32(
    value,
    'last_applied_checksum',
    where,
  );
  const flight = inFlight(value, where);
  const flightFence =
    flight === undefined ? undefined : BigInt(flight.fence_generation);
  const additive = additiveStatusFields(value, where, flightFence);
  const decoded: DecodedMigrationStatus = {
    protocol_version: version,
    target,
    ...(lastAppliedId !== undefined ? { last_applied_id: lastAppliedId } : {}),
    last_applied_ordinal: u64Field(value, 'last_applied_ordinal', where),
    ...(lastAppliedChecksum !== undefined
      ? { last_applied_checksum: lastAppliedChecksum }
      : {}),
    checksum_validation: checksumValidation(value, where),
    ...(flight !== undefined ? { in_flight: flight } : {}),
    ...(error !== undefined ? { error } : {}),
    ...additive,
  };
  return decoded;
}

// ── Plan ────────────────────────────────────────────────────────────

function plannedOperation(value: unknown, where: string): SchemaMigrationPlannedOperationResult {
  const op = object(value, where);
  const workBound = optionalU64(op, 'work_bound', where);
  const dependentFunctions = optionalStringArray(
    op,
    'dependent_functions',
    where,
  );
  const affected = optionalStringArray(
    op,
    'affected_database_registrations',
    where,
  );
  return {
    operation_index: u64Field(op, 'operation_index', where),
    operation_kind: string(op, 'operation_kind', where),
    target: string(op, 'target', where),
    ...(workBound !== undefined ? { work_bound: workBound } : {}),
    ...(dependentFunctions !== undefined
      ? { dependent_functions: dependentFunctions }
      : {}),
    ...(affected !== undefined
      ? { affected_database_registrations: affected }
      : {}),
  };
}

function plannedOperations(
  record: Record<string, unknown>,
  where: string,
): readonly SchemaMigrationPlannedOperationResult[] {
  const value = record.operations;
  if (!Array.isArray(value)) {
    throw shapeError(`${where}.operations`, 'an array');
  }
  return value.map((op, index) => plannedOperation(op, `${where}.operations[${index}]`));
}

function metadataResult(
  record: Record<string, unknown>,
  where: string,
): SchemaMigrationMetadataResult {
  return {
    id: string(record, 'id', where),
    ordinal: u64Field(record, 'ordinal', where),
    display_name: string(record, 'display_name', where),
    canonical_version: u64Field(record, 'canonical_version', where),
  };
}

function predecessorResult(
  record: Record<string, unknown>,
  where: string,
): SchemaMigrationPredecessorResult {
  const migrationId = optionalString(record, 'migration_id', where);
  return {
    ...(migrationId !== undefined ? { migration_id: migrationId } : {}),
    ordinal: u64Field(record, 'ordinal', where),
    checksum: checksum32(record.checksum, `${where}.checksum`),
  };
}

/**
 * Strictly decode a `plan` result record (the server plan for the next
 * ordinal — never a write). Throws {@link MigrationError} on every
 * refusal.
 */
export function decodeMigrationPlan(
  record: unknown,
): SchemaMigrationPlanResult {
  const where = 'plan';
  const value = object(record, where);
  const version = protocol1(value, where);
  const migration = object(value.migration, `${where}.migration`);
  const predecessor = object(value.predecessor, `${where}.predecessor`);
  const totalWorkBound = optionalU64(value, 'total_work_bound', where);
  const warningsRaw = value.warnings;
  let warnings: readonly SchemaMigrationWarning[] | undefined;
  if (warningsRaw !== undefined) {
    if (
      !Array.isArray(warningsRaw) ||
      warningsRaw.some(
        (item) =>
          typeof item !== 'string' ||
          !(KNOWN_WARNINGS as readonly string[]).includes(item),
      )
    ) {
      throw shapeError(`${where}.warnings`, 'known warning strings or absence');
    }
    warnings = warningsRaw as readonly SchemaMigrationWarning[];
  }
  const recoverySummary = optionalString(value, 'recovery_summary', where);
  const fence = value.requires_server_wide_fence;
  if (fence !== undefined && typeof fence !== 'boolean') {
    throw shapeError(`${where}.requires_server_wide_fence`, 'a boolean or absence');
  }
  return {
    protocol_version: version,
    migration: metadataResult(migration, `${where}.migration`),
    checksum: checksum32(value.checksum, `${where}.checksum`),
    predecessor: predecessorResult(predecessor, `${where}.predecessor`),
    operations: plannedOperations(value, where),
    ...(totalWorkBound !== undefined ? { total_work_bound: totalWorkBound } : {}),
    ...(warnings !== undefined ? { warnings } : {}),
    ...(recoverySummary !== undefined ? { recovery_summary: recoverySummary } : {}),
    ...(fence !== undefined ? { requires_server_wide_fence: fence } : {}),
  };
}

// ── Apply ───────────────────────────────────────────────────────────

/**
 * Strictly decode an `apply` result record. The additive
 * `fence_generation` (R1c 3a) stays optional: it is present on current
 * servers and absent on older ones, and unlike the status result there
 * is no nonempty/empty distinction to guard it. Throws
 * {@link MigrationError} on every refusal.
 */
export function decodeMigrationApply(
  record: unknown,
): SchemaMigrationApplyResult {
  const where = 'apply';
  const value = object(record, where);
  const version = protocol1(value, where);
  const appliedAt = optionalU64(value, 'applied_at_unix_micros', where);
  const fence = optionalU64(value, 'fence_generation', where);
  const error = errorField(value, where);
  const status = string(value, 'status', where);
  const decoded: SchemaMigrationApplyResult = {
    protocol_version: version,
    migration_id: string(value, 'migration_id', where),
    ordinal: u64Field(value, 'ordinal', where),
    checksum: checksum32(value.checksum, `${where}.checksum`),
    status,
    ...(appliedAt !== undefined ? { applied_at_unix_micros: appliedAt } : {}),
    ...(fence !== undefined ? { fence_generation: fence } : {}),
    ...(error !== undefined ? { error } : {}),
  };
  return decoded;
}

// ── Runner guard ────────────────────────────────────────────────────

/**
 * True when a decoded record's typed error is EXACTLY the stable
 * identity-mismatch code. A mutation runner (plan / apply) must FAIL
 * the operation on it — the server answered for a different identity —
 * while the status view may echo the serving identity for diagnostics.
 */
export function isIdentityMismatch(
  error: MigrationErrorCode | SchemaMigrationErrorCode | undefined,
): boolean {
  return error === 'schema_migration_identity_mismatch';
}
