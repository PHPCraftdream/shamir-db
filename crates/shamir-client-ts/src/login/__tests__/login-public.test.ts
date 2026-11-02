import { createHash, createHmac, webcrypto } from 'node:crypto';
import { inspect } from 'node:util';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { login as loginNode } from '../../index.js';
import { login as loginBrowser } from '../../browser.js';
import { ApiToken } from '../../entry/api-token.js';
import * as BrowserPlatform from '../../platform/browser.js';
import * as NodePlatform from '../../platform/node.js';
import { decode as decodeWire, encode as encodeWire } from '@msgpack/msgpack';
import type { LoginHttp } from '../core.js';
import type { TokenPlatform, TokenSocket, TokenSocketCloseEvent } from '../../platform/token/types.js';
import { decodeTokenFrame, decodeTokenPayload, encodeTokenPayload } from '../../core/protocol/token/ws.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';
import type { TokenHttpClient } from '../../entry/token-client.js';
import type { TokenWsClient } from '../../entry/ws/token-client.js';

const secret = `shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`;
const bytes = (n: number, value: number) => new Uint8Array(n).fill(value);
const enc = new TextEncoder();
const dec = (data: Uint8Array): unknown => decodeWire(data, { useBigInt64: true });
const encRecord = (value: unknown): Uint8Array => encodeWire(value, { useBigInt64: true });
function decodeRecord(data: Uint8Array): Record<string, unknown> {
  const value = dec(data);
  if (value === null || typeof value !== 'object' || Array.isArray(value) || value instanceof Uint8Array) throw new Error('expected record');
  return value as Record<string, unknown>;
}

const u16 = (n: number) => Buffer.from([n >>> 8, n & 255]);
const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32BE(n); return b; };
const sha = (b: Uint8Array) => createHash('sha256').update(b).digest();
const hmac = (k: Uint8Array, b: Uint8Array) => createHmac('sha256', k).update(b).digest();
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

// Independent fake-server oracle: deliberately builds the wire transcript itself.
function oracleTranscript(nonce: Uint8Array): Buffer {
  const label = enc.encode('http-login');
  return Buffer.concat([Buffer.from('SHAMIR-HTTP-LOGIN-v1'), Buffer.from([1, 3, 0]), Buffer.alloc(16, 1), Buffer.alloc(32, 2), u16(5), Buffer.from('alice'), Buffer.from(nonce), Buffer.alloc(32, 4), Buffer.alloc(16, 3), u32(19456), u32(2), u32(1), Buffer.from([19]), u16(label.length), label, Buffer.from([0, 2, 255])]);
}
function installCrypto(): void {
  vi.stubGlobal('crypto', webcrypto);
  vi.spyOn(BrowserPlatform.BrowserPlatform, 'argon2id').mockImplementation(async password => hmac(password, Buffer.from('test salted password')));
  vi.spyOn(NodePlatform.NodePlatform, 'argon2id').mockImplementation(async password => hmac(password, Buffer.from('test salted password')));
}
function loginHttp(): LoginHttp {
  let nonce: Uint8Array | undefined;
  return { async request(url, body) {
    if (url.pathname.endsWith('/begin')) { nonce = decodeRecord(body).client_nonce as Uint8Array; return { status: 200, contentType: 'application/msgpack', retryAfter: null, body: encRecord({ protocol_version: 1, binding_mode: 3, security: 0, listener_id: bytes(16, 1), handshake_id: bytes(32, 2), salt: bytes(16, 3), kdf: 'argon2id', memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19, server_nonce: bytes(32, 4) }) }; }
    if (!nonce) throw new Error('missing begin');
    const t = oracleTranscript(nonce);
    const salted = hmac(enc.encode('correct horse'), Buffer.from('test salted password'));
    const clientKey = hmac(salted, Buffer.from('Client Key'));
    const stored = sha(clientKey);
    const signature = hmac(stored, t);
    const proof = Buffer.alloc(32); for (let i = 0; i < 32; i++) proof[i] = clientKey[i]! ^ signature[i]!;
    expect(Buffer.from(decodeRecord(body).client_proof as Uint8Array).equals(proof)).toBe(true);
    const serverKey = hmac(salted, Buffer.from('Server Key'));
    const created = BigInt(Date.now() - 1000) * 1_000_000n;
    return { status: 200, contentType: 'application/msgpack', retryAfter: null, body: encRecord({ protocol_version: 1, server_signature: hmac(serverKey, t), token: secret, id: bytes(16, 0), label: 'http-login', owner: 'alice', mode: 'read_only', channels: 'both', databases: null, created_at_ns: created, expires_at_ns: created + 60_000_000_000n }) };
  } };
}
class Socket implements TokenSocket {
  sent: Uint8Array[] = []; private handler: ((data: Uint8Array) => void) | undefined; private early: Uint8Array[] = [];
  send(data: Uint8Array): void { this.sent.push(data); const frame = decodeTokenFrame(data); if ('token' in frame) return; const req = decodeTokenPayload(frame.req as Uint8Array) as { op?: string }; if (req.op === 'ping') this.deliver({ rid: frame.rid, res: encodeTokenPayload({ kind: 'pong' }) }); }
  onMessage(handler: (data: Uint8Array) => void): void { this.handler = handler; for (const data of this.early.splice(0)) handler(data); }
  onClose(_handler: (event: TokenSocketCloseEvent) => void): void {}
  close(): Promise<void> { return Promise.resolve(); }
  deliver(value: unknown): void { const data = encodeTokenPayload(value); if (this.handler) this.handler(data); else this.early.push(data); }
}
function platform(runtime: 'browser' | 'node', socket = new Socket()): TokenPlatform & { requests: URL[] } {
  const requests: URL[] = [];
  return { runtime, requests, async requestHttp(url) { requests.push(url); return { status: 200, headers: { get: () => 'application/msgpack' }, body: encodeTokenPayload({ kind: 'pong' }) }; }, async openTokenSocket() { if (runtime === 'browser') socket.send(encodeTokenPayload({ token: secret })); await socket.deliver({ ok: true, sid: bytes(32, 1), expires_at_ns: BigInt(Date.now() + 3_600_000) * 1_000_000n }); return socket; }, async close() {} };
}

