import * as http from 'node:http';
import * as https from 'node:https';
import type { ClientRequest, IncomingMessage, RequestOptions } from 'node:http';
import { tokenValueForTransport } from '../../entry/api-token.js';
import type { NodeTokenHttpConnectOptions, NodeTokenWsConnectOptions } from '../../core/types/connection.js';
import type { TokenHttpRequestInit, TokenHttpResponse } from './types.js';
import { openNodeTokenSocket } from './socket-node.js';
import { DEFAULT_TOKEN_CONNECT_TIMEOUT_MS, DEFAULT_TOKEN_REQUEST_TIMEOUT_MS, normalizeTokenHttpOptions } from '../../core/protocol/token/options.js';
import { TokenTransportError, type TokenTransportErrorCode } from '../../core/protocol/token/errors.js';

const MAX_RESPONSE_BYTES = 16 * 1024 * 1024;
function failure(code: TokenTransportErrorCode, phase: 'connect' | 'request' | 'close' = 'request'): TokenTransportError {
  return new TokenTransportError(code, code, { phase });
}
function safeHeaders(headers: http.IncomingHttpHeaders): Readonly<{ get(name: string): string | null }> {
  return Object.freeze({ get(name: string): string | null {
    const value = headers[name.toLowerCase()];
    return typeof value === 'string' ? value : Array.isArray(value) ? value.join(', ') : null;
  } });
}

