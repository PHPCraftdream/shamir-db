import { ApiToken, tokenValueForTransport } from '../../entry/api-token.js';
import type { BrowserTokenConnectOptions, BrowserTokenHttpConnectOptions, BrowserTokenWsConnectOptions } from '../../core/types/connection.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';
import { createTokenHttpClient } from '../../entry/token-client.js';
import { createTokenWsClient } from '../../entry/ws/token-ws-client.js';
import { createBrowserTokenPlatform } from '../browser.js';

import type { PublicTokenClient } from '../../entry/ws/token-client.js';

export async function connectTokenBrowser(options: BrowserTokenConnectOptions): Promise<PublicTokenClient> {
  if (!options || typeof options !== 'object' || options.auth !== 'token' || !('token' in options) || !('url' in options) || 'username' in options || 'password' in options || 'host' in options || 'port' in options || 'allow_self_signed' in options || !['http', 'ws', undefined].includes(options.channel)) throw new TokenTransportError('invalid_request', 'Invalid token connection options');
  if ('tls' in options && options.tls !== undefined || 'origin' in options && options.origin !== undefined || 'allow_self_signed' in options) throw new TokenTransportError('invalid_request', 'Node-only token options are unavailable in browsers');
  if ('signal' in options && options.signal !== undefined && (!options.signal || typeof options.signal !== 'object' || typeof options.signal.aborted !== 'boolean' || typeof options.signal.addEventListener !== 'function' || typeof options.signal.removeEventListener !== 'function')) throw new TokenTransportError('invalid_request', 'Invalid token connection options');
  if (options.signal?.aborted) throw new TokenTransportError('client_aborted', 'Token connection aborted', { phase: 'connect' });
  let token: ApiToken;
  try { token = options.token instanceof ApiToken ? options.token : ApiToken.from(options.token); tokenValueForTransport(token); }
  catch { throw new TokenTransportError('invalid_request', 'Invalid API token'); }
  const raw = options.url;
  if (typeof raw !== 'string' || raw.includes('@') || raw.trim() !== raw || !/^(?:https?|wss?):\/\/[^/?#\\\\\s]+\/?$/i.test(raw)) throw new TokenTransportError('invalid_request', 'Invalid token connection URL');
  let url: URL;
  try { url = new URL(raw); } catch { throw new TokenTransportError('invalid_request', 'Invalid token connection URL'); }
  const scheme = url.protocol;
  if (!['http:', 'https:', 'ws:', 'wss:'].includes(scheme) || url.username || url.password || url.search || url.hash || (url.pathname !== '/' && url.pathname !== '')) throw new TokenTransportError('invalid_request', 'Invalid token connection URL');
  const derived = scheme === 'http:' || scheme === 'https:' ? 'http' : 'ws';
  const channel = options.channel ?? derived;
  if (channel !== derived) throw new TokenTransportError('invalid_request', 'Token channel does not match URL scheme');
  if (options.requestTimeoutMs !== undefined && (!Number.isSafeInteger(options.requestTimeoutMs) || options.requestTimeoutMs < 0 || options.requestTimeoutMs > 0x7fffffff) || options.connectTimeoutMs !== undefined && (!Number.isSafeInteger(options.connectTimeoutMs) || options.connectTimeoutMs < 0 || options.connectTimeoutMs > 0x7fffffff)) throw new TokenTransportError('invalid_request', 'Invalid connection timeout');
  if (scheme === 'ws:' && !isLoopback(url.hostname)) throw new TokenTransportError('invalid_request', 'Browser plaintext WebSocket connections require loopback');
  if (channel === 'http') {
    const normalized: BrowserTokenHttpConnectOptions = {
      auth: 'token', channel: 'http', token, url: url.origin,
      ...(options.requestTimeoutMs === undefined ? {} : { requestTimeoutMs: options.requestTimeoutMs }),
      ...(options.connectTimeoutMs === undefined ? {} : { connectTimeoutMs: options.connectTimeoutMs }),
      ...(options.signal === undefined ? {} : { signal: options.signal }),
    };
    return Promise.resolve(createTokenHttpClient(normalized, (platformOptions) => createBrowserTokenPlatform(platformOptions)));
  }
  const normalized: BrowserTokenWsConnectOptions = {
    auth: 'token', channel: 'ws', token, url: url.origin.replace(/^http/i, 'ws'),
    ...(options.requestTimeoutMs === undefined ? {} : { requestTimeoutMs: options.requestTimeoutMs }),
    ...(options.connectTimeoutMs === undefined ? {} : { connectTimeoutMs: options.connectTimeoutMs }),
    ...(options.signal === undefined ? {} : { signal: options.signal }),
  };
  return createTokenWsClient(normalized, (platformOptions) => createBrowserTokenPlatform(platformOptions));
}
function isLoopback(host: string): boolean {
  if (host === 'localhost' || host === '[::1]' || host === '::1') return true;
  const parts = host.split('.');
  return parts.length === 4 && parts[0] === '127' && parts.slice(1).every((part) => /^(?:0|[1-9][0-9]{0,2})$/.test(part) && Number(part) <= 255);
}
