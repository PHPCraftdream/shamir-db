import type { ApiToken } from '../../entry/api-token.js';
import { tokenValueForTransport } from '../../entry/api-token.js';
import type { BrowserTokenHttpConnectOptions } from '../../core/types/connection.js';
import { normalizeTokenHttpOptions } from '../../core/protocol/token/options.js';
import { TokenTransportError, type TokenTransportErrorCode } from '../../core/protocol/token/errors.js';
import type { TokenPlatform, TokenHttpResponse } from './types.js';
import { DEFAULT_TOKEN_RESPONSE_BYTES } from './types.js';

export interface BrowserTokenHttpReader {
  read(): Promise<{ done: boolean; value?: Uint8Array }>;
  cancel(reason?: unknown): Promise<void>;
  releaseLock?(): void;
}
export interface BrowserTokenHttpResponse {
  readonly status: number;
  readonly headers: { get(name: string): string | null };
  readonly body: { getReader(): BrowserTokenHttpReader } | null;
  arrayBuffer?(): Promise<ArrayBuffer>;
}
export interface BrowserTokenHttpInit {
  method: 'GET' | 'POST'; headers: Readonly<Record<string, string>>; body?: Uint8Array; signal: AbortSignal;
  redirect: 'error'; credentials: 'omit';
}
export type TokenFetch = (url: string, init: BrowserTokenHttpInit) => Promise<BrowserTokenHttpResponse>;
const DEFAULT_REQUEST_TIMEOUT_MS = 35_000;
const DEFAULT_CONNECT_TIMEOUT_MS = 10_000;
function failure(code: TokenTransportErrorCode, phase: 'connect' | 'request' | 'close' = 'request'): TokenTransportError {
  return new TokenTransportError(code, code, { phase });
}
function nativeFetch(): TokenFetch {
  const candidate = (globalThis as unknown as { fetch?: TokenFetch }).fetch;
  if (typeof candidate !== 'function') throw failure('network_error', 'connect');
  return candidate.bind(globalThis);
}

