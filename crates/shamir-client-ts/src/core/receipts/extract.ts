/**
 * Per-alias receipt extraction from an implicit (non-transactional) batch
 * response, including NESTED results of sub-batch / for-each containers.
 * A suppressed write alias arrives as a row-less shell that still carries
 * `receipt` — it is read from the shell, never inferred from rows.
 *
 * PLATFORM-AGNOSTIC.
 */

import { normalizeVisibility, position } from '../visibility/positions.js';
import type { TransactionInfo, VisibilityState } from '../types/batch.js';
import type { AliasReceipt } from './types.js';

const MAX_U64 = 18446744073709551615n;

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/** Position parser for NESTED receipts only: the server's nested results
 * travel through `QueryValueSerializer`, which emits u64 values above
 * `i64::MAX` as `QueryValue::Big` — a DECIMAL STRING on the wire. Accepts
 * bigint / safe-integer number / decimal string with the same range
 * checks as `position()`; the top-level path keeps using `position()`. */
function nestedPosition(value: unknown): bigint {
  if (typeof value === 'string') {
    if (!/^\d+$/.test(value)) throw new Error('malformed u64 position');
    const result = BigInt(value);
    if (result < 0n || result > MAX_U64) throw new Error('malformed u64 position');
    return result;
  }
  return position(value);
}

/** Visibility mirror tolerant to decimal-string positions (nested only). */
function normalizeNestedVisibility(value: unknown): VisibilityState {
  const v = asRecord(value);
  if (v === null) throw new Error('malformed transaction visibility metadata');
  if (v.state === 'visible') return { state: 'visible' };
  if (v.state !== 'pending' || (v.reason !== 'timeout' && v.reason !== 'reopen_required')) {
    throw new Error('malformed transaction visibility metadata');
  }
  const floor = nestedPosition(v.visible_through);
  const blocker = nestedPosition(v.blocking_version);
  if (floor === MAX_U64 || blocker !== floor + 1n) {
    throw new Error('malformed transaction visibility metadata');
  }
  return { state: 'pending', visible_through: floor, blocking_version: blocker, reason: v.reason };
}

/** Validate one receipt: committed with visibility, lossless u64
 * positions. The TOP-level path uses the strict `position()` +
 * `normalizeVisibility()`; the NESTED path uses the tolerant
 * `nestedPosition()` + `normalizeNestedVisibility()` (decimal-string
 * `Big` positions). */
function receiptFrom(
  raw: unknown,
  alias: string,
  nested: boolean,
): { transaction: TransactionInfo; visibility: VisibilityState } {
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
  const parse = nested ? nestedPosition : position;
  const normalizeVis = nested ? normalizeNestedVisibility : normalizeVisibility;
  const visibility = normalizeVis(transaction.visibility);
  const normalized: TransactionInfo = { ...transaction };
  if (normalized.commit_version !== undefined) {
    normalized.commit_version = parse(normalized.commit_version);
  }
  if (normalized.snapshot_version !== undefined) {
    normalized.snapshot_version = parse(normalized.snapshot_version);
  }
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
 * container ONLY when the REQUEST entry is a sub-batch (`batch` key) or
 * a for-each (`for_each` key) — the inner request entries decide
 * recursion and the inner alias names (the server stores sub-batch
 * results in the container's `value` MAP and for-each results in its
 * `value` LIST; see `query_runner.rs`). Nested receipts surface with a
 * path-style alias (`outer.inner`, `loop[3].w`) and `repo: null`:
 * reconciliation refuses them rather than guess. The `value` of ANY
 * other op (reads, calls, writes) is application data and is never
 * inspected; a result that does not match the request shape is skipped,
 * never guessed. */
function pushResult(
  alias: string,
  result: unknown,
  entry: unknown,
  repo: string | null,
  out: AliasReceipt[],
): void {
  const record = asRecord(result);
  if (record === null) return;
  if (record.receipt !== undefined && record.receipt !== null) {
    const { transaction, visibility } = receiptFrom(
      record.receipt,
      alias,
      repo === null, // nested path
    );
    out.push({ alias, transaction, visibility, repo });
  }
  const op = asRecord(entry);
  if (op === null) return;
  if (op.batch !== undefined) {
    const innerQueries = asRecord(asRecord(op.batch)?.queries) ?? {};
    const valueMap = asRecord(record.value);
    if (valueMap === null) return; // shape mismatch: skip
    for (const [nested, nestedEntry] of Object.entries(innerQueries)) {
      pushResult(`${alias}.${nested}`, valueMap[nested], nestedEntry, null, out);
    }
    return;
  }
  if (op.for_each !== undefined) {
    const innerQueries = asRecord(asRecord(op.for_each)?.queries) ?? {};
    if (!Array.isArray(record.value)) return; // shape mismatch: skip
    record.value.forEach((iteration, i) => {
      const iter = asRecord(iteration);
      if (iter === null) return;
      for (const [nested, nestedEntry] of Object.entries(innerQueries)) {
        pushResult(`${alias}[${i}].${nested}`, iter[nested], nestedEntry, null, out);
      }
    });
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
    pushResult(alias, result, entry, entryRepo(entry), receipts);
  }
  return receipts;
}
