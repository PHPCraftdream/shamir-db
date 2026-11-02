import { describe, expect, it, vi } from 'vitest';
import { decode, encode } from '../../core/protocol/framing.js';
import { decodeTokenBinding, encodeBrowserTokenFrame } from '../../core/protocol/token/ws.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';
import { ShamirDbError } from '../../core/support/errors.js';
import { VisibilityError } from '../../core/visibility/errors.js';
import { TokenWsLifecycle } from '../ws/token-lifecycle.js';
import type { TokenSocket, TokenSocketCloseEvent } from '../../platform/token/types.js';

class MemorySocket implements TokenSocket {
  sent: Uint8Array[] = [];
  private message: ((data: Uint8Array) => void) | undefined;
  private early: Uint8Array[] = [];
  private closes: Array<(event: TokenSocketCloseEvent) => void> = [];

  async close(code = 1000): Promise<void> {
    for (const handler of this.closes) handler({ code, reason: '', wasClean: true });
  }

  send(data: Uint8Array): void { this.sent.push(data); }

  onMessage(handler: (data: Uint8Array) => void): void {
    this.message = handler;
    for (const frame of this.early.splice(0)) handler(frame);
  }

  onClose(handler: (event: TokenSocketCloseEvent) => void): void { this.closes.push(handler); }

  receive(value: unknown): void {
    const frame = encode(value);
    if (this.message) this.message(frame);
    else this.early.push(frame);
  }
}

const sid = new Uint8Array(32).fill(7);
const connect = (socket: MemorySocket) => TokenWsLifecycle.connect(
  new URL('wss://localhost/'),
  {} as never,
  undefined,
  0,
  35_000,
  async () => {
    socket.receive({ ok: true, sid, expires_at_ns: 9n });
    return socket;
  },
);

