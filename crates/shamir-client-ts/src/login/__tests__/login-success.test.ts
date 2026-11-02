import { createHash, createHmac } from 'node:crypto';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { once } from 'node:events';
import { inspect } from 'node:util';
import { describe, expect, it, vi } from 'vitest';
import { login } from '../../index.js';
import { NodePlatform } from '../../platform/node.js';
import { ApiToken } from '../../entry/api-token.js';
import { decode as decodeWire, encode as encodeWire } from '@msgpack/msgpack';
type Fields = Record<string, unknown>;
function encodeMessage(value: unknown): Uint8Array { return encodeWire(value, { useBigInt64: true }); }
function decodeRecord(buffer: Uint8Array): Fields {
  // The mock only needs the incoming request fields; decode narrowly as a record.
  const value: unknown = decodeWire(buffer, { useBigInt64: true });
  if (!value || typeof value !== 'object' || Array.isArray(value) || value instanceof Uint8Array) throw new Error('expected MsgPack record');
  return value as Fields;
}
const bytes = (n: number, value: number) => new Uint8Array(n).fill(value);
const enc = new TextEncoder();
const u16 = (n: number) => Buffer.from([n >>> 8, n & 255]);
const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32BE(n); return b; };
const sha256 = (b: Uint8Array) => createHash('sha256').update(b).digest();
const hmac = (key: Uint8Array, b: Uint8Array) => createHmac('sha256', key).update(b).digest();

