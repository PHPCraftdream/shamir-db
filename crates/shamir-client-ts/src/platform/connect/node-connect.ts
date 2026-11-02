import { ApiToken, tokenValueForTransport } from '../../entry/api-token.js';
import type { NodeConnectOptions, NodeTokenHttpConnectOptions, NodeTokenWsConnectOptions } from '../../core/types/connection.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';
import { createTokenHttpClient } from '../../entry/token-client.js';
import { createTokenWsClient } from '../../entry/ws/token-ws-client.js';
import { createNodeTokenPlatform } from '../node.js';

export type PublicTokenClient = Awaited<ReturnType<typeof createTokenWsClient>> | ReturnType<typeof createTokenHttpClient>;

const URL_PATTERN = /^(?:https?|wss?):\/\/[^/?#\\\\\s]+\/?$/i;
function parseUrl(raw: string, schemes: readonly string[]): URL {
  if (typeof raw !== 'string' || raw.includes('@') || raw.trim() !== raw || !URL_PATTERN.test(raw)) throw new TokenTransportError('invalid_request', 'Invalid token connection URL');
  let url: URL;
  try { url = new URL(raw); } catch { throw new TokenTransportError('invalid_request', 'Invalid token connection URL'); }
  if (!schemes.includes(url.protocol) || !url.hostname || url.username || url.password || url.search || url.hash || (url.pathname !== '/' && url.pathname !== '')) throw new TokenTransportError('invalid_request', 'Invalid token connection URL');
  return url;
}

export async function connectTokenNode(options: NodeConnectOptions): Promise<PublicTokenClient> {
  if (!options || typeof options !== 'object' || options.auth !== 'token' || !('token' in options) || !('url' in options) || 'username' in options || 'password' in options || 'host' in options || 'port' in options || 'allow_self_signed' in options || !['http', 'ws', undefined].includes(options.channel)) throw new TokenTransportError('invalid_request', 'Invalid token connection options');
  let token: ApiToken;
  try { token = options.token instanceof ApiToken ? options.token : ApiToken.from(options.token); tokenValueForTransport(token); }
  catch { throw new TokenTransportError('invalid_request', 'Invalid API token'); }
  if ('signal' in options && options.signal !== undefined && (!options.signal || typeof options.signal !== 'object' || typeof options.signal.aborted !== 'boolean' || typeof options.signal.addEventListener !== 'function' || typeof options.signal.removeEventListener !== 'function')) throw new TokenTransportError('invalid_request', 'Invalid token connection options');
  if (options.signal?.aborted) throw new TokenTransportError('client_aborted', 'Token connection aborted', { phase: 'connect' });
  if ('tls' in options && options.tls !== undefined && (!options.tls || typeof options.tls !== 'object' || Array.isArray(options.tls) || Object.keys(options.tls).some((key) => key !== 'ca' && key !== 'allowInsecureTlsForDevelopment') || options.tls.allowInsecureTlsForDevelopment !== undefined && options.tls.allowInsecureTlsForDevelopment !== true || options.tls.ca !== undefined && typeof options.tls.ca !== 'string' && !(options.tls.ca instanceof Uint8Array))) throw new TokenTransportError('invalid_request', 'Invalid TLS configuration');
  if ('origin' in options && options.origin !== undefined && (typeof options.origin !== 'string' || /[\r\n]/.test(options.origin))) throw new TokenTransportError('invalid_request', 'Invalid token connection options');
  const httpScheme = /^https?:\/\//i.test(options.url);
  const url = parseUrl(options.url, httpScheme ? ['http:', 'https:'] : ['ws:', 'wss:']);
  const derived = httpScheme ? 'http' : 'ws';
  const channel = options.channel ?? derived;
  if (channel !== derived) throw new TokenTransportError('invalid_request', 'Token channel does not match URL scheme');

  if (options.requestTimeoutMs !== undefined && (!Number.isSafeInteger(options.requestTimeoutMs) || options.requestTimeoutMs < 0 || options.requestTimeoutMs > 0x7fffffff) || options.connectTimeoutMs !== undefined && (!Number.isSafeInteger(options.connectTimeoutMs) || options.connectTimeoutMs < 0 || options.connectTimeoutMs > 0x7fffffff)) throw new TokenTransportError('invalid_request', 'Invalid connection timeout');
  if ('tls' in options && options.tls !== undefined && url.protocol !== 'https:' && url.protocol !== 'wss:') throw new TokenTransportError('invalid_request', 'TLS options require a secure URL');
  if (url.protocol === 'http:' || url.protocol === 'ws:') {
    if (!isLoopback(url.hostname)) throw new TokenTransportError('invalid_request', 'Plaintext token connections require loopback');
  }
  if (channel === 'http') {
    const normalized: NodeTokenHttpConnectOptions = {
      auth: 'token', channel: 'http', token, url: url.origin,
      ...(options.requestTimeoutMs === undefined ? {} : { requestTimeoutMs: options.requestTimeoutMs }),
      ...(options.connectTimeoutMs === undefined ? {} : { connectTimeoutMs: options.connectTimeoutMs }),
      ...(options.signal === undefined ? {} : { signal: options.signal }),
      ...('tls' in options && options.tls ? { tls: options.tls } : {}),
      ...('origin' in options && options.origin ? { origin: options.origin } : {}),
    };
    return Promise.resolve(createTokenHttpClient(normalized, (platformOptions) => createNodeTokenPlatform(platformOptions)));
  }
  const normalized: NodeTokenWsConnectOptions = {
    auth: 'token', channel: 'ws', token, url: url.origin.replace(/^http/i, 'ws'),
    ...(options.requestTimeoutMs === undefined ? {} : { requestTimeoutMs: options.requestTimeoutMs }),
    ...(options.connectTimeoutMs === undefined ? {} : { connectTimeoutMs: options.connectTimeoutMs }),
    ...(options.signal === undefined ? {} : { signal: options.signal }),
    ...('tls' in options && options.tls ? { tls: options.tls } : {}),
    ...('origin' in options && options.origin ? { origin: options.origin } : {}),
  };
  return createTokenWsClient(normalized, (platformOptions) => createNodeTokenPlatform(platformOptions));
}
function isLoopback(host: string): boolean {
  if (host === 'localhost' || host === '[::1]' || host === '::1') return true;
  const parts = host.split('.');
  return parts.length === 4 && parts[0] === '127' && parts.slice(1).every((part) => /^(?:0|[1-9][0-9]{0,2})$/.test(part) && Number(part) <= 255);
}
