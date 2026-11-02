import { describe, expect, it, vi, afterEach } from 'vitest';
import { inspect } from 'node:util';
import { connect as connectNode, ApiToken as NodeApiToken } from '../../../index.js';
import { connect as connectBrowser, ApiToken as BrowserApiToken } from '../../../browser.js';
import { TokenTransportError } from '../../../core/protocol/token/errors.js';
import { ShamirClient } from '../../../core/client/client.js';
import { Batch } from '../../../core/builders/write/batch.js';
import { Query } from '../../../core/builders/read/query.js';
import { subscribe } from '../../../core/builders/stream/subscribe.js';
import type { ConnectOptions, NodeConnectOptions, BrowserConnectOptions } from '../../../core/types/connection.js';
import type { TokenHttpClient } from '../../../entry/token-client.js';
import type { TokenWsClient, PublicTokenClient } from '../../../entry/ws/token-client.js';
import { decodeTokenFrame, decodeTokenPayload, encodeTokenPayload } from '../../../core/protocol/token/ws.js';
import * as NodePlatformModule from '../../../platform/node.js';
import * as BrowserPlatformModule from '../../../platform/browser.js';
import type { TokenPlatform, TokenSocket, TokenSocketCloseEvent } from '../../../platform/token/types.js';

const TOKEN_VALUE = `shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`;
const token = NodeApiToken.from(TOKEN_VALUE);
afterEach(() => { vi.restoreAllMocks(); });

class FakeSocket implements TokenSocket {
  readonly sent: Uint8Array[] = [];
  private message: ((bytes: Uint8Array) => void) | undefined;
  private readonly early: Uint8Array[] = [];
  private nextSubId = 7;
  send(bytes: Uint8Array): void {
    this.sent.push(bytes);
    const envelope = decodeTokenFrame(bytes);
    const request = decodeTokenPayload(envelope.req as Uint8Array) as { op?: string; batch?: { queries?: Record<string, Record<string, unknown>> } };
    if (request.op === 'ping') { this.deliver({ rid: envelope.rid, res: encodeTokenPayload({ kind: 'pong' }) }); return; }
    if (request.op !== 'execute') return;
    const queries = request.batch?.queries ?? {};
    const results = Object.fromEntries(Object.entries(queries).map(([alias, query]) => {
      if ('subscribe' in query) return [alias, { value: { sub: this.nextSubId++ }, records: [] }];
      if ('unsubscribe' in query) return [alias, { records: [] }];
      return [alias, { records: [{ name: 'ok' }] }];
    }));
    this.deliver({ rid: envelope.rid, res: encodeTokenPayload({ kind: 'batch', response: { id: 1, results, execution_plan: [], execution_time_us: 1 } }) });
  }
  onMessage(handler: (bytes: Uint8Array) => void): void { this.message = handler; for (const frame of this.early.splice(0)) handler(frame); }
  onClose(_handler: (event: TokenSocketCloseEvent) => void): void {}
  close(): Promise<void> { return Promise.resolve(); }
  deliver(value: unknown): void { const frame = encodeTokenPayload(value); if (this.message) this.message(frame); else this.early.push(frame); }
}
function fakePlatform(runtime: 'node' | 'browser', socket: FakeSocket): TokenPlatform & { requests: URL[] } {
  const requests: URL[] = [];
  return {
    runtime, requests,
    async requestHttp(url, _token, init) {
      requests.push(url);
      const request = init.body ? decodeTokenPayload(init.body) as { batch?: { queries?: Record<string, unknown> } } : undefined;
      const results = Object.fromEntries(Object.keys(request?.batch?.queries ?? {}).map((alias) => [alias, { records: [{ name: 'ok' }] }]));
      const body = init.method === 'GET' ? encodeTokenPayload({ kind: 'pong' }) : encodeTokenPayload({ kind: 'batch', response: { id: 1, results, execution_plan: [], execution_time_us: 1 } });
      return { status: 200, headers: { get: (name: string) => name.toLowerCase() === 'content-type' ? 'application/msgpack' : null }, body };
    },
    async openTokenSocket() { socket.deliver({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 99n }); return socket; },
    async close() {},
  };
}
function expectRestDenial(action: () => unknown): void {
  try { action(); } catch (error) { expect(error).toBeInstanceOf(TokenTransportError); expect(error).toMatchObject({ code: 'not_supported_over_rest' }); return; }
  throw new Error('expected REST capability denial');
}

