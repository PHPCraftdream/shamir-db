import { CursorIterator } from '../../core/client/cursor-iterator.js';
import { validateTokenBatchResponse } from '../token-response.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';
import type { ReadQuery } from '../../core/types/read/query.js';
import { createCursor, fetchNext, cancelCursor } from '../../core/builders/read/cursor.js';
import type { WireValue } from '../../core/types/write/write.js';

export type TokenCursorRequest = (request: object) => Promise<Record<string, unknown>>;
function record(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) && !(value instanceof Uint8Array); }
type CursorRow = Record<string, WireValue>;
function validId(value: unknown): boolean { return typeof value === 'bigint' ? value >= 0n && value <= 0xffff_ffff_ffff_ffffn : typeof value === 'number' && Number.isSafeInteger(value) && value >= 0; }
export class TokenCursor extends CursorIterator {
  private terminal = false;
  private terminalCallbacks = new Set<() => void>();
  private lifecycleUnsubscribe: (() => void) | undefined;
  constructor(private readonly send: TokenCursorRequest, db: string, query: ReadQuery | { build(): ReadQuery }, pageSize: number) {
    super(async (request) => {
      if (this.terminal) throw new TokenTransportError('connection_closed', 'Token cursor is closed');
      const op = (request as { op?: unknown }).op;
      let wire: object;
      if (op === 'create_cursor') wire = { ...createCursor(db, query, pageSize), query_version: 4 };
      else if (op === 'fetch_next') wire = fetchNext((request as { cursor_id: never }).cursor_id, (request as { page_size?: number }).page_size);
      else wire = cancelCursor((request as { cursor_id: never }).cursor_id);
      const response = await this.send(wire);
      if (this.terminal) throw new TokenTransportError('connection_closed', 'Token cursor is closed');
      if (op === 'cancel_cursor') {
        const id = (request as { cursor_id?: unknown }).cursor_id;
        if (response.kind !== 'cursor_closed' || response.cursor_id !== id) throw new TokenTransportError('protocol_error', 'Malformed cursor close response');
        return response;
      }
      const queryResult = response.page;
      if (response.kind !== 'cursor_page' || !validId(response.cursor_id) || typeof response.has_more !== 'boolean' || !record(queryResult) || !Array.isArray(queryResult.records) || !queryResult.records.every((row) => record(row)) || Object.keys(queryResult).some((key) => key === 'records_idmsgpack' || key === 'interner_delta')) throw new TokenTransportError('protocol_error', 'Malformed cursor page');
      try { validateTokenBatchResponse({ id: null, results: { cursor: queryResult }, execution_plan: [], execution_time_us: 0 }); } catch { throw new TokenTransportError('protocol_error', 'Malformed cursor page'); }
      return response;
    }, db, query, pageSize);
  }
  onTerminal(callback: () => void): void { if (this.terminal) callback(); else this.terminalCallbacks.add(callback); }
  attachTerminalSignal(signal: AbortSignal): void { if (signal.aborted) this.notifyTerminal(); else { const abort: () => void = () => this.notifyTerminal(); signal.addEventListener('abort', abort, { once: true }); this.lifecycleUnsubscribe = () => signal.removeEventListener('abort', abort); } }
  private notifyTerminal(): void { if (this.terminal) return; this.terminal = true; super.terminate(); for (const callback of this.terminalCallbacks) callback(); this.terminalCallbacks.clear(); this.lifecycleUnsubscribe?.(); }
  override async next(): Promise<IteratorResult<CursorRow>> { if (this.terminal) return { done: true, value: undefined }; const result = await super.next(); if (!this.terminal && result.done) this.notifyTerminal(); return this.terminal ? { done: true, value: undefined } : result; }
  override async return(): Promise<IteratorResult<CursorRow>> { if (this.terminal) return { done: true, value: undefined }; try { return await super.return(); } finally { this.notifyTerminal(); } }
  override async close(): Promise<void> { if (this.terminal) return; try { await super.close(); } finally { this.notifyTerminal(); } }
  override terminate(): void { this.notifyTerminal(); }
}
