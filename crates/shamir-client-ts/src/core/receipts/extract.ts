/**
 * Per-alias receipt extraction from an implicit (non-transactional) batch
 * response, including NESTED results. A suppressed write alias arrives as
 * a row-less shell that still carries `receipt` — it is read from the
 * shell, never inferred from rows.
 *
 * PLATFORM-AGNOSTIC.
 */

import { normalizeVisibility, position } from '../visibility/positions.js';
import type { TransactionInfo, VisibilityState } from '../types/batch.js';
import type { AliasReceipt } from './types.js';

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/** Validate one receipt exactly like the transactional path: committed
 * with visibility, lossless u64 positions. */
function receiptFrom(raw: unknown, alias: string): {
  transaction: TransactionInfo;
  visibility: VisibilityState;
} {
  const transaction = raw as TransactionInfo;
  if (
    transaction === null ||
    typeof transaction !== 'object' ||
    transaction.status !== 'committed' ||
    transaction.reason != null ||
    transaction.visibility == null
  ) {
    throw new Error(`malformed receipt metadata for alias '${alias}'`);
  }
  const normalized: TransactionInfo = { ...transaction };
  if (normalized.commit_version !== undefined) {
    normalized.commit_version = position(normalized.commit_version);
  }
  if (normalized.snapshot_version !== undefined) {
    normalized.snapshot_version = position(normalized.snapshot_version);
  }
  const visibility = normalizeVisibility(normalized.visibility);
  normalized.visibility = visibility;
  return { transaction: normalized, visibility };
}

/** Derive the repository from a batch entry's write-op table reference:
 * `"table"` → "main", `["repo", "table"]` → repo. Anything else (nested
 * sub-batch, unknown shape) → `null`: refuse to guess. */
function entryRepo(entry: unknown): string | null {
  const op = asRecord(entry);
  if (op === null) return null;
  for (const key of ['insert_into', 'update', 'set', 'delete_from'] as const) {
    const ref = op[key];
    if (typeof ref === 'string') return 'main';
    if (Array.isArray(ref) && typeof ref[0] === 'string') return ref[0];
    if (ref !== undefined) return null;
  }
  return null;
}

/** Emit `alias`'s own receipt (if any) and descend into its `value`
 * container. Nested receipts (sub-batch: `value` is a MAP of
 * alias → serialized QueryResult; for-each: a LIST of iteration maps —
 * see `query_runner.rs`) surface with a path-style alias
 * (`outer.inner`, `loop[3].w`) and `repo: null`: reconciliation refuses
 * them rather than guess. */
function pushResult(
  alias: string,
  result: unknown,
  repo: string | null,
  out: AliasReceipt[],
): void {
  const record = asRecord(result);
  if (record === null) return;
  if (record.receipt !== undefined && record.receipt !== null) {
    const { transaction, visibility } = receiptFrom(record.receipt, alias);
    out.push({ alias, transaction, visibility, repo });
  }
  const value = record.value;
  if (Array.isArray(value)) {
    value.forEach((iteration, i) => {
      const iter = asRecord(iteration);
      if (iter === null) return;
      for (const [nested, nestedResult] of Object.entries(iter)) {
        pushResult(`${alias}[${i}].${nested}`, nestedResult, null, out);
      }
    });
    return;
  }
  const map = asRecord(value);
  if (map === null) return;
  for (const [nested, nestedResult] of Object.entries(map)) {
    pushResult(`${alias}.${nested}`, nestedResult, null, out);
  }
}

/** Extract every write alias's receipt. `results` is the raw decoded
 * `BatchResponse.results` — each entry is narrowed with runtime checks
 * inside this module (the typed `QueryResult` interface does not carry
 * the wire-only `receipt` field). */
export function extractReceipts(
  request: { queries: Record<string, unknown> },
  results: Readonly<Record<string, unknown>>,
): AliasReceipt[] {
  const receipts: AliasReceipt[] = [];
  for (const [alias, entry] of Object.entries(request.queries)) {
    const result = results[alias];
    if (result === undefined) continue;
    pushResult(alias, result, entryRepo(entry), receipts);
  }
  return receipts;
}
