import type { TransactionInfo } from '../types/write/batch.js';
import type { VisibilityState, VisibilityWait } from './types.js';

export function position(value: unknown): bigint {
  if (typeof value === 'number' && (!Number.isSafeInteger(value) || value < 0)) {
    throw new Error('malformed u64 position');
  }
  if (typeof value !== 'number' && typeof value !== 'bigint') {
    throw new Error('malformed u64 position');
  }
  const result = BigInt(value);
  if (result < 0n || result > 18446744073709551615n) throw new Error('malformed u64 position');
  return result;
}

export function normalizeWait(wait: VisibilityWait): VisibilityWait {
  switch (wait.mode) {
    case 'strict': return wait;
    case 'bounded': return { mode: 'bounded', timeout_ms: position(wait.timeout_ms) };
  }
}

export function normalizeVisibility(value: VisibilityState): VisibilityState {
  if (value == null) throw new Error('malformed transaction visibility metadata');
  if (value.state === 'visible') return value;
  if (value.state !== 'pending' || (value.reason !== 'timeout' && value.reason !== 'reopen_required')) {
    throw new Error('malformed transaction visibility metadata');
  }
  const floor = position(value.visible_through);
  const blocker = position(value.blocking_version);
  if (floor === 18446744073709551615n || blocker !== floor + 1n) {
    throw new Error('malformed transaction visibility metadata');
  }
  return { ...value, visible_through: floor, blocking_version: blocker };
}

export function validateTransactionVisibility(transaction: TransactionInfo): void {
  if (transaction.status === 'indeterminate') {
    // Mirror Rust: visibility absent, reason absent, attempted commit_version
    // required (no success claim).
    if (transaction.visibility !== undefined || transaction.reason != null
      || transaction.commit_version === undefined) {
      throw new Error('malformed transaction visibility metadata');
    }
    transaction.commit_version = position(transaction.commit_version);
    if (transaction.snapshot_version !== undefined) transaction.snapshot_version = position(transaction.snapshot_version);
    return;
  }
  if (transaction.status === 'aborted') {
    if (transaction.visibility !== undefined) throw new Error('malformed transaction visibility metadata');
    return;
  }
  if (transaction.status !== 'committed' || transaction.reason != null || transaction.visibility == null) {
    throw new Error('malformed transaction visibility metadata');
  }
  if (transaction.commit_version !== undefined) transaction.commit_version = position(transaction.commit_version);
  if (transaction.snapshot_version !== undefined) transaction.snapshot_version = position(transaction.snapshot_version);
  transaction.visibility = normalizeVisibility(transaction.visibility);
}
