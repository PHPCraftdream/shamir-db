/**
 * Execution context — neutral binding interface shared by `Query`, `Batch`,
 * and `Db` without creating import cycles.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { BatchResponse } from './types/batch.js';

/** Internal binding passed to bound Query / Batch instances. */
export interface ExecCtx {
  exec(batch: object): Promise<BatchResponse>;
}
