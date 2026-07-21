/**
 * Typed DB error surface for the TS client.
 *
 * The server sends a rich error-code vocabulary in `DbResponse::Error { code,
 * message }` (see `crates/shamir-query-types/src/wire/db_message.rs` and the
 * `shamir-server` `db_handler`). Historically the TS ws-client collapsed this
 * into a single interpolated `Error` string, forcing callers to regex the
 * message to tell a retryable failure (timeout / lock / tx-conflict) from a
 * fatal one (validation / permission). {@link ShamirDbError} preserves the
 * typed `code` and a `retryable` classification so callers branch on
 * properties, not string matching.
 *
 * PLATFORM-AGNOSTIC.
 */

/**
 * Server error codes that are transient — the same request may succeed on a
 * retry (possibly after a backoff, or against the leader for
 * `read_only_replica`). This is the single canonical classification consumed by
 * both the ws-client and (mirrored in Rust) the node binding.
 *
 * - `timeout` / `lock_timeout` — the op exceeded its time / lock budget.
 * - `tx_conflict` — MVCC/SSI write-conflict; retry the transaction.
 * - `read_only_replica` — the write hit a read-only node; retry against the
 *   leader (a redirect, semantically retryable).
 */
export const RETRYABLE_ERROR_CODES: ReadonlySet<string> = new Set([
  'timeout',
  'lock_timeout',
  'tx_conflict',
  'read_only_replica',
]);

/** `true` if `code` is a transient failure worth retrying. */
export function isRetryableCode(code: string): boolean {
  return RETRYABLE_ERROR_CODES.has(code);
}

/**
 * `true` if `code` is an optimistic-concurrency (CAS) version conflict
 * (FG-2). The caller should re-read the current version, then retry the
 * write with the fresh `expected_version`. NOT in
 * {@link RETRYABLE_ERROR_CODES} because a blind retry without re-reading
 * would fail identically — the caller MUST re-read first.
 */
export function isVersionConflict(err: unknown): boolean {
  return (
    err instanceof ShamirDbError && err.code === 'version_conflict'
  );
}

/**
 * A typed DB-layer error carrying the server's `code` and a `retryable`
 * classification. Thrown/rejected in place of a plain `Error` for every
 * `DbResponse::Error` the server returns.
 *
 * `message` keeps the human-readable `db error [code]: message` form for
 * logging continuity, but callers should branch on `.code` / `.retryable`.
 */
export class ShamirDbError extends Error {
  /** Server error code (e.g. `"timeout"`, `"validation"`, `"tx_conflict"`). */
  readonly code: string;
  /** Human-readable detail returned by the server (without the code prefix). */
  readonly detail: string;
  /** Whether retrying the same request may succeed (see {@link isRetryableCode}). */
  readonly retryable: boolean;

  constructor(code: string, detail: string) {
    super(`db error [${code}]: ${detail}`);
    this.name = 'ShamirDbError';
    this.code = code;
    this.detail = detail;
    this.retryable = isRetryableCode(code);
    // Restore prototype chain (TS target < ES2015 interop safety).
    Object.setPrototypeOf(this, ShamirDbError.prototype);
  }
}

/**
 * Raised when a pending request or a connection attempt exceeds its client-side
 * deadline (Finding 2.2). The server bounds only `Execute` via
 * `max_execution_time_secs` — `Ping`, `TxCommit`, `CreateScramUser`, or a lost
 * response id would otherwise hang the caller forever. Carries a stable
 * `code: 'client_timeout'` and is always `retryable` (the operation may succeed
 * on a fresh attempt / connection).
 */
export class ShamirTimeoutError extends Error {
  readonly code = 'client_timeout';
  readonly retryable = true;
  /** `"request"` (no response within `requestTimeoutMs`) or `"connect"`. */
  readonly phase: 'request' | 'connect';

  constructor(phase: 'request' | 'connect', timeoutMs: number) {
    super(
      phase === 'connect'
        ? `connection attempt timed out after ${timeoutMs}ms`
        : `request timed out after ${timeoutMs}ms (no server response)`,
    );
    this.name = 'ShamirTimeoutError';
    this.phase = phase;
    Object.setPrototypeOf(this, ShamirTimeoutError.prototype);
  }
}
