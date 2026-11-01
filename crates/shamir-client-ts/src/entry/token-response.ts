import type { BatchResponse, TransactionInfo } from '../core/types/write/batch.js';
import { validateTransactionVisibility } from '../core/visibility/positions.js';
import { TokenTransportError } from '../core/protocol/token/errors.js';

function bad(): never { throw new TokenTransportError('protocol_error', 'Malformed batch response'); }
function record(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) && !(value instanceof Uint8Array); }
function safeNumber(value: unknown): number {
  if (typeof value === 'bigint') value = value <= BigInt(Number.MAX_SAFE_INTEGER) && value >= 0n ? Number(value) : NaN;
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) return bad();
  return value;
}
function wire(value: unknown, depth = 0): boolean {
  if (depth > 64) return false;
  if (value === null || typeof value === 'string' || typeof value === 'boolean' || typeof value === 'bigint' || value instanceof Uint8Array) return true;
  if (typeof value === 'number') return Number.isFinite(value);
  if (Array.isArray(value)) return value.every((item) => wire(item, depth + 1));
  return record(value) && Object.values(value).every((item) => wire(item, depth + 1));
}
function u64(value: unknown): boolean { try { return typeof value === 'number' ? Number.isSafeInteger(value) && value >= 0 : typeof value === 'bigint' && value >= 0n && value <= 0xffff_ffff_ffff_ffffn; } catch { return false; } }
function validateTransaction(tx: unknown): tx is Record<string, unknown> {
  if (!record(tx) || !u64(tx.tx_id) || !['committed','aborted','indeterminate'].includes(String(tx.status)) || typeof tx.materialized !== 'boolean') return false;
  tx.tx_id = safeNumber(tx.tx_id);
  validateTransactionVisibility(tx as unknown as TransactionInfo);
  return true;
}
function validateDdlState(state: unknown): boolean {
  if (!record(state)) return false;
  switch (state.kind) {
    case 'in_progress': case 'unknown': return true;
    case 'succeeded': return state.completed_at !== undefined && (state.completed_at = safeNumber(state.completed_at)) >= 0;
    case 'succeeded_via_crash_recovery': return state.completed_at_restart !== undefined && (state.completed_at_restart = safeNumber(state.completed_at_restart)) >= 0;
    case 'failed': return typeof state.detail === 'string';
    default: return false;
  }
}

/** Validates an untrusted token batch response; binary and bigint WireValue legacy variants are preserved. */
export function validateTokenBatchResponse(value: Record<string, unknown>): BatchResponse {
  try {
    if (!record(value) || !Object.prototype.hasOwnProperty.call(value, 'id') || !wire(value.id) || !record(value.results) || !Array.isArray(value.execution_plan) || !value.execution_plan.every((row) => Array.isArray(row) && row.every((item) => typeof item === 'string'))) return bad();
    if (value.interner_delta !== undefined) return bad();
    if (value.execution_time_us === undefined) return bad();
    value.execution_time_us = safeNumber(value.execution_time_us);
    for (const query of Object.values(value.results)) {
      if (!record(query) || !Array.isArray(query.records) || !query.records.every((row) => record(row) && Object.values(row).every((cell) => wire(cell)))) return bad();
      if (query.value !== undefined && !wire(query.value)) return bad();
      if (query.stats !== undefined) {
        if (!record(query.stats) || typeof query.stats.index_used !== 'string' && query.stats.index_used !== null) return bad();
        for (const k of ['records_scanned','records_returned','execution_time_us']) {
          if (query.stats[k] === undefined) return bad();
          query.stats[k] = safeNumber(query.stats[k]);
        }
      }
      if (query.pagination !== undefined) {
        if (!record(query.pagination) || typeof query.pagination.has_next !== 'boolean' || typeof query.pagination.has_prev !== 'boolean') return bad();
        for (const k of ['total_count','total_pages','current_page','page_size']) if (query.pagination[k] !== undefined) query.pagination[k] = safeNumber(query.pagination[k]);
      }
      if (query.versions !== undefined && (!Array.isArray(query.versions) || !query.versions.every(u64))) return bad();
      if (query.skipped !== undefined && typeof query.skipped !== 'boolean') return bad();
      if (query.op_id !== undefined && typeof query.op_id !== 'string') return bad();
      if (query.ddl_status !== undefined && !validateDdlState(query.ddl_status)) return bad();
      if (query.receipt !== undefined && !validateTransaction(query.receipt)) return bad();
      if (query.corrupt_records !== undefined && (!Array.isArray(query.corrupt_records) || !query.corrupt_records.every((r) => record(r) && typeof r.table === 'string' && typeof r.id === 'string'))) return bad();
      if (query.explain !== undefined) {
        const explain = query.explain;
        if (!record(explain) || !['KeysetSeek','OrderLimitFast','Index2','IndexScan','SortedIndexScan','AndRangeIndexScan','CounterShortcut','MinMaxIndex','FullScan'].includes(String(explain.plan_type))) return bad();
        if (explain.index_used !== undefined && typeof explain.index_used !== 'string') return bad();
        if (explain.estimated_rows !== undefined) explain.estimated_rows = safeNumber(explain.estimated_rows);
      }
    }
    if (value.edge_provenance !== undefined) {
      if (!record(value.edge_provenance)) return bad();
      for (const deps of Object.values(value.edge_provenance)) if (!record(deps) || Object.values(deps).some((kind) => !['explicit','data_flow','both'].includes(String(kind)))) return bad();
    }
    if (value.transaction !== undefined && !validateTransaction(value.transaction)) return bad();
    return value as unknown as BatchResponse;
  } catch { return bad(); }
}
