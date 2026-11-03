import type { LoginErrorCode } from './errors.js';
import { LoginError } from './errors.js';
import type { LoginMetadata, LoginOptions, LoginResult, LoginScope } from './types.js';
import { ApiToken } from '../entry/api-token.js';
import { buildLoginTranscript, canonicalUser, decodeMessage, encodeMessage, freezeScope, type Challenge, type Scope } from './protocol.js';
import { validateKdfParams } from '../core/protocol/scram.js';
import { TokenTransportError } from '../core/protocol/token/errors.js';
import type { Argon2Params } from '../core/support/platform.js';
import type { PublicTokenClient } from '../entry/ws/token-client.js';

export interface LoginCrypto {
  randomBytes(n: number): Uint8Array;
  argon2id(password: Uint8Array, salt: Uint8Array, p: Argon2Params): Promise<Uint8Array>;
  sha256(data: Uint8Array): Promise<Uint8Array>;
  hmac(key: Uint8Array, data: Uint8Array): Promise<Uint8Array>;
  equal(a: Uint8Array, b: Uint8Array): boolean;
}

export interface LoginHttp {
  request(url: URL, body: Uint8Array, signal: AbortSignal, timeout: number, ca?: string | Uint8Array): Promise<{
    status: number;
    contentType: string | null;
    retryAfter: string | null;
    body: Uint8Array;
  }>;
}

export type LoginConnect = (
  token: ApiToken,
  url: URL,
  channel: 'http' | 'ws',
  signal?: AbortSignal,
  requestTimeoutMs?: number,
) => Promise<PublicTokenClient>;

function err(code: LoginErrorCode, status?: number, retry?: number): LoginError {
  return new LoginError(code, retry, status);
}

function exact(o: Record<string, unknown>, keys: readonly string[]): void {
  if (Object.keys(o).length !== keys.length || keys.some((k) => !Object.hasOwn(o, k))) {
    throw err('protocol_error');
  }
}

function bin(v: unknown, n: number): Uint8Array {
  if (!(v instanceof Uint8Array) || v.length !== n) throw err('protocol_error');
  return v;
}

function int(v: unknown, max = 0xffffffff): number {
  if (typeof v === 'bigint') {
    if (v < 0n || v > BigInt(max)) throw err('protocol_error');
    return Number(v);
  }
  if (typeof v !== 'number' || !Number.isInteger(v) || v < 0 || v > max) {
    throw err('protocol_error');
  }
  return v;
}

function big(v: unknown): bigint {
  if (typeof v === 'bigint' && v >= 0n && v <= 0xffffffffffffffffn) return v;
  if (typeof v === 'number' && Number.isSafeInteger(v) && v >= 0) return BigInt(v);
  throw err('protocol_error');
}

function expectedSecurity(url: URL, external: boolean): number {
  if (url.protocol === 'https:') return external ? 2 : 1;
  if (external) throw err('security_mismatch');
  if (url.protocol === 'http:' && (['localhost', '127.0.0.1', '::1', '[::1]'].includes(url.hostname) ||
      /^127\.(?:\d{1,3}\.){2}\d{1,3}$/.test(url.hostname) && url.hostname.split('.').slice(1).every((part) => Number(part) <= 255))) return 0;
  throw err('security_mismatch');
}

function validTimeout(v: unknown): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 && v <= 0x7fffffff;
}

