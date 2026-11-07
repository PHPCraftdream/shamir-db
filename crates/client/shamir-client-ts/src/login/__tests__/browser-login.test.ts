import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Mock } from 'vitest';
import { ApiToken } from '../../entry/api-token.js';
import { loginBrowser } from '../browser/browser.js';
import { browserLoginHttp } from '../browser/browser-http.js';
import type { LoginHttp } from '../core.js';
import { encodeMessage, freezeScope, canonicalUser, buildLoginTranscript } from '../protocol.js';
import { FailureRecorder, withFailureDiagnostics } from '../../__tests__/connection/token/support/failure-diagnostics.js';

const bytes = (n: number, value: number) => new Uint8Array(n).fill(value);

type ReadResult = { done: boolean; value?: Uint8Array };
function response(status = 200, chunks: Uint8Array[] = [], headers: Record<string, string> = {}) {
  const retained: Uint8Array[] = [];
  const reader = {
    read: vi.fn(async (): Promise<ReadResult> => {
      if (!chunks.length) return { done: true };
      const chunk = chunks.shift()!;
      retained.push(chunk);
      return { done: false, value: chunk };
    }),
    cancel: vi.fn(async () => {}), releaseLock: vi.fn(),
  };
  const body = { getReader: () => reader, cancel: vi.fn(async () => {}) };
  return { value: { status, headers: { get: (name: string) => {
    const key = Object.keys(headers).find(k => k.toLowerCase() === name.toLowerCase());
    return key === undefined ? null : headers[key];
  } }, body }, reader, body, retained };
}

type FetchArgs = [url: string, init: { method: 'POST'; headers: Readonly<Record<string, string>>; body: Uint8Array; credentials: 'omit'; redirect: 'error'; signal: AbortSignal }];
let fetchStub: Mock<FetchArgs, Promise<unknown>> = vi.fn<FetchArgs, Promise<unknown>>();
beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); vi.useRealTimers(); });

