import { createHash, createHmac } from 'node:crypto';
import { describe, expect, it, vi } from 'vitest';
import { decode as decodeWire, encode as encodeWire } from '@msgpack/msgpack';
import { LoginError } from '../errors.js';
import { performLogin, type LoginCrypto, type LoginHttp } from '../core.js';

type Fields = Record<string, unknown>;
const bytes = (n: number, value: number) => new Uint8Array(n).fill(value);
const enc = new TextEncoder();
const u16 = (n: number) => Buffer.from([n >>> 8, n & 255]);
const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32BE(n); return b; };
const sha = (b: Uint8Array) => createHash('sha256').update(b).digest();
const mac = (key: Uint8Array, data: Uint8Array) => createHmac('sha256', key).update(data).digest();
const encode = (v: unknown) => encodeWire(v, { useBigInt64: true });
function record(b: Uint8Array): Fields { return decodeWire(b, { useBigInt64: true }) as Fields; }

function asNumber(value: unknown): number { return typeof value === 'bigint' ? Number(value) : value as number; }
function oracle(user: string, nonce: Uint8Array, challenge: Fields, scope: Fields): Buffer {
  const label = enc.encode(scope.label as string);
  const dbs = scope.databases as string[] | null;
  const parts: Buffer[] = [Buffer.from('SHAMIR-HTTP-LOGIN-v1', 'ascii'), Buffer.from([1, 3, asNumber(challenge.security)]),
    Buffer.from(challenge.listener_id as Uint8Array), Buffer.from(challenge.handshake_id as Uint8Array),
    u16(enc.encode(user).length), Buffer.from(enc.encode(user)), Buffer.from(nonce),
    Buffer.from(challenge.server_nonce as Uint8Array), Buffer.from(challenge.salt as Uint8Array),
    u32(asNumber(challenge.memory_kb)), u32(asNumber(challenge.time)), u32(asNumber(challenge.parallelism)),
    Buffer.from([asNumber(challenge.argon2_version)]), u16(label.length), Buffer.from(label),
    Buffer.from([scope.mode === 'read_only' ? 0 : 1, scope.channels === 'rest' ? 0 : scope.channels === 'ws' ? 1 : 2,
      dbs === null ? 255 : dbs.length])];
  for (const db of dbs ?? []) { const value = Buffer.from(enc.encode(db)); parts.push(u16(value.length), value); }
  return Buffer.concat(parts);
}
function challenge(overrides: Fields = {}): Fields {
  return { protocol_version: 1, binding_mode: 3, security: 0, listener_id: bytes(16, 1), handshake_id: bytes(32, 2),
    salt: bytes(16, 3), kdf: 'argon2id', memory_kb: 19456, time: 2, parallelism: 1, argon2_version: 19,
    server_nonce: bytes(32, 4), ...overrides };
}
function harness(options: { reply?: Fields; argon?: () => Promise<Uint8Array>; success?: boolean } = {}) {
  const nonce = bytes(32, 9);
  const beginScope: Fields[] = [];
  const tracked: Uint8Array[] = [];
  let finishes = 0;
  const crypto: LoginCrypto = {
    randomBytes: () => nonce.slice(),
    async argon2id(password, _salt, _params) {
      tracked.push(password);
      return options.argon ? options.argon() : bytes(32, 5);
    },
    async sha256(data) { const result = new Uint8Array(sha(data)); tracked.push(result); return result; },
    async hmac(key, data) { const result = new Uint8Array(mac(key, data)); tracked.push(result); return result; },
    equal(a, b) { return a.length === b.length && a.every((x, i) => x === b[i]); },
  };
  const http: LoginHttp = { async request(url, body) {
    const path = url.pathname.endsWith('/begin') ? 'begin' : 'finish';
    if (path === 'begin') {
      const request = record(body); beginScope.push(request.token as Fields);
      return { status: 200, contentType: 'application/msgpack', retryAfter: null, body: encode(options.reply ?? challenge()) };
    }
    finishes++;
    const request = record(body);
    const ch = options.reply ?? challenge();
    const scope = beginScope[0];
    const t = oracle('alice', nonce, ch, scope);
    const salted = bytes(32, 5);
    const clientKey = mac(salted, enc.encode('Client Key'));
    const serverKey = mac(salted, enc.encode('Server Key'));
    const clientSignature = mac(sha(clientKey), t);
    const proof = Buffer.alloc(32); for (let i = 0; i < 32; i++) proof[i] = clientKey[i] ^ clientSignature[i];
    expect(Buffer.from(request.client_proof as Uint8Array).equals(proof)).toBe(true);
    const now = BigInt(Date.now()) * 1_000_000n;
    const token = `shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`;
    const success = { protocol_version: 1, server_signature: mac(serverKey, t), token, id: bytes(16, 0), label: scope.label,
      owner: 'alice', mode: scope.mode, channels: scope.channels, databases: scope.databases,
      created_at_ns: now, expires_at_ns: now + 60_000_000_000n };
    return { status: 200, contentType: 'application/msgpack', retryAfter: null, body: encode(success) };
  } };
  const connect = async () => { throw new Error('connect must not run'); };
  return { crypto, http, tracked, beginScope, finishes, getFinishes: () => finishes, connect };
}
const opts = (url = 'http://127.0.0.1/') => ({ username: 'ALICE', password: 'sensitive-password', url });

