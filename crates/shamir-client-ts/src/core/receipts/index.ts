/**
 * Receipts barrel (VIS-5b).
 *
 * PLATFORM-AGNOSTIC.
 */

export { extractReceipts } from './extract.js';
export { reconcileReceipts } from './reconcile.js';
export { decodeChangesSince, requestChangesSince } from './changes-since.js';
export { executeWithReceipts } from './execute.js';
export type {
  AliasReceipt,
  ChangesSinceEvent,
  ChangesSinceRecordChange,
  ChangesSinceResult,
  RepoObservation,
} from './types.js';
