import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { once } from 'node:events';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { NodePlatform } from '../../platform/node.js';
import { loginNode } from '../node/node.js';
import { nodeLoginHttp } from '../node/node-http.js';
import { encode as encodeWire } from '@msgpack/msgpack';

const bytes = (n: number, value = 1) => new Uint8Array(n).fill(value);
const encodeMessage = (value: unknown) => encodeWire(value, { useBigInt64: true });

const challenge = () => ({ protocol_version: 1, binding_mode: 3, security: 0,
  listener_id: bytes(16), handshake_id: bytes(32, 2), salt: bytes(16, 3), kdf: 'argon2id',
  memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19, server_nonce: bytes(32, 4) });
const read = async (req: IncomingMessage) => {
  const chunks: Buffer[] = [];
  for await (const part of req) chunks.push(Buffer.from(part));
  return Buffer.concat(chunks);
};
const send = (res: ServerResponse, status: number, value: unknown, type = 'application/msgpack', headers: Record<string, string> = {}) => {
  res.writeHead(status, { 'Content-Type': type, Connection: 'close', ...headers });
  res.end(encodeMessage(value));
};
async function fixture(handler: (req: IncomingMessage, res: ServerResponse) => void) {
  const server = createServer(handler);
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('missing loopback address');
  return { server, url: `http://127.0.0.1:${address.port}/` };
}
async function cleanup(server: ReturnType<typeof createServer>) {
  server.closeAllConnections();
  if (server.listening) { const closed = once(server, 'close'); server.close(); await closed; }
}
const auth = { username: 'alice', password: 'password-secret', url: '' };

const readAnd = (req: IncomingMessage, action: () => void, res: ServerResponse) => {
  void read(req).then(action).catch(() => { if (!res.destroyed) res.destroy(); });
};

afterEach(() => vi.restoreAllMocks());