describe('token websocket codec and lifecycle', () => {
  it('validates bindings and canonical browser token framing', () => {
    const token = `shk1.${'A'.repeat(22)}.${'A'.repeat(43)}`;
    expect(decodeTokenBinding(encode({ ok: true, sid, expires_at_ns: 12n })).expiresAtNs).toBe(12n);
    expect(() => decodeTokenBinding(encode({ ok: false, sid, expires_at_ns: 12n }))).toThrow();
    expect(() => decodeTokenBinding(encode({ ok: true, sid: new Uint8Array(32), expires_at_ns: 12n }))).toThrow();
    expect(() => decodeTokenBinding(encode({ ok: true, sid, expires_at_ns: 0n }))).toThrow();
    expect(() => decodeTokenBinding(encode({ ok: true, sid, expires_at_ns: 12.5 }))).toThrow();
    expect(() => decodeTokenBinding(encode({ ok: true, sid, expires_at_ns: 12n, extra: true }))).toThrow();
    expect(encodeBrowserTokenFrame(token).byteLength).toBeLessThanOrEqual(256);
    expect(() => encodeBrowserTokenFrame('tkn_' + 'a'.repeat(67))).toThrow();
  });

  it('binds an early frame, returns sid copies and correlates responses', async () => {
    const socket = new MemorySocket();
    const lifecycle = await connect(socket);
    const copy = lifecycle.sid;
    copy.fill(0);
    expect(lifecycle.sid).toEqual(sid);

    const first = lifecycle.request({ op: 'ping' });
    const second = lifecycle.request({ op: 'ping' });
    socket.receive({ rid: 99, res: encode({ kind: 'pong' }) });
    socket.receive({ error: 'rate_limited' });
    socket.receive({ rid: 2, res: encode({ kind: 'pong', n: 2 }) });
    socket.receive({ rid: 1, res: encode({ kind: 'pong', n: 1 }) });

    await expect(second).resolves.toMatchObject({ n: 2 });
    await expect(first).resolves.toMatchObject({ n: 1 });
    await lifecycle.close();
  });

  it('ignores unsolicited rid-less errors without ending the session', async () => {
    const socket = new MemorySocket();
    const lifecycle = await connect(socket);
    const ping = lifecycle.request({ op: 'ping' });
    socket.receive({ error: 'rate_limited' });
    socket.receive({ rid: 1, res: encode({ kind: 'pong' }) });
    await expect(ping).resolves.toMatchObject({ kind: 'pong' });
    expect(lifecycle.sid).toEqual(sid);
    await lifecycle.close();
  });
  it('connects in browser mode and preserves local pre-binding failure codes', async () => {
    const token = {} as never;
    const localError = new TokenTransportError('invalid_request', 'local invalid URL');
    await expect(TokenWsLifecycle.connect(new URL('ws://remote.example/'), token, undefined, 0, 10, () => { throw localError; }, true)).rejects.toBe(localError);

    const socket = new MemorySocket();
    const timeout = TokenWsLifecycle.connect(new URL('wss://localhost/'), token, undefined, 1, 10, async () => socket, true);
    await expect(timeout).rejects.toMatchObject({ code: 'client_timeout' });

    const controller = new AbortController();
    const abortSocket = new MemorySocket();
    const aborting = TokenWsLifecycle.connect(new URL('wss://localhost/'), token, controller.signal, 0, 10, async () => abortSocket, true);
    controller.abort();
    await expect(aborting).rejects.toMatchObject({ code: 'client_aborted' });

    const rejectedSocket = new MemorySocket();
    const rejected = TokenWsLifecycle.connect(new URL('wss://localhost/'), token, undefined, 0, 10, async () => {
      setTimeout(() => { void rejectedSocket.close(4000); }, 0);
      return rejectedSocket;
    }, true);
    await expect(rejected).rejects.toMatchObject({ code: 'authentication_failed' });
  });
  it('registers push handlers, routes pushes and clears them on close', async () => {
    const socket = new MemorySocket();
    const lifecycle = await connect(socket);
    const subscription = { terminate: vi.fn() };
    const handler = vi.fn();
    lifecycle.registerSubscription(4, subscription, handler);
    socket.receive({ push: 'ready', sub: 4, seq: 0 });
    expect(handler).toHaveBeenCalledWith({ push: 'ready', sub: 4, seq: 0 });

    await lifecycle.close();
    expect(subscription.terminate).toHaveBeenCalledTimes(1);
    lifecycle.unregisterSubscription(4);
    expect(() => socket.receive({ push: 'ready', sub: 4, seq: 1 })).not.toThrow();
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it('rejects wrapped application errors and preserves visibility timeout metadata', async () => {
    const socket = new MemorySocket();
    const lifecycle = await connect(socket);
    const transportFailure = lifecycle.request({ op: 'ping' });
    socket.receive({ rid: 1, res: encode({ kind: 'error', code: 'rate_limited', message: 'hidden detail' }) });
    await expect(transportFailure).rejects.toMatchObject({ code: 'rate_limited' });

    const applicationFailure = lifecycle.request({ op: 'ping' });
    socket.receive({ rid: 2, res: encode({ kind: 'error', code: 'tx_conflict', message: 'hidden detail' }) });
    await expect(applicationFailure).rejects.toBeInstanceOf(ShamirDbError);
    await expect(applicationFailure).rejects.toMatchObject({ code: 'tx_conflict', detail: 'Remote application error' });
    expect(lifecycle.sid).toEqual(sid);

    const visibilityFailure = lifecycle.request({ op: 'ping' });
    socket.receive({ rid: 3, res: encode({ kind: 'as_of_visibility_timeout', code: 'as_of_visibility_timeout', message: 'hidden detail', requested: 12n, visible_through: 9n, blocking_version: 10n }) });
    await expect(visibilityFailure).rejects.toBeInstanceOf(VisibilityError);
    await expect(visibilityFailure).rejects.toMatchObject({ code: 'as_of_visibility_timeout', requested: 12n, visible_through: 9n, blocking_version: 10n, detail: 'Visibility wait timed out' });
    expect(lifecycle.sid).toEqual(sid);
    await lifecycle.close();
  });

  it('preserves application errors and terminates malformed replies', async () => {
    const socket = new MemorySocket();
    const lifecycle = await connect(socket);
    const limited = lifecycle.request({ op: 'ping' });
    socket.receive({ rid: 1, error: 'rate_limited' });
    await expect(limited).rejects.toMatchObject({ code: 'rate_limited' });
    expect(lifecycle.sid).toEqual(sid);

    const bad = lifecycle.request({ op: 'ping' });
    socket.receive({ rid: 2, res: encode([]) });
    await expect(bad).rejects.toBeInstanceOf(TokenTransportError);
    expect(lifecycle.sid).toHaveLength(0);
  });
  it('cleans aborted requests and leaves zero timeout unbounded', async () => {
    const socket = new MemorySocket();
    const lifecycle = await connect(socket);
    const abort = new AbortController();
    const pending = lifecycle.request({ op: 'ping' }, { signal: abort.signal });
    abort.abort();
    await expect(pending).rejects.toMatchObject({ code: 'client_aborted' });
    const noTimeout = lifecycle.request({ op: 'ping' }, { timeoutMs: 0 });
    socket.receive({ rid: 2, res: encode({ kind: 'pong' }) });
    await noTimeout;
    await lifecycle.close();
  });
});
