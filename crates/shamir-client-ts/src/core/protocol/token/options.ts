import type { TokenTlsOptions, TokenHttpConnectOptions } from '../../types/connection.js';
import type { BatchRequest } from '../../types/write/batch.js';
import type { VisibilityWait } from '../../visibility/types.js';
import { tokenValueForTransport } from '../../../entry/api-token.js';

export const DEFAULT_TOKEN_REQUEST_TIMEOUT_MS = 35_000;
export const DEFAULT_TOKEN_CONNECT_TIMEOUT_MS = 10_000;
const MAX_TIMEOUT = 0x7fff_ffff;

export interface NormalizedTokenHttpOptions extends Omit<TokenHttpConnectOptions, 'url' | 'requestTimeoutMs' | 'connectTimeoutMs'> {
  readonly url: URL;
  readonly pingUrl: URL;
  readonly requestUrl: URL;
  readonly requestTimeoutMs: number;
  readonly connectTimeoutMs: number;
  readonly tls?: TokenTlsOptions;
  readonly origin?: string;
}

function timeout(value: number | undefined, fallback: number): number {
  const result = value ?? fallback;
  if (!Number.isSafeInteger(result) || result < 0 || result > MAX_TIMEOUT) throw new TypeError('Invalid timeout');
  return result;
}

export function normalizeTokenHttpOptions(options: TokenHttpConnectOptions & { tls?: TokenTlsOptions; origin?: string }): NormalizedTokenHttpOptions {
  if (!options || options.auth !== 'token' || options.channel !== 'http') throw new TypeError('Invalid token HTTP options');
  try { tokenValueForTransport(options.token); } catch { throw new TypeError('Invalid API token handle'); }
  const requestTimeoutMs = timeout(options.requestTimeoutMs, DEFAULT_TOKEN_REQUEST_TIMEOUT_MS);
  const connectTimeoutMs = timeout(options.connectTimeoutMs, DEFAULT_TOKEN_CONNECT_TIMEOUT_MS);
  const raw = options.url;
  if (typeof raw !== 'string' || raw.trim() !== raw || !/^https?:\/\/[^/?#\\\\\s]+\/?$/i.test(raw)) throw new TypeError('Invalid token HTTP URL');
  let url: URL;
  try { url = new URL(raw); } catch { throw new TypeError('Invalid token HTTP URL'); }
  const authority = raw.slice(raw.indexOf('//') + 2).replace(/\/$/, '');
  if (!/^https?:$/.test(url.protocol) || !url.hostname || authority.includes('@') || url.username || url.password ||
      (url.pathname !== '/' && url.pathname !== '') || url.search || url.hash ||
      /(?:^|\/)(?:\.|%2e)(?:\.|%2e)?(?:\/|$)/i.test(raw)) {
    throw new TypeError('Token HTTP URL must be an origin root');
  }
  if (options.signal !== undefined && (!options.signal || typeof options.signal !== 'object' ||
      typeof options.signal.addEventListener !== 'function' || typeof options.signal.removeEventListener !== 'function' ||
      typeof options.signal.aborted !== 'boolean')) throw new TypeError('Invalid token HTTP options');
  if (options.tls !== undefined && (!options.tls || typeof options.tls !== 'object' || Array.isArray(options.tls) ||
      Object.keys(options.tls).some((key) => key !== 'ca' && key !== 'allowInsecureTlsForDevelopment') ||
      (options.tls.ca !== undefined && typeof options.tls.ca !== 'string' && !(options.tls.ca instanceof Uint8Array)) ||
      (options.tls.allowInsecureTlsForDevelopment !== undefined && options.tls.allowInsecureTlsForDevelopment !== true))) {
    throw new TypeError('Invalid TLS options');
  }
  if (options.origin !== undefined && (typeof options.origin !== 'string' || /[\r\n]/.test(options.origin))) throw new TypeError('Invalid token HTTP options');
  const normalized = new URL(`${url.protocol}//${url.host}/`);
  return {
    auth: 'token', channel: 'http', token: options.token, requestTimeoutMs, connectTimeoutMs,
    pingUrl: new URL('/shamir/v1/ping', normalized), requestUrl: new URL('/shamir/v1/request', normalized), url: normalized,
    ...(options.signal ? { signal: options.signal } : {}), ...(options.tls ? { tls: options.tls } : {}),
    ...(options.origin ? { origin: options.origin } : {}),
  };
}

export interface TokenRequestOptions { signal?: AbortSignal; timeoutMs?: number; }
export type TokenHttpRequest =
  | { op: 'ping' }
  | { op: 'execute'; db: string; batch: BatchRequest; query_version: 4 }
  | { op: 'wait_for_visibility'; db: string; repo: string; version: bigint; visibility_wait: VisibilityWait; query_version: 4 }
  | { op: 'get_ddl_op_status'; db: string; repo: string; table: string; op_id: string }
  | { op: 'tx_begin'; db: string; repo: string; query_version: 4; isolation?: 'snapshot' | 'serializable' }
  | { op: 'tx_execute'; db: string; tx_handle: number; batch: BatchRequest; query_version: 4 }
  | { op: 'tx_commit'; db: string; tx_handle: number; query_version: 4; visibility_wait?: VisibilityWait }
  | { op: 'tx_rollback'; db: string; tx_handle: number }
  | { op: 'create_cursor'; db: string; query: object; page_size: number; query_version?: number }
  | { op: 'fetch_next'; db?: string; cursor_id: number | bigint; page_size?: number }
  | { op: 'cancel_cursor'; db?: string; cursor_id: number | bigint };
