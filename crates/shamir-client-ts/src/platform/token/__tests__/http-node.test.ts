import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import * as https from 'node:https';
import { describe, expect, it, vi } from 'vitest';
import { ApiToken } from '../../../entry/api-token.js';
import { createNodeTokenPlatform } from '../http-node.js';
import { TokenHttpTransport } from '../../../core/protocol/token/http.js';
import { decode, encode } from '../../../core/protocol/framing.js';
import { Query } from '../../../core/builders/read/query.js';
import { Batch } from '../../../core/builders/write/batch.js';
import type { NodeTokenHttpConnectOptions } from '../../../core/types/connection.js';

const rawToken = 'shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';
const token = ApiToken.from(rawToken);
type Reply = { status?: number; headers?: Record<string, string>; body?: Uint8Array; hang?: boolean; abort?: boolean };
type Seen = { url: string; method?: string; authorization?: string; contentType?: string; body: Buffer };
function startServer(handler: (request: IncomingMessage, response: ServerResponse, seen: Seen) => void) {
  const seen: Seen[] = [];
  const server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on('data', chunk => chunks.push(Buffer.from(chunk)));
    request.on('end', () => {
      const item: Seen = { url: request.url ?? '', method: request.method, authorization: request.headers.authorization, contentType: request.headers['content-type'], body: Buffer.concat(chunks) };
      seen.push(item);
      handler(request, response, item);
    });
  });
  const ready = new Promise<number>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve((server.address() as AddressInfo).port));
  });
  return { server, seen, ready };
}
const options = (port: number, extra: Partial<NodeTokenHttpConnectOptions> = {}): NodeTokenHttpConnectOptions => ({ auth: 'token', channel: 'http', url: `http://127.0.0.1:${port}`, token, ...extra });
function msgpackReply(value: unknown, status = 200, headers: Record<string, string> = {}): Reply {
  return { status, headers: { 'content-type': 'application/msgpack', ...headers }, body: encode(value) };
}
function answer(reply: Reply) {
  return (_request: IncomingMessage, response: ServerResponse) => {
    response.writeHead(reply.status ?? 200, reply.headers);
    if (reply.hang) return;
    if (reply.abort) { response.write(Buffer.from([0x81])); response.socket?.destroy(); return; }
    response.end(reply.body ?? encode({ kind: 'pong' }));
  };
}
async function shutdown(server: ReturnType<typeof createServer>, clients: Array<{ close(): Promise<void> }>): Promise<void> {
  await Promise.allSettled(clients.map(client => client.close()));
  server.closeAllConnections();
  await new Promise<void>(resolve => server.close(() => resolve()));
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

describe('Node token HTTP adapter', () => {
  it('validates options and honors zero/default timeout options', async () => {
    const fixture = startServer(answer(msgpackReply({ kind: 'pong' })));
    const port = await fixture.ready;
    const platform = createNodeTokenPlatform(options(port, { requestTimeoutMs: 0, connectTimeoutMs: 0 }));
    try {
      const transport = new TokenHttpTransport(new URL(`http://127.0.0.1:${port}/`), token, platform);
      await expect(transport.request({ op: 'ping' })).resolves.toEqual({ kind: 'pong' });
      await expect(Promise.resolve().then(() => createNodeTokenPlatform(options(port, { url: `http://user:pass@127.0.0.1:${port}` })))).rejects.toThrow();
    } finally { await shutdown(fixture.server, [platform]); }
  });

  it('uses auth header only and performs ping plus builder-backed msgpack execute without interner fields', async () => {
    const fixture = startServer((_request, response, seen) => {
      if (seen.method === 'GET') { response.writeHead(200, { 'content-type': 'application/msgpack' }); response.end(encode({ kind: 'pong' })); return; }
      response.writeHead(200, { 'content-type': 'application/msgpack' });
      response.end(encode({ kind: 'batch', response: { id: 'b', results: {}, execution_plan: [], execution_time_us: 1 } }));
    });
    const port = await fixture.ready;
    const platform = createNodeTokenPlatform(options(port));
    const transport = new TokenHttpTransport(new URL(`http://127.0.0.1:${port}/`), token, platform);
    try {
      await expect(transport.request({ op: 'ping' })).resolves.toEqual({ kind: 'pong' });
      const batch = Batch.create('b').add('read', Query.from('items').select(['name'])).build();
      await transport.request({ op: 'execute', db: 'app', batch, query_version: 4 });
      expect(fixture.seen).toHaveLength(2);
      for (const item of fixture.seen) {
        expect(item.authorization).toBe(`Bearer ${rawToken}`);
        expect(item.url).not.toContain(rawToken);
        expect(item.body.toString()).not.toContain(rawToken);
      }
      expect(fixture.seen[0]).toMatchObject({ method: 'GET', url: '/shamir/v1/ping', body: Buffer.alloc(0) });
      expect(fixture.seen[1]).toMatchObject({ method: 'POST', url: '/shamir/v1/request', contentType: 'application/msgpack' });
      const wire = decode(fixture.seen[1].body) as Record<string, unknown>;
      expect(wire).toMatchObject({ op: 'execute', db: 'app', query_version: 4 });
      expect(wire).not.toHaveProperty('interner_epochs');
      expect(wire).not.toHaveProperty('records_idmsgpack');
    } finally { await shutdown(fixture.server, [transport]); }
  });

  it.each([[400,'http_bad_request'],[401,'unauthorized'],[403,'protocol_error'],[404,'http_not_found'],[405,'http_method_not_allowed'],[408,'http_timeout'],[413,'http_body_too_large'],[415,'http_unsupported_media_type'],[431,'http_bad_headers'],[429,'rate_limited'],[503,'server_unavailable']] as const)('maps status %i to %s', async (status, code) => {
    const fixture = startServer(answer({ status, headers: { 'retry-after': '2' }, body: Buffer.from('peer secret') }));
    const port = await fixture.ready;
    const platform = createNodeTokenPlatform(options(port));
    const transport = new TokenHttpTransport(new URL(`http://127.0.0.1:${port}/`), token, platform);
    const result = transport.request({ op: 'ping' });
    try {
      await expect(result).rejects.toMatchObject({ code, ...(status === 429 || status === 503 ? { retryAfterMs: 2000, retryable: true } : {}) });
    } finally { await shutdown(fixture.server, [transport]); }
  });

  it('does not follow redirects', async () => {
    let secondHits = 0;
    const second = createServer((_request, response) => { secondHits++; response.end(encode({ kind: 'pong' })); });
    await new Promise<void>(resolve => second.listen(0, '127.0.0.1', resolve));
    const target = (second.address() as AddressInfo).port;
    const fixture = startServer((_request, response) => { response.writeHead(302, { location: `http://127.0.0.1:${target}/` }); response.end(); });
    const port = await fixture.ready;
    const platform = createNodeTokenPlatform(options(port));
    const transport = new TokenHttpTransport(new URL(`http://127.0.0.1:${port}/`), token, platform);
    const result = transport.request({ op: 'ping' });
    try { await expect(result).rejects.toMatchObject({ code: 'protocol_error' }); expect(fixture.seen).toHaveLength(1); expect(secondHits).toBe(0); }
    finally { await shutdown(fixture.server, [transport]); second.closeAllConnections(); await new Promise<void>(resolve => second.close(() => resolve())); }
  });

  it('caps streamed response at 16 MiB and maps an aborted response', async () => {
    const large = startServer((_request, response) => { response.writeHead(200, { 'content-type': 'application/msgpack' }); response.end(Buffer.alloc(16 * 1024 * 1024 + 1)); });
    const port = await large.ready;
    const platform = createNodeTokenPlatform(options(port));
    const transport = new TokenHttpTransport(new URL(`http://127.0.0.1:${port}/`), token, platform);
    const result = transport.request({ op: 'ping' });
    try { await expect(result).rejects.toMatchObject({ code: 'http_body_too_large' }); }
    finally { await shutdown(large.server, [transport]); }

    const broken = startServer(answer({ abort: true }));
    const brokenPort = await broken.ready;
    const brokenPlatform = createNodeTokenPlatform(options(brokenPort));
    const brokenTransport = new TokenHttpTransport(new URL(`http://127.0.0.1:${brokenPort}/`), token, brokenPlatform);
    const aborted = brokenTransport.request({ op: 'ping' });
    try { await expect(aborted).rejects.toMatchObject({ code: 'network_error' }); }
    finally { await shutdown(broken.server, [brokenTransport]); }
  });

  it('aborts on request deadline after the socket has connected', async () => {
    const accepted = deferred<void>();
    const fixture = startServer((_request, _response) => { accepted.resolve(); });
    const port = await fixture.ready;
    const platform = createNodeTokenPlatform(options(port, { requestTimeoutMs: 0 }));
    const transport = new TokenHttpTransport(new URL(`http://127.0.0.1:${port}/`), token, platform, { requestTimeoutMs: 40 });
    const result = transport.request({ op: 'ping' });
    try { const rejection = expect(result).rejects.toMatchObject({ code: 'client_timeout' }); await accepted.promise; await rejection; }
    finally { await shutdown(fixture.server, [transport]); }
  });

  it('honors platform lifetime abort signal on direct requests', async () => {
    const accepted = deferred<void>();
    const fixture = startServer((_request, _response) => { accepted.resolve(); });
    const port = await fixture.ready;
    const lifetime = new AbortController();
    const platform = createNodeTokenPlatform(options(port, { signal: lifetime.signal }));
    const requestSignal = new AbortController();
    expect(requestSignal.signal).not.toBe(lifetime.signal);
    const pending = platform.requestHttp(new URL(`http://127.0.0.1:${port}/shamir/v1/ping`), token, { method: 'GET', headers: {}, signal: requestSignal.signal, redirect: 'error', credentials: 'omit' });
    try {
      const rejection = expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
      await accepted.promise;
      lifetime.abort();
      await rejection;
      await expect(platform.requestHttp(new URL(`http://127.0.0.1:${port}/shamir/v1/ping`), token, { method: 'GET', headers: {}, signal: new AbortController().signal, redirect: 'error', credentials: 'omit' })).rejects.toMatchObject({ code: 'client_aborted' });
    } finally { await shutdown(fixture.server, [platform]); }
  });

  it('honors request and lifetime abort signals and closes active/idle owned agents only', async () => {
    const accepted = deferred<void>();
    let requests = 0;
    const fixture = startServer((_request, response) => {
      requests++;
      if (requests === 1) { response.writeHead(200, { 'content-type': 'application/msgpack' }); response.end(encode({ kind: 'pong' })); }
      else accepted.resolve();
    });
    const port = await fixture.ready;
    const platform = createNodeTokenPlatform(options(port));
    const transport = new TokenHttpTransport(new URL(`http://127.0.0.1:${port}/`), token, platform);
    try {
      await expect(transport.request({ op: 'ping' })).resolves.toEqual({ kind: 'pong' });
      const controller = new AbortController();
      const pending = transport.request({ op: 'ping' }, { signal: controller.signal });
      await accepted.promise;
      const rejection = expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
      controller.abort();
      await rejection;
      const agentDestroy = vi.spyOn(https.Agent.prototype, 'destroy');
      const globalAgentDestroy = vi.spyOn(https.globalAgent, 'destroy');
      try {
        const lifetimePending = transport.request({ op: 'ping' });
        await accepted.promise;
        const lifetimeRejection = expect(lifetimePending).rejects.toMatchObject({ code: 'connection_closed' });
        await transport.close();
        await lifetimeRejection;
        expect(agentDestroy).toHaveBeenCalledTimes(1);
        expect(globalAgentDestroy).not.toHaveBeenCalled();
        await expect(transport.request({ op: 'ping' })).rejects.toMatchObject({ code: 'connection_closed' });
      } finally {
        agentDestroy.mockRestore();
        globalAgentDestroy.mockRestore();
      }
    } finally { await shutdown(fixture.server, [transport]); }
  });

});