describe('Node login lifecycle HTTP', () => {
  it('maps malformed begin and finish responses without leaking secrets', async () => {
    const cases: Array<{ name: string; send(res: ServerResponse): void; code: string }> = [
      { name: 'garbled MsgPack', send: r => { r.writeHead(200, { 'Content-Type': 'application/msgpack' }); r.end(Buffer.from([0xc1])); }, code: 'protocol_error' },
      { name: 'truncated MsgPack', send: r => { r.writeHead(200, { 'Content-Type': 'application/msgpack' }); r.end(Buffer.from([0x81, 0xa1])); }, code: 'protocol_error' },
      { name: 'oversized response', send: r => { r.writeHead(200, { 'Content-Type': 'application/msgpack' }); r.end(Buffer.alloc(4097)); }, code: 'http_body_too_large' },
      { name: 'wrong content type', send: r => send(r, 200, {}, 'text/plain'), code: 'http_unsupported_media_type' },
      { name: 'redirect', send: r => send(r, 302, {}, 'application/msgpack', { Location: 'http://127.0.0.1/elsewhere' }), code: 'protocol_error' },
    ];
    for (const phase of ['begin', 'finish'] as const) {
      for (const item of cases) {
        vi.spyOn(NodePlatform, 'argon2id').mockImplementation(async () => bytes(32, 9));
        let finishes = 0;
        const f = await fixture((req, res) => {
          readAnd(req, () => {
            if (req.url?.endsWith('/begin')) {
              if (phase === 'begin') item.send(res); else send(res, 200, challenge());
            } else {
              finishes++;
              if (phase === 'finish') item.send(res); else send(res, 500, {});
            }
          }, res);
        });
        try {
          const error = await loginNode({ ...auth, url: f.url }).catch(e => e);
          expect(error, `${phase}: ${item.name}`).toMatchObject({ code: item.code });
          expect(finishes).toBe(phase === 'finish' ? 1 : 0);
          expect(String(error)).not.toContain(auth.password);
          expect(String(error)).not.toContain('raw-token-secret');
          expect((error as Error & { cause?: unknown }).cause).toBeUndefined();
        } finally { await cleanup(f.server); }
      }
    }
  });

  it('accepts the transport 4096-byte limit before protocol decoding', async () => {
    const f = await fixture((_req, res) => {
      res.writeHead(200, { 'Content-Type': 'application/msgpack', Connection: 'close' });
      res.end(Buffer.alloc(4096));
    });
    const signal = new AbortController().signal;
    try {
      const response = await nodeLoginHttp.request(new URL(f.url), new Uint8Array(), signal, 1000);
      expect(response.body).toHaveLength(4096);
      await expect(loginNode({ ...auth, url: f.url })).rejects.toMatchObject({ code: 'protocol_error' });
    } finally { await cleanup(f.server); }
  });

  it('never follows redirects; enforces content type on success and error; maps HTTP status contracts', async () => {
    const matrix: Array<{ name: string; status: number; value: unknown; headers?: Record<string, string>; type?: string; code: string; retryAfterMs?: number }> = [
      { name: 'redirect', status: 302, value: {}, headers: { Location: 'http://127.0.0.1/elsewhere' }, code: 'protocol_error' },
      { name: 'wrong successful content type', status: 200, value: challenge(), type: 'text/plain', code: 'http_unsupported_media_type' },
      { name: 'wrong error content type', status: 401, value: { error: 'authentication_failed' }, type: 'text/plain', code: 'http_unsupported_media_type' },
      { name: '400 contract', status: 400, value: { error: 'invalid_request' }, code: 'invalid_request' },
      { name: '405 contract', status: 405, value: { error: 'method_not_allowed' }, code: 'http_method_not_allowed' },
      { name: '413 contract', status: 413, value: { error: 'request_too_large' }, code: 'http_body_too_large' },
      { name: '415 contract', status: 415, value: { error: 'unsupported_media_type' }, code: 'http_unsupported_media_type' },
      { name: '431 contract', status: 431, value: { error: 'request_headers_too_large' }, code: 'http_bad_headers' },
      { name: 'not found', status: 404, value: { error: 'not_found' }, code: 'unsupported_server' },
      { name: 'request timeout status', status: 408, value: { error: 'request_timeout' }, code: 'http_timeout' },
      { name: 'rate limit retry contract', status: 429, value: { error: 'rate_limited', retry_after: 7 }, headers: { 'Retry-After': '7' }, code: 'rate_limited', retryAfterMs: 7000 },
      { name: 'busy retry contract', status: 503, value: { error: 'server_busy', retry_after: 1 }, headers: { 'Retry-After': '1' }, code: 'server_busy', retryAfterMs: 1000 },
      { name: '429 retry header mismatch', status: 429, value: { error: 'rate_limited', retry_after: 7 }, headers: { 'Retry-After': '8' }, code: 'protocol_error' },
      { name: '503 retry header missing', status: 503, value: { error: 'server_busy', retry_after: 1 }, code: 'protocol_error' },
      { name: '429 wrong body marker', status: 429, value: { error: 'server_busy', retry_after: 7 }, headers: { 'Retry-After': '7' }, code: 'protocol_error' },
      ...(['origin_forbidden', 'authority_forbidden', 'proxy_forbidden', 'login_write_forbidden', 'token_self_service_disabled', 'login_read_only_replica', 'login_account_denied', 'token_label_exists', 'token_limit_reached'] as const).map(error => ({ name: `403 ${error}`, status: 403, value: { error }, code: error })),
      { name: 'unknown forbidden code', status: 403, value: { error: 'unknown' }, code: 'protocol_error' },
    ];
    for (const item of matrix) {
      let requests = 0;
      const f = await fixture((_req, res) => { requests++; send(res, item.status, item.value, item.type, item.headers); });
      try {
        const error = await loginNode({ ...auth, url: f.url }).catch(e => e);
        expect(error, item.name).toMatchObject({ code: item.code });
        if (item.retryAfterMs !== undefined) expect(error.retryAfterMs).toBe(item.retryAfterMs);
        if (item.name === 'redirect') expect(requests).toBe(1);
      } finally { await cleanup(f.server); }
    }
  });

  it('rejects even an empty Content-Encoding response', async () => {
    const f = await fixture((_req, res) => {
      res.writeHead(200, { 'Content-Type': 'application/msgpack', 'Content-Encoding': '', Connection: 'close' });
      res.end(encodeMessage(challenge()));
    });
    try {
      await expect(loginNode({ ...auth, url: f.url })).rejects.toMatchObject({ code: 'protocol_error' });
    } finally { await cleanup(f.server); }
  });

  it('handles dropped connections during begin and finish', async () => {
    vi.spyOn(NodePlatform, 'argon2id').mockImplementation(async () => bytes(32, 9));
    for (const phase of ['begin', 'finish'] as const) {
      let finishes = 0;
      const f = await fixture((req, res) => {
        readAnd(req, () => {
          if (req.url?.endsWith('/begin') && phase === 'begin') { req.socket.destroy(); return; }
          if (req.url?.endsWith('/begin')) send(res, 200, challenge());
          else { finishes++; req.socket.destroy(); }
        }, res);
      });
      try {
        await expect(loginNode({ ...auth, url: f.url })).rejects.toMatchObject({ code: 'network_error' });
        expect(finishes).toBe(phase === 'finish' ? 1 : 0);
      } finally { await cleanup(f.server); }
    }
  });

  it('aborts and times out during begin and finish, and rejects an already-aborted signal', async () => {
    vi.spyOn(NodePlatform, 'argon2id').mockImplementation(async () => bytes(32, 9));
    for (const phase of ['begin', 'finish'] as const) {
      for (const mode of ['abort', 'timeout'] as const) {
        let reached!: () => void;
        const entered = new Promise<void>(resolve => { reached = resolve; });
        let finishReached = false;
        const f = await fixture((req, res) => {
          readAnd(req, () => {
            if (req.url?.endsWith('/begin')) {
              if (phase === 'begin') reached(); else send(res, 200, challenge());
            } else { finishReached = true; reached(); }
          }, res);
        });
        const controller = new AbortController();
        try {
          const promise = loginNode({ ...auth, url: f.url, signal: controller.signal, ...(mode === 'timeout' ? { [phase === 'begin' ? 'connectTimeoutMs' : 'requestTimeoutMs']: 20 } : {}) });
          const rejection = expect(promise).rejects.toMatchObject({ code: mode === 'abort' ? 'client_aborted' : 'client_timeout' });
          if (mode === 'abort') await entered;
          if (mode === 'abort') controller.abort();
          await rejection;
          if (mode === 'abort' && phase === 'finish') expect(finishReached).toBe(true);
        } finally { await cleanup(f.server); }
      }
    }
    const controller = new AbortController(); controller.abort();
    await expect(loginNode({ ...auth, url: 'http://127.0.0.1/', signal: controller.signal })).rejects.toMatchObject({ code: 'client_aborted' });
  });
});
