import { describe, expect, it, vi } from 'vitest';
import { ApiToken } from '../../api-token.js';
import { Batch } from '../../../core/builders/write/batch.js';
import { Query } from '../../../core/builders/read/query.js';
import { decodeTokenFrame, decodeTokenPayload, encodeTokenPayload } from '../../../core/protocol/token/ws.js';
import { createTokenWsClient } from '../token-ws-client.js';
import type { TokenSocket, TokenSocketCloseEvent, TokenPlatform } from '../../../platform/token/types.js';

class WireSocket implements TokenSocket {
  readonly frames: Uint8Array[] = [];
  private message: ((data: Uint8Array) => void) | undefined;
  private readonly early: Uint8Array[] = [];
  async close(): Promise<void> {}
  send(data: Uint8Array): void {
    this.frames.push(data);
    const outer = decodeTokenFrame(data);
    const request = decodeTokenPayload(outer.req as Uint8Array) as { op?: string };
    if (request.op === 'execute') this.receive({ rid: outer.rid, res: encodeTokenPayload({ kind: 'batch', response: { id: 1, results: { read: { records: [{ name: 'ok' }] } }, execution_plan: [], execution_time_us: 1 } }) });
  }
  onMessage(handler: (data: Uint8Array) => void): void { this.message = handler; for (const frame of this.early.splice(0)) handler(frame); }
  onClose(_handler: (event: TokenSocketCloseEvent) => void): void {}
  receive(value: unknown): void { const data = encodeTokenPayload(value); if (this.message) this.message(data); else this.early.push(data); }
}

describe('token WebSocket execute wire shape', () => {
  it('uses name-keyed queries and does not issue an interner operation', async () => {
    const token = ApiToken.from(`shk1.${'A'.repeat(21)}A.${'A'.repeat(42)}A`);
    const socket = new WireSocket();
    const requestHttp = vi.fn(async () => { throw new Error('HTTP must not be used'); });
    const platform: TokenPlatform = { runtime: 'node', requestHttp, async openTokenSocket() { return socket; }, async close() {} };
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' }, () => platform, async () => {
      socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n });
      return socket;
    });
    const result = await client.executeWithTouch('db', Batch.create().add('read', Query.from('items').select(['name'])).build());
    expect(socket.frames).toHaveLength(1);
    const outer = decodeTokenFrame(socket.frames[0]!);
    const request = decodeTokenPayload(outer.req as Uint8Array) as { op: string; query_version: number; batch: { queries: Record<string, Record<string, unknown>> } };
    expect(request.op).toBe('execute');
    expect(request.query_version).toBe(4);
    expect(Object.keys(request.batch.queries)).toEqual(['read']);
    expect(request.batch.queries.read).toMatchObject({ from: 'items', select: ['name'] });
    expect(JSON.stringify(request)).not.toMatch(/interner/);
    expect(result.results.read?.records).toEqual([{ name: 'ok' }]);
    expect(requestHttp).not.toHaveBeenCalled();
    await client.close();
  });
});
