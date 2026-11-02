import { describe, expect, it, vi, afterEach } from 'vitest';
import { inspect } from 'node:util';
import { ApiToken as NodeApiToken } from '../../../index.js';
import { ApiToken as BrowserApiToken } from '../../../browser.js';
import { connect as connectNode } from '../../../index.js';
import { connect as connectBrowser } from '../../../browser.js';
import { TokenTransportError } from '../../../core/protocol/token/errors.js';
import { ShamirClient } from '../../../core/client/client.js';
import { Batch } from '../../../core/builders/write/batch.js';
import { Query } from '../../../core/builders/read/query.js';
import { subscribe } from '../../../core/builders/stream/subscribe.js';
import { encodeTokenPayload, decodeTokenFrame, decodeTokenPayload } from '../../../core/protocol/token/ws.js';
import * as NodePlatformModule from '../../../platform/node.js';
import * as BrowserPlatformModule from '../../../platform/browser.js';
import type { TokenPlatform, TokenSocket, TokenSocketCloseEvent } from '../../../platform/token/types.js';

const TOKEN_VALUE = `shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`;
const token = NodeApiToken.from(TOKEN_VALUE);

afterEach(() => { vi.restoreAllMocks(); });

function expectRestDenial(action: () => unknown): void {
  try {
    action();
  } catch (error) {
    expect(error).toBeInstanceOf(TokenTransportError);
    expect(error).toMatchObject({ code: 'not_supported_over_rest' });
    return;
  }
  throw new Error('expected REST capability denial');
}

class FakeSocket implements TokenSocket {
  readonly sent: Uint8Array[] = [];
  private message: ((bytes: Uint8Array) => void) | undefined;
  private readonly early: Uint8Array[] = [];
  send(bytes: Uint8Array): void {
    this.sent.push(bytes);
    const envelope = decodeTokenFrame(bytes);
    const request = decodeTokenPayload(envelope.req as Uint8Array) as {
      op?: string;
      batch?: { queries?: Record<string, Record<string, unknown>> };
    };
    if (request.op === 'ping') {
      this.deliver({
        rid: envelope.rid,
        res: encodeTokenPayload({ kind: 'pong' }),
      });
    }
    if (request.op === 'execute') {
      const queries = request.batch?.queries ?? {};
      const aliases = Object.keys(queries);
      const results = Object.fromEntries(aliases.map((alias) => {
        const query = queries[alias] ?? {};
        if (Object.prototype.hasOwnProperty.call(query, 'subscribe')) {
          return [alias, { value: { sub: 7 }, records: [] }];
        }
        if (Object.prototype.hasOwnProperty.call(query, 'unsubscribe')) {
          return [alias, { records: [] }];
        }
        return [alias, { records: [{ name: 'ok' }] }];
      }));
      this.deliver({
        rid: envelope.rid,
        res: encodeTokenPayload({
          kind: 'batch',
          response: { id: 1, results, execution_plan: [], execution_time_us: 1 },
        }),
      });
    }
  }
  onMessage(handler: (bytes: Uint8Array) => void): void { this.message = handler; for (const frame of this.early.splice(0)) handler(frame); }
  onClose(_handler: (event: TokenSocketCloseEvent) => void): void {}
  close(): Promise<void> { return Promise.resolve(); }
  deliver(value: unknown): void { const frame = encodeTokenPayload(value); if (this.message) this.message(frame); else this.early.push(frame); }
}

function fakePlatform(runtime: 'node' | 'browser', socket: FakeSocket): TokenPlatform & { requests: URL[]; closed: number } {
  const requests: URL[] = [];
  const state = { closed: 0 };
  return {
    runtime,
    requests,
    get closed() { return state.closed; },
    async requestHttp(url, _token, init) {
      requests.push(url);
      const request = init.body ? decodeTokenPayload(init.body) as { op?: string; batch?: { queries?: Record<string, unknown> } } : undefined;
      const aliases = Object.keys(request?.batch?.queries ?? {});
      const results = Object.fromEntries(aliases.map((alias) => [alias, { records: [{ name: 'ok' }] }]));
      const body = init.method === 'GET' ? encodeTokenPayload({ kind: 'pong' }) : encodeTokenPayload({ kind: 'batch', response: { id: 1, results, execution_plan: [], execution_time_us: 1 } });
      return { status: 200, headers: { get: (name: string) => name.toLowerCase() === 'content-type' ? 'application/msgpack' : null }, body };
    },
    async openTokenSocket() { socket.deliver({ ok: true, sid: new Uint8Array(32).fill(1), expires_at_ns: 99n }); return socket; },
    async close() { state.closed++; },
  };
}

