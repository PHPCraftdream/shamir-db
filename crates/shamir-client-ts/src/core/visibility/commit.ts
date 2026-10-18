import type { TransactionInfo } from '../types/write/batch.js';
import { CURRENT_QUERY_LANG_VERSION } from '../protocol/scram.js';
import { normalizeWait, validateTransactionVisibility } from './positions.js';
import type { VisibilityWait } from './types.js';

/** A single attempt; transport uncertainty is not a committed/pending receipt. */
export async function commitTransaction(
  send: (request: object) => Promise<Record<string, unknown>>,
  db: string,
  txHandle: number,
  wait?: VisibilityWait,
): Promise<TransactionInfo> {
  const visibilityWait = wait === undefined ? undefined : normalizeWait(wait);
  let response: Record<string, unknown>;
  try {
    response = await send({
      op: 'tx_commit', query_version: CURRENT_QUERY_LANG_VERSION, db, tx_handle: txHandle,
      ...(visibilityWait === undefined ? {} : { visibility_wait: visibilityWait }),
    });
  } catch (error) {
    if (error instanceof Error && 'phase' in error && error.phase === 'request') {
      Object.assign(error, { retryable: false, acknowledgement: 'unknown' });
    }
    throw error;
  }
  if (response.kind !== 'tx_committed' || response.transaction === undefined) {
    throw new Error(`unexpected DbResponse kind for tx_commit: ${response.kind}`);
  }
  const transaction = response.transaction as TransactionInfo;
  validateTransactionVisibility(transaction);
  return transaction;
}