function optionsCheck(o: LoginOptions): void {
  if (!o || typeof o !== 'object' || typeof o.username !== 'string' ||
      typeof o.password !== 'string' || typeof o.url !== 'string') throw err('invalid_request');
  const keys = ['username','password','url','label','mode','channels','databases','scope','externalTls',
    'listenerId','expectedKdf','signal','timeoutMs','connectTimeoutMs','requestTimeoutMs','ttl','ca'];
  if (Object.keys(o).some((k) => !keys.includes(k)) || 'ttl' in o) throw err('invalid_request');
  for (const n of ['timeoutMs', 'connectTimeoutMs', 'requestTimeoutMs'] as const) {
    if (o[n] !== undefined && !validTimeout(o[n])) throw err('invalid_request');
  }
  if (o.ca !== undefined && !(typeof o.ca === 'string' && o.ca.length > 0) && !(o.ca instanceof Uint8Array && o.ca.length > 0)) throw err('invalid_request');
  if ((o.externalTls !== undefined && typeof o.externalTls !== 'boolean') ||
      (o.listenerId !== undefined && (!(o.listenerId instanceof Uint8Array) || o.listenerId.length !== 16))) {
    throw err('invalid_request');
  }
  if (o.expectedKdf !== undefined) {
    const kdf = o.expectedKdf as unknown as Record<string, unknown>;
    if (!o.expectedKdf || Object.keys(kdf).length !== 3 ||
        Object.keys(kdf).some((k) => !['memoryKb','time','parallelism'].includes(k)) ||
        !['memoryKb','time','parallelism'].every((k) => Number.isInteger(kdf[k]))) throw err('invalid_request');
    try { validateKdfParams({ ...o.expectedKdf, argon2Version: 19 }); } catch { throw err('invalid_request'); }
  }
  if (o.signal !== undefined && (!o.signal || typeof o.signal.aborted !== 'boolean' ||
      typeof o.signal.addEventListener !== 'function' || typeof o.signal.removeEventListener !== 'function')) {
    throw err('invalid_request');
  }
  if (o.scope === null) throw err('invalid_request');
  if (o.scope && ['label','mode','channels','databases'].some((k) => Object.hasOwn(o, k))) {
    throw err('invalid_request');
  }
}