describe('browser HTTP transport', () => {
  it('uses a credential-free POST with exact headers and URL', async () => {
    const r = response(200, [bytes(3, 7)], { 'Content-Type': 'application/msgpack' });
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(r.value); vi.stubGlobal('fetch', fetchStub);
    const result = await browserLoginHttp.request(new URL('https://example.test/login'), bytes(2, 1), new AbortController().signal, 0);
    expect(fetchStub).toHaveBeenCalledTimes(1);
    const [url, init] = fetchStub.mock.calls[0]!;
    expect(url).toBe('https://example.test/login');
    expect(init).toMatchObject({ method: 'POST', credentials: 'omit', redirect: 'error', headers: {
      'Content-Type': 'application/msgpack', Accept: 'application/msgpack',
    } });
    expect(init.headers).not.toHaveProperty('Authorization');
    expect(result.body).toEqual(bytes(3, 7));
    expect(result.contentType).toBe('application/msgpack');
    expect(r.retained[0]).toEqual(bytes(3, 0));
    expect(vi.getTimerCount()).toBe(0);
  });

  it('accepts exactly 4096 bytes and wipes the offending oversized chunk', async () => {
    const ok = response(200, [bytes(4096, 1)]);
    const oversizedChunk = bytes(4097, 2); const oversized = response(200, [oversizedChunk]);
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValueOnce(ok.value).mockResolvedValueOnce(oversized.value);
    vi.stubGlobal('fetch', fetchStub);
    expect((await browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 0)).body).toHaveLength(4096);
    await expect(browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 0))
      .rejects.toMatchObject({ code: 'http_body_too_large' });
    expect(oversizedChunk).toEqual(bytes(4097, 0));
    expect(vi.getTimerCount()).toBe(0);
  });

  it('preserves the successful result while wiping every retained chunk', async () => {
    const first = bytes(2, 7); const second = bytes(3, 9); const r = response(200, [first, second]);
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(r.value); vi.stubGlobal('fetch', fetchStub);
    const result = await browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 0);
    expect(result.body).toEqual(new Uint8Array([7, 7, 9, 9, 9]));
    expect(first).toEqual(bytes(2, 0)); expect(second).toEqual(bytes(3, 0));
    expect(r.retained).toEqual([first, second]);
  });

  it('wipes the offending and prior chunks when the response exceeds the limit', async () => {
    const prior = bytes(3, 4); const offending = bytes(4094, 5); const r = response(200, [prior, offending]);
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(r.value); vi.stubGlobal('fetch', fetchStub);
    await expect(browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 0))
      .rejects.toMatchObject({ code: 'http_body_too_large' });
    expect(prior).toEqual(bytes(3, 0)); expect(offending).toEqual(bytes(4094, 0));
  });

  it('rejects encodings including an empty encoding and cancels bodies', async () => {
    for (const encoding of ['', 'gzip']) {
      const r = response(200, [], { 'Content-Encoding': encoding });
      fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(r.value); vi.stubGlobal('fetch', fetchStub);
      await expect(browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 0))
        .rejects.toMatchObject({ code: 'protocol_error' });
      expect(r.body.cancel).toHaveBeenCalled(); vi.unstubAllGlobals();
    }
    expect(vi.getTimerCount()).toBe(0);
  });

  it('rejects redirects and cancels their bodies', async () => {
    const r = response(302);
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(r.value); vi.stubGlobal('fetch', fetchStub);
    await expect(browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 0))
      .rejects.toMatchObject({ code: 'protocol_error' });
    expect(r.body.cancel).toHaveBeenCalled(); expect(vi.getTimerCount()).toBe(0);
  });

  it('returns garbled bodies as transport bytes', async () => {
    const r = response(200, [new Uint8Array([0xff, 0x00, 0x81])]);
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(r.value); vi.stubGlobal('fetch', fetchStub);
    await expect(browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 0))
      .resolves.toMatchObject({ body: new Uint8Array([0xff, 0x00, 0x81]) });
    expect(r.retained[0]).toEqual(bytes(3, 0)); expect(vi.getTimerCount()).toBe(0);
  });

  it('settles promptly on caller abort while fetch is pending and cancels late response', async () => {
    const recorder = new FailureRecorder();
    await withFailureDiagnostics(recorder, undefined, async () => {
      let resolveFetch!: (value: ReturnType<typeof response>['value']) => void;
      let entered!: () => void; const fetchEntered = new Promise<void>(resolve => { entered = resolve; });
      fetchStub = vi.fn<FetchArgs, Promise<unknown>>(() => { entered(); return new Promise(resolve => { resolveFetch = resolve; }); });
      vi.stubGlobal('fetch', fetchStub);
      const controller = new AbortController();
      recorder.mark('fetch started');
      const pending = browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), controller.signal, 0);
      const rejection = expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
      await fetchEntered; recorder.mark('abort called'); controller.abort();
      // Record what the promise actually settled with, expected or not.
      await rejection.then(
        () => recorder.mark('promise settled with the expected client_aborted rejection'),
        (error: unknown) => { recorder.mark(`promise settled with an unexpected outcome: ${String(error)}`); throw error; },
      );
      const late = response(); let canceled!: () => void;
      const cancelEntered = new Promise<void>(resolve => { canceled = resolve; });
      late.reader.cancel.mockImplementation(async () => { canceled(); });
      resolveFetch(late.value); recorder.mark('late response delivered'); await cancelEntered;
      recorder.note('late cancel calls', late.reader.cancel.mock.calls.length);
      expect(late.reader.cancel).toHaveBeenCalled();
      recorder.mark('late response cancel observed');
    });
  });

  it('wipes a value delivered with done=true after caller abort', async () => {
    const r = response(); let readEntered!: () => void; const entered = new Promise<void>(resolve => { readEntered = resolve; });
    let resolveRead!: (value: ReadResult) => void;
    let wiped!: () => void; const wipeObserved = new Promise<void>(resolve => { wiped = resolve; });
    r.reader.read.mockImplementation(() => { readEntered(); return new Promise(resolve => { resolveRead = resolve; }); });
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(r.value); vi.stubGlobal('fetch', fetchStub);
    const controller = new AbortController();
    const pending = browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), controller.signal, 0);
    const rejection = expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
    await entered; controller.abort(); await rejection;
    expect(r.reader.cancel).toHaveBeenCalled(); expect(r.reader.releaseLock).toHaveBeenCalled();
    const lateChunk = bytes(4, 8);
    const originalFill = lateChunk.fill.bind(lateChunk);
    vi.spyOn(lateChunk, 'fill').mockImplementation((value, ...args) => {
      const result = originalFill(value, ...args);
      if (value === 0) wiped();
      return result;
    });
    resolveRead({ done: true, value: lateChunk });
    await wipeObserved;
    expect(lateChunk).toEqual(bytes(4, 0));
  });

  it.each(['readerror', 'callerabort', 'timeout'] as const)('wipes prior chunks when a stream ends by %s', async reason => {
    const prior = bytes(3, 6); const r = response();
    let readEntered!: () => void; const entered = new Promise<void>(resolve => { readEntered = resolve; });
    let rejectRead!: (error: Error) => void; let resolveRead!: (value: ReadResult) => void;
    r.reader.read.mockResolvedValueOnce({ done: false, value: prior }).mockImplementationOnce(() => {
      readEntered();
      return new Promise((resolve, reject) => { resolveRead = resolve; rejectRead = reject; });
    });
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(r.value); vi.stubGlobal('fetch', fetchStub);
    const controller = new AbortController();
    const pending = browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), controller.signal, reason === 'timeout' ? 10 : 0);
    const rejection = expect(pending).rejects.toMatchObject({ code: reason === 'readerror' ? 'network_error' : reason === 'callerabort' ? 'client_aborted' : 'client_timeout' });
    await entered;
    expect(Array.from(prior)).toEqual([6, 6, 6]);
    if (reason === 'readerror') rejectRead(new Error('reader secret'));
    else if (reason === 'callerabort') controller.abort();
    else await vi.advanceTimersByTimeAsync(10);
    await rejection;
    expect(Array.from(prior)).toEqual([0, 0, 0]);
    if (reason !== 'readerror') {
      const lateChunk = bytes(4, 8);
      const originalFill = lateChunk.fill.bind(lateChunk);
      let wiped!: () => void; const wipeObserved = new Promise<void>(resolve => { wiped = resolve; });
      vi.spyOn(lateChunk, 'fill').mockImplementation((value, ...args) => {
        const result = originalFill(value, ...args);
        if (value === 0) wiped();
        return result;
      });
      resolveRead({ done: false, value: lateChunk });
      await wipeObserved;
      expect(Array.from(lateChunk)).toEqual([0, 0, 0, 0]);
    }
  });

  it('reports timeout distinctly and sanitizes reader failures', async () => {
    let readEntered!: () => void; const entered = new Promise<void>(resolve => { readEntered = resolve; });
    let resolveRead!: (value: ReadResult) => void;
    const timed = response(); timed.reader.read.mockImplementation(() => { readEntered(); return new Promise(resolve => { resolveRead = resolve; }); });
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(timed.value); vi.stubGlobal('fetch', fetchStub);
    const pending = browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 10);
    const rejection = expect(pending).rejects.toMatchObject({ code: 'client_timeout' });
    await entered; await vi.advanceTimersByTimeAsync(10); await rejection;
    expect(timed.reader.cancel).toHaveBeenCalled(); expect(vi.getTimerCount()).toBe(0);
    resolveRead({ done: true });

    vi.useRealTimers();
    const failing = response(); failing.reader.read.mockRejectedValue(new Error('reader secret'));
    failing.reader.cancel.mockRejectedValue(new Error('cancel secret'));
    fetchStub = vi.fn<FetchArgs, Promise<unknown>>().mockResolvedValue(failing.value); vi.stubGlobal('fetch', fetchStub);
    await expect(browserLoginHttp.request(new URL('https://example.test/'), bytes(1, 0), new AbortController().signal, 0))
      .rejects.toMatchObject({ code: 'network_error' });
  });
});

