import type { ApiToken } from '../../entry/api-token.js';
import { tokenValueForTransport } from '../../entry/api-token.js';
import { encodeBrowserTokenFrame } from '../../core/protocol/token/ws.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';
import type { TokenSocket, TokenSocketOptions, TokenSocketCloseEvent } from './types.js';

const MAX_QUEUE_MESSAGES = 64;
const MAX_QUEUE_BYTES = 1024 * 1024;
const MAX_FRAME_BYTES = 16 * 1024 * 1024;
const CLOSE_GRACE_MS = 250;
type BrowserMessageEvent = { readonly data: unknown };
type BrowserCloseEvent = { readonly code: number; readonly reason: string; readonly wasClean: boolean };
type BrowserSocketEvent = BrowserMessageEvent | BrowserCloseEvent | { readonly type: string };
export type BrowserListener = (event: BrowserSocketEvent) => void;
interface BrowserWebSocketLike {
  readonly readyState: number; readonly protocol: string; binaryType: string;
  send(data: Uint8Array): void; close(code?: number, reason?: string): void;
  addEventListener(type: string, callback: BrowserListener, options?: { once?: boolean }): void;
  removeEventListener(type: string, callback: BrowserListener): void;
}
interface NativeBrowserSocket {
  readonly readyState: number; readonly protocol: string; binaryType: string;
  send(data: Uint8Array): void; close(code?: number, reason?: string): void;
  addEventListener(type: string, callback: (event: unknown) => void, options?: { once?: boolean }): void;
  removeEventListener(type: string, callback: (event: unknown) => void): void;
}
type NativeBrowserSocketConstructor = new (url: string, protocols: string[]) => NativeBrowserSocket;
type BrowserWebSocketFactory = (url: string, protocols: string[]) => BrowserWebSocketLike;
function nativeSocket(url: string, protocols: string[]): BrowserWebSocketLike {
  const constructor = (globalThis as unknown as { WebSocket?: NativeBrowserSocketConstructor }).WebSocket;
  if (!constructor) throw new Error('WebSocket unavailable');
  const native = new constructor(url, protocols);
  const wrappers = new Map<BrowserListener, (event: unknown) => void>();
  return {
    get readyState() { return native.readyState; }, get protocol() { return native.protocol; },
    get binaryType() { return native.binaryType; }, set binaryType(value: string) { native.binaryType = value; },
    send(data) { native.send(data); }, close(code, reason) { native.close(code, reason); },
    addEventListener(type, callback, options) {
      let wrapper = wrappers.get(callback);
      if (!wrapper) { wrapper = (event) => callback(event as BrowserSocketEvent); wrappers.set(callback, wrapper); }
      native.addEventListener(type, wrapper, options);
    },
    removeEventListener(type, callback) {
      const wrapper = wrappers.get(callback);
      if (wrapper) { native.removeEventListener(type, wrapper); wrappers.delete(callback); }
    },
  };
}
function validEndpoint(url: URL): boolean {
  return (url.pathname === '/' || url.pathname === '/shamir/v1/ws/browser') && !url.username && !url.password && !url.search && !url.hash;
}
function loopback(hostname: string): boolean {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, '');
  return host === 'localhost' || host.endsWith('.localhost') || host === '::1' || /^127(?:\.\d{1,3}){3}$/.test(host);
}
function failure(code: 'invalid_request' | 'client_aborted' | 'client_timeout' | 'protocol_error' | 'network_error' | 'connection_closed'): TokenTransportError {
  return new TokenTransportError(code, 'Token WebSocket connection failed', { phase: code === 'connection_closed' ? 'close' : 'connect' });
}
function asMessage(event: BrowserSocketEvent): event is BrowserMessageEvent { return 'data' in event; }
function asClose(event: BrowserSocketEvent): event is BrowserCloseEvent { return 'code' in event && 'wasClean' in event; }
export function isBrowserLoopback(hostname: string): boolean { return loopback(hostname); }

