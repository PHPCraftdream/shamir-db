import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { once } from 'node:events';
import { EventEmitter } from 'node:events';
import type { ClientRequest } from 'node:http';
import type { RequestOptions } from 'node:http';
import type { RequestOptions as HttpsRequestOptions } from 'node:https';
import { describe, expect, it, vi, afterEach } from 'vitest';
import { decode as decodeWire, encode as encodeWire } from '@msgpack/msgpack';
import * as NodePlatform from '../../platform/node.js';
import { loginNode } from '../node/node.js';
import { createNodeLoginHttp } from '../node/node-http.js';
type Fields = Record<string, unknown>;
function encodeMessage(value: unknown): Uint8Array { return encodeWire(value, { useBigInt64: true }); }
function decodeRecord(buffer: Uint8Array): Fields {
  const value: unknown = decodeWire(buffer, { useBigInt64: true });
  if (!value || typeof value !== 'object' || Array.isArray(value) || value instanceof Uint8Array) throw new Error('expected MsgPack record');
  return value as Fields;
}

const bytes = (n: number, value: number) => new Uint8Array(n).fill(value);
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });
type FakeResponse = EventEmitter & { statusCode: number; headers: Record<string, string>; destroy: () => void };
type FakeRequest = EventEmitter & { end: (body?: Uint8Array) => void; destroy: () => void };
function injectedHttp(onRequest: (request: FakeRequest, response: (response: FakeResponse) => void) => void, tlsOptions?: (options: RequestOptions | HttpsRequestOptions) => void) {
  const requester = (_url: URL, options: RequestOptions | HttpsRequestOptions, callback: (response: IncomingMessage) => void): ClientRequest => {
    const request = Object.assign(new EventEmitter(), { end: (_body?: Uint8Array) => {}, destroy: () => {} }) as FakeRequest;
    if (tlsOptions) tlsOptions(options);
    onRequest(request, response => callback(response as unknown as IncomingMessage));
    return request as unknown as ClientRequest;
  };
  return createNodeLoginHttp(requester, requester);
}
function fakeResponse(): FakeResponse { return Object.assign(new EventEmitter(), { statusCode: 200, headers: { 'content-type': 'application/msgpack' }, destroy: () => {} }); }
const body = (value: unknown) => encodeMessage(value);
const readBody = async (req: IncomingMessage): Promise<Uint8Array> => {
  const chunks: Uint8Array[] = [];
  for await (const chunk of req) chunks.push(new Uint8Array(chunk));
  return Buffer.concat(chunks);
};

async function listen(handler: (req: IncomingMessage, res: ServerResponse) => void) {
  const server = createServer(handler);
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('missing loopback address');
  return { server, url: `http://127.0.0.1:${address.port}/` };
}

function reply(res: ServerResponse, status: number, value: unknown, contentType = 'application/msgpack') {
  res.writeHead(status, { 'Content-Type': contentType, Connection: 'close' });
  res.end(body(value));
}

function close(server: ReturnType<typeof createServer>): Promise<void> {
  server.closeAllConnections();
  const closed = once(server, 'close');
  server.close();
  return closed.then(() => undefined);
}

