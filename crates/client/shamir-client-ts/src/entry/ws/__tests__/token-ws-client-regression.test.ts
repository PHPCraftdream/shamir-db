import { describe, expect, it, vi } from 'vitest';
import { ApiToken } from '../../api-token.js';
import { subscribe } from '../../../core/builders/stream/subscribe.js';
import { Batch } from '../../../core/builders/write/batch.js';
import { Query } from '../../../core/builders/read/query.js';
import { createTokenWsClient } from '../token-ws-client.js';
import { createTokenOperations } from '../../token-client.js';
import { TokenTransportError } from '../../../core/protocol/token/errors.js';
import { decodeTokenFrame, decodeTokenPayload, encodeTokenPayload } from '../../../core/protocol/token/ws.js';
import { encode } from '../../../core/protocol/framing.js';
import type { TokenSocket, TokenSocketCloseEvent, TokenPlatform } from '../../../platform/token/types.js';

class TestSocket implements TokenSocket {
  frames: Uint8Array[] = [];
  private message: ((data: Uint8Array) => void) | undefined;
  private early: Uint8Array[] = [];
  private closes: Array<(event: TokenSocketCloseEvent) => void> = [];
  onSend: ((data: Uint8Array) => void) | undefined;
  async close(code = 1000): Promise<void> { for (const handler of this.closes) handler({ code, reason: 'peer detail', wasClean: true }); }
  send(data: Uint8Array): void { this.frames.push(data); this.onSend?.(data); }
  onMessage(handler: (data: Uint8Array) => void): void { this.message = handler; for (const frame of this.early.splice(0)) handler(frame); }
  onClose(handler: (event: TokenSocketCloseEvent) => void): void { this.closes.push(handler); }
  receive(value: unknown): void { const data = encode(value); if (this.message) this.message(data); else this.early.push(data); }
}

const token = ApiToken.from(`shk1.${'A'.repeat(21)}A.${'A'.repeat(42)}A`);

