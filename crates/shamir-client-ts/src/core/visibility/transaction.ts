import type { ShamirClient } from '../client/client.js';
import { Tx } from '../client/db.js';
import { ShamirDbError } from '../support/errors.js';
import { validateTransactionVisibility } from './positions.js';
import type { TransactionOptions, TransactionResult } from './types.js';

/** Commit attempts consume the handle; unknown acknowledgement never triggers rollback/replay. */
export async function runTransaction<T>(
  client: ShamirClient,
  db: string,
  fn: (tx: Tx) => Promise<T>,
  opts?: TransactionOptions,
): Promise<TransactionResult<T>> {
  const opened = await client.txBegin(db, opts?.repo ?? 'main', opts?.isolation);
  let commitAttempted = false;
  try {
    const value = await fn(new Tx(client, db, opened.tx_handle));
    commitAttempted = true;
    const transaction = await client.txCommit(db, opened.tx_handle, opts?.visibilityWait);
    validateTransactionVisibility(transaction);
    if (transaction.status === 'aborted') {
      throw Object.assign(new ShamirDbError(transaction.reason ?? 'tx_error',
        `transaction aborted: ${transaction.reason ?? 'unknown'}`), { transaction });
    }
    if (transaction.status === 'indeterminate') {
      throw Object.assign(new ShamirDbError(transaction.reason ?? 'wal_requires_reopen',
        'transaction outcome indeterminate: WAL result unknown; reopen and reconcile, do not replay writes'), { transaction });
    }
    const visibility = transaction.visibility;
    if (visibility === undefined) throw new Error('missing committed visibility');
    if (visibility.state === 'pending') {
      return { status: 'committed', state: 'pending', value, transaction, visibility };
    }
    return { status: 'committed', state: 'visible', value, transaction, visibility };
  } finally {
    if (!commitAttempted) await client.txRollback(db, opened.tx_handle).catch(() => {});
  }
}
