/**
 * The typed migration error — the TypeScript mirror of
 * `crates/client/shamir-client/src/receipts/schema_migration/error.rs`
 * (`MigrationError`, `MigrationError::from_server`,
 * `is_deferred_message`) plus the attempt token of
 * `.../resolve.rs::UnknownApply`.
 *
 * Definitive server rejections preserve the stable §5.3 code and the
 * server message verbatim; nothing here retries or falls back. Every
 * variant carries `retryable = false`: the migration client API never
 * auto-retries a rejected or uncertain migration — a definitive refusal
 * must not be replayed and an UNKNOWN apply outcome must be reconciled
 * through the status query with the attempt token
 * (`resolve_unknown_apply`), never resent.
 *
 * `mapMigrationServerError(code, message)` mirrors
 * `MigrationError::from_server` exactly:
 *   - the old peer's unsupported-operation decoder report is recognized
 *     NARROWLY as `code === 'invalid_request'` with the exact message
 *     `Unknown operation type` → `unsupported_server`;
 *   - the server's deferred admission is recognized in EXACTLY its two
 *     observed shapes (raw `schema_migration_operation_deferred:` message
 *     prefix, or the coarse `query` code wrapping the
 *     `BatchError::QueryError` Display) → `deferred_operation`;
 *   - the nine §5.3 stable codes map one-to-one;
 *   - everything else falls back to `server`, preserving the original
 *     code and message (`server_code`).
 *
 * PLATFORM-AGNOSTIC.
 */

import type { SchemaMigrationErrorCode } from '../../../../types/schema/migration-protocol.js';

// ── Attempt token ───────────────────────────────────────────────────

/**
 * Opaque attempt token bound BEFORE an apply send: the database and
 * target identity, and the exact id / ordinal / canonical checksum of
 * the body that was in flight (`resolve.rs::UnknownApply`). Fields are
 * opaque to callers; they exist for diagnostics and for the
 * identical-body validation inside `resolve_unknown_apply`. The ordinal
 * is a `bigint` (u64 wire convention); the canonical checksum is the
 * lowercase-hex string of the 32 body bytes.
 */
export interface UnknownApply {
  /** Database the uncertain apply was sent to. */
  readonly db: string;
  /** Target database identity the apply named (base58 RecordId string). */
  readonly databaseIdentity: string;
  /** Migration id of the body that was in flight. */
  readonly migrationId: string;
  /** Ordinal of the body that was in flight. */
  readonly ordinal: bigint;
  /** Lowercase-hex canonical checksum of the body that was in flight. */
  readonly checksum: string;
}

// ── Typed error ─────────────────────────────────────────────────────

/** The exact raw prefix the deferred admission carries (design §5.3). */
const DEFERRED_PREFIX = 'schema_migration_operation_deferred:';

/** Local (client-side) error codes, disjoint from the §5.3 server set. */
export type MigrationLocalErrorCode =
  | 'invalid_set'
  | 'canonical'
  | 'client'
  | 'unsupported_server'
  | 'server'
  | 'deferred_operation'
  | 'shape_mismatch'
  | 'journal_unverified'
  | 'drift'
  | 'journal_beyond_set'
  | 'nothing_to_apply'
  | 'attempt_does_not_match_set'
  | 'unknown_outcome';

/**
 * Full code union of {@link MigrationError}: the nine §5.3 stable wire
 * codes (`SchemaMigrationErrorCode`) plus the local codes above.
 */
export type MigrationErrorCode = SchemaMigrationErrorCode | MigrationLocalErrorCode;

/**
 * Typed failure of the application-schema migration client API
 * (`MigrationError` in Rust, flattened onto a `code` discriminator — the
 * TS convention of this package). `message` always carries the verbatim
 * diagnostic; `retryable` is ALWAYS `false`.
 */