describe('performLogin security invariants (injected, no network)', () => {
  it.each([
    ['below KDF floor', { memory_kb: 19455 }, 'kdf_params_rejected', false],
    ['time below KDF floor', { time: 1 }, 'kdf_params_rejected', false],
    ['zero KDF parallelism', { parallelism: 0 }, 'kdf_params_rejected', false],
    ['above KDF maximum', { memory_kb: 262145 }, 'kdf_params_rejected', false],
    ['wrong Argon version', { argon2_version: 18 }, 'kdf_params_rejected', false],
    ['wrong exact KDF tuple', { memory_kb: 32768 }, 'kdf_params_rejected', false],
    ['invalid Argon version', { argon2_version: 0x14 }, 'kdf_params_rejected', false],
    ['memory encoded as string', { memory_kb: '19456' }, 'protocol_error', false],
    ['memory encoded as fraction float', { memory_kb: 19456.5 }, 'protocol_error', false],
    ['short salt', { salt: bytes(15, 3) }, 'protocol_error', false],
    ['short server nonce', { server_nonce: bytes(31, 4) }, 'protocol_error', false],
    ['short listener id', { listener_id: bytes(15, 1) }, 'protocol_error', false],
    ['short handshake id', { handshake_id: bytes(31, 2) }, 'protocol_error', false],
    ['wrong protocol version', { protocol_version: 2 }, 'protocol_error', false],
    ['wrong binding mode', { binding_mode: 2 }, 'protocol_error', false],
    ['wrong KDF name', { kdf: 'argon2i' }, 'protocol_error', false],
    ['unknown security class', { security: 3 }, 'security_mismatch', false],
    ['wrong security class', { security: 1 }, 'security_mismatch', false],
    ['wrong listener pin', { listener_id: bytes(16, 8) }, 'security_mismatch', true],
    ['zero server nonce', { server_nonce: bytes(32, 0) }, 'protocol_error', false],
    ['equal nonce', { server_nonce: bytes(32, 9) }, 'protocol_error', false],
    ['extra challenge field', { unexpected: true }, 'protocol_error', false],
  ] as [string, Fields, string, boolean][])('rejects %s before Argon2 and finish', async (_name, fields, code, hasPin) => {
    const f = harness({ reply: challenge(fields) });
    const error = await performLogin({ ...opts(), expectedKdf: { memoryKb: 19456, time: 2, parallelism: 1 },
      ...(hasPin ? { listenerId: bytes(16, 1) } : {}) }, f.crypto, f.http, f.connect).catch(e => e);
    expect(error).toMatchObject({ code });
    expect(f.tracked).toHaveLength(0);
    expect(f.getFinishes()).toBe(0);
  });

  it('accepts security class 2 over external TLS with exact KDF', async () => {
    const f = harness({ reply: challenge({ security: 2 }) });
    await expect(performLogin({ ...opts('https://example.com/'), externalTls: true,
      expectedKdf: { memoryKb: 19456, time: 2, parallelism: 1 } }, f.crypto, f.http, f.connect)).resolves.toBeDefined();
    expect(f.getFinishes()).toBe(1);
  });

  it('copies CA bytes before awaiting begin', async () => {
    const ca = bytes(2, 1);
    const f = harness({ reply: challenge({ security: 1 }) });
    const seen: (string | Uint8Array | undefined)[] = [];
    const http: LoginHttp = { request: async (url, body, signal, timeout, passedCa) => {
      seen.push(passedCa);
      return f.http.request(url, body, signal, timeout, passedCa);
    } };
    const pending = performLogin({ ...opts('https://example.com/'), ca }, f.crypto, http, f.connect);
    ca.fill(8);
    await pending;
    expect(seen).toHaveLength(2);
    expect(seen.every(value => value instanceof Uint8Array && value[0] === 1)).toBe(true);
  });

  it('accepts non-shortest uint64 integer fields in the challenge', async () => {
    const f = harness({ reply: challenge({ protocol_version: 1n, binding_mode: 3n, security: 0n,
      memory_kb: 19456n, time: 2n, parallelism: 1n, argon2_version: 19n }) });
    await expect(performLogin(opts(), f.crypto, f.http, f.connect)).resolves.toBeDefined();
    expect(f.getFinishes()).toBe(1);
  });

  it.each([
    ['memory below floor', { memoryKb: 19455, time: 2, parallelism: 1 }],
    ['time above limit', { memoryKb: 19456, time: 9, parallelism: 1 }],
    ['nonpositive parallelism', { memoryKb: 19456, time: 2, parallelism: -1 }],
  ] as [string, { memoryKb: number; time: number; parallelism: number }][])('rejects local expected KDF %s before HTTP', async (_name, expectedKdf) => {
    const f = harness();
    await expect(performLogin({ ...opts(), expectedKdf }, f.crypto, f.http, f.connect))
      .rejects.toMatchObject({ code: 'invalid_request' });
    expect(f.beginScope).toHaveLength(0);
  });

  it('accepts security class 1 directly', async () => {
    const f = harness({ reply: challenge({ security: 1 }) });
    await expect(performLogin({ ...opts('https://example.com/'),
      expectedKdf: { memoryKb: 19456, time: 2, parallelism: 1 } }, f.crypto, f.http, f.connect)).resolves.toBeDefined();
    expect(f.getFinishes()).toBe(1);
  });

  it('accepts loopback plaintext at 127.1.2.3 through injected HTTP', async () => {
    const f = harness();
    await expect(performLogin(opts('http://127.1.2.3/'), f.crypto, f.http, f.connect)).resolves.toBeDefined();
    expect(f.getFinishes()).toBe(1);
  });

  it.each(['http://example.com/', 'http://192.0.2.1/'])('rejects remote plaintext URL %s before HTTP', async url => {
    const f = harness();
    await expect(performLogin(opts(url), f.crypto, f.http, f.connect)).rejects.toMatchObject({ code: 'security_mismatch' });
    expect(f.beginScope).toHaveLength(0);
  });

  it('freezes caller-owned scope and listener pin before awaiting begin', async () => {
    let release!: (value: Awaited<ReturnType<LoginHttp['request']>>) => void;
    let beginCaptured!: () => void;
    const captured = new Promise<void>(resolve => { beginCaptured = resolve; });
    const f = harness();
    const http: LoginHttp = { request: async (url, body, signal, timeout) => {
      const response = await f.http.request(url, body, signal, timeout);
      if (url.pathname.endsWith('/begin')) {
        beginCaptured();
        return new Promise(resolve => { release = resolve; });
      }
      return response;
    } };
    const databases = ['alpha'];
    const scope = { label: 'fixed-label', mode: 'read_only' as const, channels: 'rest' as const, databases };
    const pin = bytes(16, 1);
    const pending = performLogin({ ...opts(), scope, listenerId: pin }, f.crypto, http, f.connect);
    await captured;
    databases[0] = 'changed'; pin.fill(7); scope.label = 'changed-label';
    release({ status: 200, contentType: 'application/msgpack', retryAfter: null, body: encode(challenge()) });
    const result = await pending;
    expect(result.metadata).toMatchObject({ label: 'fixed-label', databases: ['alpha'] });
    expect(f.beginScope[0]).toEqual({ label: 'fixed-label', mode: 'read_only', channels: 'rest', databases: ['alpha'] });
    expect(f.getFinishes()).toBe(1);
  });

  it.each([
    ['rest', 'http', true], ['rest', 'ws', false], ['ws', 'http', false], ['ws', 'ws', true],
    ['both', 'http', true], ['both', 'ws', true],
  ] as const)('enforces %s channel for %s continuation before connecting', async (channels, channel, allowed) => {
    const f = harness();
    let calls = 0;
    const connector = async () => { calls++; return {} as never; };
    const result = await performLogin({ ...opts(), scope: { label: 'scope', mode: 'read_only', channels, databases: null } },
      f.crypto, f.http, connector);
    if (allowed) await expect(result.connect({ channel })).resolves.toBeDefined();
    else await expect(result.connect({ channel })).rejects.toMatchObject({ code: 'token_channel_denied' });
    expect(calls).toBe(allowed ? 1 : 0);
  });

  it('keeps overall timeout distinct when transport rejects client_aborted on internal abort', async () => {
    vi.useFakeTimers();
    try {
      let entered!: () => void;
      const started = new Promise<void>(resolve => { entered = resolve; });
      const f = harness();
      const http: LoginHttp = { request: (_url, _body, signal) => new Promise((_resolve, reject) => {
        entered();
        signal.addEventListener('abort', () => reject(new LoginError('client_aborted')), { once: true });
      }) };
      const pending = performLogin({ ...opts(), timeoutMs: 10 }, f.crypto, http, f.connect);
      await started;
      const rejection = expect(pending).rejects.toMatchObject({ code: 'client_timeout' });
      await vi.advanceTimersByTimeAsync(10);
      await rejection;
    } finally { vi.useRealTimers(); }
  });

  it('preserves caller abort as client_aborted', async () => {
    let entered!: () => void;
    const started = new Promise<void>(resolve => { entered = resolve; });
    const controller = new AbortController();
    const f = harness();
    const http: LoginHttp = { request: (_url, _body, signal) => new Promise((_resolve, reject) => {
      entered();
      signal.addEventListener('abort', () => reject(new LoginError('client_aborted')), { once: true });
    }) };
    const pending = performLogin({ ...opts(), signal: controller.signal }, f.crypto, http, f.connect);
    await started;
    const rejection = expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
    controller.abort();
    await rejection;
  });

  it('zeros all retained crypto buffers on successful proof verification', async () => {
    const f = harness();
    await performLogin({ ...opts(), scope: { label: 'scope', databases: ['alpha'], mode: 'read_only', channels: 'rest' } }, f.crypto, f.http, f.connect);
    expect(f.getFinishes()).toBe(1);
    expect(f.tracked.length).toBeGreaterThan(4);
    for (const secret of f.tracked) expect([...secret].every(x => x === 0)).toBe(true);
  });

  it('zeros derived keys and proof when server signature verification fails', async () => {
    const f = harness();
    const http: LoginHttp = { async request(url, body, signal, timeout) {
      const response = await f.http.request(url, body, signal, timeout);
      if (url.pathname.endsWith('/finish')) {
        const result = record(response.body); (result.server_signature as Uint8Array)[0] ^= 1;
        return { ...response, body: encode(result) };
      }
      return response;
    } };
    await expect(performLogin(opts(), f.crypto, http, f.connect)).rejects.toMatchObject({ code: 'server_signature_mismatch', status: 200 });
    expect(f.getFinishes()).toBe(1);
    expect(f.tracked.length).toBeGreaterThan(4);
    for (const secret of f.tracked) expect([...secret].every(x => x === 0)).toBe(true);
  });

  it('retains password until rejected Argon2 settles, then wipes it', async () => {
    let rejectKdf!: (reason: Error) => void;
    let entered!: () => void;
    const started = new Promise<void>(resolve => { entered = resolve; });
    const f = harness({ argon: () => { entered(); return new Promise((_resolve, reject) => { rejectKdf = reject; }); } });
    const pending = performLogin(opts(), f.crypto, f.http, f.connect);
    await started;
    const password = f.tracked[0];
    expect([...password].some(x => x !== 0)).toBe(true);
    rejectKdf(new Error('KDF failed'));
    await expect(pending).rejects.toMatchObject({ code: 'network_error' });
    expect([...password].every(x => x === 0)).toBe(true);
  });

  it('abort returns promptly during KDF but retains password until late KDF result is wiped', async () => {
    let release!: (value: Uint8Array) => void;
    let entered!: () => void;
    const started = new Promise<void>(resolve => { entered = resolve; });
    const controller = new AbortController();
    const f = harness({ argon: () => { entered(); return new Promise(resolve => { release = resolve; }); } });
    const pending = performLogin({ ...opts(), signal: controller.signal }, f.crypto, f.http, f.connect);
    await started;
    const password = f.tracked[0];
    expect([...password].some(x => x !== 0)).toBe(true);
    const rejection = expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
    controller.abort();
    await rejection;
    expect([...password].some(x => x !== 0)).toBe(true);
    const late = bytes(32, 6);
    const originalFill = late.fill.bind(late);
    let wipedResolve!: () => void;
    const wiped = new Promise<void>(resolve => { wipedResolve = resolve; });
    const fillSpy = vi.spyOn(late, 'fill').mockImplementation((value, start, end) => {
      const result = originalFill(value, start, end);
      if (value === 0) wipedResolve();
      return result;
    });
    try {
      release(late);
      await wiped;
      expect([...password].every(x => x === 0)).toBe(true);
      expect([...late].every(x => x === 0)).toBe(true);
      expect(f.getFinishes()).toBe(0);
    } finally { fillSpy.mockRestore(); }
  });

  it('timeout returns promptly during KDF but retains password until late KDF result is wiped', async () => {
    vi.useFakeTimers();
    try {
      let release!: (value: Uint8Array) => void;
      let entered!: () => void;
      const started = new Promise<void>(resolve => { entered = resolve; });
      const controller = new AbortController();
      const f = harness({ argon: () => { entered(); return new Promise(resolve => { release = resolve; }); } });
      const pending = performLogin({ ...opts(), timeoutMs: 10, signal: controller.signal }, f.crypto, f.http, f.connect);
      await started;
      const password = f.tracked[0];
      expect([...password].some(x => x !== 0)).toBe(true);
      const rejection = expect(pending).rejects.toMatchObject({ code: 'client_timeout' });
      await vi.advanceTimersByTimeAsync(10);
      controller.abort();
      await rejection;
      expect([...password].some(x => x !== 0)).toBe(true);
      const late = bytes(32, 6);
      const originalFill = late.fill.bind(late);
      let wipedResolve!: () => void;
      const wiped = new Promise<void>(resolve => { wipedResolve = resolve; });
      const fillSpy = vi.spyOn(late, 'fill').mockImplementation((value, start, end) => {
        const result = originalFill(value, start, end);
        if (value === 0) wipedResolve();
        return result;
      });
      try {
        release(late);
        await wiped;
        expect([...password].every(x => x === 0)).toBe(true);
        expect([...late].every(x => x === 0)).toBe(true);
        expect(f.getFinishes()).toBe(0);
      } finally { fillSpy.mockRestore(); }
    } finally { vi.useRealTimers(); }
  });

  it('forwards a copied CA bundle through a gated begin request', async () => {
    let entered!: () => void;
    let release!: (value: Awaited<ReturnType<LoginHttp['request']>>) => void;
    const started = new Promise<void>(resolve => { entered = resolve; });
    const ca = bytes(3, 4);
    const f = harness({ reply: challenge({ security: 1 }) });
    const seen: (string | Uint8Array | undefined)[] = [];
    const http: LoginHttp = { request: async (url, body, signal, timeout, passedCa) => {
      seen.push(passedCa);
      const response = await f.http.request(url, body, signal, timeout, passedCa);
      if (url.pathname.endsWith('/begin')) { entered(); return new Promise(resolve => { release = resolve; }); }
      return response;
    } };
    const pending = performLogin({ ...opts('https://example.com/'), ca }, f.crypto, http, f.connect);
    await started;
    ca.fill(9);
    release({ status: 200, contentType: 'application/msgpack', retryAfter: null, body: encode(challenge({ security: 1 })) });
    await pending;
    expect(seen).toHaveLength(2);
    expect(seen.every(value => value instanceof Uint8Array && value.every(x => x === 4))).toBe(true);
  });

  it.each([
    { name: 'empty string', ca: '' }, { name: 'zero bytes', ca: bytes(0, 0) }, { name: 'false', ca: false },
    { name: 'null', ca: null }, { name: 'number', ca: 7 },
  ])('rejects invalid CA option: $name before HTTP', async ({ ca }) => {
    const f = harness();
    await expect(performLogin({ ...opts('https://example.com/'), ca: ca as never }, f.crypto, f.http, f.connect))
      .rejects.toMatchObject({ code: 'invalid_request' });
    expect(f.beginScope).toHaveLength(0);
  });

  it('rejects unknown insecure options before HTTP', async () => {
    for (const extra of [{ insecure: true }, { allowInsecure: true },
      { rejectUnauthorized: false, allowInsecureTlsForDevelopment: true }]) {
      const f = harness();
      await expect(performLogin({ ...opts(), ...extra } as never, f.crypto, f.http, f.connect))
        .rejects.toMatchObject({ code: 'invalid_request' });
      expect(f.beginScope).toHaveLength(0);
    }
  });

  it.each(['begin', 'finish'] as const)('distinguishes timeout and caller abort during %s', async phase => {
    for (const reason of ['timeout', 'caller'] as const) {
      vi.useFakeTimers();
      try {
        let entered!: () => void;
        const started = new Promise<void>(resolve => { entered = resolve; });
        const controller = new AbortController();
        const f = harness();
        const http: LoginHttp = { request: (url, body, signal, timeout, ca) => {
          if (url.pathname.endsWith(`/${phase}`)) return new Promise((_resolve, reject) => {
            entered(); signal.addEventListener('abort', () => reject(new LoginError('client_aborted')), { once: true });
          });
          return f.http.request(url, body, signal, timeout, ca);
        } };
        const pending = performLogin({ ...opts(), timeoutMs: 20, signal: reason === 'caller' ? controller.signal : undefined }, f.crypto, http, f.connect);
        await started;
        const expected = expect(pending).rejects.toMatchObject({ code: reason === 'caller' ? 'client_aborted' : 'client_timeout' });
        if (reason === 'caller') controller.abort(); else await vi.advanceTimersByTimeAsync(20);
        await expected;
      } finally { vi.useRealTimers(); }
    }
  });

  it('wipes retained begin, finish, and error response bodies', async () => {
    for (const failing of [false, true]) {
      const f = harness();
      const bodies: Uint8Array[] = [];
      const http: LoginHttp = { async request(url, body, signal, timeout, ca) {
        const response = await f.http.request(url, body, signal, timeout, ca);
        if (failing && url.pathname.endsWith('/finish')) {
          const errorBody = encode({ error: 'authentication_failed' });
          bodies.push(errorBody);
          return { ...response, status: 401, body: errorBody };
        }
        bodies.push(response.body);
        return response;
      } };
      if (failing) await expect(performLogin(opts(), f.crypto, http, f.connect)).rejects.toMatchObject({ code: 'authentication_failed' });
      else await performLogin(opts(), f.crypto, http, f.connect);
      expect(bodies).toHaveLength(2);
      for (const body of bodies) expect([...body].every(x => x === 0)).toBe(true);
    }
  });

  it.each(['begin', 'finish'] as const)('wipes a late %s response after abort', async phase => {
    let entered!: () => void;
    let release!: (value: Awaited<ReturnType<LoginHttp['request']>>) => void;
    const started = new Promise<void>(resolve => { entered = resolve; });
    const controller = new AbortController();
    const f = harness();
    const lateBody = encode(challenge());
    const originalFill = lateBody.fill.bind(lateBody);
    let wipedResolve!: () => void;
    const wiped = new Promise<void>(resolve => { wipedResolve = resolve; });
    const fillSpy = vi.spyOn(lateBody, 'fill').mockImplementation((value, start, end) => {
      const result = originalFill(value, start, end);
      if (value === 0) wipedResolve();
      return result;
    });
    const http: LoginHttp = { request: (url, body, signal, timeout, ca) => {
      if (url.pathname.endsWith(`/${phase}`)) return new Promise(resolve => { entered(); release = resolve; });
      return f.http.request(url, body, signal, timeout, ca);
    } };
    try {
      const pending = performLogin({ ...opts(), signal: controller.signal }, f.crypto, http, f.connect);
      await started;
      const rejection = expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
      controller.abort();
      await rejection;
      release({ status: 200, contentType: 'application/msgpack', retryAfter: null, body: lateBody });
      await wiped;
      expect([...lateBody].every(x => x === 0)).toBe(true);
    } finally { fillSpy.mockRestore(); }
  });
});
