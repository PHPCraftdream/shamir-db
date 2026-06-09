/**
 * Call-operation builder — the CODE that constructs the wire shape
 * declared in `../types/call.ts`. Mirrors
 * `crates/shamir-query-types/src/call/mod.rs`.
 *
 * Provides an ergonomic constructor:
 *   - `call(name, params?, opts?)` → CallOp
 *
 * PLATFORM-AGNOSTIC.
 */

import type { FilterValue } from '../types/filter.js';
import type { CallOp } from '../types/call.js';

/**
 * Build a `CallOp`. `repo` is always emitted (default "main").
 * `params` is included only when a non-empty array is given.
 */
export function call(
  name: string,
  params?: FilterValue[],
  opts?: { repo?: string },
): CallOp {
  const op: CallOp = {
    call: name,
    repo: opts?.repo ?? 'main',
  };
  if (params !== undefined && params.length > 0) {
    op.params = params;
  }
  return op;
}