describe('public connect contract', () => {
  it('accepts Node remote plaintext HTTP and WebSocket URLs', async () => {
    const httpPlatform = fakePlatform('node', new FakeSocket());
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(httpPlatform);
    const http = await connectNode({ auth: 'token', url: 'http://127.attacker.example', token });
    expect(http.channel).toBe('http'); await http.close();
    const wsPlatform = fakePlatform('node', new FakeSocket());
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(wsPlatform);
    const ws = await connectNode({ auth: 'token', url: 'ws://db.remote.example', token });
    expect(ws.channel).toBe('ws'); await ws.close();
  });
  it('rejects invalid Node and browser options before platform construction and preserves SCRAM', async () => {
    const nodeFactory = vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform');
    const browserFactory = vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform');
    const controller = new AbortController(); controller.abort();
    const invalidNode: unknown[] = [
      { auth: 'token', url: 'https://db.example/path', token },
      { auth: 'token', url: 'https://db.example?query=x', token },
      { auth: 'token', url: 'https://user:pass@db.example', token },
      { auth: 'token', url: 'https://db.example', token, tls: [] },
      { auth: 'token', url: 'https://db.example', token, signal: controller.signal },
    ];
    for (const [index, options] of invalidNode.entries()) await expect(connectNode(options as never)).rejects.toMatchObject({ code: index === 4 ? 'client_aborted' : 'invalid_request' });
    await expect(connectNode({ auth: 'token', channel: 'http', url: 'wss://db.example', token } as never)).rejects.toMatchObject({ code: 'invalid_request' });
    const invalidBrowser: unknown[] = [
      { auth: 'token', url: 'https://db.example/path', token },
      { auth: 'token', url: 'https://db.example', token, tls: {} },
      { auth: 'token', url: 'https://db.example', token, origin: 'https://other.example' },
      { auth: 'token', url: 'https://db.example', token, requestTimeoutMs: -1 },
      { auth: 'token', url: 'https://db.example', token, tls: { allowInsecureTlsForDevelopment: false } },
      { auth: 'token', url: 'ws://db.remote.example', token },
    ];
    for (const options of invalidBrowser) await expect(connectBrowser(options as never)).rejects.toMatchObject({ code: 'invalid_request' });
    await expect(connectBrowser({ auth: 'token', channel: 'http', url: 'wss://db.example', token } as never)).rejects.toMatchObject({ code: 'invalid_request' });
    expect(nodeFactory).not.toHaveBeenCalled(); expect(browserFactory).not.toHaveBeenCalled();
    const client = Object.create(ShamirClient.prototype) as ShamirClient;
    const connectSpy = vi.spyOn(ShamirClient, 'connect').mockResolvedValue(client);
    const options: ConnectOptions = { host: 'localhost', port: 7000, username: 'alice', password: 'safe' };
    const nodeResult: Promise<ShamirClient> = connectNode(options);
    const browserResult: Promise<ShamirClient> = connectBrowser(options);
    await expect(nodeResult).resolves.toBe(client); await expect(browserResult).resolves.toBe(client);
    expect(connectSpy).toHaveBeenNthCalledWith(1, NodePlatformModule.NodePlatform, options);
    expect(connectSpy).toHaveBeenNthCalledWith(2, BrowserPlatformModule.BrowserPlatform, options);
  });
  it('denies REST-only operations before sending', async () => {
    const platform = fakePlatform('node', new FakeSocket());
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(platform);
    const client = await connectNode({ auth: 'token', url: 'https://db.example', token });
    expectRestDenial(() => client.db('db').cursor(Query.from('items'), 10));
    await expect(client.db('db').tx(async () => 'x')).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    await expect(client.db('db').runLive(Batch.create().add('read', Query.from('items')))).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    expect(platform.requests).toHaveLength(0); await client.close();
  });
  it('routes two subscriptions and makes repeated unsubscribe idempotent', async () => {
    const socket = new FakeSocket();
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(fakePlatform('node', socket));
    const client = await connectNode({ auth: 'token', url: 'wss://db.example', token });
    const live = await client.db('db').runLive(Batch.create().add('first', subscribe({ store: 'main', table: 'items' })).add('second', subscribe({ store: 'main', table: 'other' })));
    await live.subs.first!.unsubscribe(); await live.subs.first!.unsubscribe(); await live.subs.second!.unsubscribe();
    expect(socket.sent).toHaveLength(3);
    expect(socket.sent.slice(1).map((frame) => {
      const request = decodeTokenPayload(decodeTokenFrame(frame).req as Uint8Array) as { batch?: { queries?: Record<string, { unsubscribe?: number }> } };
      return Object.values(request.batch?.queries ?? {})[0]?.unsubscribe;
    })).toEqual([7, 8]);
    await client.close();
  });
  it('checks public overload returns without invoking connect', () => {
    type Assert<T extends true> = T;
    type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends (<T>() => T extends B ? 1 : 2) ? true : false;
    type NodeParams = Assert<Equal<Parameters<typeof connectNode>, [NodeConnectOptions]>>;
    type BrowserParams = Assert<Equal<Parameters<typeof connectBrowser>, [BrowserConnectOptions]>>;
    type NodeReturn = Assert<Equal<ReturnType<typeof connectNode>, Promise<ShamirClient | PublicTokenClient>>>;
    type BrowserReturn = Assert<Equal<ReturnType<typeof connectBrowser>, Promise<ShamirClient | PublicTokenClient>>>;
    type Checks = [NodeParams, BrowserParams, NodeReturn, BrowserReturn]; void (0 as unknown as Checks);
  });
  it('exports no platform internals and redacts public token values', async () => {
    const node = await import('../../../index.js'); const browser = await import('../../../browser.js');
    expect(Object.keys(node)).not.toContain('TokenWsClient'); expect(Object.keys(node)).not.toContain('TokenHttpClient');
    expect(Object.keys(browser)).not.toContain('TokenWsClient'); expect(Object.keys(browser)).not.toContain('TokenHttpClient');
    expect(Object.keys(node)).not.toContain('connectTokenNode'); expect(Object.keys(browser)).not.toContain('connectTokenBrowser');
    expect(Object.keys(browser)).not.toContain('createNodeTokenPlatform'); expect(Object.keys(browser)).not.toContain('createBrowserTokenPlatform');
    let error: unknown;
    try { await connectNode({ auth: 'token', url: `https://db.example/${TOKEN_VALUE}`, token }); } catch (caught) { error = caught; }
    expect(`${String(error)} ${String((error as Error).stack)} ${inspect(error)}`).not.toContain(TOKEN_VALUE);
    expect(String(token)).toBe('[REDACTED]'); expect(JSON.stringify(token)).toBe('"[REDACTED]"');
    expect(inspect(token)).toBe('ApiToken [REDACTED]'); expect(Reflect.ownKeys(token)).toEqual([]); expect(structuredClone(token)).toEqual({});
  });
});
