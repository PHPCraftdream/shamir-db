import { describe, expect, it, vi } from 'vitest';
import { decode } from '../../../core/protocol/framing.js';
import { ApiToken } from '../../../entry/api-token.js';
import { openNodeTokenSocket } from '../socket-node.js';
import type { TokenSocketOptions } from '../types.js';

const mocks = vi.hoisted(() => ({ last: undefined as unknown, options: undefined as unknown as Record<string, unknown> }));
vi.mock('ws', async () => {
  const { EventEmitter: Emitter } = await vi.importActual<typeof import('node:events')>('node:events');
  class MockSocket extends Emitter {
  static OPEN = 1; static CLOSED = 3;
  readyState = 0; protocol = ''; terminated = false; sent: Uint8Array[] = [];
  constructor(_url: URL, protocols: string | string[], options: Record<string, unknown>) { super(); mocks.last = this; mocks.options = options; this.protocol = typeof protocols === 'string' ? protocols : protocols[0] ?? ''; }
  send(data: Uint8Array): void { this.sent.push(data.slice()); }
  close(code = 1000): void { this.readyState = 3; this.emit('close', code, Buffer.from('private peer reason')); }
  terminate(): void { this.terminated = true; this.readyState = 3; this.emit('close', 1006, Buffer.alloc(0)); }
  open(protocol = 'shamir-v1'): void { this.protocol = protocol; this.readyState = 1; this.emit('open'); }
  }
  return MockSocket;
});
interface FakeWs { open(protocol?: string): void; close(code?: number): void; emit(event: string, ...args: unknown[]): boolean; readonly protocol: string; readonly sent: Uint8Array[]; readonly terminated: boolean; }

const TOKEN = 'shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';
const token = ApiToken.from(TOKEN);
const options = { auth: 'token', channel: 'ws', url: 'wss://example.test', token } as const;
const socketOptions = (connectTimeoutMs = 1000, signal = new AbortController().signal): TokenSocketOptions => ({ signal, connectTimeoutMs });
const start = (timeout = 1000, signal?: AbortSignal) => {
  const ca = Uint8Array.of(1, 2, 3);
  const pending = openNodeTokenSocket(new URL('wss://example.test/shamir/v1/ws'), token, { ...options, tls: { ca } }, socketOptions(timeout, signal));
  return { pending, ca };
};

describe('Node token socket adapter', () => {
  it('uses native bearer auth, required subprotocol, TLS settings, and sends no token frame', async () => {
    const { pending } = start(); const socket = mocks.last as FakeWs; expect(socket).toBeDefined();

    expect(mocks.options).toMatchObject({ headers: { Authorization: `Bearer ${TOKEN}` }, perMessageDeflate: false, maxPayload: 16 * 1024 * 1024, minVersion: 'TLSv1.3', ALPNProtocols: ['http/1.1'], rejectUnauthorized: true, ca: Buffer.from([1, 2, 3]) });
    expect(socket?.protocol).toBe('shamir-v1'); socket?.open(); const adapter = await pending;
    expect(socket?.sent).toHaveLength(0); expect(adapter).toBeDefined();
  });
  it('allows only the explicit insecure TLS switch and checks protocol negotiation', async () => {
    const pending = openNodeTokenSocket(new URL('wss://example.test'), token, { ...options, tls: { allowInsecureTlsForDevelopment: true } }, socketOptions());
    expect(mocks.options).toMatchObject({ minVersion: 'TLSv1.3', rejectUnauthorized: false });
    (mocks.last as FakeWs).open('wrong'); await expect(pending).rejects.toMatchObject({ code: 'protocol_error' });
  });
  it.each([401, 403, 429, 503])('maps HTTP upgrade status %i without peer details', async (status) => {
    const pending = openNodeTokenSocket(new URL('wss://example.test'), token, options, socketOptions());
    const response = { statusCode: status, destroy: vi.fn(), on: vi.fn((_event: string, _handler: () => void): void => {}) };
    (mocks.last as FakeWs).emit('unexpected-response', {}, response);
    await expect(pending).rejects.toMatchObject({ status, message: 'Token WebSocket connection failed', code: status === 401 ? 'unauthorized' : status === 403 ? 'permission_denied' : status === 429 ? 'rate_limited' : 'server_unavailable' });
    expect(response.destroy).toHaveBeenCalled();
  });
  it('buffers early messages within count and byte bounds and replays early close once', async () => {
    const { pending } = start(); const socket = mocks.last as FakeWs; socket?.open(); const adapter = await pending;
    socket?.emit('message', Buffer.from([1, 2]), true); const message = vi.fn(); adapter.onMessage(message); expect(message).toHaveBeenCalledWith(new Uint8Array([1, 2]));
    socket?.close(1000); const closed = vi.fn(); adapter.onClose(closed); expect(closed).toHaveBeenCalledWith({ code: 1000, reason: '', wasClean: true });
    expect(() => adapter.send(new Uint8Array())).toThrow();
  });
  it('bounds queued data and makes close idempotent', async () => {
    const { pending } = start(); const socket = mocks.last as FakeWs; socket.open(); const adapter = await pending;
    for (let i = 0; i < 65; i++) socket.emit('message', Buffer.of(i), true);
    const closeA = adapter.close(); const closeB = adapter.close(); expect(closeA).toBe(closeB); socket.close(); await closeA;
    expect(socket.terminated).toBe(false);
  });
  it('handles connect timeout and abort without secret-bearing errors', async () => {
    const { pending } = start(1); await expect(pending).rejects.toMatchObject({ code: 'client_timeout' });
    const controller = new AbortController(); const attempt = start(1000, controller.signal); controller.abort(); await expect(attempt.pending).rejects.toMatchObject({ code: 'client_aborted' });
  });
  it('decodes no application message before lifecycle registers its callback', async () => {
    const { pending } = start(); const socket = mocks.last as FakeWs; socket?.open(); const adapter = await pending;
    socket?.emit('message', Buffer.from([0x81, 0xa1, 0x78, 0x01]), true);
    const handler = vi.fn(); adapter.onMessage(handler); expect(handler).toHaveBeenCalledTimes(1); expect(decode(handler.mock.calls[0]?.[0] ?? new Uint8Array())).toEqual({ x: 1 });
  });
});
