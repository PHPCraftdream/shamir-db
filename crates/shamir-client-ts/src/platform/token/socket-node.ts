import WS from 'ws';
import type { ClientOptions } from 'ws';
import type { ClientRequest, IncomingMessage } from 'node:http';
import type { ApiToken } from '../../entry/api-token.js';
import { tokenValueForTransport } from '../../entry/api-token.js';
import type { NodeTokenWsConnectOptions } from '../../core/types/connection.js';
import { TokenTransportError, type TokenTransportErrorCode } from '../../core/protocol/token/errors.js';
import type { TokenSocket, TokenSocketCloseEvent, TokenSocketOptions } from './types.js';

const MAX_QUEUE_MESSAGES = 64;
const MAX_QUEUE_BYTES = 1024 * 1024;
const MAX_FRAME_BYTES = 16 * 1024 * 1024;
const CLOSE_GRACE_MS = 250;
function failure(code: TokenTransportErrorCode, phase: 'connect' | 'close' = 'connect', status?: number): TokenTransportError {
  return new TokenTransportError(code, 'Token WebSocket connection failed', { phase, ...(status === undefined ? {} : { status }) });
}
function validEndpoint(url: URL): boolean {
  return (url.pathname === '/' || url.pathname === '/shamir/v1/ws') && !url.username && !url.password && !url.search && !url.hash;
}
function statusFailure(status: number): TokenTransportError {
  const code: TokenTransportErrorCode = status === 401 ? 'unauthorized' : status === 403 ? 'permission_denied' : status === 429 ? 'rate_limited' : status === 503 ? 'server_unavailable' : 'network_error';
  return failure(code, 'connect', status);
}
function tlsOptions(options: NodeTokenWsConnectOptions, secure: boolean): Partial<ClientOptions> & { ALPNProtocols?: string[] } {
  if (!secure) return {};
  const tls = options.tls;
  return { minVersion: 'TLSv1.3', ALPNProtocols: ['http/1.1'], rejectUnauthorized: tls?.allowInsecureTlsForDevelopment !== true,
    ...(tls?.ca === undefined ? {} : { ca: typeof tls.ca === 'string' ? tls.ca : Buffer.from(tls.ca) }) };
}