describe('token websocket client regressions', () => {
  it('rejects pessimistic transactions before invoking the callback or sending', async () => {
    const socket = new TestSocket();
    const platform: TokenPlatform = { runtime: 'node', async requestHttp() { throw new Error('unexpected HTTP request'); }, async openTokenSocket() { return socket; }, async close() {} };
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => platform, async () => { socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n }); return socket; });
    const callback = vi.fn();
    await expect(client.db('db').tx(callback, { isolation: 'pessimistic' as never })).rejects.toMatchObject({ code: 'invalid_request' });
    expect(callback).not.toHaveBeenCalled(); expect(socket.frames).toHaveLength(0); await client.close();
  });

  it('unsubscribes every server grant when the local subscription cap is exceeded', async () => {
    const socket = new TestSocket();
    const platform: TokenPlatform = { runtime: 'node', async requestHttp() { throw new Error('unexpected HTTP request'); }, async openTokenSocket() { return socket; }, async close() {} };
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => platform, async () => { socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n }); return socket; });
    const batch = Batch.create(); const names: string[] = [];
    for (let index = 0; index < 129; index++) { const alias = `live${index}`; names.push(alias); batch.add(alias, subscribe({ store: 'main', table: 'items' })); }
    const live = client.db('db').runLive(batch);
    socket.onSend = (frame) => {
      if (socket.frames.length === 1) return;
      const outer = decodeTokenFrame(frame); const request = decodeTokenPayload(outer.req as Uint8Array) as { batch?: { queries?: Record<string, unknown> } };
      if (request.batch?.queries?._unsub !== undefined) socket.receive({ rid: outer.rid, res: encodeTokenPayload({ kind: 'batch', response: { id: 1, results: {}, execution_plan: [], execution_time_us: 1 } }) });
    };
    const results = Object.fromEntries(names.map((alias, id) => [alias, { value: { sub: id }, records: [] }]));
    socket.receive({ rid: 1, res: encodeTokenPayload({ kind: 'batch', response: { id: 1, results, execution_plan: [], execution_time_us: 1 } }) });
    const rejection = expect(live).rejects.toMatchObject({ code: 'token_limit_reached' }); await rejection;
    expect(socket.frames).toHaveLength(130);
    const unsubscriptions = socket.frames.slice(1).map((frame) => decodeTokenPayload(decodeTokenFrame(frame).req as Uint8Array) as { batch: { queries: { _unsub: { unsubscribe: number } } } });
    const unsubscribedIds = unsubscriptions.map((request) => request.batch.queries._unsub.unsubscribe);
    expect(unsubscribedIds).toHaveLength(129); expect(unsubscribedIds).toEqual(expect.arrayContaining(Array.from({ length: 129 }, (_, id) => id)));
    socket.onSend = undefined; const usable = client.ping(); socket.receive({ rid: 131, res: encodeTokenPayload({ kind: 'pong' }) });
    await expect(usable).resolves.toMatchObject({ kind: 'pong' }); await client.close();
  });

  it('closes platform on rejected socket connection and preserves original failure', async () => {
    const socket = new TestSocket(); const closedPlatform = vi.fn(async () => { throw new Error('cleanup failure'); });
    const original = new TokenTransportError('network_error', 'sanitized connect failure');
    const client = createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => ({ runtime: 'node', requestHttp: async () => { throw new Error('unexpected HTTP request'); }, openTokenSocket: async () => socket, close: closedPlatform }), async () => { throw original; });
    await expect(client).rejects.toBe(original); expect(closedPlatform).toHaveBeenCalledTimes(1);
  });

  it('closes platform when browser rejects remote ws plaintext', async () => {
    const closedPlatform = vi.fn(async () => {}); const openSocket = vi.fn(async () => new TestSocket());
    const client = createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'ws://remote.example' } as never, () => ({ runtime: 'browser', requestHttp: async () => { throw new Error('unexpected HTTP request'); }, openTokenSocket: openSocket, close: closedPlatform }));
    await expect(client).rejects.toMatchObject({ code: 'invalid_request' }); expect(openSocket).not.toHaveBeenCalled(); expect(closedPlatform).toHaveBeenCalledTimes(1);
  });

  it('reuses nested batch builders and rejects cyclic wire input', async () => {
    const sent: unknown[] = [];
    const operations = createTokenOperations({ auth: 'token', channel: 'http', token, url: 'https://example.test' }, { async request(request) { sent.push(request); return { kind: 'batch', response: { id: 1, results: {}, execution_plan: [], execution_time_us: 1 } }; }, async close() {} });
    const shared = Batch.create().add('read', Query.from('items')).build();
    const nested = Batch.create().subBatch('first', shared).subBatch('second', shared).build();
    await expect(operations.execute('db', nested)).resolves.toBeDefined();
    // Direct wire assembly keeps the cycle itself intact for validator coverage.
    const cyclic: { id: number; queries: Record<string, unknown> } = { id: 4, queries: {} }; cyclic.queries.loop = { batch: cyclic };
    await expect(operations.execute('db', cyclic)).rejects.toMatchObject({ code: 'token_request_denied' }); expect(sent).toHaveLength(1);
  });

  it('rejects an invalid URL asynchronously', async () => {
    let pending: Promise<unknown> | undefined;
    expect(() => { pending = createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'not a URL' } as never, () => { throw new Error('platform factory must not run'); }); }).not.toThrow();
    await expect(pending).rejects.toThrow(TypeError);
  });

  it('aborts the client signal on explicit close', async () => {
    const socket = new TestSocket();
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => ({ runtime: 'node', requestHttp: async () => { throw new Error('unused'); }, openTokenSocket: async () => socket, close: async () => {} }), async () => { socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n }); return socket; });
    const signal = client.terminalController.signal; await client.close(); expect(signal.aborted).toBe(true);
  });

  it('ends pending and future subscription reads with done after terminal close', async () => {
    const socket = new TestSocket();
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => ({ runtime: 'node', requestHttp: async () => { throw new Error('unused'); }, openTokenSocket: async () => socket, close: async () => {} }), async () => { socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n }); return socket; });
    const live = client.db('db').runLive(Batch.create().add('live', subscribe({ store: 'main', table: 'items' })));
    socket.receive({ rid: 1, res: encodeTokenPayload({ kind: 'batch', response: { id: 1, results: { live: { value: { sub: 7 }, records: [] } }, execution_plan: [], execution_time_us: 1 } }) });
    const result = await live; const pending = result.subs.live!.next(); await client.close();
    await expect(pending).resolves.toBeUndefined(); await expect(result.subs.live!.next()).resolves.toBeUndefined();
  });

  it('rejects admin transaction batches before sending', async () => {
    const socket = new TestSocket();
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => ({ runtime: 'node', requestHttp: async () => { throw new Error('unused'); }, openTokenSocket: async () => socket, close: async () => {} }), async () => { socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n }); return socket; });
    const opening = client.txBegin('db', 'main');
    socket.receive({ rid: 1, res: encodeTokenPayload({ kind: 'tx_opened', tx_handle: 3, snapshot_version: 1n, isolation: 'snapshot' }) });
    const opened = await opening;
    // Direct wire assembly exercises the denied administrative operation.
    const forbidden = Batch.create().add('bad', { admin: {} } as never).build();
    await expect(client.txExecute('db', opened.tx_handle, forbidden)).rejects.toMatchObject({ code: 'token_request_denied' });
    expect(socket.frames).toHaveLength(1);
    socket.onSend = (frame) => { const outer = decodeTokenFrame(frame); socket.receive({ rid: outer.rid, res: encodeTokenPayload({ kind: 'tx_rolled_back' }) }); };
    await client.close();
  });

  it('cleans transactions, subscriptions and cursors on remote terminal close', async () => {
    const socket = new TestSocket();
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => ({ runtime: 'node', requestHttp: async () => { throw new Error('unused'); }, openTokenSocket: async () => socket, close: async () => {} }), async () => {
      socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n });
      return socket;
    });
    const opening = client.txBegin('db', 'main');
    socket.receive({ rid: 1, res: encodeTokenPayload({ kind: 'tx_opened', tx_handle: 4, snapshot_version: 1n, isolation: 'snapshot' }) });
    const opened = await opening;
    const live = client.db('db').runLive(Batch.create().add('live', subscribe({ store: 'main', table: 'items' })));
    socket.receive({ rid: 2, res: encodeTokenPayload({ kind: 'batch', response: { id: 1, results: { live: { value: { sub: 9 }, records: [] } }, execution_plan: [], execution_time_us: 1 } }) });
    const subscription = (await live).subs.live!;
    const waitingRead = subscription.next();
    const cursor = client.db('db').cursor(Query.from('items'), 10);
    const frameCount = socket.frames.length;
    socket.onSend = undefined;
    await socket.close(4011);
    await expect(waitingRead).resolves.toBeUndefined();
    await expect(subscription.next()).resolves.toBeUndefined();
    await expect(cursor.next()).resolves.toMatchObject({ done: true });
    await expect(client.txExecute('db', opened.tx_handle, Batch.create().add('read', Query.from('items')).build())).rejects.toMatchObject({ code: 'connection_closed' });
    expect(client.terminalController.signal.aborted).toBe(true);
    expect(socket.frames).toHaveLength(frameCount);
    await client.close();
  });

  it('routes pushes and makes unsubscribe idempotent', async () => {
    const socket = new TestSocket();
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => ({ runtime: 'node', requestHttp: async () => { throw new Error('unused'); }, openTokenSocket: async () => socket, close: async () => {} }), async () => {
      socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n });
      return socket;
    });
    const live = client.db('db').runLive(Batch.create().add('first', subscribe({ store: 'main', table: 'items' })).add('second', subscribe({ store: 'main', table: 'items' })));
    socket.receive({ push: 'event', sub: 7, seq: 0, data: Uint8Array.of(1) });
    socket.receive({ rid: 1, res: encodeTokenPayload({ kind: 'batch', response: { id: 1, results: { first: { value: { sub: 7 }, records: [] }, second: { value: { sub: 8 }, records: [] } }, execution_plan: [], execution_time_us: 1 } }) });
    const result = await live;
    const firstEvent = result.subs.first!.next();
    socket.receive({ push: 'event', sub: 8, seq: 0, data: Uint8Array.of(2) });
    await expect(firstEvent).resolves.toMatchObject({ kind: 'event', seq: 0, data: Uint8Array.of(1) });
    await expect(result.subs.second!.next()).resolves.toMatchObject({ kind: 'event', seq: 0, data: Uint8Array.of(2) });
    socket.onSend = (frame) => { const outer = decodeTokenFrame(frame); socket.receive({ rid: outer.rid, res: encodeTokenPayload({ kind: 'batch', response: { id: 1, results: {}, execution_plan: [], execution_time_us: 1 } }) }); };
    const unsubscribe = result.subs.first!.unsubscribe();
    expect(result.subs.first!.unsubscribe()).toBe(unsubscribe);
    const unsubscribeRequest = decodeTokenPayload(decodeTokenFrame(socket.frames[1]!).req as Uint8Array) as { op: string; query_version?: number; batch?: { queries: { _unsub: { unsubscribe: number } } } };
    expect(unsubscribeRequest.batch?.queries._unsub.unsubscribe).toBe(7);
    expect(socket.frames.slice(1).map((frame) => decodeTokenPayload(decodeTokenFrame(frame).req as Uint8Array))).toHaveLength(1);
    await unsubscribe;
    await expect(result.subs.first!.next()).resolves.toBeUndefined();
    socket.receive({ push: 'event', sub: 7, seq: 1, data: Uint8Array.of(3) });
    const waitingSecond = result.subs.second!.next();
    socket.receive({ push: 'event', sub: 8, seq: 1, data: Uint8Array.of(4) });
    await expect(waitingSecond).resolves.toMatchObject({ kind: 'event', seq: 1, data: Uint8Array.of(4) });
    await client.close();
    await expect(result.subs.second!.next()).resolves.toBeUndefined();
  });
  it('returns a stable async iterable iterator', async () => {
    const platform: TokenPlatform = { runtime: 'node', async requestHttp() { throw new Error('unexpected HTTP request'); }, async openTokenSocket() { throw new Error('unused socket factory override expected'); }, async close() {} };
    const socket = new TestSocket();
    const client = await createTokenWsClient({ auth: 'token', channel: 'ws', token, url: 'wss://example.test' } as never, () => platform, async () => { socket.receive({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 1n }); return socket; });
    const live = client.db('db').runLive(Batch.create().add('live', subscribe({ store: 'main', table: 'items', deliver: 'records' }, { initial: true, fromVersion: 1 })));
    expect(socket.frames).toHaveLength(1);
    socket.receive({ rid: 1, res: encode({ kind: 'batch', response: { id: 1, results: { live: { value: { sub: 7 }, records: [] } }, execution_plan: [], execution_time_us: 1 } }) });
    const result = await live; const iterator = result.subs.live![Symbol.asyncIterator]();
    expect(iterator).toBe(result.subs.live![Symbol.asyncIterator]()); expect(iterator[Symbol.asyncIterator]()).toBe(iterator); await client.close();
  });
});
