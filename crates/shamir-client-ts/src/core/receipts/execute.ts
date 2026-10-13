/**
 * `executeWithReceipts` — run a NON-transactional batch and extract
 * per-alias implicit commit receipts. Pending is a committed result:
 * never an error, never resent.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { BatchResponse } from '../types/batch.js';
import type { VisibilityWait } from '../visibility/types.js';
import { extractReceipts } from './extract.js';
import type { AliasReceipt } from './types.js';

interface ExecClient {
  execute(db: string, batch: object, visibilityWait?: VisibilityWait): Promise<BatchResponse>;
}

export async function executeWithReceipts(
  client: ExecClient,
  db: string,
  batch: object,
  visibilityWait?: VisibilityWait,
): Promise<{ response: BatchResponse; receipts: AliasReceipt[] }> {
  // batch.transactional is never touched: implicit commits keep their
  // per-alias receipts; an explicitly transactional batch carries a
  // single top-level receipt instead.
  let response: BatchResponse;
  try {
    response = await client.execute(db, batch, visibilityWait);
  } catch (error) {
    // Same classification as the transactional commit adapter: a request
    // failure after send leaves the batch acknowledgements UNKNOWN —
    // never retryable, never auto-resent.
    if (error instanceof Error && 'phase' in error && error.phase === 'request') {
      Object.assign(error, { retryable: false, acknowledgement: 'unknown' });
    }
    throw error;
  }
  // QueryResult (types/batch.ts) has no typed `receipt` field, but the
  // wire object does — hand the raw results to the runtime-narrowed
  // extractor (the brief forbids widening the over-limit interface).
  const results = response.results as Readonly<Record<string, unknown>>;
  const queries = (batch as { queries?: Record<string, unknown> }).queries ?? {};
  return { response, receipts: extractReceipts({ queries }, results) };
}