export function openBrowserTokenSocket(url: URL, token: ApiToken, opts: TokenSocketOptions, factory: BrowserWebSocketFactory = nativeSocket): Promise<TokenSocket> {
  if ((url.protocol !== 'ws:' && url.protocol !== 'wss:') || !validEndpoint(url) || (url.protocol === 'ws:' && !loopback(url.hostname))) return Promise.reject(failure('invalid_request'));
  if (opts.signal.aborted) return Promise.reject(failure('client_aborted'));
  const target = new URL(url.href); if (target.pathname === '/') target.pathname = '/shamir/v1/ws/browser';
  let socket: BrowserWebSocketLike;
  try { socket = factory(target.href, ['shamir-v1']); socket.binaryType = 'arraybuffer'; }
  catch (error) { return Promise.reject(error instanceof TokenTransportError ? error : failure('network_error')); }

  return new Promise((resolve, reject) => {
    let settled = false; let closed = false; let queueBytes = 0; let terminalEvent: TokenSocketCloseEvent | undefined; let closePromise: Promise<void> | undefined;
    const queue: Uint8Array[] = [];
    let messageHandler: ((data: Uint8Array) => void) | undefined; let closeHandler: ((event: TokenSocketCloseEvent) => void) | undefined;
    const cleanupTerminal = (): void => { socket.removeEventListener('message', onMessage); socket.removeEventListener('close', onClose); socket.removeEventListener('error', onSocketError); queue.length = 0; queueBytes = 0; messageHandler = undefined; closeHandler = undefined; };
    const removeTerminalListeners = (): void => { socket.removeEventListener('message', onMessage); socket.removeEventListener('close', onClose); socket.removeEventListener('error', onSocketError); };
    let timer: ReturnType<typeof setTimeout> | undefined;
    const cleanupSetup = (): void => { if (timer !== undefined) clearTimeout(timer); opts.signal.removeEventListener('abort', onAbort); socket.removeEventListener('open', onOpen); socket.removeEventListener('error', onError); socket.removeEventListener('close', onSetupClose); };
    const fail = (error: Error): void => { if (settled) return; settled = true; cleanupSetup(); reject(error); };
    const notifyClose = (event: TokenSocketCloseEvent): void => {
      if (closed) return; closed = true; queue.length = 0; queueBytes = 0;
      if (closeHandler) { closeHandler(event); cleanupTerminal(); } else terminalEvent = event;
    };
    const onError = (): void => { fail(failure('network_error')); try { socket.close(); } catch { /* best effort */ } };
    const onSetupClose = (): void => fail(new TokenTransportError('authentication_failed', 'Token WebSocket authentication failed', { phase: 'connect' }));
    const onAbort = (): void => { fail(failure('client_aborted')); try { socket.close(); } catch { /* best effort */ } };
    const onMessage = (event: BrowserSocketEvent): void => {
      if (!asMessage(event)) return;
      const data = event.data; const bytes = data instanceof ArrayBuffer ? new Uint8Array(data) : data instanceof Uint8Array ? data : undefined;
      if (!bytes) { void adapter.close(1003); return; }
      if (bytes.byteLength > MAX_FRAME_BYTES) { void adapter.close(1009); return; }
      if (messageHandler) { messageHandler(bytes); return; }
      if (queue.length >= MAX_QUEUE_MESSAGES || queueBytes + bytes.byteLength > MAX_QUEUE_BYTES) { void adapter.close(1009); return; }
      queue.push(bytes); queueBytes += bytes.byteLength;
    };
    const onClose = (event: BrowserSocketEvent): void => {
      if (!asClose(event)) return;
      notifyClose({ code: event.code, reason: '', wasClean: event.wasClean });
    };
    const onSocketError = (): void => { notifyClose({ code: 1006, reason: '', wasClean: false, error: failure('network_error') }); if (socket.readyState !== 3) { try { socket.close(); } catch { /* best effort */ } } };
    const adapter: TokenSocket = {
      send(data) { if (closed || socket.readyState !== 1) throw failure('connection_closed'); if (data.byteLength > MAX_FRAME_BYTES) throw failure('protocol_error'); socket.send(data); },
      onMessage(handler) { messageHandler = handler; while (queue.length > 0) { const item = queue.shift(); if (!item) continue; queueBytes -= item.byteLength; messageHandler(item); } },
      onClose(handler) { closeHandler = handler; if (terminalEvent) { const event = terminalEvent; terminalEvent = undefined; handler(event); cleanupTerminal(); } },
      close(code = 1000, reason = '') {
        if (closePromise) return closePromise;
        if (closed || socket.readyState === 3) { notifyClose({ code, reason: '', wasClean: code === 1000 }); return Promise.resolve(); }
        closePromise = new Promise<void>((done) => {
          let complete = false;
          const finish = (event: TokenSocketCloseEvent): void => { if (complete) return; complete = true; clearTimeout(grace); socket.removeEventListener('close', onGraceClose); notifyClose(event); if (closeHandler) cleanupTerminal(); else removeTerminalListeners(); done(); };
          const onGraceClose = (event: BrowserSocketEvent): void => { if (asClose(event)) finish({ code: event.code, reason: '', wasClean: event.wasClean }); };
          const grace = setTimeout(() => finish({ code, reason: '', wasClean: false }), CLOSE_GRACE_MS);
          socket.addEventListener('close', onGraceClose, { once: true });
          try { socket.close(code, reason); } catch { finish({ code, reason: '', wasClean: false }); }
        });
        return closePromise;
      },
    };
    const onOpen = (): void => {
      if (settled) return;
      if (socket.protocol !== 'shamir-v1') { fail(failure('protocol_error')); try { socket.close(); } catch { /* best effort */ } return; }
      socket.addEventListener('message', onMessage); socket.addEventListener('close', onClose); socket.addEventListener('error', onSocketError);
      try { socket.send(encodeBrowserTokenFrame(tokenValueForTransport(token))); }
      catch { fail(failure('protocol_error')); socket.removeEventListener('message', onMessage); socket.removeEventListener('close', onClose); socket.removeEventListener('error', onSocketError); try { socket.close(); } catch { /* best effort */ } return; }
      settled = true; cleanupSetup(); resolve(adapter);
    };
    socket.addEventListener('open', onOpen, { once: true }); socket.addEventListener('error', onError, { once: true }); socket.addEventListener('close', onSetupClose, { once: true });
    opts.signal.addEventListener('abort', onAbort, { once: true });
    if (opts.connectTimeoutMs > 0) timer = setTimeout(() => { fail(failure('client_timeout')); try { socket.close(); } catch { /* best effort */ } }, opts.connectTimeoutMs);
    if (opts.signal.aborted) onAbort();
  });
}
