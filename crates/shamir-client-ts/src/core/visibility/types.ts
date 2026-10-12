import type { TransactionInfo } from '../types/batch.js';

export type VisibilityWait =
  | { mode: 'strict' }
  | { mode: 'bounded'; timeout_ms: number | bigint };
export type VisibilityPendingReason = 'timeout' | 'reopen_required';
export type VisibilityState =
  | { state: 'visible' }
  | { state: 'pending'; visible_through: bigint; blocking_version: bigint; reason: VisibilityPendingReason };
export type TransactionResult<T> = {
  status: 'committed';
  value: T;
  transaction: TransactionInfo;
} & (
  | { state: 'visible'; visibility: Extract<VisibilityState, { state: 'visible' }> }
  | { state: 'pending'; visibility: Extract<VisibilityState, { state: 'pending' }> }
);
export interface TransactionOptions {
  repo?: string;
  isolation?: 'snapshot' | 'serializable';
  visibilityWait?: VisibilityWait;
}