function parseUrl(raw: string): URL {
  if (raw.trim() !== raw || !/^https?:\/\/[^/?#\\\s]+\/?$/i.test(raw)) throw err('invalid_request');
  const authority = raw.replace(/^https?:\/\//i, '').split('/')[0];
  if (authority.includes('@')) throw err('invalid_request');
  let u: URL;
  try { u = new URL(raw); } catch { throw err('invalid_request'); }
  if (u.username || u.password || u.search || u.hash || u.pathname !== '/' || !u.hostname) throw err('invalid_request');
  return u;
}

function scopeFor(o: LoginOptions): Scope {
  return freezeScope(o.scope ?? { label: o.label, mode: o.mode, channels: o.channels, databases: o.databases });
}

function makeAbort(parent?: AbortSignal, timeout = 0) {
  const c = new AbortController();
  let origin: 'caller' | 'timeout' | undefined;
  const abort = (why: 'caller' | 'timeout') => {
    if (origin !== undefined) return;
    origin = why;
    c.abort();
  };
  const onAbort = () => abort('caller');
  if (parent?.aborted) abort('caller');
  else parent?.addEventListener('abort', onAbort, { once: true });
  const timer = timeout ? setTimeout(() => abort('timeout'), timeout) : undefined;
  return { c, origin: () => origin, close() { if (timer) clearTimeout(timer); parent?.removeEventListener('abort', onAbort); } };
}

function aborted(signal: AbortSignal, parent?: AbortSignal, origin?: 'caller' | 'timeout'): never {
  throw err(origin === 'caller' ? 'client_aborted' : origin === 'timeout' ? 'client_timeout' : parent?.aborted ? 'client_aborted' : signal.aborted ? 'client_timeout' : 'network_error');
}

async function awaitGuarded<T>(promise: Promise<T>, signal: AbortSignal, parent?: AbortSignal,
  origin?: () => 'caller' | 'timeout' | undefined): Promise<T> {
  if (signal.aborted) { void promise.catch(() => {}); aborted(signal, parent, origin?.()); }
  let remove = () => {};
  const stop = new Promise<never>((_, reject) => {
    const onAbort = () => reject(err(origin?.() === 'caller' ? 'client_aborted' : 'client_timeout'));
    signal.addEventListener('abort', onAbort, { once: true });
    remove = () => signal.removeEventListener('abort', onAbort);
  });
  try { return await Promise.race([promise, stop]); }
  catch (error) { if (signal.aborted) aborted(signal, parent, origin?.()); throw error; }
  finally { remove(); }
}

async function awaitSecret(promise: Promise<Uint8Array>, signal: AbortSignal, parent?: AbortSignal,
  origin?: () => 'caller' | 'timeout' | undefined): Promise<Uint8Array> {
  let abortedEarly = signal.aborted;
  const guarded = promise.then((value) => {
    if (abortedEarly || signal.aborted) value.fill(0);
    return value;
  });
  if (signal.aborted) { void guarded.catch(() => {}); aborted(signal, parent, origin?.()); }
  let remove = () => {};
  const stop = new Promise<never>((_, reject) => {
    const onAbort = () => { abortedEarly = true; reject(err(origin?.() === 'caller' ? 'client_aborted' : 'client_timeout')); };
    signal.addEventListener('abort', onAbort, { once: true });
    remove = () => signal.removeEventListener('abort', onAbort);
  });
  try { return await Promise.race([guarded, stop]); } finally { remove(); }
}

async function secretHmac(crypto: LoginCrypto, key: Uint8Array, data: Uint8Array,
  signal: AbortSignal, parent?: AbortSignal, origin?: () => 'caller' | 'timeout' | undefined): Promise<Uint8Array> {
  const keyCopy = new Uint8Array(key);
  const dataCopy = new Uint8Array(data);
  let work: Promise<Uint8Array>;
  try { work = crypto.hmac(keyCopy, dataCopy); }
  catch (error) { keyCopy.fill(0); dataCopy.fill(0); throw error; }
  work = work.finally(() => { keyCopy.fill(0); dataCopy.fill(0); });
  const result = await awaitSecret(work, signal, parent, origin);
  if (result.length !== 32) { result.fill(0); throw err('protocol_error'); }
  return result;
}

async function secretSha(crypto: LoginCrypto, data: Uint8Array,
  signal: AbortSignal, parent?: AbortSignal, origin?: () => 'caller' | 'timeout' | undefined): Promise<Uint8Array> {
  const dataCopy = new Uint8Array(data);
  let work: Promise<Uint8Array>;
  try { work = crypto.sha256(dataCopy); }
  catch (error) { dataCopy.fill(0); throw error; }
  work = work.finally(() => dataCopy.fill(0));
  const result = await awaitSecret(work, signal, parent, origin);
  if (result.length !== 32) { result.fill(0); throw err('protocol_error'); }
  return result;
}

function statusError(r: { status: number; body: Uint8Array; retryAfter: string | null }): LoginError {
  const status = r.status;
  let code: LoginErrorCode;
  const exactError = (expected: string) => {
    const b = decodeMessage(r.body);
    if (Object.keys(b).length !== 1 || b.error !== expected) throw err('protocol_error', status);
  };
  if (status === 400) { exactError('invalid_request'); code = 'invalid_request'; }
  else if (status === 401) { exactError('authentication_failed'); code = 'authentication_failed'; }
  else if (status === 403) {
    const b = decodeMessage(r.body);
    if (Object.keys(b).length !== 1 || typeof b.error !== 'string') throw err('protocol_error', status);
    const allowed: Record<string, LoginErrorCode> = {
      origin_forbidden:'origin_forbidden', authority_forbidden:'authority_forbidden', proxy_forbidden:'proxy_forbidden',
      login_write_forbidden:'login_write_forbidden', token_self_service_disabled:'token_self_service_disabled',
      login_read_only_replica:'login_read_only_replica', login_account_denied:'login_account_denied',
      token_label_exists:'token_label_exists', token_limit_reached:'token_limit_reached',
    };
    if (!Object.hasOwn(allowed, b.error)) throw err('protocol_error', status);
    code = allowed[b.error];
  } else if (status === 404) code = 'unsupported_server';
  else if (status === 405) { exactError('method_not_allowed'); code = 'http_method_not_allowed'; }
  else if (status === 408) { exactError('request_timeout'); code = 'http_timeout'; }
  else if (status === 413) { exactError('request_too_large'); code = 'http_body_too_large'; }
  else if (status === 415) { exactError('unsupported_media_type'); code = 'http_unsupported_media_type'; }
  else if (status === 431) { exactError('request_headers_too_large'); code = 'http_bad_headers'; }
  else if (status === 429 || status === 503) {
    const name = status === 429 ? 'rate_limited' : 'server_busy';
    const b = decodeMessage(r.body);
    const n = int(b.retry_after, 3600);
    if (Object.keys(b).length !== 2 || b.error !== name || n < 1 || r.retryAfter !== String(n)) {
      throw err('protocol_error', status);
    }
    return err(name, status, n * 1000);
  } else code = 'network_error';
  return err(code, status);
}

function sameDatabases(a: unknown, b: readonly string[] | null): boolean {
  if (b === null) return a === null;
  return Array.isArray(a) && a.length === b.length && a.every((x, i) => typeof x === 'string' && x === b[i]);
}

function decodeId(s: string): Uint8Array {
  const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  let acc = 0; let bits = 0; const out: number[] = [];
  for (const c of s) {
    const n = chars.indexOf(c);
    if (n < 0) throw err('protocol_error');
    acc = acc * 64 + n; bits += 6;
    if (bits >= 8) { bits -= 8; out.push((acc >> bits) & 255); acc &= (1 << bits) - 1; }
  }
  if (acc !== 0 || out.length !== 16) throw err('protocol_error');
  return Uint8Array.from(out);
}

function immutableResult(token: ApiToken, metadata: LoginMetadata, expiresAtNs: bigint, url: URL,
  connect: LoginConnect, defaultTimeout: number): LoginResult {
  function connectResult(options: { channel: 'http'; signal?: AbortSignal; requestTimeoutMs?: number }): Promise<import('../entry/token-client.js').TokenHttpClient>;
  function connectResult(options: { channel: 'ws'; signal?: AbortSignal; requestTimeoutMs?: number }): Promise<import('../entry/ws/token-client.js').TokenWsClient>;
  function connectResult(options: { channel: 'http' | 'ws'; signal?: AbortSignal; requestTimeoutMs?: number }): Promise<PublicTokenClient>;
  async function connectResult(options: { channel: 'http' | 'ws'; signal?: AbortSignal; requestTimeoutMs?: number }): Promise<PublicTokenClient> {
    if (!options || typeof options !== 'object' || options.signal !== undefined &&
        (!options.signal || typeof options.signal.aborted !== 'boolean' ||
         typeof options.signal.addEventListener !== 'function' || typeof options.signal.removeEventListener !== 'function') ||
        options.requestTimeoutMs !== undefined && !validTimeout(options.requestTimeoutMs)) throw err('invalid_request');
    if (BigInt(Date.now()) * 1_000_000n >= expiresAtNs) {
      throw new TokenTransportError('token_expired', 'Token expired; log in again');
    }
    const channel = options.channel;
    if (channel !== 'http' && channel !== 'ws') throw err('invalid_request');
    const allowed = channel === 'http' ? ['rest', 'both'] : ['ws', 'both'];
    if (!allowed.includes(metadata.channels)) throw new TokenTransportError('token_channel_denied', 'Token channel is not allowed');
    const connectUrl = new URL(url);
    if (channel === 'ws') connectUrl.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    const result = await connect(token, connectUrl, channel, options.signal, options.requestTimeoutMs ?? defaultTimeout);
    if (BigInt(Date.now()) * 1_000_000n >= expiresAtNs) {
      await result.close();
      throw new TokenTransportError('token_expired', 'Token expired; log in again');
    }
    return guardClient(result);
  }
  function guardClient<T extends object>(target: T): T {
    const proxies = new WeakMap<object, object>();
    const expired = () => BigInt(Date.now()) * 1_000_000n >= expiresAtNs;
    const expiryError = () => new TokenTransportError('token_expired', 'Token expired; log in again');
    const wrapValue = <V>(value: V): V => {
      if (typeof value !== 'object' || value === null || Array.isArray(value) || value instanceof Uint8Array ||
          value instanceof Promise || Object.getPrototypeOf(value) === Object.prototype) return value;
      const cached = proxies.get(value);
      if (cached) return cached as V;
      const proxy = new Proxy(value, handler);
      proxies.set(value, proxy);
      return proxy as V;
    };
    const handler: ProxyHandler<object> = {
      get(subject, key) {
        const value: unknown = Reflect.get(subject, key, subject);
        if (typeof value !== 'function') return value;
        return (...args: unknown[]) => {
          if (key !== 'close' && expired()) {
            const failure = expiryError();
            if (['ping','execute','executeWithTouch','waitForVisibility','getDdlOpStatus','run','rows','tx','runLive','unsubscribe','ex','next','return','throw'].includes(String(key))) {
              return Promise.reject(failure);
            }
            throw failure;
          }
          let returned: unknown;
          try { returned = Reflect.apply(value, subject, args); }
          catch (error) { throw error; }
          if (returned instanceof Promise) {
            return returned.catch((error: unknown) => {
              if (error instanceof TokenTransportError && error.code === 'token_expired') throw expiryError();
              throw error;
            });
          }
          return wrapValue(returned);
        };
      },
    };
    return wrapValue(target);
  }
  return Object.freeze({ token, metadata: Object.freeze(metadata), connect: connectResult });
}

export async function performLogin(o: LoginOptions, crypto: LoginCrypto, http: LoginHttp,
  connect: LoginConnect): Promise<LoginResult> {
  optionsCheck(o);
  const ca = o.ca === undefined ? undefined : typeof o.ca === 'string' ? o.ca : new Uint8Array(o.ca);
  const url = parseUrl(o.url);
  const scope = scopeFor(o);
  const user = canonicalUser(o.username);
  const sec = expectedSecurity(url, o.externalTls === true);
  const listenerId = o.listenerId === undefined ? undefined : new Uint8Array(o.listenerId);
  const expectedKdf = o.expectedKdf ? { ...o.expectedKdf } : undefined;
  const parent = o.signal;
  const timeoutMs = o.timeoutMs ?? 0;
  const connectTimeout = o.connectTimeoutMs ?? 10_000;
  const requestTimeout = o.requestTimeoutMs ?? 120_000;
  const password = o.password;
  const guard = makeAbort(parent, timeoutMs);
  const origin = guard.origin;
  let pw: Uint8Array | undefined;
  let salted: Uint8Array | undefined;
  let clientKey: Uint8Array | undefined;
  let serverKey: Uint8Array | undefined;
  let stored: Uint8Array | undefined;
  let clientSignature: Uint8Array | undefined;
  let proof: Uint8Array | undefined;
  let expectedSignature: Uint8Array | undefined;
  let transcript: Uint8Array | undefined;
  let encodedProofBody: Uint8Array | undefined;
  try {
    if (guard.c.signal.aborted) aborted(guard.c.signal, parent, origin());
    const nonce = crypto.randomBytes(32);
    if (nonce.length !== 32 || !nonce.some((b) => b !== 0)) throw err('network_error');
    const post = async (path: string, payload: unknown, timeout: number) => {
      const body = encodeMessage(payload);
      const received: { response?: Awaited<ReturnType<LoginHttp['request']>> } = {};
      let handedOff = false;
      try {
        if (guard.c.signal.aborted) aborted(guard.c.signal, parent, origin());
        const responsePromise = http.request(new URL(`/shamir/v1/auth/${path}`, url), body, guard.c.signal, timeout, ca).then((response) => {
          received.response = response;
          if (guard.c.signal.aborted) response.body.fill(0);
          return response;
        });
        const response = await awaitGuarded(responsePromise, guard.c.signal, parent, origin);
        if (!Number.isInteger(response.status) || response.status < 100 || response.status > 599) {
          response.body.fill(0);
          throw err('protocol_error');
        }
        handedOff = true;
        return response;
      } finally {
        if (!handedOff) received.response?.body.fill(0);
        body.fill(0);
      }
    };
    const begin = await post('begin', {
      protocol_version: 1, user, client_nonce: nonce, binding_mode: 3,
      token: { label: scope.label, mode: scope.mode, channels: scope.channels, databases: scope.databases },
    }, connectTimeout);
    const parsed = (() => {
      try {
        if (begin.contentType !== 'application/msgpack') throw err('http_unsupported_media_type', begin.status);
        if (begin.status !== 200) throw statusError(begin);
        const r = decodeMessage(begin.body);
        exact(r, ['protocol_version','binding_mode','security','listener_id','handshake_id','salt','kdf',
          'memory_kb','time','parallelism','argon2_version','server_nonce']);
        if (int(r.protocol_version, 255) !== 1 || int(r.binding_mode, 255) !== 3 || r.kdf !== 'argon2id') throw err('protocol_error');
        const version = int(r.argon2_version, 255);
        const challenge: Challenge = {
          security: int(r.security, 255), listenerId: bin(r.listener_id, 16), handshakeId: bin(r.handshake_id, 32),
          salt: bin(r.salt, 16), serverNonce: bin(r.server_nonce, 32), memoryKb: int(r.memory_kb),
          time: int(r.time), parallelism: int(r.parallelism),
        };
        if (challenge.security !== sec || (listenerId && !crypto.equal(listenerId, challenge.listenerId))) throw err('security_mismatch');
        if (version !== 19) throw err('kdf_params_rejected');
        if (!challenge.serverNonce.some((x) => x !== 0) || crypto.equal(nonce, challenge.serverNonce)) throw err('protocol_error');
        try { validateKdfParams({ memoryKb: challenge.memoryKb, time: challenge.time,
          parallelism: challenge.parallelism, argon2Version: version }); } catch { throw err('kdf_params_rejected'); }
        if (expectedKdf && (expectedKdf.memoryKb !== challenge.memoryKb || expectedKdf.time !== challenge.time ||
            expectedKdf.parallelism !== challenge.parallelism)) throw err('kdf_params_rejected');
        return { challenge, version };
      } finally { begin.body.fill(0); }
    })();
    const { challenge, version } = parsed;
    transcript = buildLoginTranscript(user, nonce, challenge, version, scope);
    pw = new TextEncoder().encode(password);
    const kdfParams: Argon2Params = { memoryKb: challenge.memoryKb, time: challenge.time,
      parallelism: challenge.parallelism };
    const passwordBytes = new Uint8Array(pw);
    const originalPassword = pw;
    let kdfPromise: Promise<Uint8Array>;
    try { kdfPromise = crypto.argon2id(passwordBytes, challenge.salt, kdfParams); }
    catch (error) { passwordBytes.fill(0); originalPassword.fill(0); throw error; }
    kdfPromise = kdfPromise.finally(() => {
      passwordBytes.fill(0); originalPassword.fill(0);
    });
    pw = undefined;
    salted = await awaitSecret(kdfPromise, guard.c.signal, parent, origin);
    if (salted.length !== 32) throw err('protocol_error');
    clientKey = await secretHmac(crypto, salted, new TextEncoder().encode('Client Key'), guard.c.signal, parent, origin);
    serverKey = await secretHmac(crypto, salted, new TextEncoder().encode('Server Key'), guard.c.signal, parent, origin);
    stored = await secretSha(crypto, clientKey, guard.c.signal, parent, origin);
    clientSignature = await secretHmac(crypto, stored, transcript, guard.c.signal, parent, origin);
    salted.fill(0); salted = undefined;
    proof = new Uint8Array(32);
    for (let i = 0; i < 32; i++) proof[i] = clientKey[i] ^ clientSignature[i];
    encodedProofBody = encodeMessage({ protocol_version: 1, handshake_id: challenge.handshakeId, client_proof: proof });
    clientKey.fill(0); clientKey = undefined;
    stored.fill(0); stored = undefined;
    clientSignature.fill(0); clientSignature = undefined;
    if (guard.c.signal.aborted) aborted(guard.c.signal, parent, origin());
    const receivedFinish: { response?: Awaited<ReturnType<LoginHttp['request']>> } = {};
    let finish: Awaited<ReturnType<LoginHttp['request']>>;
    try {
      if (guard.c.signal.aborted) aborted(guard.c.signal, parent, origin());
      const finishPromise = http.request(new URL('/shamir/v1/auth/finish', url), encodedProofBody,
        guard.c.signal, requestTimeout, ca).then((response) => {
          receivedFinish.response = response;
          if (guard.c.signal.aborted) response.body.fill(0);
          return response;
        });
      finish = await awaitGuarded(finishPromise, guard.c.signal, parent, origin);
    } catch (error) {
      receivedFinish.response?.body.fill(0);
      throw error;
    } finally { encodedProofBody.fill(0); encodedProofBody = undefined; proof.fill(0); proof = undefined; }
    const finishBody = finish.body;
    try {
      if (!Number.isInteger(finish.status) || finish.status < 100 || finish.status > 599) throw err('protocol_error');
      if (finish.contentType !== 'application/msgpack') throw err('http_unsupported_media_type', finish.status);
      if (finish.status !== 200) throw statusError(finish);
      const success = decodeMessage(finishBody);
      exact(success, ['protocol_version','server_signature','token','id','label','owner','mode','channels',
        'databases','created_at_ns','expires_at_ns']);
      if (int(success.protocol_version, 255) !== 1) throw err('protocol_error');
      const serverSignature = bin(success.server_signature, 32);
      try {
        expectedSignature = await secretHmac(crypto, serverKey, transcript, guard.c.signal, parent, origin);
        const signatureMatches = expectedSignature.length === 32 && crypto.equal(expectedSignature, serverSignature);
        expectedSignature.fill(0); expectedSignature = undefined;
        if (!signatureMatches) throw err('server_signature_mismatch', finish.status);
        if (typeof success.token !== 'string' || success.token.length !== 71 || typeof success.label !== 'string' ||
            success.label !== scope.label || success.mode !== scope.mode || success.channels !== scope.channels ||
            typeof success.owner !== 'string' || success.owner !== user || !sameDatabases(success.databases, scope.databases)) {
          throw err('protocol_error');
        }
        const id = bin(success.id, 16);
        if (!crypto.equal(id, decodeId(success.token.slice(5, 27)))) throw err('protocol_error');
        const created = big(success.created_at_ns);
        const expires = big(success.expires_at_ns);
        const now = BigInt(Date.now()) * 1_000_000n;
        if (created <= 0n || created > now + 5_000_000_000n || expires <= now || expires <= created ||
            expires - created > 86_400_000_000_000n) throw err('protocol_error');
        let token: ApiToken;
        try { token = ApiToken.from(success.token); } catch { throw err('protocol_error'); }
        const metadata: LoginMetadata = {
          id: new Uint8Array(id), label: scope.label, owner: user, mode: scope.mode, channels: scope.channels,
          databases: scope.databases === null ? null : Object.freeze([...scope.databases]),
          createdAtNs: created, expiresAtNs: expires,
        };
        return immutableResult(token, metadata, expires, url, connect, requestTimeout);
      } finally { serverSignature.fill(0); }
    } finally { finishBody.fill(0); }
  } catch (e) {
    if (e instanceof LoginError || e instanceof TokenTransportError) throw e;
    if (guard.c.signal.aborted) aborted(guard.c.signal, parent, origin());
    throw err('network_error');
  } finally {
    guard.close();
    pw?.fill(0); salted?.fill(0); clientKey?.fill(0); serverKey?.fill(0); stored?.fill(0);
    clientSignature?.fill(0); proof?.fill(0); expectedSignature?.fill(0); transcript?.fill(0);
    encodedProofBody?.fill(0); listenerId?.fill(0);
  }
}

export type { LoginScope };
