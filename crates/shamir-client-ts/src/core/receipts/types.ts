/**
 * Typed per-alias receipts of implicit (non-transactional) batches and the
 * typed `changes_since` read (VIS-5b).
 *
 * PLATFORM-AGNOSTIC.
 */

import type { TransactionInfo } from '../types/write/batch.js';
import type { VisibilityState } from '../visibility/types.js';

/** One write alias's implicit commit receipt. */
export interface AliasReceipt {
  /** The batch alias this receipt belongs to. */
  alias: string;
  /** Validated committed transaction metadata (`error` carries any
   * published `wal_durability` warning, independently of visibility). */
  transaction: TransactionInfo;
  /** Settled-prefix observation at acknowledgement time. */
  visibility: VisibilityState;
  /** Repository the position belongs to, derived from the request's
   * table reference; `null` when it cannot be derived unambiguously
   * (nested sub-batch / for-each) — reconciliation refuses to guess. */
  repo: string | null;
}

/** One repository observation produced by `reconcileReceipts`. */
export interface RepoObservation {
  repo: string;
  /** The highest pending `commit_version` among that repo's receipts. */
  version: bigint;
  visibility: VisibilityState;
}

/** Record-level change inside a {@link ChangesSinceEvent}. On the wire
 * `key` travels as MessagePack bin (framing gives Uint8Array) while the
 * optional `value` travels as an ARRAY OF BYTE INTEGERS (the server's
 * `serde_opt_bytes_compat`); the decoder normalizes both to Uint8Array. */
export interface ChangesSinceRecordChange {
  table: string;
  key: Uint8Array;
  op: 'Put' | 'Delete';
  value: Uint8Array | null;
}

/** One committed transaction projected for the changelog journal. */
export interface ChangesSinceEvent {
  repo: string;
  commit_version: bigint;
  tx_id: bigint;
  actor: unknown;
  /** Best-effort projection wall clock; never used for ordering. */
  timestamp_ns: bigint;
  changes: ChangesSinceRecordChange[];
}

/** Typed `changes_since` result. `visible_through` is the settled prefix W
 * sampled BEFORE the journal read: a settlement sample, NOT a coverage
 * certificate — empty `events` is not proof a pending write aborted, and
 * the event cursor must not be advanced to `visible_through` from this
 * field alone. Positions decode exactly as `bigint`; note the server
 * currently casts them through `i64`, so values above `i64::MAX` are not
 * producible by the server today. */
export interface ChangesSinceResult {
  changes_since: bigint;
  events: ChangesSinceEvent[];
  gap_at: bigint | null;
  visible_through: bigint | null;
}
