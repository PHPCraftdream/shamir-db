import { decode, encode, MAX_FRAME_SIZE_DEFAULT } from '../framing.js';
import type { ApiToken } from '../../../entry/api-token.js';
import { authorizationHeader } from './auth.js';
import { TokenTransportError, isTokenTransportErrorCode, type TokenTransportErrorCode } from './errors.js';
import { ShamirDbError, isRetryableCode } from '../../support/errors.js';
import { VisibilityError } from '../../visibility/errors.js';
import { position } from '../../visibility/positions.js';
import type { TokenPlatform } from '../../../platform/token/types.js';
import type { TokenHttpRequest, TokenRequestOptions } from './options.js';
import type { TokenResponseMap } from './transport.js';
export type { TokenResponseMap } from './transport.js';

export const MAX_TOKEN_RESPONSE_BYTES = MAX_FRAME_SIZE_DEFAULT;
const SAFE_CODE = /^[a-z][a-z0-9_]{0,63}$/;
function object(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) && !(value instanceof Uint8Array); }
function fail(code: TokenTransportErrorCode, status?: number): TokenTransportError { return new TokenTransportError(code, 'Token HTTP request failed', { ...(status === undefined ? {} : { status }) }); }
function appError(value: unknown, status: number): Error | undefined {
  if (object(value) && value.kind === 'as_of_visibility_timeout') {
    if (value.code !== 'as_of_visibility_timeout' || typeof value.message !== 'string') throw fail('protocol_error', status);
    try {
      const requested = position(value.requested);
      const visible_through = position(value.visible_through);
      const blocking_version = position(value.blocking_version);
      return new VisibilityError({ kind: 'as_of_visibility_timeout', code: 'as_of_visibility_timeout', message: 'Visibility wait timed out', requested, visible_through, blocking_version });
    } catch {
      throw fail('protocol_error', status);
    }
  }
  if (!object(value) || value.kind !== 'error' || typeof value.code !== 'string' || typeof value.message !== 'string') return undefined;
  if (!SAFE_CODE.test(value.code)) return fail(status === 403 ? 'protocol_error' : 'http_bad_request', status);
  if (value.code === 'timeout' || value.code === 'lock_timeout' || value.code === 'tx_conflict' || value.code === 'read_only_replica' || value.code === 'wal_retention_exceeded') return new ShamirDbError(value.code, 'Remote application error');
  if (isTokenTransportErrorCode(value.code)) return new TokenTransportError(value.code, 'Token HTTP request failed', { status, retryable: isRetryableCode(value.code) });
  return new ShamirDbError(value.code, 'Remote application error');
}
function validateResponse(value: unknown): TokenResponseMap {
  if (!object(value)) throw fail('protocol_error');
  if (value.kind === 'pong') return value;
  if (value.kind === 'as_of_visibility_timeout') {
    const error = appError(value, 200);
    if (error) throw error;
  }
  if (value.kind === 'error') {
    const error = appError(value, 200);
    if (error) throw error;
  }
  const batch = value.kind === 'batch' && object(value.response) ? value.response : undefined;
  if (batch && object(batch.results) && Array.isArray(batch.execution_plan) && batch.execution_plan.every((step: unknown) => Array.isArray(step) && step.every((item: unknown) => typeof item === 'string')) && (typeof batch.execution_time_us === 'number' || typeof batch.execution_time_us === 'bigint') && Number.isSafeInteger(Number(batch.execution_time_us)) && Number(batch.execution_time_us) >= 0 && Object.prototype.hasOwnProperty.call(batch, 'id')) return value;
  if (value.kind === 'visibility_observed' && 'version' in value && 'visibility' in value) return value;
  if (value.kind === 'ddl_op_status' && 'status' in value) return value;
  throw fail('protocol_error');
}
function retryAfter(value: string | null): number | undefined {
  if (value === null) return undefined;
  const trimmed = value.trim();
  if (trimmed.length === 0) return undefined;
  if (/^\d+$/.test(trimmed)) {
    const milliseconds = Number(trimmed) * 1000;
    if (Number.isFinite(milliseconds)) return Math.min(milliseconds, 0x7fff_ffff);
  }
  const date = Date.parse(value);
  return Number.isNaN(date) ? undefined : Math.min(Math.max(0, date - Date.now()), 0x7fff_ffff);
}

export class TokenHttpTransport {
  readonly channel = 'http' as const;
  private readonly lifetime = new AbortController();
  private readonly active = new Set<AbortController>();
  private closed = false;
  constructor(private readonly baseUrl: URL, private readonly token: ApiToken, private readonly platform: TokenPlatform,
    private readonly transportOptions: { requestTimeoutMs?: number; signal?: AbortSignal } = {}) {}

