import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ApiToken } from '../../../entry/api-token.js';
import type { BrowserTokenHttpConnectOptions } from '../../../core/types/connection.js';
import { TokenTransportError } from '../../../core/protocol/token/errors.js';
import type { BrowserTokenHttpInit, BrowserTokenHttpResponse, BrowserTokenHttpReader, TokenFetch } from '../http-browser.js';
import { createBrowserTokenPlatform } from '../http-browser.js';

const rawToken = `shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`;
const token = () => ApiToken.from(rawToken);
const opts = (extra: Partial<BrowserTokenHttpConnectOptions> = {}): BrowserTokenHttpConnectOptions => ({ auth: 'token', channel: 'http', url: 'https://example.test', token: token(), ...extra });
const response = (status: number, body: BrowserTokenHttpResponse['body'] = null): BrowserTokenHttpResponse => ({ status, headers: { get: () => null }, body });
const reader = (read: BrowserTokenHttpReader['read'], cancel: BrowserTokenHttpReader['cancel'] = async () => {}): BrowserTokenHttpReader => ({ read, cancel, releaseLock() {} });
const requestInit = (signal = new AbortController().signal) => ({ method: 'POST' as const, headers: { 'Content-Type': 'application/msgpack', Accept: 'application/msgpack', 'X-Test': 'yes' }, body: new Uint8Array([1]), signal, redirect: 'error' as const, credentials: 'omit' as const });
const invoke = (fetcher: TokenFetch, init = requestInit(), settings = opts()) => createBrowserTokenPlatform(settings, fetcher).requestHttp(new URL('https://example.test/shamir/v1/request'), token(), init);
const neverResponse: TokenFetch = (_url, _init) => new Promise(() => {});

 describe('browser token HTTP transport', () => {
  it('preserves caller headers, replaces authorization, and enforces browser fetch flags', async () => {
    let seen: BrowserTokenHttpInit | undefined;
    const result = await invoke(async (_url, init) => { seen = init; return response(200); });
    expect(result.status).toBe(200);
    expect(seen?.headers).toMatchObject({ 'Content-Type': 'application/msgpack', Accept: 'application/msgpack', 'X-Test': 'yes', Authorization: `Bearer ${rawToken}` });
    expect(seen?.credentials).toBe('omit');
    expect(seen?.redirect).toBe('error');
  });

  it('maps peer-forged and ordinary fetch errors to network_error', async () => {
    const fake = Object.assign(new Error('do not expose'), { name: 'TokenTransportError', code: 'client_aborted' });
    await expect(invoke(async () => { throw fake; })).rejects.toMatchObject({ code: 'network_error' });
    await expect(invoke(async () => { throw new Error('secret'); })).rejects.toMatchObject({ code: 'network_error' });
    expect(TokenTransportError).toBeDefined();
  });

  it('enforces the streamed response cap even if reader cancellation rejects', async () => {
    const payload = new Uint8Array(16 * 1024 * 1024 + 1);
    await expect(invoke(async () => response(200, { getReader: () => reader(async () => ({ done: false, value: payload }), async () => { throw new Error('cancel'); }) }))).rejects.toMatchObject({ code: 'http_body_too_large' });
  });

  it('returns uniform 401 when unread-body cancellation rejects', async () => {
    const result = await invoke(async () => response(401, { getReader: () => reader(async () => new Promise(() => {}), async () => { throw new Error('cancel'); }) }));
    expect(result).toMatchObject({ status: 401, body: new Uint8Array() });
  });

  it('honors request and lifetime abort signals', async () => {
    const requestController = new AbortController();
    const platform = createBrowserTokenPlatform(opts(), neverResponse);
    const pending = platform.requestHttp(new URL('https://example.test/shamir/v1/request'), token(), requestInit(requestController.signal));
    const rejected = expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
    requestController.abort();
    await rejected;
    await platform.close();

    const lifetime = new AbortController();
    const lifetimePlatform = createBrowserTokenPlatform(opts({ signal: lifetime.signal }), neverResponse);
    const lifetimePending = lifetimePlatform.requestHttp(new URL('https://example.test/shamir/v1/request'), token(), requestInit());
    const lifetimeRejected = expect(lifetimePending).rejects.toMatchObject({ code: 'client_aborted' });
    lifetime.abort();
    await lifetimeRejected;
    await lifetimePlatform.close();
  });

  it('assigns connect phase to header deadline and request phase after headers', async () => {
    const before = createBrowserTokenPlatform(opts({ connectTimeoutMs: 5, requestTimeoutMs: 0 }), neverResponse);
    try { await expect(before.requestHttp(new URL('https://example.test/shamir/v1/request'), token(), requestInit())).rejects.toMatchObject({ code: 'client_timeout', phase: 'connect' }); }
    finally { await before.close(); }

    const after = createBrowserTokenPlatform(opts({ connectTimeoutMs: 0, requestTimeoutMs: 5 }), async () => response(200, { getReader: () => reader(async () => new Promise(() => {})) }));
    try { await expect(after.requestHttp(new URL('https://example.test/shamir/v1/request'), token(), requestInit())).rejects.toMatchObject({ code: 'client_timeout', phase: 'request' }); }
    finally { await after.close(); }
  });

  it('closes active requests and rejects future requests', async () => {
    const platform = createBrowserTokenPlatform(opts(), neverResponse);
    const pending = platform.requestHttp(new URL('https://example.test/shamir/v1/request'), token(), requestInit());
    const rejected = expect(pending).rejects.toMatchObject({ code: 'connection_closed', phase: 'close' });
    await platform.close();
    await rejected;
    await expect(platform.requestHttp(new URL('https://example.test/shamir/v1/request'), token(), requestInit())).rejects.toMatchObject({ code: 'connection_closed' });
  });

  it('rejects TLS and origin settings', async () => {
    expect(() => createBrowserTokenPlatform({ ...opts(), tls: {} as never }, async () => response(200))).toThrow(TokenTransportError);
    expect(() => createBrowserTokenPlatform({ ...opts(), origin: 'https://example.test' } as unknown as BrowserTokenHttpConnectOptions, async () => response(200))).toThrow(TokenTransportError);
  });

  it('keeps the browser adapter import graph free of Node built-ins', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const seen = new Set<string>();
    const visit = (file: string): void => {
      if (seen.has(file)) return;
      seen.add(file);
      const source = readFileSync(file, 'utf8');
      expect(source, file).not.toMatch(/(?:from\s*|import\s*\()\s*['"]node:/);
      const imports = source.matchAll(/(?:import\s+(?:type\s+)?(?:[^'";]*?\s+from\s+)?|export\s+[^'";]*?\s+from\s*)['"](\.[^'"]+)['"]/g);
      for (const match of imports) {
        const target = join(dirname(file), match[1]!);
        const resolved = target.endsWith('.js') ? target.slice(0, -3) + '.ts' : target;
        if (!existsSync(resolved)) throw new Error(`Missing browser graph import: ${resolved}`);
        visit(resolved);
      }
    };
    visit(join(here, '..', 'http-browser.ts'));
  });
});