export function openNodeTokenSocket(url: URL, token: ApiToken, options: NodeTokenWsConnectOptions, opts: TokenSocketOptions): Promise<TokenSocket> {
  if ((url.protocol !== 'ws:' && url.protocol !== 'wss:') || !validEndpoint(url)) return Promise.reject(failure('invalid_request'));
  if (opts.signal.aborted) return Promise.reject(failure('client_aborted'));
  const nodeOptions = { ...options, ...(opts.tls === undefined ? {} : { tls: opts.tls }), ...(opts.origin === undefined ? {} : { origin: opts.origin }) };
  const target = new URL(url.href);
  if (target.pathname === '/') target.pathname = '/shamir/v1/ws';
  const headers: Record<string, string> = { Authorization: `Bearer ${tokenValueForTransport(token)}` };
  if (nodeOptions.origin) headers.Origin = nodeOptions.origin;
  let socket: WS;
  try { socket = new WS(target, 'shamir-v1', { headers, perMessageDeflate: false, maxPayload: MAX_FRAME_BYTES, ...tlsOptions(nodeOptions, target.protocol === 'wss:') }); }
  catch { return Promise.reject(failure('invalid_request')); }

  return new Promise((resolve, reject) => {
    let settled = false; let opened = false; let closed = false; let queueBytes = 0;
    const queue: Uint8Array[] = [];
    let messageHandler: ((data: Uint8Array) => void) | undefined;
    let closeHandler: ((event: TokenSocketCloseEvent) => void) | undefined;
    let terminalEvent: TokenSocketCloseEvent | undefined;
    let closePromise: Promise<void> | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const cleanupTerminal = (): void => { socket.removeListener('message', onMessage); socket.removeListener('close', onClose); socket.removeListener('error', onSocketError); socket.removeListener('error', onSetupError); socket.removeListener('close', onSetupClose); socket.removeListener('unexpected-response', onUnexpectedResponse); queue.length = 0; queueBytes = 0; messageHandler = undefined; closeHandler = undefined; socket.removeListener('error', ignoreSocketError); socket.on('error', ignoreSocketError); };
    const removeTerminalListeners = (): void => { socket.removeListener('message', onMessage); socket.removeListener('close', onClose); socket.removeListener('error', onSocketError); socket.removeListener('error', ignoreSocketError); socket.on('error', ignoreSocketError); };
    const ignoreSocketError = (): void => {};
    const fail = (error: Error): void => { if (settled) return; settled = true; cleanupSetup(); reject(error); };
    const cleanupSetup = (): void => { if (timer !== undefined) clearTimeout(timer); opts.signal.removeEventListener('abort', onAbort); if (!opened) { socket.removeListener('message', onMessage); socket.removeListener('close', onClose); socket.removeListener('error', onSocketError); } socket.removeListener('open', onOpen); socket.removeListener('close', onSetupClose); socket.removeListener('unexpected-response', onUnexpectedResponse); };
    const notifyClose = (event: TokenSocketCloseEvent): void => {
      if (closed) return; closed = true; queue.length = 0; queueBytes = 0;
      if (closeHandler) { closeHandler(event); cleanupTerminal(); } else terminalEvent = event;
    };
    const onSetupError = (error: NodeJS.ErrnoException): void => {
      if (!opened) fail(failure(error.code?.startsWith('ERR_TLS_') || error.code?.startsWith('CERT_') || error.code === 'UNABLE_TO_VERIFY_LEAF_SIGNATURE' ? 'tls_configuration_error' : 'network_error'));
      else socket.removeListener('error', onSetupError);
    };
    const onSetupClose = (): void => { if (!opened) fail(failure('network_error')); };
    const onUnexpectedResponse = (_request: ClientRequest, response: IncomingMessage): void => {
      const status = response.statusCode ?? 0;
      response.on('error', () => {}); response.destroy(); fail(statusFailure(status));
    };
    const onAbort = (): void => { fail(failure('client_aborted')); socket.terminate(); };
    const adapter: TokenSocket = {
      send(data) { if (closed || socket.readyState !== WS.OPEN) throw failure('connection_closed', 'close'); if (data.byteLength > MAX_FRAME_BYTES) throw failure('protocol_error', 'close'); socket.send(data, { binary: true }); },
      onMessage(handler) { messageHandler = handler; while (queue.length > 0) { const data = queue.shift(); if (!data) continue; queueBytes -= data.byteLength; messageHandler(data); } },
      onClose(handler) { closeHandler = handler; if (terminalEvent) { const event = terminalEvent; terminalEvent = undefined; handler(event); cleanupTerminal(); } },
      close(code = 1000, reason = '') {
        if (closePromise) return closePromise;
        if (closed || socket.readyState === WS.CLOSED) { notifyClose({ code, reason: '', wasClean: code === 1000 }); return Promise.resolve(); }
        closePromise = new Promise<void>((done) => {
          let complete = false;
          const finish = (event: TokenSocketCloseEvent): void => {
            if (complete) return; complete = true; clearTimeout(grace); socket.removeListener('close', onGraceClose);
            notifyClose(event); if (closeHandler) cleanupTerminal(); else removeTerminalListeners(); if (socket.readyState !== WS.CLOSED) socket.terminate(); done();
          };
          const onGraceClose = (receivedCode: number): void => finish({ code: receivedCode, reason: '', wasClean: receivedCode === 1000 });
          const grace = setTimeout(() => finish({ code, reason: '', wasClean: false }), CLOSE_GRACE_MS);
          socket.once('close', onGraceClose);
          try { socket.close(code, reason); } catch { finish({ code, reason: '', wasClean: false }); }
        });
        return closePromise;
      },
    };
    const onMessage = (data: Buffer | ArrayBuffer | Buffer[], binary: boolean): void => {
      if (!binary) { void adapter.close(1003); return; }
      const bytes = data instanceof Buffer ? new Uint8Array(data) : data instanceof ArrayBuffer ? new Uint8Array(data) : new Uint8Array(Buffer.concat(data));
      if (bytes.byteLength > MAX_FRAME_BYTES) { void adapter.close(1009); return; }
      if (messageHandler) { messageHandler(bytes); return; }
      if (queue.length >= MAX_QUEUE_MESSAGES || queueBytes + bytes.byteLength > MAX_QUEUE_BYTES) { void adapter.close(1009); return; }
      queue.push(bytes); queueBytes += bytes.byteLength;
    };
    const onClose = (code: number): void => { socket.removeListener('message', onMessage); notifyClose({ code, reason: '', wasClean: code === 1000 }); };
    const onSocketError = (): void => { notifyClose({ code: 1006, reason: '', wasClean: false, error: failure('network_error', 'close') }); if (closeHandler) cleanupTerminal(); else removeTerminalListeners(); if (socket.readyState !== WS.CLOSED) socket.terminate(); };
    const onOpen = (): void => {
      if (socket.protocol !== 'shamir-v1') { fail(failure('protocol_error')); socket.terminate(); return; }
      if (settled) return; settled = true; opened = true; socket.on('message', onMessage); socket.on('close', onClose); cleanupSetup(); resolve(adapter);
    };
    socket.on('error', onSocketError);
    socket.on('error', onSetupError); socket.on('close', onSetupClose); socket.on('unexpected-response', onUnexpectedResponse); socket.on('open', onOpen);
    opts.signal.addEventListener('abort', onAbort, { once: true });
    if (opts.connectTimeoutMs > 0) timer = setTimeout(() => { fail(failure('client_timeout')); socket.terminate(); }, opts.connectTimeoutMs);
    if (opts.signal.aborted) onAbort();
  });
}