  async request(request: TokenHttpRequest, options: TokenRequestOptions = {}): Promise<TokenResponseMap> {
    if (!request || typeof request !== 'object') throw fail('token_request_denied');
    const op = (request as { op?: unknown }).op;
    if (op !== 'ping' && op !== 'execute' && op !== 'wait_for_visibility' && op !== 'get_ddl_op_status') {
      throw fail(typeof op === 'string' && ['tx_begin','tx_execute','tx_commit','tx_rollback','create_cursor','fetch_next','cancel_cursor','subscribe','unsubscribe'].includes(op) ? 'not_supported_over_rest' : 'token_request_denied');
    }
    if (this.closed) throw fail('connection_closed');
    const ping = request.op === 'ping';
    const controller = new AbortController();
    this.active.add(controller);
    const signals = [this.lifetime.signal, this.transportOptions.signal, options.signal].filter((signal): signal is AbortSignal => signal !== undefined);
    if (signals.some(signal => signal.aborted)) {
      this.active.delete(controller);
      throw fail(this.closed ? 'connection_closed' : 'client_aborted');
    }
    const abort = (): void => controller.abort();
    for (const signal of signals) {
      if (signal.aborted) controller.abort();
      else signal.addEventListener('abort', abort, { once: true });
    }
    const requestedTimeout = options.timeoutMs;
    if (requestedTimeout !== undefined && (!Number.isSafeInteger(requestedTimeout) || requestedTimeout < 0 || requestedTimeout > 0x7fff_ffff)) {
      this.active.delete(controller);
      for (const signal of signals) signal.removeEventListener('abort', abort);
      throw fail('invalid_request');
    }
    const timeoutMs = requestedTimeout ?? this.transportOptions.requestTimeoutMs ?? 35_000;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let onControllerAbort: (() => void) | undefined;
    let timedOut = false;
    if (timeoutMs > 0) timer = setTimeout(() => { timedOut = true; controller.abort(); }, timeoutMs);
    try {
      const headers: Record<string, string> = { Authorization: authorizationHeader(this.token), Accept: 'application/msgpack' };
      const body = ping ? undefined : encode(request);
      if (body) headers['Content-Type'] = 'application/msgpack';
      const pending = Promise.resolve().then(() => this.platform.requestHttp(new URL(ping ? '/shamir/v1/ping' : '/shamir/v1/request', this.baseUrl), this.token,
        { method: ping ? 'GET' : 'POST', headers, ...(body ? { body } : {}), signal: controller.signal, redirect: 'error', credentials: 'omit' }));
      const aborted = new Promise<never>((_, reject) => {
        const rejectAbort = (): void => reject(fail(this.closed ? 'connection_closed' : timedOut ? 'client_timeout' : 'client_aborted'));
        if (controller.signal.aborted) rejectAbort();
        else {
          onControllerAbort = rejectAbort;
          controller.signal.addEventListener('abort', onControllerAbort, { once: true });
        }
      });
      const response = await Promise.race([pending, aborted]);
      if (response.status === 401) throw fail('unauthorized', 401);
      if (response.body.byteLength > MAX_TOKEN_RESPONSE_BYTES) throw fail('http_body_too_large', response.status);
      if (response.status !== 200) {
        const statusCode: TokenTransportErrorCode = response.status === 400 ? 'http_bad_request' : response.status === 403 ? 'protocol_error' :
          response.status === 404 ? 'http_not_found' : response.status === 405 ? 'http_method_not_allowed' : response.status === 408 ? 'http_timeout' :
          response.status === 413 ? 'http_body_too_large' : response.status === 415 ? 'http_unsupported_media_type' : response.status === 431 ? 'http_bad_headers' :
          response.status === 429 ? 'rate_limited' : response.status === 503 ? 'server_unavailable' : 'protocol_error';
        if ((response.status === 400 || response.status === 403) && response.headers.get('content-type')?.split(';', 1)[0]?.trim().toLowerCase() === 'application/msgpack') {
          try { const error = appError(decode(response.body), response.status); if (error) throw error; } catch (error) {
            if (error instanceof TokenTransportError || error instanceof ShamirDbError) throw error;
          }
        }
        const retryAfterMs = response.status === 429 || response.status === 503 ? retryAfter(response.headers.get('retry-after')) : undefined;
        throw new TokenTransportError(statusCode, 'Token HTTP request failed', { status: response.status, ...(retryAfterMs === undefined ? {} : { retryAfterMs }), retryable: response.status === 429 || response.status === 503 });
      }
      if (response.headers.get('content-type')?.split(';', 1)[0]?.trim().toLowerCase() !== 'application/msgpack') throw fail('protocol_error', response.status);
      try { return validateResponse(decode(response.body)); }
      catch (error) { if (error instanceof TokenTransportError || error instanceof ShamirDbError) throw error; throw fail('protocol_error', response.status); }
    } catch (error) {
      if (timedOut) throw fail('client_timeout');
      if (this.closed) throw fail('connection_closed');
      if (error instanceof TokenTransportError || error instanceof ShamirDbError) throw error;
      if (signals.some(signal => signal.aborted) || controller.signal.aborted) throw fail('client_aborted');
      throw fail('network_error');
    } finally {
      if (timer !== undefined) clearTimeout(timer);
      if (onControllerAbort !== undefined) controller.signal.removeEventListener('abort', onControllerAbort);
      for (const signal of signals) signal.removeEventListener('abort', abort);
      this.active.delete(controller);
    }
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.lifetime.abort();
    for (const controller of this.active) controller.abort();
    this.active.clear();
    try { await this.platform.close(); } catch { throw fail('network_error'); }
  }
}