describe('injected Node login transport', () => {
  it('copies successful response views and wipes their source', async () => {
    const backing = Buffer.from([8, 1, 2, 3, 9]); const source = backing.subarray(1, 4);
    const http = injectedHttp((_request, respond) => { const response = fakeResponse(); respond(response); response.emit('data', source); response.emit('end'); });
    const result = await http.request(new URL('http://example.test'), new Uint8Array(), new AbortController().signal, 0);
    expect([...result.body]).toEqual([1, 2, 3]);
    expect([...source]).toEqual([0, 0, 0]);
    expect([...backing]).toEqual([8, 0, 0, 0, 9]);
  });

  it('wipes earlier and offending views when the body exceeds the limit', async () => {
    const first = Buffer.alloc(3000, 1); const offending = Buffer.alloc(2000, 2);
    const http = injectedHttp((_request, respond) => { const response = fakeResponse(); respond(response); response.emit('data', first); response.emit('data', offending); });
    await expect(http.request(new URL('http://example.test'), new Uint8Array(), new AbortController().signal, 0)).rejects.toMatchObject({ code: 'http_body_too_large' });
    expect(first.every(value => value === 0)).toBe(true); expect(offending.every(value => value === 0)).toBe(true);
  });

  it.each(['error', 'aborted', 'close'] as const)('wipes chunks on response %s', async event => {
    const source = Buffer.alloc(3, 7);
    const http = injectedHttp((_request, respond) => { const response = fakeResponse(); respond(response); response.emit('data', source); response.emit(event); });
    await expect(http.request(new URL('http://example.test'), new Uint8Array(), new AbortController().signal, 0)).rejects.toMatchObject({ code: 'network_error' });
    expect(source.every(value => value === 0)).toBe(true);
  });

  it('wipes late data after caller abort', async () => {
    let response!: FakeResponse; const source = Buffer.alloc(2, 9); const controller = new AbortController();
    const http = injectedHttp((_request, respond) => { response = fakeResponse(); respond(response); });
    const pending = http.request(new URL('http://example.test'), new Uint8Array(), controller.signal, 0); controller.abort();
    await expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
    response.emit('data', source); expect(source.every(value => value === 0)).toBe(true);
  });

  it('wipes late data after timeout', async () => {
    vi.useFakeTimers(); let response!: FakeResponse; const source = Buffer.alloc(2, 9);
    const http = injectedHttp((_request, respond) => { response = fakeResponse(); respond(response); });
    const pending = http.request(new URL('http://example.test'), new Uint8Array(), new AbortController().signal, 10);
    const rejection = expect(pending).rejects.toMatchObject({ code: 'client_timeout' });
    await vi.advanceTimersByTimeAsync(10); await rejection;
    response.emit('data', source); expect(source.every(value => value === 0)).toBe(true);
  });

  it('wipes late data after rejected headers', async () => {
    const source = Buffer.alloc(2, 9); let response!: FakeResponse;
    const http = injectedHttp((_request, respond) => { response = fakeResponse(); response.headers['content-encoding'] = 'gzip'; respond(response); });
    await expect(http.request(new URL('http://example.test'), new Uint8Array(), new AbortController().signal, 0)).rejects.toMatchObject({ code: 'protocol_error' });
    response.emit('data', source); expect(source.every(value => value === 0)).toBe(true);
  });

  it('keeps synchronous requester completion from ending the body or starting a timer', async () => {
    vi.useFakeTimers(); let endCalls = 0; let destroyCalls = 0; let response!: FakeResponse;
    const requester = (_url: URL, _options: RequestOptions | HttpsRequestOptions, callback: (response: IncomingMessage) => void): ClientRequest => {
      const request = Object.assign(new EventEmitter(), { end: () => { endCalls++; }, destroy: () => { destroyCalls++; } }) as FakeRequest;
      response = fakeResponse(); callback(response as unknown as IncomingMessage);
      response.emit('end');
      return request as unknown as ClientRequest;
    };
    const http = createNodeLoginHttp(requester, requester);
    const result = await http.request(new URL('http://example.test'), new Uint8Array(), new AbortController().signal, 10);
    expect(result.status).toBe(200); expect(endCalls).toBe(0); expect(destroyCalls).toBe(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('passes verified TLS 1.3 options and the CA bundle', async () => {
    let captured: RequestOptions | HttpsRequestOptions | undefined; let response!: FakeResponse;
    const ca = new Uint8Array([1, 2, 3]);
    const http = injectedHttp((_request, respond) => { response = fakeResponse(); respond(response); response.emit('end'); }, options => { captured = options; });
    const pending = http.request(new URL('https://example.test'), new Uint8Array(), new AbortController().signal, 0, ca);
    response.emit('end'); await pending;
    expect(captured).toMatchObject({ rejectUnauthorized: true, minVersion: 'TLSv1.3', ALPNProtocols: ['http/1.1'], ca: Buffer.from(ca) });
    expect(captured).not.toHaveProperty('rejectUnauthorized', false);
  });

  it('does not pass TLS options to the HTTP requester', async () => {
    let captured: RequestOptions | HttpsRequestOptions | undefined;
    const http = injectedHttp((_request, respond) => { const response = fakeResponse(); respond(response); response.emit('end'); }, options => { captured = options; });
    await http.request(new URL('http://example.test'), new Uint8Array(), new AbortController().signal, 0, new Uint8Array([1, 2]));
    expect(captured).not.toHaveProperty('ca');
    expect(captured).not.toHaveProperty('rejectUnauthorized');
  });

  it('passes a string CA bundle unchanged', async () => {
    let captured: RequestOptions | HttpsRequestOptions | undefined;
    const http = injectedHttp((_request, respond) => { const response = fakeResponse(); respond(response); response.emit('end'); }, options => { captured = options; });
    await http.request(new URL('https://example.test'), new Uint8Array(), new AbortController().signal, 0, 'test CA');
    expect(captured).toHaveProperty('ca', 'test CA');
  });
});

describe('loginNode HTTP contract', () => {
  it('sends canonical begin fields and reports the public authentication failure', async () => {
    let begin: Record<string, unknown> | undefined;
    const fixture = await listen((req, res) => {
      void readBody(req).then(raw => {
        const message = decodeRecord(raw);
        if (req.url?.endsWith('/begin')) {
          begin = message;
          reply(res, 401, { error: 'authentication_failed' });
        } else reply(res, 401, { error: 'authentication_failed' });
      }).catch(() => res.destroy());
    });
    try {
      await expect(loginNode({ username: 'ALICE', password: 'secret', url: fixture.url }))
        .rejects.toMatchObject({ code: 'authentication_failed' });
      expect(begin).toMatchObject({ protocol_version: 1, user: 'alice', binding_mode: 3,
        token: { label: 'http-login', mode: 'read_only', channels: 'rest', databases: null } });
      expect(begin?.client_nonce).toBeInstanceOf(Uint8Array);
    } finally { await close(fixture.server); }
  });

  it('rejects an unknown challenge field before invoking finish', async () => {
    let finishes = 0;
    const argon = vi.spyOn(NodePlatform.NodePlatform, 'argon2id');
    const fixture = await listen((req, res) => {
      void readBody(req).then(() => {
        if (req.url?.endsWith('/begin')) reply(res, 200, {
          protocol_version: 1, binding_mode: 3, security: 0, listener_id: bytes(16, 1),
          handshake_id: bytes(32, 2), salt: bytes(16, 3), kdf: 'argon2id', memory_kb: 19456,
          time: 2, parallelism: 1, argon2_version: 19, server_nonce: bytes(32, 4), extra: true,
        });
        else { finishes++; reply(res, 500, {}); }
      }).catch(() => res.destroy());
    });
    try {
      await expect(loginNode({ username: 'alice', password: 'secret', url: fixture.url }))
        .rejects.toMatchObject({ code: 'protocol_error' });
      expect(finishes).toBe(0);
      expect(argon).not.toHaveBeenCalled();
    } finally { argon.mockRestore(); await close(fixture.server); }
  });

  it('rejects signed or out-of-range challenge integers before Argon2', async () => {
    let finishes = 0;
    const argon = vi.spyOn(NodePlatform.NodePlatform, 'argon2id');
    const fixture = await listen((req, res) => {
      void readBody(req).then(() => {
        if (req.url?.endsWith('/begin')) reply(res, 200, {
          protocol_version: 1, binding_mode: 3, security: 0, listener_id: bytes(16, 1),
          handshake_id: bytes(32, 2), salt: bytes(16, 3), kdf: 'argon2id', memory_kb: 19456,
          time: -1, parallelism: 1, argon2_version: 19, server_nonce: bytes(32, 4),
        });
        else { finishes++; reply(res, 500, {}); }
      }).catch(() => res.destroy());
    });
    try {
      await expect(loginNode({ username: 'alice', password: 'secret', url: fixture.url }))
        .rejects.toMatchObject({ code: 'protocol_error' });
      expect(finishes).toBe(0);
      expect(argon).not.toHaveBeenCalled();
    } finally { argon.mockRestore(); await close(fixture.server); }
  });
});