export function createNodeTokenPlatform(options: NodeTokenHttpConnectOptions | NodeTokenWsConnectOptions): import('./types.js').TokenPlatform {
  const normalized = normalizeTokenHttpOptions({ ...options, channel: 'http', url: options.url.replace(/^ws:/i, 'http:').replace(/^wss:/i, 'https:') });
  const requestTimeout = normalized.requestTimeoutMs ?? DEFAULT_TOKEN_REQUEST_TIMEOUT_MS;
  const connectTimeout = normalized.connectTimeoutMs ?? DEFAULT_TOKEN_CONNECT_TIMEOUT_MS;
  const insecure = normalized.tls?.allowInsecureTlsForDevelopment === true;
  const commonTls: https.AgentOptions = {
    keepAlive: true, minVersion: 'TLSv1.3', ALPNProtocols: ['http/1.1'],
    ...(normalized.tls?.ca ? { ca: typeof normalized.tls.ca === 'string' ? normalized.tls.ca : Buffer.from(normalized.tls.ca) } : {}),
    rejectUnauthorized: !insecure,
  };
  const httpAgent = new http.Agent({ keepAlive: true });
  const httpsAgent = new https.Agent(commonTls);
  let closed = false;
  const pending = new Set<ClientRequest>();
  const activeFailures = new Map<ClientRequest, (error: TokenTransportError) => void>();
  const sockets = new Set<Promise<TokenSocket>>();
  const onLifetimeAbort = (): void => { closed = true; for (const opening of sockets) void opening.then((socket) => socket.close(), () => {}); };
  options.signal?.addEventListener('abort', onLifetimeAbort, { once: true });



  return {
    openTokenSocket(url, token, opts) {
      if (closed || normalized.signal?.aborted) return Promise.reject(failure('connection_closed', 'close'));
      const nodeOptions = { ...(options as NodeTokenWsConnectOptions), ...(opts.tls === undefined ? {} : { tls: opts.tls }), ...(opts.origin === undefined ? {} : { origin: opts.origin }) };
      const opening = openNodeTokenSocket(url, token, nodeOptions, opts);
      sockets.add(opening);
      void opening.then((socket) => { if (closed) void socket.close(); }, () => { sockets.delete(opening); });
      return opening;
    },
    async requestHttp(url: URL, token, init: TokenHttpRequestInit): Promise<TokenHttpResponse> {
      if (closed) throw failure('connection_closed', 'close');
      if (init.signal.aborted || normalized.signal?.aborted) throw failure('client_aborted');
      if (url.origin !== normalized.url.origin || (url.href !== normalized.pingUrl.href && url.href !== normalized.requestUrl.href)) throw failure('protocol_error');
      if (url.protocol !== 'http:' && url.protocol !== 'https:') throw failure('protocol_error');
      const secure = url.protocol === 'https:';
      const requestOptions: RequestOptions = {
        protocol: url.protocol, hostname: url.hostname.replace(/^\[|\]$/g, ''), port: url.port || undefined,
        path: `${url.pathname}${url.search}`, method: init.method,
        headers: { ...init.headers, ...(normalized.origin ? { Origin: normalized.origin } : {}), Authorization: `Bearer ${tokenValueForTransport(token)}` },
        agent: secure ? httpsAgent : httpAgent,
        ...(secure ? { ALPNProtocols: ['http/1.1'] } : {}),
      };
      return new Promise((resolve, reject) => {
        let settled = false;
        let connectTimer: ReturnType<typeof setTimeout> | undefined;
        let requestTimer: ReturnType<typeof setTimeout> | undefined;
        let request: ClientRequest | undefined;
        const socketListeners: Array<() => void> = [];
        const cleanup = (): void => {
          if (connectTimer !== undefined) clearTimeout(connectTimer);
          if (requestTimer !== undefined) clearTimeout(requestTimer);
          init.signal.removeEventListener('abort', onAbort);
          normalized.signal?.removeEventListener('abort', onAbort);
          for (const remove of socketListeners) remove();
          if (request) { pending.delete(request); activeFailures.delete(request); }
        };
        const finishError = (error: TokenTransportError): void => {
          if (settled) return;
          settled = true; cleanup();
          request?.destroy();
          reject(error);
        };
        const finishResponse = (response: IncomingMessage, body: Buffer): void => {
          if (settled) return;
          settled = true; cleanup();
          resolve({ status: response.statusCode ?? 0, headers: safeHeaders(response.headers), body });
        };
        const onAbort = (): void => finishError(failure('client_aborted'));
        const chunks: Buffer[] = [];
        let total = 0;
        try {
          request = (secure ? https.request : http.request)(requestOptions, (response) => {
            if (connectTimer !== undefined) { clearTimeout(connectTimer); connectTimer = undefined; }
            const status = response.statusCode ?? 0;
            if (status === 401) {
              response.on('error', () => {});
              finishResponse(response, Buffer.alloc(0));
              response.destroy();
              request?.destroy();
              return;
            }
            if (status >= 300 && status < 400) { response.on('error', () => {}); response.destroy(); finishError(failure('protocol_error')); return; }
            response.on('data', (chunk: Buffer | Uint8Array) => {
              if (settled) return;
              const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
              total += bytes.byteLength;
              if (total > MAX_RESPONSE_BYTES) { finishError(failure('http_body_too_large')); return; }
              chunks.push(bytes);
            });
            response.on('aborted', () => finishError(failure('network_error')));
            response.on('error', () => finishError(failure('network_error')));
            response.on('close', () => { if (!settled && !response.complete) finishError(failure('network_error')); });
            response.on('end', () => finishResponse(response, Buffer.concat(chunks, total)));
          });
        } catch { finishError(failure('network_error', 'connect')); return; }
        pending.add(request);
        activeFailures.set(request, finishError);
        init.signal.addEventListener('abort', onAbort, { once: true });
        normalized.signal?.addEventListener('abort', onAbort, { once: true });
        if (connectTimeout > 0) connectTimer = setTimeout(() => finishError(failure('client_timeout', 'connect')), connectTimeout);
        if (requestTimeout > 0) requestTimer = setTimeout(() => finishError(failure('client_timeout')), requestTimeout);
        request.on('socket', (socket) => {
          const onConnect = (): void => { if (connectTimer !== undefined) { clearTimeout(connectTimer); connectTimer = undefined; } };
          if (secure) {
            const tlsSocket = socket as import('node:tls').TLSSocket;
            if (request?.reusedSocket) onConnect();
            else { tlsSocket.once('secureConnect', onConnect); socketListeners.push(() => tlsSocket.removeListener('secureConnect', onConnect)); }
          } else {
            const netSocket = socket as import('node:net').Socket;
            if (netSocket.connecting === false || request?.reusedSocket) onConnect();
            else { netSocket.once('connect', onConnect); socketListeners.push(() => netSocket.removeListener('connect', onConnect)); }
          }
        });
        request.on('error', (error: NodeJS.ErrnoException) => {
          if (settled) return;
          const tlsFailure = secure && (error.code?.startsWith('CERT_') || error.code?.startsWith('ERR_TLS_') || error.code === 'DEPTH_ZERO_SELF_SIGNED_CERT' || error.code === 'UNABLE_TO_VERIFY_LEAF_SIGNATURE');
          finishError(failure(tlsFailure ? 'tls_configuration_error' : 'network_error'));
        });
        request.on('response', () => { if (connectTimer !== undefined) { clearTimeout(connectTimer); connectTimer = undefined; } });
        try { if (init.body) request.write(init.body); request.end(); }
        catch { finishError(failure('network_error')); }
      });
    },
    async close(): Promise<void> {
      if (closed) return;
      closed = true;
      options.signal?.removeEventListener('abort', onLifetimeAbort);
      for (const request of pending) activeFailures.get(request)?.(failure('connection_closed', 'close'));
      pending.clear(); activeFailures.clear();
      const opened = await Promise.allSettled(sockets);
      for (const result of opened) if (result.status === 'fulfilled') await result.value.close();
      sockets.clear();
      httpAgent.destroy(); httpsAgent.destroy();
    },
  };
}