describe('browser login dependency injection', () => {
  it('sends only canonical username and freezes requested database scope', async () => {
    let captured: Uint8Array | undefined;
    const http: LoginHttp = { async request(_url, requestBody) {
      captured = requestBody.slice();
      return { status: 401, contentType: 'application/msgpack', retryAfter: null,
        body: encodeMessage({ error: 'authentication_failed' }) };
    } };
    await expect(loginBrowser({ username: 'ALICE', password: 'p', url: 'http://127.0.0.1/',
      scope: { label: 'test', databases: ['alpha', 'beta'] } }, http)).rejects.toMatchObject({ code: 'authentication_failed' });
    const { decodeMessage } = await import('../protocol.js');
    expect(decodeMessage(captured!).user).toBe('alice');
    expect(decodeMessage(captured!).token).toEqual({ label: 'test', mode: 'read_only', channels: 'rest', databases: ['alpha', 'beta'] });
  });

  it('canonicalizes Unicode usernames and rejects unsafe ones before making an HTTP request', async () => {
    expect(canonicalUser('Ａlice')).toBe('alice'); expect(canonicalUser('e\u0301')).toBe('é');
    expect(canonicalUser('ΟΔΥΣΣΕΥΣ')).toBe('οδυσσευσ'); expect(() => canonicalUser('xÅ')).toThrow();
    expect(() => canonicalUser('각')).toThrow();
    let requests = 0; const http: LoginHttp = { async request() { requests++; throw new Error('unexpected request'); } };
    for (const username of ['a\u202eb', 'a\ue000', 'a\uffff']) {
      await expect(loginBrowser({ username, password: 'p', url: 'http://127.0.0.1/' }, http)).rejects.toMatchObject({ code: 'invalid_request' });
    }
    expect(requests).toBe(0);
  });

  it('pins the independently specified login transcript bytes', () => {
    const transcript = buildLoginTranscript('alice', bytes(32, 0xaa), {
      security: 0, listenerId: bytes(16, 0xdd), handshakeId: bytes(32, 0xee), salt: bytes(16, 0xcc),
      serverNonce: bytes(32, 0xbb), memoryKb: 19456, time: 2, parallelism: 1,
    }, 19, freezeScope());
    const expected = Buffer.concat([
      Buffer.from('5348414d49522d485454502d4c4f47494e2d7631', 'hex'), Buffer.from('010300', 'hex'),
      Buffer.alloc(16, 0xdd), Buffer.alloc(32, 0xee), Buffer.from('0005616c696365', 'hex'), Buffer.alloc(32, 0xaa),
      Buffer.alloc(32, 0xbb), Buffer.alloc(16, 0xcc), Buffer.from('00004c00000000020000000113', 'hex'),
      Buffer.from('000a687474702d6c6f67696e0000ff', 'hex'),
    ]);
    expect(transcript).toHaveLength(186);
    expect(Buffer.from(transcript.subarray(0, 20)).toString('hex')).toBe('5348414d49522d485454502d4c4f47494e2d7631');
    expect(Buffer.from(transcript)).toEqual(expected);
  });

  it('enforces sorted ASCII database names and the exact 512-byte scope boundary', () => {
    const full = freezeScope({ label: 'A'.repeat(128), databases: ['A'.repeat(255), 'B'.repeat(120)] });
    expect(full.databases).toEqual(['A'.repeat(255), 'B'.repeat(120)]);
    expect(() => freezeScope({ label: 'A'.repeat(128), databases: ['A'.repeat(255), 'B'.repeat(121)] })).toThrow();
    expect(() => freezeScope({ databases: ['beta', 'alpha'] })).toThrow(); expect(() => freezeScope({ databases: ['é'] })).toThrow();
  });

  it('rejects invalid options without I/O', async () => {
    let requests = 0; const http: LoginHttp = { async request() { requests++; throw new Error('unexpected request'); } };
    const base = { username: 'alice', password: 'p', url: 'http://127.0.0.1/' };
    const cases: Array<{ options: unknown; code: string }> = [
      { options: null, code: 'invalid_request' }, { options: [], code: 'invalid_request' },
      { options: { ...base, ttl: 1 }, code: 'invalid_request' }, { options: { ...base, timeoutMs: -1 }, code: 'invalid_request' },
      { options: { ...base, url: 'http://127.0.0.1/path' }, code: 'invalid_request' },
      { options: { ...base, externalTls: true }, code: 'security_mismatch' },
    ];
    for (const { options, code } of cases) await expect(loginBrowser(options as never, http)).rejects.toMatchObject({ code });
    expect(requests).toBe(0);
  });
});

it('keeps token secrets out of the public string representation', () => {
  const secret = 'shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';
  const token = ApiToken.from(secret);
  expect(String(token)).not.toContain(secret); expect(JSON.stringify(token)).not.toContain(secret); expect(token.toString()).not.toContain(secret);
});
