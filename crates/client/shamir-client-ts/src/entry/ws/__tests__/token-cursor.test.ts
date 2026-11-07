import { describe, expect, it } from 'vitest';
import { TokenCursor } from '../token-cursor.js';
import type { ReadQuery } from '../../../core/types/read/query.js';

describe('TokenCursor terminal behavior', () => {
  it('drops buffered and in-flight records after terminal close', async () => {
    let resolve!: (value: Record<string, unknown>) => void;
    const pending = new Promise<Record<string, unknown>>((done) => { resolve = done; });
    const cursor = new TokenCursor(() => pending, 'db', {} as ReadQuery, 10);
    const controller = new AbortController(); cursor.attachTerminalSignal(controller.signal);
    const next = cursor.next(); controller.abort();
    resolve({ kind: 'cursor_page', cursor_id: 1, has_more: false, page: { records: [{ value: 1 }] } });
    await expect(next).resolves.toMatchObject({ done: true });
    await expect(cursor.next()).resolves.toMatchObject({ done: true });
  });
  it('validates cursor page and cancellation response shapes', async () => {
    const cursor = new TokenCursor(async (request) => (request as { op: string }).op === 'create_cursor'
      ? { kind: 'cursor_page', cursor_id: 1, has_more: false, page: { records: [], stats: { records_scanned: 0, records_returned: 0, execution_time_us: 0, index_used: null } } }
      : { kind: 'cursor_closed', cursor_id: 1 }, 'db', {} as ReadQuery, 1);
    await expect(cursor.next()).resolves.toMatchObject({ done: true });
    const malformed = new TokenCursor(async () => ({ kind: 'wrong' }), 'db', {} as ReadQuery, 1);
    await expect(malformed.next()).rejects.toMatchObject({ code: 'protocol_error' });
  });
});