export function createBrowserTokenPlatform(options: BrowserTokenHttpConnectOptions, fetchOverride?: TokenFetch): TokenPlatform & { close(): Promise<void> } {
  let normalized: ReturnType<typeof normalizeTokenHttpOptions>;
  try { normalized = normalizeTokenHttpOptions(options); }
  catch { throw failure('invalid_request'); }
  if (normalized.tls !== undefined || normalized.origin !== undefined) throw failure('invalid_request');
  const requestTimeout = options.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
  const connectTimeout = options.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS;
  const fetcher = fetchOverride ?? nativeFetch();
  const lifetime = new AbortController();
  const active = new Set<AbortController>();
  let closed = false;
  const onLifetimeAbort = (): void => lifetime.abort();
  if (options.signal?.aborted) onLifetimeAbort();
  else options.signal?.addEventListener('abort', onLifetimeAbort, { once: true });

  return {
    async requestHttp(url: URL, token: ApiToken, init): Promise<TokenHttpResponse> {
      if (closed) throw failure('connection_closed', 'close');
      if (options.signal?.aborted || init.signal.aborted) throw failure('client_aborted');
      if (!(url instanceof URL) || (url.href !== normalized.pingUrl.href && url.href !== normalized.requestUrl.href)) throw failure('invalid_request');
      const controller = new AbortController(); active.add(controller);
      let timedOut = false; let aborted = false; let phase: 'connect' | 'request' = 'connect'; let settledAbort: (() => void) | undefined;
      const abortWait = new Promise<never>((_, reject) => { settledAbort = () => reject(failure(timedOut ? 'client_timeout' : closed ? 'connection_closed' : 'client_aborted', timedOut ? phase : closed ? 'close' : 'request')); });
      const abort = (): void => { aborted = true; controller.abort(); settledAbort?.(); };
      const abortInit = (): void => abort();
      init.signal.addEventListener('abort', abortInit, { once: true });
      lifetime.signal.addEventListener('abort', abort, { once: true });
      let requestTimer: ReturnType<typeof setTimeout> | undefined;
      // Browser fetch has no observable connection/TLS phase: connectTimeout covers fetch until response headers arrive.
      let connectTimer: ReturnType<typeof setTimeout> | undefined;
      const clearTimers = (): void => { if (requestTimer !== undefined) clearTimeout(requestTimer); if (connectTimer !== undefined) clearTimeout(connectTimer); };
      const timeout = (): void => { timedOut = true; controller.abort(); settledAbort?.(); };
      if (requestTimeout > 0) requestTimer = setTimeout(timeout, requestTimeout);
      if (connectTimeout > 0) connectTimer = setTimeout(timeout, connectTimeout);
      let reader: BrowserTokenHttpReader | undefined;
      const cancelReader = (): void => { if (reader) { try { void Promise.resolve(reader.cancel()).catch(() => {}); } catch { /* cancellation is best-effort */ } } };
      controller.signal.addEventListener('abort', cancelReader, { once: true });
      try {
        const headers = { ...init.headers, Authorization: `Bearer ${tokenValueForTransport(token)}` };
        const pending = Promise.resolve().then(() => fetcher(url.href, { method: init.method, headers, ...(init.body ? { body: init.body } : {}), signal: controller.signal, redirect: 'error', credentials: 'omit' })).catch(() => { throw failure('network_error', phase); });
        pending.then((lateResponse) => { if (controller.signal.aborted && lateResponse.body) { try { const lateReader = lateResponse.body.getReader(); try { void Promise.resolve(lateReader.cancel()).catch(() => {}); } catch { /* best-effort cleanup */ } try { lateReader.releaseLock?.(); } catch { /* best-effort cleanup */ } } catch { /* best-effort cleanup */ } } }, () => {});
        const response = await Promise.race([pending, abortWait]);
        phase = 'request';
        if (connectTimer !== undefined) { clearTimeout(connectTimer); connectTimer = undefined; }
        if (response.status === 401) {
          if (response.body) { try { reader = response.body.getReader(); cancelReader(); } catch { /* unauthorized response remains uniform */ } }
          return { status: 401, headers: response.headers, body: new Uint8Array() };
        }
        if (!response.body) return { status: response.status, headers: response.headers, body: new Uint8Array() };
        reader = response.body.getReader();
        const chunks: Uint8Array[] = []; let size = 0;
        for (;;) {
          const pendingRead = Promise.resolve().then(() => reader!.read()).catch(() => { throw failure('network_error', 'request'); });
          const part = await Promise.race([pendingRead, abortWait]);
          if (part.done) break;
          if (part.value) {
            size += part.value.byteLength;
            if (size > DEFAULT_TOKEN_RESPONSE_BYTES) { cancelReader(); throw failure('http_body_too_large'); }
            chunks.push(part.value);
          }
        }
        const body = new Uint8Array(size); let offset = 0;
        for (const chunk of chunks) { body.set(chunk, offset); offset += chunk.byteLength; }
        return { status: response.status, headers: response.headers, body };
      } catch (error) {
        if (error instanceof TokenTransportError) throw error;
        if (timedOut) throw failure('client_timeout');
        if (closed) throw failure('connection_closed', 'close');
        if (aborted) throw failure('client_aborted');
        throw failure('network_error');
      } finally {
        clearTimers(); init.signal.removeEventListener('abort', abortInit); lifetime.signal.removeEventListener('abort', abort);
        controller.signal.removeEventListener('abort', cancelReader); active.delete(controller);
        try { reader?.releaseLock?.(); } catch { /* release lock is best-effort */ }
      }
    },
    async close(): Promise<void> {
      if (closed) return; closed = true; lifetime.abort();
      for (const controller of active) controller.abort(); active.clear();
      options.signal?.removeEventListener('abort', onLifetimeAbort);
    },
  };
}
