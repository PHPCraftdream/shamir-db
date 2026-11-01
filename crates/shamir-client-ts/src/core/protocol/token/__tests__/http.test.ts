import { describe, expect, it } from 'vitest';
import { TokenHttpTransport } from '../http.js';
import { encode } from '../../framing.js';
import { ApiToken } from '../../../../entry/api-token.js';
import type { TokenPlatform } from '../../../../platform/token/types.js';
import { normalizeTokenHttpOptions, DEFAULT_TOKEN_CONNECT_TIMEOUT_MS, DEFAULT_TOKEN_REQUEST_TIMEOUT_MS } from '../options.js';
import { TokenTransportError } from '../errors.js';
import { ShamirDbError } from '../../../support/errors.js';
import { VisibilityError } from '../../../visibility/errors.js';
import { Batch } from '../../../builders/write/batch.js';
import { Query } from '../../../builders/read/query.js';

const token = ApiToken.from(`shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`);
function platform(status: number, value: unknown, contentType = 'application/msgpack'): TokenPlatform {
  return { requestHttp: async () => ({ status, headers: { get: name => name === 'content-type' ? contentType : null }, body: encode(value) }), close: async () => {} };
}
const base = new URL('https://example.test/');

describe('token HTTP protocol', () => {
  it('maps uniform unauthorized without decoding body', async () => {
    for (const body of [new Uint8Array([0xc1]), new Uint8Array([0x81, 0xa1, 0x61])]) {
      const p: TokenPlatform = { requestHttp: async () => ({ status: 401, headers: { get: () => 'application/msgpack' }, body }), close: async () => {} };
      await expect(new TokenHttpTransport(base, token, p).request({ op: 'ping' })).rejects.toMatchObject({ code: 'unauthorized' });
    }
  });
  it.each([400, 403])('sanitizes decoded application errors for HTTP %i', async status => {
    for (const code of ['known_code', 'unknown_safe_code', 'password']) {
      const transport = new TokenHttpTransport(base, token, platform(status, { kind: 'error', code, message: 'shk1.secret-secret-secret' }));
      await expect(transport.request({ op: 'ping' })).rejects.toMatchObject({ detail: 'Remote application error' });
      await expect(transport.request({ op: 'ping' })).rejects.not.toHaveProperty('message', 'shk1.secret-secret-secret');
    }
  });
  it('maps decoded application error codes to typed sanitized errors for HTTP and protocol responses', async () => {
    for (const status of [200, 403]) {
      const known = new TokenHttpTransport(base, token, platform(status, { kind: 'error', code: 'token_read_only', message: 'secret detail' }));
      await expect(known.request({ op: 'ping' })).rejects.toBeInstanceOf(TokenTransportError);
      await expect(known.request({ op: 'ping' })).rejects.toMatchObject({ code: 'token_read_only', message: 'Token HTTP request failed' });
      const unknown = new TokenHttpTransport(base, token, platform(status, { kind: 'error', code: 'safe_unknown_code', message: 'secret detail' }));
      await expect(unknown.request({ op: 'ping' })).rejects.toBeInstanceOf(ShamirDbError);
      await expect(unknown.request({ op: 'ping' })).rejects.toMatchObject({ code: 'safe_unknown_code', detail: 'Remote application error' });
    }
  });
  it('races timeout, explicit abort, and close against a platform that ignores abort', async () => {
    const hanging: TokenPlatform = { requestHttp: () => new Promise(() => {}), close: async () => {} };

    const timed = new TokenHttpTransport(base, token, hanging);
    await expect(timed.request({ op: 'ping' }, { timeoutMs: 5 })).rejects.toMatchObject({ code: 'client_timeout' });

    const abortController = new AbortController();
    const aborted = new TokenHttpTransport(base, token, hanging);
    const abortResult = expect(aborted.request({ op: 'ping' }, { signal: abortController.signal })).rejects.toMatchObject({ code: 'client_aborted' });
    abortController.abort();
    await abortResult;

    const closed = new TokenHttpTransport(base, token, hanging);
    const closeResult = expect(closed.request({ op: 'ping' })).rejects.toMatchObject({ code: 'connection_closed' });
    await closed.close();
    await closeResult;

    let calls = 0;
    const preabortedPlatform: TokenPlatform = { requestHttp: () => { calls++; return new Promise(() => {}); }, close: async () => {} };
    const preabortedController = new AbortController();
    preabortedController.abort();
    const preaborted = new TokenHttpTransport(base, token, preabortedPlatform);
    await expect(preaborted.request({ op: 'ping' }, { signal: preabortedController.signal })).rejects.toMatchObject({ code: 'client_aborted' });
    expect(calls).toBe(0);
  });
  it('requires the response media type and valid batch envelope', async () => {
    const badType = new TokenHttpTransport(base, token, platform(200, { kind: 'pong' }, 'text/plain'));
    await expect(badType.request({ op: 'ping' })).rejects.toMatchObject({ code: 'protocol_error' });
    const malformed = new TokenHttpTransport(base, token, platform(200, { result: {} }));
    await expect(malformed.request({ op: 'ping' })).rejects.toMatchObject({ code: 'protocol_error' });
  });
  it('surfaces sanitized typed visibility timeouts from execute and REST visibility waits losslessly', async () => {
    const cases = [
      { requested: 42, visible_through: 40, blocking_version: 41 },
      { requested: 9_007_199_254_740_993n, visible_through: 9_007_199_254_740_991n, blocking_version: 9_007_199_254_740_992n },
    ];
    for (const metadata of cases) {
      const response = { kind: 'as_of_visibility_timeout', code: 'as_of_visibility_timeout', message: 'peer secret and shk1.token', ...metadata };
      for (const request of [
        { op: 'execute', db: 'd', batch: Batch.create().add('read', Query.from('items')).build(), query_version: 4 },
        { op: 'wait_for_visibility', db: 'd', repo: 'r', version: 1n, visibility_wait: { mode: 'strict' }, query_version: 4 },
      ] as const) {
        const transport = new TokenHttpTransport(base, token, platform(200, response));
        const rejected = transport.request(request);
        await expect(rejected).rejects.toBeInstanceOf(VisibilityError);
        await expect(rejected).rejects.toMatchObject({ code: 'as_of_visibility_timeout', requested: BigInt(metadata.requested), visible_through: BigInt(metadata.visible_through), blocking_version: BigInt(metadata.blocking_version), detail: 'Visibility wait timed out' });
        try { await rejected; } catch (error) {
          expect(error).toBeInstanceOf(VisibilityError);
          expect(String(error)).not.toContain('peer secret');
          expect((error as Error).message).not.toContain('peer secret');
          expect((error as Error).stack).not.toContain('peer secret');
          expect(String(error)).not.toContain('shk1.token');
          expect((error as Error).message).not.toContain('shk1.token');
          expect((error as Error).stack).not.toContain('shk1.token');
        }
      }
    }
  });
  it.each([
    { kind: 'as_of_visibility_timeout', code: 'wrong_code', message: 'secret', requested: 1, visible_through: 0, blocking_version: 1 },
    { kind: 'as_of_visibility_timeout', code: 'as_of_visibility_timeout', message: 'secret', requested: -1, visible_through: 0, blocking_version: 1 },
  ])('rejects malformed visibility timeout metadata as a protocol error', async response => {
    await expect(new TokenHttpTransport(base, token, platform(200, response)).request({ op: 'wait_for_visibility', db: 'd', repo: 'r', version: 1n, visibility_wait: { mode: 'strict' }, query_version: 4 })).rejects.toMatchObject({ code: 'protocol_error' });
  });
  it('allows REST visibility requests', async () => {
    const response = { kind: 'visibility_observed', version: 1, visibility: { state: 'visible' } };
    const transport = new TokenHttpTransport(base, token, platform(200, response));
    await expect(transport.request({ op: 'wait_for_visibility', db: 'd', repo: 'r', version: 1n, visibility_wait: { mode: 'strict' }, query_version: 4 })).resolves.toEqual(response);
  });
  it('accepts a valid nested batch response', async () => {
    const response = { kind: 'batch', response: { id: 1, results: { read: { records: [] } }, execution_plan: [['read']], execution_time_us: 42 } };
    const transport = new TokenHttpTransport(base, token, platform(200, response));
    await expect(transport.request({ op: 'ping' })).resolves.toEqual(response);
  });
  it('maps HTTP statuses and Retry-After', async () => {
    const cases: Array<[number, string]> = [[400, 'http_bad_request'], [403, 'protocol_error'], [404, 'http_not_found'], [405, 'http_method_not_allowed'], [408, 'http_timeout'], [413, 'http_body_too_large'], [415, 'http_unsupported_media_type'], [431, 'http_bad_headers'], [429, 'rate_limited'], [503, 'server_unavailable'], [500, 'protocol_error']];
    for (const [status, code] of cases) {
      const p = platform(status, {}, 'application/msgpack');
      for (const [header, retryAfterMs] of [['Wed, 21 Oct 2015 07:28:00 GMT', 0], ['2', 2000], ['invalid', undefined], ['', undefined]] as const) {
        const retryPlatform: TokenPlatform = { ...p, requestHttp: async (url, t, options) => ({ ...(await p.requestHttp(url, t, options)), headers: { get: name => name === 'retry-after' ? header : name === 'content-type' ? 'application/msgpack' : null } }) };
        const rejection = new TokenHttpTransport(base, token, retryPlatform).request({ op: 'ping' });
        await expect(rejection).rejects.toMatchObject({ code, ...(status === 429 || status === 503 ? { retryable: true } : {}) });
        try { await rejection; } catch (error) {
          expect(error).toBeInstanceOf(TokenTransportError);
          const actualRetryAfterMs = (error as TokenTransportError).retryAfterMs;
          if (retryAfterMs === undefined || (status !== 429 && status !== 503)) expect(actualRetryAfterMs).toBeUndefined();
          else expect(actualRetryAfterMs).toBe(retryAfterMs);
        }
      }
    }
  });
  it('rejects invalid origin URLs and applies timeout defaults while allowing zero', () => {
    for (const url of ['http:example.test', 'https://@example.test', 'https://example.test/../', 'https://example.test/?', 'https://example.test/#', 'ftp://example.test', 'https://example.test\\\\path', 'https://example.test/\r']) {
      expect(() => normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token, url })).toThrow();
    }
    const defaults = normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token, url: 'https://example.test' });
    expect(defaults.requestTimeoutMs).toBe(DEFAULT_TOKEN_REQUEST_TIMEOUT_MS);
    expect(defaults.requestTimeoutMs).toBe(35_000);
    expect(defaults.connectTimeoutMs).toBe(DEFAULT_TOKEN_CONNECT_TIMEOUT_MS);
    expect(defaults.connectTimeoutMs).toBe(10_000);
    const zero = normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token, url: 'https://example.test', requestTimeoutMs: 0, connectTimeoutMs: 0 });
    expect(zero.requestTimeoutMs).toBe(0);
    expect(zero.connectTimeoutMs).toBe(0);
  });
  it('rejects invalid options and non-handle token values', () => {
    expect(() => normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token, url: 'https://example.test/?' })).toThrow();
    expect(() => normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token, url: 'https://example.test', tls: { allowInsecureTlsForDevelopment: true } })).not.toThrow();
    expect(() => normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token, url: 'https://example.test', requestTimeoutMs: 2 ** 31 })).toThrow();
    // Deliberately bypass the static literal type to exercise runtime rejection.
    expect(() => normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token, url: 'https://example.test', tls: { allowInsecureTlsForDevelopment: false as unknown as true } })).toThrow();
    expect(() => normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token, url: 'https://example.test', requestTimeoutMs: 0 })).not.toThrow();
    expect(() => normalizeTokenHttpOptions({ auth: 'token', channel: 'http', token: {} as never, url: 'https://example.test' })).toThrow();
  });
});