// This oracle intentionally does not call production transcript or wire helpers.
function transcript(user: string, nonce: Uint8Array, databases: readonly string[] | null): Buffer {
  const label = enc.encode('http-login');
  const scope = [u16(label.length), label, Buffer.from([0, 0, databases === null ? 255 : databases.length])];
  for (const name of databases ?? []) { const b = enc.encode(name); scope.push(u16(b.length), b); }
  return Buffer.concat([
    Buffer.from('SHAMIR-HTTP-LOGIN-v1', 'ascii'), Buffer.from([1, 3, 0]),
    Buffer.alloc(16, 1), Buffer.alloc(32, 2), u16(enc.encode(user).length), enc.encode(user),
    Buffer.from(nonce), Buffer.alloc(32, 4), Buffer.alloc(16, 3), u32(19456), u32(2), u32(1),
    Buffer.from([19]), ...scope,
  ]);
}
function readBody(req: IncomingMessage): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => chunks.push(chunk));
    req.on('end', () => resolve(Buffer.concat(chunks)));
    req.on('error', reject);
  });
}
function send(res: ServerResponse, status: number, value: unknown): void {
  res.writeHead(status, { 'Content-Type': 'application/msgpack', Connection: 'close' });
  res.end(Buffer.from(encodeMessage(value)));
}
async function fixture(password: string, options: { realArgon?: boolean; signatureTamper?: boolean; mutate?: (reply: Fields) => void } = {}) {
  let calls = 0;
  let proofChecks = 0;
  const server = createServer((req, res) => {
    void (async () => {
      const body = await readBody(req);
      const message = decodeRecord(body);
      if (req.url?.endsWith('/begin')) {
        calls++;
        if (req.method !== 'POST' || req.headers['content-type'] !== 'application/msgpack' || req.headers.authorization !== undefined || body.length === 0) throw new Error('invalid begin HTTP request');
        if (Object.keys(message).sort().join(',') !== 'binding_mode,client_nonce,protocol_version,token,user') throw new Error('invalid begin fields');
        if (message.protocol_version !== 1 || message.binding_mode !== 3 || (message.user !== 'alice' && message.user !== 'unknown')) throw new Error('unexpected begin');
        if (!message.token || typeof message.token !== 'object' || Array.isArray(message.token) || message.token instanceof Uint8Array) throw new Error('invalid begin token');
        const token = message.token as Fields;
        if (Object.keys(token).sort().join(',') !== 'channels,databases,label,mode') throw new Error('invalid token fields');
        if (token.label !== 'http-login' || token.mode !== 'read_only' || token.channels !== 'rest' || token.databases !== null) throw new Error('unexpected token scope');
        currentUser = message.user as string;
        const nonce = message.client_nonce;
        if (!(nonce instanceof Uint8Array) || nonce.length !== 32 || nonce.every(byte => byte === 0)) throw new Error('invalid client nonce');
        send(res, 200, {
          protocol_version: 1, binding_mode: 3, security: 0, listener_id: bytes(16, 1), handshake_id: bytes(32, 2),
          salt: bytes(16, 3), kdf: 'argon2id', memory_kb: 19456, time: 2, parallelism: 1,
          argon2_version: 19, server_nonce: bytes(32, 4),
        });
        // Keep the nonce associated with this one-request fixture for finish.
        currentNonce = new Uint8Array(nonce);
        return;
      }
      if (!req.url?.endsWith('/finish') || !currentNonce) throw new Error('unexpected route');
      if (req.method !== 'POST' || req.headers['content-type'] !== 'application/msgpack' || req.headers.authorization !== undefined || body.length === 0) throw new Error('invalid finish HTTP request');
      if (Object.keys(message).sort().join(',') !== 'client_proof,handshake_id,protocol_version') throw new Error('invalid finish fields');
      const t = transcript(currentUser ?? 'alice', currentNonce, null);
      const pw = enc.encode(password);
      const salted = options.realArgon
        ? await NodePlatform.argon2id(pw, bytes(16, 3), { memoryKb: 19456, time: 2, parallelism: 1 })
        : hmac(pw, Buffer.concat([Buffer.from('test salted password'), Buffer.from([3])]));
      const clientKey = hmac(salted, Buffer.from('Client Key'));
      const serverKey = hmac(salted, Buffer.from('Server Key'));
      const clientSignature = hmac(sha256(clientKey), t);
      const expectedProof = Buffer.alloc(32);
      for (let i = 0; i < 32; i++) expectedProof[i] = clientKey[i] ^ clientSignature[i];
      if (currentUser === 'unknown' || !(message.client_proof instanceof Uint8Array) || !Buffer.from(message.client_proof).equals(expectedProof)) {
        proofChecks++;
        send(res, 401, { error: 'authentication_failed' });
        return;
      }
      proofChecks++;
      const created = BigInt(Date.now() - 1000) * 1_000_000n;
      const token = `shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`;
      const reply: Fields = {
        protocol_version: 1, server_signature: hmac(serverKey, t), token, id: bytes(16, 0),
        label: 'http-login', owner: currentUser ?? 'alice', mode: 'read_only', channels: 'rest', databases: null,
        created_at_ns: created, expires_at_ns: created + 60_000_000_000n,
      };
      if (options.signatureTamper) (reply.server_signature as Uint8Array)[0] ^= 1;
      options.mutate?.(reply);
      send(res, 200, reply);
    })().catch(() => {
      if (!res.headersSent) send(res, 500, { error: 'fixture_failure' });
      else res.destroy();
    });
  });
  let currentNonce: Uint8Array | undefined;
  let currentUser: string | undefined;
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('missing loopback address');
  return {
    server, url: `http://127.0.0.1:${address.port}/`,
    get calls() { return calls; }, get proofChecks() { return proofChecks; },
  };
}
async function close(server: ReturnType<typeof createServer>): Promise<void> {
  server.closeAllConnections();
  const closed = once(server, 'close');
  server.close();
  await closed;
}
const deterministicArgon = (pw: Uint8Array) => hmac(pw, Buffer.concat([Buffer.from('test salted password'), Buffer.from([3])]));
const loginWith = (url: string, pw = 'correct horse battery staple', username = 'ALICE') => login({ username, password: pw, url });