export class MigrationError extends Error {
  /** Discriminator: a §5.3 stable code or a local code (see above). */
  readonly code: MigrationErrorCode;
  /** Always `false`: migration errors are never auto-retried. */
  readonly retryable: false = false;
  /** Original server code on the `server` fallback (and `unsupported_server`), verbatim. */
  readonly serverCode?: string;
  /** The opaque attempt token of an UNKNOWN apply outcome. */
  readonly attempt?: UnknownApply;
  /** Pending operation index of an in-doubt journal event, when reported. */
  readonly operationIndex?: bigint;
  /** The server-recorded evidence of an in-doubt event, when reported. */
  readonly reason?: string;

  constructor(
    code: MigrationErrorCode,
    message: string,
    options?: {
      serverCode?: string;
      attempt?: UnknownApply;
      operationIndex?: bigint;
      reason?: string;
    },
  ) {
    super(message);
    this.name = 'MigrationError';
    this.code = code;
    this.serverCode = options?.serverCode;
    this.attempt = options?.attempt;
    this.operationIndex = options?.operationIndex;
    this.reason = options?.reason;
  }
}

/**
 * Map a definitive server rejection (`DbResponse::Error` surfaced with its
 * code and message) into a {@link MigrationError}, preserving code+message
 * and narrowly recognizing the old peer's unsupported-operation decoder
 * text rather than every `invalid_request` — the exact mirror of
 * `MigrationError::from_server`.
 */
export function mapMigrationServerError(
  code: string,
  message: string,
): MigrationError {
  // The old peer's decoder reports the unknown typed operation with
  // exactly this message (its Syntax Display passes it through
  // unchanged); match it exactly, not every `invalid_request`.
  if (code === 'invalid_request' && message === 'Unknown operation type') {
    return new MigrationError('unsupported_server', message, { serverCode: code });
  }
  if (isDeferredMessage(code, message)) {
    return new MigrationError('deferred_operation', message, { serverCode: code });
  }
  switch (code) {
    case 'schema_migration_in_progress':
      return new MigrationError(code, message, { serverCode: code });
    case 'schema_migration_in_doubt':
      return new MigrationError(code, message, { serverCode: code });
    case 'schema_migration_checksum_mismatch':
      return new MigrationError(code, message, { serverCode: code });
    case 'schema_migration_gap':
      return new MigrationError(code, message, { serverCode: code });
    case 'schema_migration_predecessor_mismatch':
      return new MigrationError(code, message, { serverCode: code });
    case 'schema_migration_follower_read_only':
      return new MigrationError(code, message, { serverCode: code });
    case 'schema_migration_durability_unavailable':
      return new MigrationError(code, message, { serverCode: code });
    case 'schema_migration_postcondition_failed':
      return new MigrationError(code, message, { serverCode: code });
    case 'schema_migration_identity_mismatch':
      return new MigrationError(code, message, { serverCode: code });
    default:
      // Unrecognized code: preserve the ORIGINAL code and message.
      return new MigrationError('server', message, { serverCode: code });
  }
}

/**
 * True when `(code, message)` is the server's deferred admission, in
 * EXACTLY one of its two observed shapes — never a broad substring match:
 *
 * - RAW: the message itself starts with
 *   `schema_migration_operation_deferred:` (any code), or
 * - WRAPPED: the code is the coarse `query` and the message is the
 *   `BatchError::QueryError` Display of a deferred admission,
 *   `Query '<alias>' failed: schema_migration_operation_deferred: …`
 *   (shamir-query-types `batch_error.rs`), with the alias possibly empty.
 */
function isDeferredMessage(code: string, message: string): boolean {
  if (message.startsWith(DEFERRED_PREFIX)) {
    return true;
  }
  if (code !== 'query') {
    return false;
  }
  if (!message.startsWith("Query '")) {
    return false;
  }
  const separator = "' failed: ";
  const afterOpen = message.slice("Query '".length);
  const separatorIndex = afterOpen.indexOf(separator);
  if (separatorIndex === -1) {
    return false;
  }
  return afterOpen.slice(separatorIndex + separator.length).startsWith(DEFERRED_PREFIX);
}