describe('public login token handoff', () => {
  it('logs in through the browser public entry and continues over HTTP and buffered WS', async () => {
    installCrypto();
    const browserFactory = vi.spyOn(BrowserPlatform, 'createBrowserTokenPlatform');
    const result = await loginBrowser({ username: 'ALICE', password: 'correct horse', url: 'http://127.0.0.1/', scope: { label: 'http-login', mode: 'read_only', channels: 'both' } }, loginHttp());
    expect(result.token).toBeInstanceOf(ApiToken); expect(result.metadata.owner).toBe('alice');
    const httpPlatform = platform('browser'); browserFactory.mockReturnValue(httpPlatform);
    const http: Promise<TokenHttpClient> = result.connect({ channel: 'http' });
    const httpClient = await http; await expect(httpClient.ping()).resolves.toEqual({ kind: 'pong' });
    expect(browserFactory).toHaveBeenCalledWith(expect.objectContaining({ channel: 'http', token: result.token })); expect(httpPlatform.requests).toHaveLength(1); await httpClient.close();
    const wsSocket = new Socket(); const wsPlatform = platform('browser', wsSocket); browserFactory.mockReturnValue(wsPlatform);
    const ws: Promise<TokenWsClient> = result.connect({ channel: 'ws' }); const wsClient = await ws;
    await expect(wsClient.ping()).resolves.toEqual({ kind: 'pong' });
    expect(browserFactory).toHaveBeenCalledWith(expect.objectContaining({ channel: 'ws', token: result.token, url: 'ws://127.0.0.1' }));
    expect(wsPlatform.requests).toHaveLength(0); expect(wsSocket.sent).toHaveLength(2); expect(decodeTokenFrame(wsSocket.sent[0]!)).toEqual({ token: secret }); await wsClient.close();
    const publicJson = JSON.stringify(result, (_key: string, value: unknown): unknown => (typeof value === 'bigint' ? value.toString() : value)); expect(publicJson).not.toContain(secret); expect(String(result)).not.toContain(secret); expect(inspect(result)).not.toContain(secret);
  });

  it('uses Node public login continuation over HTTP and WS without sending a token frame', async () => {
    installCrypto();
    const nodeFactory = vi.spyOn(NodePlatform, 'createNodeTokenPlatform');
    const result = await loginNode({ username: 'alice', password: 'correct horse', url: 'http://127.0.0.1/', scope: { label: 'http-login', channels: 'both' } }, loginHttp());
    const nodePlatform = platform('node'); nodeFactory.mockReturnValue(nodePlatform);
    const client: TokenHttpClient = await result.connect({ channel: 'http' });
    await expect(client.ping()).resolves.toEqual({ kind: 'pong' }); await client.close();
    expect(nodeFactory).toHaveBeenCalledWith(expect.objectContaining({ token: result.token, channel: 'http' }));
    expect(nodePlatform.requests).toHaveLength(1);

    const wsSocket = new Socket(); const wsPlatform = platform('node', wsSocket); nodeFactory.mockReturnValue(wsPlatform);
    const ws: Promise<TokenWsClient> = result.connect({ channel: 'ws' }); const wsClient = await ws;
    await expect(wsClient.ping()).resolves.toEqual({ kind: 'pong' });
    expect(nodeFactory).toHaveBeenCalledWith(expect.objectContaining({ token: result.token, channel: 'ws', url: 'ws://127.0.0.1' }));
    expect(wsPlatform.requests).toHaveLength(0); expect(wsSocket.sent).toHaveLength(1);
    expect(decodeTokenFrame(wsSocket.sent[0]!)).not.toHaveProperty('token');
    await wsClient.close();
    const handle = result.token; expect(handle.toString()).toBe('[REDACTED]'); expect(JSON.stringify(handle)).not.toContain(secret); expect(inspect(handle)).not.toContain(secret);
  });

  it('rejects post-creation expiry with typed token-expired error and no secret disclosure', async () => {
    installCrypto();
    const factory = vi.spyOn(BrowserPlatform, 'createBrowserTokenPlatform');
    const fake = platform('browser'); factory.mockReturnValue(fake);
    const result = await loginBrowser({ username: 'alice', password: 'correct horse', url: 'http://127.0.0.1/', scope: { channels: 'both' } }, loginHttp());
    const clientPromise = result.connect({ channel: 'http' });
    const client = await clientPromise;
    const db = client.db('app');
    const query = db.query('items').select(['name']);
    const batch = db.batch();
    const built = batch.build();
    const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 120_000);
    try {
      await expect(client.ping()).rejects.toBeInstanceOf(TokenTransportError);
      await expect(client.ping()).rejects.toMatchObject({ code: 'token_expired' });
      await expect(client.ping()).rejects.toSatisfy((error: unknown) => /log in again/i.test(String(error)));
      await expect(result.connect({ channel: 'ws' })).rejects.toMatchObject({ code: 'token_expired' });
      await expect(db.run(query)).rejects.toMatchObject({ code: 'token_expired' });
      await expect(db.execute(built)).rejects.toMatchObject({ code: 'token_expired' });
      await expect(query.ex()).rejects.toMatchObject({ code: 'token_expired' });
      await expect(batch.run()).rejects.toMatchObject({ code: 'token_expired' });
      expect(fake.requests).toHaveLength(0);
    } finally {
      clock.mockRestore();
      await client.close();
    }
  });

  it('keeps browser runtime imports free of Node and websocket packages', async () => {
    const { readFile } = await import('node:fs/promises');
    const { dirname, resolve } = await import('node:path');
    const { fileURLToPath } = await import('node:url');
    const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../');
    const visited = new Set<string>();
    const walk = async (file: string): Promise<void> => {
      const path = resolve(file); if (visited.has(path)) return; visited.add(path);
      const source = await readFile(path, 'utf8');
      expect(source).not.toMatch(/(?:from\s*|import\s*\()\s*['"](?:node:[^'"]+|ws(?:\/[^'"]*)?)['"]/);
      for (const match of source.matchAll(/(?:from\s*|import\s*\()(['"])(\.\.?\/[^'"]+)\1/g)) {
        const target = resolve(dirname(path), match[2]!.replace(/\.js$/, '.ts')); await readFile(target, 'utf8'); await walk(target);
      }
    };
    await walk(resolve(root, 'browser.ts'));
    expect([...visited].some(path => path.endsWith('/platform/connect/node-connect.ts'))).toBe(false);
    const nodeGraph = new Set<string>();
    const walkNode = async (file: string): Promise<void> => { const path = resolve(file); if (nodeGraph.has(path)) return; nodeGraph.add(path); const source = await readFile(path, 'utf8'); expect(source).not.toContain('connectTokenBrowser'); for (const match of source.matchAll(/(?:from\s*|import\s*\()(['"])(\.\.?\/[^'"]+)\1/g)) { const target = resolve(dirname(path), match[2]!.replace(/\.js$/, '.ts')); await readFile(target, 'utf8'); await walkNode(target); } };
    await walkNode(resolve(root, 'index.ts'));
  });
});

it('retains the SCRAM connect overload as a compile-only probe', () => {
  type Assert<T extends true> = T;
  type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends (<T>() => T extends B ? 1 : 2) ? true : false;
  const probe = (): void => { const node: Promise<import('../../core/client/client.js').ShamirClient> = import('../../index.js').then(({ connect }) => connect({ host: 'localhost', port: 7000, username: 'alice', password: 'password' })); void node; };
  type Result = Assert<Equal<ReturnType<typeof loginNode>, ReturnType<typeof loginBrowser>>>;
  void probe; void (0 as unknown as Result);
});