describe('public Node login success contract', () => {
  it('verifies an independently assembled transcript and real minimum Argon2id proof', async () => {
    const f = await fixture('correct horse battery staple', { realArgon: true });
    try {
      const result = await loginWith(f.url);
      expect(f.proofChecks).toBe(1);
      expect(result.token).toBeInstanceOf(ApiToken);
      const tokenValue = 'shk1.' + 'A'.repeat(22) + '.' + 'A'.repeat(43);
      expect(result.token.toString()).toBe('[REDACTED]');
      expect(JSON.stringify(result, (_key: string, value: unknown): unknown => (typeof value === 'bigint' ? value.toString() : value))).not.toContain(tokenValue);
      expect(inspect(result)).not.toContain(tokenValue);
      expect(result.metadata).toMatchObject({ label: 'http-login', owner: 'alice', mode: 'read_only', channels: 'rest', databases: null });
      expect(result.metadata.id).toEqual(bytes(16, 0));
      expect(result.metadata.expiresAtNs).toBeGreaterThan(BigInt(Date.now()) * 1_000_000n);
      expect(transcript('alice', bytes(32, 2), null).length).toBe(166 + Buffer.byteLength('alice', 'utf8') + 15);
    } finally { await close(f.server); }
  }, 60_000);

  it('supports repeated successful logins and exposes identical wrong-password/unknown-user errors', async () => {
    const f = await fixture('correct horse battery staple');
    const argon = vi.spyOn(NodePlatform, 'argon2id').mockImplementation(async pw => deterministicArgon(pw));
    try {
      const a = await loginWith(f.url);
      const b = await loginWith(f.url);
      expect(a.token.toString()).toBe(b.token.toString());
      const wrong = await loginWith(f.url, 'wrong password').catch(error => error);
      const unknown = await loginWith(f.url, 'unknown account password', 'unknown').catch(error => error);
      expect(wrong).toMatchObject({ code: 'authentication_failed', status: 401 });
      expect(unknown).toMatchObject({ code: 'authentication_failed', status: 401 });
      expect(f.proofChecks).toBe(4);
      expect(f.calls).toBe(4);
    } finally { argon.mockRestore(); await close(f.server); }
  });

  it('discards the token when the server signature is tampered', async () => {
    const f = await fixture('correct horse battery staple', { signatureTamper: true });
    const argon = vi.spyOn(NodePlatform, 'argon2id').mockImplementation(async pw => deterministicArgon(pw));
    try { await expect(loginWith(f.url)).rejects.toMatchObject({ code: 'authentication_failed' }); }
    finally { argon.mockRestore(); await close(f.server); }
  });

  it.each([
    ['missing protocol version', (r: Fields) => { delete r.protocol_version; }],
    ['protocol version mismatch', (r: Fields) => { r.protocol_version = 2; }],
    ['server signature length', (r: Fields) => { r.server_signature = bytes(31, 0); }],
    ['id length', (r: Fields) => { r.id = bytes(15, 0); }],
    ['id mismatch', (r: Fields) => { r.id = bytes(16, 1); }],
    ['token prefix', (r: Fields) => { r.token = `bad.${'A'.repeat(22)}.${'A'.repeat(43)}`; }],
    ['token id noncanonical last bits', (r: Fields) => { r.token = `shk1.${'A'.repeat(21)}B.${'A'.repeat(43)}`; }],
    ['invalid token', (r: Fields) => { r.token = 'not-a-token'; }],
    ['label mismatch', (r: Fields) => { r.label = 'other'; }],
    ['label wrong type', (r: Fields) => { r.label = 1; }],
    ['owner mismatch', (r: Fields) => { r.owner = 'bob'; }],
    ['owner wrong type', (r: Fields) => { r.owner = 1; }],
    ['mode mismatch', (r: Fields) => { r.mode = 'read_write'; }],
    ['channels mismatch', (r: Fields) => { r.channels = 'both'; }],
    ['database type', (r: Fields) => { r.databases = 'db'; }],
    ['databases array mismatch', (r: Fields) => { r.databases = ['db']; }],
    ['created timestamp negative', (r: Fields) => { r.created_at_ns = -1n; }],
    ['created timestamp zero', (r: Fields) => { r.created_at_ns = 0n; }],
    ['created timestamp future', (r: Fields) => { r.created_at_ns = BigInt(Date.now() + 60_000) * 1_000_000n; }],
    ['expires beyond 24 hours', (r: Fields) => { r.expires_at_ns = r.created_at_ns as bigint + 86_400_000_000_001n; }],
    ['expires before created', (r: Fields) => { r.expires_at_ns = (r.created_at_ns as bigint) - 1n; }],
    ['expired timestamp', (r: Fields) => { r.expires_at_ns = 0n; }],
    ['timestamp type', (r: Fields) => { r.expires_at_ns = 'later'; }],
    ['extra field', (r: Fields) => { r.extra = true; }],
  ] as const)('rejects a signed success reply with invalid %s', async (_name, mutate) => {
    const f = await fixture('correct horse battery staple', { mutate });
    const argon = vi.spyOn(NodePlatform, 'argon2id').mockImplementation(async pw => deterministicArgon(pw));
    try { await expect(loginWith(f.url)).rejects.toMatchObject({ code: 'protocol_error' }); }
    finally { argon.mockRestore(); await close(f.server); }
  });
});