describe('public connect contract', () => {
  it('selects HTTP by scheme and supports ping and builder-based db execution in Node', async () => {
    const socket = new FakeSocket();
    const platform = fakePlatform('node', socket);
    const factory = vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(platform);
    const httpClient = await connectNode({ auth: 'token', url: 'https://db.example', token: TOKEN_VALUE });
    expect(factory).toHaveBeenCalledWith(expect.objectContaining({ channel: 'http', token: expect.any(NodeApiToken), url: 'https://db.example/' }));
    expect(await httpClient.ping()).toEqual({ kind: 'pong' });
    expect(platform.requests).toHaveLength(1);
    expect(await httpClient.db('db').run(Query.from('items').select(['name']))).toMatchObject({ records: [{ name: 'ok' }] });
    expect(platform.requests.map((url) => url.pathname)).toEqual(['/shamir/v1/ping', '/shamir/v1/request']);
    expect(socket.sent).toHaveLength(0);
    await httpClient.close();
  });

  it('selects WebSocket explicitly and never uses HTTP in Node', async () => {
    const socket = new FakeSocket();
    const platform = fakePlatform('node', socket);
    const factory = vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(platform);
    const wsClient = await connectNode({ auth: 'token', channel: 'ws', url: 'wss://db.example', token });
    expect(factory).toHaveBeenCalledWith(expect.objectContaining({ channel: 'ws', url: 'wss://db.example' }));
    expect(await wsClient.ping()).toEqual({ kind: 'pong' });
    expect(await wsClient.db('db').run(Query.from('items').select(['name']))).toMatchObject({ records: [{ name: 'ok' }] });
    expect(platform.requests).toHaveLength(0);
    expect(socket.sent).toHaveLength(2);
    await wsClient.close();
  });

  it('selects browser HTTP and browser WebSocket with normalized opaque handles', async () => {
    const httpSocket = new FakeSocket();
    const httpPlatform = fakePlatform('browser', httpSocket);
    const httpFactory = vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(httpPlatform);
    const httpClient = await connectBrowser({ auth: 'token', url: 'https://db.example', token: TOKEN_VALUE });
    expect(httpFactory).toHaveBeenCalledWith(expect.objectContaining({ channel: 'http', token: expect.any(BrowserApiToken) }));
    expect(httpPlatform.requests).toHaveLength(0);
    expect(await httpClient.ping()).toEqual({ kind: 'pong' });
    expect(await httpClient.db('db').run(Query.from('items').select(['name']))).toMatchObject({ records: [{ name: 'ok' }] });
    await httpClient.close();

    const wsSocket = new FakeSocket();
    const wsPlatform = fakePlatform('browser', wsSocket);
    const wsFactory = vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(wsPlatform);
    const wsClient = await connectBrowser({ auth: 'token', channel: 'ws', url: 'wss://db.example', token });
    expect(wsFactory).toHaveBeenCalledWith(expect.objectContaining({ channel: 'ws', url: 'wss://db.example' }));
    expect(await wsClient.ping()).toEqual({ kind: 'pong' });
    expect(await wsClient.db('db').run(Query.from('items').select(['name']))).toMatchObject({ records: [{ name: 'ok' }] });
    expect(wsPlatform.requests).toHaveLength(0);
    await wsClient.close();
  });

  it('rejects invalid Node and browser configuration before platform construction', async () => {
    const nodeFactory = vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform');
    const browserFactory = vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform');
    const abortedController = new AbortController();
    abortedController.abort();
    const badNode: unknown[] = [
      { auth: 'token', url: 'https://db.example/path', token },
      { auth: 'token', url: 'https://db.example?query=secret', token },
      { auth: 'token', url: 'https://user:pass@db.example', token },
      { auth: 'token', url: 'https://db.example', token, channel: 'ws' },
      { auth: 'token', url: 'https://db.example', token, username: 'u', password: 'p' },
      { auth: 'token', url: 'https://db.example', token, tls: [] },
      { auth: 'token', url: 'https://db.example', token: Object.create(NodeApiToken.prototype) },
      { auth: 'token', url: 'https://db.example', token, signal: abortedController.signal },
      { auth: 'token', url: 'http://127.attacker.example', token },
      { token, url: 'https://db.example' },
      { auth: 'unknown', token, url: 'https://db.example' },
    ];
    for (const options of badNode) {
      await expect(connectNode(options as never)).rejects.toBeInstanceOf(TokenTransportError);
    }
    const badBrowser: unknown[] = [
      { auth: 'token', url: 'https://db.example/path', token },
      { auth: 'token', url: 'https://db.example', token, tls: {} },
      { auth: 'token', url: 'https://db.example', token, origin: 'https://other.example' },
      { auth: 'token', url: 'https://db.example', token, allow_self_signed: true },
      { auth: 'token', url: 'wss://db.example', channel: 'http', token },
      { auth: 'token', url: 'ws://127.attacker.example', token },
      { token, url: 'https://db.example' },
      { auth: 'token', url: 'https://db.example', token, username: 'u' },
    ];
    for (const options of badBrowser) {
      await expect(connectBrowser(options as never)).rejects.toBeInstanceOf(TokenTransportError);
    }
    expect(nodeFactory).not.toHaveBeenCalled();
    expect(browserFactory).not.toHaveBeenCalled();
  });

  it('rejects HTTP-only subscription batch work with the typed REST denial before I/O', async () => {
    const platform = fakePlatform('node', new FakeSocket());
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(platform);
    const client = await connectNode({ auth: 'token', url: 'https://db.example', token });
    const subscriptionBatch = Batch.create().add('sub', subscribe({ store: 'main', table: 'items' })).build();
    await expect(client.execute('db', subscriptionBatch)).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    expect(platform.requests).toHaveLength(0);
    await client.close();
  });

  it('rejects HTTP db cursor, transaction, and live operations before I/O in both entries', async () => {
    const nodePlatform = fakePlatform('node', new FakeSocket());
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(nodePlatform);
    const nodeClient = await connectNode({
      auth: 'token',
      url: 'https://db.example',
      token,
    });
    const nodeDb = nodeClient.db('db');
    expectRestDenial(() => nodeDb.cursor(Query.from('items'), 10));
    const nodeCallback = vi.fn(async () => 'unused');
    await expect(nodeDb.tx(nodeCallback)).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    expect(nodeCallback).not.toHaveBeenCalled();
    await expect(nodeDb.runLive(Batch.create().add('read', Query.from('items')))).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    expect(nodePlatform.requests).toHaveLength(0);
    await nodeClient.close();

    const browserPlatform = fakePlatform('browser', new FakeSocket());
    vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(browserPlatform);
    const browserClient = await connectBrowser({
      auth: 'token',
      url: 'https://db.example',
      token,
    });
    const browserDb = browserClient.db('db');
    expectRestDenial(() => browserDb.cursor(Query.from('items'), 10));
    const browserCallback = vi.fn(async () => 'unused');
    await expect(browserDb.tx(browserCallback)).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    expect(browserCallback).not.toHaveBeenCalled();
    await expect(browserDb.runLive(Batch.create().add('read', Query.from('items')))).rejects.toMatchObject({ code: 'not_supported_over_rest' });
    expect(browserPlatform.requests).toHaveLength(0);
    await browserClient.close();
  });

  it('routes WS subscriptions, pushes, and unsubscribe through each public entry', async () => {
    const nodeSocket = new FakeSocket();
    const nodePlatform = fakePlatform('node', nodeSocket);
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(nodePlatform);
    const nodeClient = await connectNode({
      auth: 'token',
      url: 'wss://db.example',
      token,
    });
    const nodeLive = await nodeClient.db('db').runLive(
      Batch.create().add('live', subscribe({ store: 'main', table: 'items' })),
    );
    const nodeNext = nodeLive.subs.live!.next();
    nodeSocket.deliver({
      push: 'event',
      sub: 7,
      seq: 0,
      data: Uint8Array.of(4, 2),
    });
    await expect(nodeNext).resolves.toMatchObject({
      kind: 'event',
      seq: 0,
      data: Uint8Array.of(4, 2),
    });
    await nodeLive.subs.live!.unsubscribe();
    expect(nodeSocket.sent).toHaveLength(2);
    const nodeUnsubscribe = decodeTokenPayload(decodeTokenFrame(nodeSocket.sent[1]!).req as Uint8Array) as { batch?: { queries?: Record<string, Record<string, unknown>> } };
    expect(nodeUnsubscribe.batch?.queries?._unsub).toEqual({ unsubscribe: 7 });
    expect(nodePlatform.requests).toHaveLength(0);
    await nodeClient.close();

    const browserSocket = new FakeSocket();
    const browserPlatform = fakePlatform('browser', browserSocket);
    vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(browserPlatform);
    const browserClient = await connectBrowser({
      auth: 'token',
      url: 'wss://db.example',
      token,
    });
    const browserLive = await browserClient.db('db').runLive(
      Batch.create().add('live', subscribe({ store: 'main', table: 'items' })),
    );
    const browserNext = browserLive.subs.live!.next();
    browserSocket.deliver({
      push: 'event',
      sub: 7,
      seq: 1,
      data: Uint8Array.of(8, 5),
    });
    await expect(browserNext).resolves.toMatchObject({
      kind: 'event',
      seq: 1,
      data: Uint8Array.of(8, 5),
    });
    await browserLive.subs.live!.unsubscribe();
    expect(browserSocket.sent).toHaveLength(2);
    expect(browserPlatform.requests).toHaveLength(0);
    await browserClient.close();
  });

  it('routes the duplicate WS subscription fixture through both public entries', async () => {
    const nodeSocket = new FakeSocket();
    const nodePlatform = fakePlatform('node', nodeSocket);
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(nodePlatform);
    const nodeClient = await connectNode({
      auth: 'token',
      url: 'wss://db.example',
      token,
    });
    const nodeLive = await nodeClient.db('db').runLive(
      Batch.create().add('live', subscribe({ store: 'main', table: 'items' })),
    );
    const nodeNext = nodeLive.subs.live!.next();
    nodeSocket.deliver({
      push: 'event',
      sub: 7,
      seq: 0,
      data: Uint8Array.of(4, 2),
    });
    await expect(nodeNext).resolves.toMatchObject({
      kind: 'event',
      seq: 0,
      data: Uint8Array.of(4, 2),
    });
    await nodeLive.subs.live!.unsubscribe();
    expect(nodeSocket.sent).toHaveLength(2);
    const nodeUnsubscribe = decodeTokenPayload(decodeTokenFrame(nodeSocket.sent[1]!).req as Uint8Array) as { batch?: { queries?: Record<string, Record<string, unknown>> } };
    expect(nodeUnsubscribe.batch?.queries?._unsub).toEqual({ unsubscribe: 7 });
    expect(nodePlatform.requests).toHaveLength(0);
    await nodeClient.close();

    const browserSocket = new FakeSocket();
    const browserPlatform = fakePlatform('browser', browserSocket);
    vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(browserPlatform);
    const browserClient = await connectBrowser({
      auth: 'token',
      url: 'wss://db.example',
      token,
    });
    const browserLive = await browserClient.db('db').runLive(
      Batch.create().add('live', subscribe({ store: 'main', table: 'items' })),
    );
    const browserNext = browserLive.subs.live!.next();
    browserSocket.deliver({
      push: 'event',
      sub: 7,
      seq: 1,
      data: Uint8Array.of(8, 5),
    });
    await expect(browserNext).resolves.toMatchObject({
      kind: 'event',
      seq: 1,
      data: Uint8Array.of(8, 5),
    });
    await browserLive.subs.live!.unsubscribe();
    expect(browserSocket.sent).toHaveLength(2);
    expect(browserPlatform.requests).toHaveLength(0);
    await browserClient.close();
  });

  it('covers scheme-derived and explicit HTTP channel selection across Node and browser', async () => {
    const nodeHttpPlatform = fakePlatform('node', new FakeSocket());
    const nodeFactory = vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(nodeHttpPlatform);
    const nodeHttp = await connectNode({
      auth: 'token',
      url: 'https://db.example',
      token,
    });
    expect(nodeFactory).toHaveBeenCalledWith(expect.objectContaining({ channel: 'http' }));
    expect(nodeHttp.channel).toBe('http');
    await nodeHttp.close();

    const nodeWsPlatform = fakePlatform('node', new FakeSocket());
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(nodeWsPlatform);
    const nodeWs = await connectNode({
      auth: 'token',
      url: 'wss://db.example',
      token,
    });
    expect(nodeWs.channel).toBe('ws');
    await nodeWs.close();

    const nodeExplicitHttpPlatform = fakePlatform('node', new FakeSocket());
    vi.spyOn(NodePlatformModule, 'createNodeTokenPlatform').mockReturnValue(nodeExplicitHttpPlatform);
    const nodeExplicitHttp = await connectNode({
      auth: 'token',
      channel: 'http',
      url: 'http://127.0.0.1',
      token,
    });
    expect(nodeExplicitHttp.channel).toBe('http');
    await nodeExplicitHttp.close();

    const browserHttpPlatform = fakePlatform('browser', new FakeSocket());
    const browserFactory = vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(browserHttpPlatform);
    const browserHttp = await connectBrowser({
      auth: 'token',
      url: 'https://db.example',
      token,
    });
    expect(browserFactory).toHaveBeenCalledWith(expect.objectContaining({ channel: 'http' }));
    expect(browserHttp.channel).toBe('http');
    await browserHttp.close();

    const browserWsPlatform = fakePlatform('browser', new FakeSocket());
    vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(browserWsPlatform);
    const browserWs = await connectBrowser({
      auth: 'token',
      url: 'wss://db.example',
      token,
    });
    expect(browserWs.channel).toBe('ws');
    await browserWs.close();

    const browserExplicitHttpPlatform = fakePlatform('browser', new FakeSocket());
    vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(browserExplicitHttpPlatform);
    const browserExplicitHttp = await connectBrowser({
      auth: 'token',
      channel: 'http',
      url: 'http://localhost',
      token,
    });
    expect(browserExplicitHttp.channel).toBe('http');
    await browserExplicitHttp.close();
  });

  it('routes browser WS push and verifies unsubscribe envelope', async () => {
    const browserSocket = new FakeSocket();
    const browserPlatform = fakePlatform('browser', browserSocket);
    vi.spyOn(BrowserPlatformModule, 'createBrowserTokenPlatform').mockReturnValue(browserPlatform);
    const browserClient = await connectBrowser({
      auth: 'token',
      url: 'wss://db.example',
      token,
    });
    const browserLive = await browserClient.db('db').runLive(
      Batch.create().add('live', subscribe({ store: 'main', table: 'items' })),
    );
    const browserNext = browserLive.subs.live!.next();
    browserSocket.deliver({
      push: 'event',
      sub: 7,
      seq: 2,
      data: Uint8Array.of(3, 1),
    });
    await expect(browserNext).resolves.toMatchObject({
      kind: 'event',
      seq: 2,
      data: Uint8Array.of(3, 1),
    });
    await browserLive.subs.live!.unsubscribe();
    expect(browserSocket.sent).toHaveLength(2);
    const envelope = decodeTokenFrame(browserSocket.sent[1]!);
    const unsubscribe = decodeTokenPayload(envelope.req as Uint8Array) as {
      batch?: { queries?: Record<string, Record<string, unknown>> };
    };
    expect(unsubscribe.batch?.queries?._unsub).toEqual({ unsubscribe: 7 });
    expect(browserPlatform.requests).toHaveLength(0);
    await browserClient.close();
  });

  it('preserves SCRAM overload result and passes original options and platform through', async () => {
    const client = Object.create(ShamirClient.prototype) as ShamirClient;
    const connectSpy = vi.spyOn(ShamirClient, 'connect').mockResolvedValue(client);
    const opts = {
      host: 'localhost',
      port: 7000,
      username: 'alice',
      password: 'safe',
      origin: 'https://localhost',
    };
    const nodePromise: Promise<ShamirClient> = connectNode(opts);
    await expect(nodePromise).resolves.toBe(client);
    expect(connectSpy).toHaveBeenCalledWith(NodePlatformModule.NodePlatform, opts);
    const browserPromise: Promise<ShamirClient> = connectBrowser(opts);
    await expect(browserPromise).resolves.toBe(client);
    expect(connectSpy).toHaveBeenLastCalledWith(BrowserPlatformModule.BrowserPlatform, opts);
  });

  it('keeps entry exports isolated and redacts public token values', async () => {
    const nodeExports = await import('../../../index.js');
    const browserExports = await import('../../../browser.js');
    expect(nodeExports.connect).toBeTypeOf('function');
    expect(browserExports.connect).toBeTypeOf('function');
    expect(nodeExports.ApiToken).toBe(browserExports.ApiToken);
    expect((await import('../../../index.js')).ShamirClient).toBe(ShamirClient);
    expect((await import('../../../browser.js')).ShamirClient).toBe(ShamirClient);
    const secretUrl = `https://db.example/${TOKEN_VALUE}`;
    let configError: unknown;
    try { await connectNode({ auth: 'token', url: secretUrl, token }); } catch (error) { configError = error; }
    expect(configError).toBeInstanceOf(TokenTransportError);
    expect(`${String(configError)} ${String((configError as Error).stack)} ${inspect(configError)}`).not.toContain(TOKEN_VALUE);
    expect(Object.keys(nodeExports)).toContain('connectLocal');
    expect(Object.keys(nodeExports)).not.toContain('tokenValueForTransport');
    expect(Object.keys(browserExports)).not.toContain('tokenValueForTransport');
    expect(Object.keys(browserExports)).not.toContain('createBrowserTokenPlatform');
    expect(String(token)).toBe('[REDACTED]');
    expect(JSON.stringify(token)).toBe('"[REDACTED]"');
    expect(inspect(token)).toBe('ApiToken [REDACTED]');
    expect(Reflect.ownKeys(token)).toEqual([]);
    expect(structuredClone(token)).toEqual({});
  });
});

