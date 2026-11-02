import type { TokenWsConnectOptions } from '../../core/types/connection.js';
import type { TokenSocket } from '../../platform/token/types.js';
import type { TokenHttpRequest, TokenRequestOptions } from '../../core/protocol/token/options.js';
import { TokenTransportError, isTokenTransportErrorCode } from '../../core/protocol/token/errors.js';
import { ShamirDbError } from '../../core/support/errors.js';
import { decodeTokenBinding, decodeTokenFrame, decodeTokenPayload, decodeTokenPush, encodeTokenRequest } from '../../core/protocol/token/ws.js';
export type TokenWsSocketFactory = (url: URL, token: TokenWsConnectOptions['token'], signal: AbortSignal, timeoutMs: number) => Promise<TokenSocket>;
export type TokenWsRequest = TokenHttpRequest | { op: 'create_cursor'; db: string; query: object; page_size: number; query_version?: number } | { op: 'fetch_next'; cursor_id: number | bigint; page_size?: number } | { op: 'cancel_cursor'; cursor_id: number | bigint };
type Pending = { resolve(value: Record<string, unknown>): void; reject(error: Error): void; timer?: ReturnType<typeof setTimeout>; abort?: () => void };
type CursorResource = { close(): Promise<void>; terminate?(): void; onTerminal?(callback: () => void): void };
type SubscriptionResource = { terminate(): void };
const MAX_SUBSCRIPTIONS = 128;
function record(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) && !(value instanceof Uint8Array); }
export class TokenWsLifecycle {
  readonly expiresAtNs: bigint;
  private readonly pending = new Map<number, Pending>();
  private readonly cursors = new Set<CursorResource>();
  private readonly subscriptions = new Map<number, SubscriptionResource>();
  private readonly earlyPushes = new Map<number, Array<NonNullable<ReturnType<typeof decodeTokenPush>>>>();
  private bufferedPushCount = 0;
  private readonly terminalController = new AbortController();
  private readonly terminalCallbacks = new Set<(error: Error) => void>();
  get terminalSignal(): AbortSignal { return this.terminalController.signal; }
  onTerminal(callback: (error: Error) => void): () => void { if (this.terminated) { callback(new TokenTransportError('connection_closed', 'Token WebSocket is closed')); return () => undefined; } this.terminalCallbacks.add(callback); return () => this.terminalCallbacks.delete(callback); }
  private nextRid = 1; private terminated = false; private binding: Uint8Array | undefined;
  private lifetimeSignal: AbortSignal | undefined; private lifetimeAbort: (() => void) | undefined;
  private closePromise: Promise<void> | undefined;
  private constructor(private readonly socket: TokenSocket, sid: Uint8Array, expires: bigint, private readonly requestTimeoutMs: number) { this.binding = sid.slice(); this.expiresAtNs = expires; }
  get sid(): Uint8Array { return this.binding?.slice() ?? new Uint8Array(); }
  static async connect(url: URL, token: TokenWsConnectOptions['token'], signal: AbortSignal | undefined, timeoutMs: number, requestTimeoutMs: number, factory: TokenWsSocketFactory, browser = false): Promise<TokenWsLifecycle> {
    if (![timeoutMs, requestTimeoutMs].every((v) => Number.isSafeInteger(v) && v >= 0 && v <= 0x7fff_ffff)) throw new TokenTransportError('invalid_request', 'Invalid WebSocket timeout', { phase: 'connect' });
    if (signal?.aborted) throw new TokenTransportError('client_aborted', 'Token WebSocket connection failed', { phase: 'connect' });
    const controller = new AbortController(); let socket: TokenSocket | undefined; let settled = false; let timer: ReturnType<typeof setTimeout> | undefined;
    let rejectOpen: (error: Error) => void = () => undefined;
    const cleanup = (): void => { if (timer) clearTimeout(timer); timer = undefined; signal?.removeEventListener('abort', abort); };
    const fail = (error: Error): void => { if (settled) return; settled = true; cleanup(); controller.abort(); if (socket) void socket.close().catch(() => undefined); rejectOpen(browser ? new TokenTransportError('authentication_failed', 'Token WebSocket authentication failed', { phase: 'connect' }) : error); };
    const abort = (): void => { controller.abort(); fail(new TokenTransportError('client_aborted', 'Token WebSocket connection failed', { phase: 'connect' })); };
    return new Promise<TokenWsLifecycle>((resolve, reject) => {
      rejectOpen = reject; signal?.addEventListener('abort', abort, { once: true });
      if (timeoutMs > 0) timer = setTimeout(() => fail(new TokenTransportError('client_timeout', 'Token WebSocket connection failed', { phase: 'connect' })), timeoutMs);
      let opening: Promise<TokenSocket>; try { opening = factory(url, token, controller.signal, timeoutMs); } catch { fail(new TokenTransportError(browser ? 'authentication_failed' : 'network_error', 'Token WebSocket connection failed', { phase: 'connect' })); return; }
      const failBinding = (error: Error): void => { if (browser) fail(new TokenTransportError('authentication_failed', 'Token WebSocket authentication failed', { phase: 'connect' })); else fail(error); };
      opening.then((opened) => {
        if (settled || controller.signal.aborted) { void opened.close().catch(() => undefined); return; }
        socket = opened; let lifecycle: TokenWsLifecycle | undefined;
        opened.onClose((event) => { if (!settled) failBinding(browser ? new TokenTransportError('authentication_failed', 'Token WebSocket authentication failed', { phase: 'connect' }) : closeError(event.code)); else lifecycle?.onClose(event.code); });
        opened.onMessage((bytes) => {
          if (settled) { lifecycle?.receive(bytes); return; }
          try {
            const binding = decodeTokenBinding(bytes); settled = true; cleanup();
            lifecycle = new TokenWsLifecycle(opened, binding.sid, binding.expiresAtNs, requestTimeoutMs);
            lifecycle.lifetimeSignal = signal; lifecycle.lifetimeAbort = () => { lifecycle!.terminate(new TokenTransportError('client_aborted', 'Token WebSocket connection aborted')); void opened.close().catch(() => undefined); };
            signal?.addEventListener('abort', lifecycle.lifetimeAbort, { once: true }); resolve(lifecycle);
          } catch { failBinding(new TokenTransportError('protocol_error', 'Invalid token binding', { phase: 'connect' })); }
        });
      }, (error: unknown) => failBinding(error instanceof TokenTransportError ? error : new TokenTransportError('network_error', 'Token WebSocket connection failed', { phase: 'connect' })));
    });
  }
  request(request: TokenWsRequest, options: TokenRequestOptions = {}): Promise<Record<string, unknown>> { return this.requestInner(request, options); }
  private async requestInner(request: TokenWsRequest, options: TokenRequestOptions): Promise<Record<string, unknown>> {
    if (typeof request !== 'object' || request === null || Array.isArray(request) || typeof (request as { op?: unknown }).op !== 'string' || Object.keys(request).length > 64) return Promise.reject(new TokenTransportError('invalid_request', 'Invalid token request'));
    const timeout = options.timeoutMs ?? this.requestTimeoutMs;
    if (!Number.isSafeInteger(timeout) || timeout < 0 || timeout > 0x7fff_ffff) return Promise.reject(new TokenTransportError('invalid_request', 'Invalid request timeout'));
    if (options.signal?.aborted) return Promise.reject(new TokenTransportError('client_aborted', 'Token request aborted'));
    if (this.terminated || !this.binding) return Promise.reject(new TokenTransportError('connection_closed', 'Token WebSocket is closed'));
    let rid: number | undefined;
    for (let attempts = 0; attempts <= 0xffff_ffff; attempts++) { const candidate = this.nextRid >>> 0; this.nextRid = (candidate + 1) >>> 0; if (!this.pending.has(candidate)) { rid = candidate; break; } }
    if (rid === undefined) return Promise.reject(new TokenTransportError('protocol_error', 'Request id space exhausted'));
    const id = rid;
    return new Promise((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const cleanup = (): void => { this.pending.delete(id); if (timer) clearTimeout(timer); if (abort) options.signal?.removeEventListener('abort', abort); };
      const abort = options.signal ? (): void => { cleanup(); reject(new TokenTransportError('client_aborted', 'Token request aborted')); } : undefined;
      if (options.signal?.aborted) { reject(new TokenTransportError('client_aborted', 'Token request aborted')); return; }
      if (abort) options.signal!.addEventListener('abort', abort, { once: true });
      if (timeout > 0) timer = setTimeout(() => { cleanup(); reject(new TokenTransportError('client_timeout', 'Token request timed out')); }, timeout);
      this.pending.set(id, { resolve: (value) => { cleanup(); resolve(value); }, reject: (error) => { cleanup(); reject(error); }, ...(timer ? { timer } : {}), ...(abort ? { abort } : {}) });
      try { this.socket.send(encodeTokenRequest(this.binding!, id, request)); } catch { cleanup(); reject(new TokenTransportError('network_error', 'Token request failed')); }
    });
  }
  registerCursor(cursor: CursorResource): void { this.cursors.add(cursor); cursor.onTerminal?.(() => this.cursors.delete(cursor)); }
  unregisterCursor(cursor: CursorResource): void { this.cursors.delete(cursor); }
  registerSubscription(id: number, sub: SubscriptionResource, handler: (push: NonNullable<ReturnType<typeof decodeTokenPush>>) => void): void {
    if (!Number.isSafeInteger(id) || id < 0 || this.subscriptions.size >= MAX_SUBSCRIPTIONS && !this.subscriptions.has(id)) throw new TokenTransportError('token_limit_reached', 'Subscription limit reached');
    this.subscriptions.set(id, sub); this.handlers.set(id, handler);
    const queued = this.earlyPushes.get(id); if (queued) { this.earlyPushes.delete(id); this.bufferedPushCount -= queued.length; for (const push of queued) handler(push); }
  }
  unregisterSubscription(id: number): void { this.subscriptions.delete(id); this.handlers.delete(id); const queue = this.earlyPushes.get(id); if (queue) this.bufferedPushCount -= queue.length; this.earlyPushes.delete(id); }
  private receive(bytes: Uint8Array): void {
    if (this.terminated) return;
    try {
      const frame = decodeTokenFrame(bytes), push = decodeTokenPush(frame);
      if (push) { const handler = this.handlers.get(push.sub); if (handler) handler(push); else if (this.bufferedPushCount < MAX_SUBSCRIPTIONS * 256) { const queue = this.earlyPushes.get(push.sub) ?? []; if (queue.length < 256) { queue.push(push); this.bufferedPushCount++; this.earlyPushes.set(push.sub, queue); } } return; }
      const raw = frame.rid; if (typeof raw !== 'number' || !Number.isInteger(raw) || raw < 0 || raw > 0xffff_ffff) throw new Error();
      const pending = this.pending.get(raw); if (!pending) return;
      if (Object.keys(frame).some((key) => !['rid','res','error'].includes(key)) || Object.prototype.hasOwnProperty.call(frame, 'res') === Object.prototype.hasOwnProperty.call(frame, 'error')) throw new Error();
      if (frame.error !== undefined) pending.reject(applicationError(frame.error));
      else if (frame.res instanceof Uint8Array) { const result = decodeTokenPayload(frame.res); if (!record(result)) throw new Error(); pending.resolve(result); }
      else throw new Error();
    } catch { this.terminate(new TokenTransportError('protocol_error', 'Invalid token response')); void this.socket.close(1002, 'protocol error').catch(() => undefined); }
  }
  private onClose(code: number): void { if (this.terminated) return; this.terminate(closeError(code)); }
  private terminate(error: Error): void {
    if (this.terminated) return;
    this.terminated = true; this.terminalController.abort(); if (this.lifetimeAbort) this.lifetimeSignal?.removeEventListener('abort', this.lifetimeAbort);
    for (const callback of this.terminalCallbacks) { try { callback(error); } catch { /* terminal observers cannot interrupt cleanup */ } }
    this.terminalCallbacks.clear();
    for (const pending of this.pending.values()) pending.reject(error);
    this.pending.clear();
    for (const cursor of this.cursors) { cursor.terminate?.(); }
    this.cursors.clear();
    for (const sub of this.subscriptions.values()) sub.terminate();
    this.subscriptions.clear(); this.handlers.clear(); this.earlyPushes.clear(); this.bufferedPushCount = 0; this.binding = undefined;
  }
  async close(): Promise<void> { if (this.closePromise) return this.closePromise; if (this.terminated) return; this.terminate(new TokenTransportError('connection_closed', 'Token WebSocket is closed')); this.closePromise = this.socket.close(); return this.closePromise; }
}
function applicationError(value: unknown): Error {
  if (typeof value === 'string' && /^[a-z][a-z0-9_]{0,63}$/.test(value)) return isTokenTransportErrorCode(value) ? new TokenTransportError(value, 'Token request failed') : new ShamirDbError(value, 'Remote application error');
  if (!record(value) || value.kind !== 'error' || typeof value.code !== 'string' || !/^[a-z][a-z0-9_]{0,63}$/.test(value.code) || typeof value.message !== 'string') return new TokenTransportError('protocol_error', 'Malformed token error response');
  return isTokenTransportErrorCode(value.code) ? new TokenTransportError(value.code, 'Token request failed') : new ShamirDbError(value.code, 'Remote application error');
}
function closeError(code: number): TokenTransportError { const mapped = code === 4000 ? 'authentication_failed' : code === 4010 ? 'token_sid_mismatch' : code === 4011 ? 'token_revoked' : code === 4012 ? 'token_expired' : code === 4013 ? 'token_channel_denied' : code === 4014 ? 'token_plaintext_write_denied' : code === 1013 ? 'server_busy' : code === 1000 ? 'connection_closed' : [1002,1003,1009].includes(code) ? 'protocol_error' : 'network_error'; return new TokenTransportError(mapped, 'Token WebSocket closed', { phase: 'close' }); }
