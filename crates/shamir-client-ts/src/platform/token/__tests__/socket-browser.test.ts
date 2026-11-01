import { describe, expect, it, vi } from 'vitest';
import { decode } from '../../../core/protocol/framing.js';
import { ApiToken } from '../../../entry/api-token.js';
import { openBrowserTokenSocket } from '../socket-browser.js';
import type { BrowserListener } from '../socket-browser.js';

import type { TokenSocketOptions } from '../types.js';

const TOKEN = 'shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';
class FakeSocket {
  readyState = 0; binaryType = ''; protocol = 'shamir-v1';
  readonly sent: Uint8Array[] = [];
  private readonly listeners = new Map<string, Set<BrowserListener>>();
  addEventListener(type: string, callback: BrowserListener, _options?: { once?: boolean }): void { const listeners = this.listeners.get(type) ?? new Set(); listeners.add(callback); this.listeners.set(type, listeners); }
  removeEventListener(type: string, callback: BrowserListener): void { this.listeners.get(type)?.delete(callback); }
  emit(type: string, event: Parameters<BrowserListener>[0] = { type }): void { for (const callback of this.listeners.get(type) ?? []) callback(event); }
  send(data: Uint8Array): void { this.sent.push(data.slice()); }
  close(code = 1000): void { this.readyState = 3; this.emit('close', { code, reason: TOKEN, wasClean: true }); }
}
const options = (connectTimeoutMs = 1000, signal = new AbortController().signal): TokenSocketOptions => ({ signal, connectTimeoutMs });
const openFake = (socket: FakeSocket, opts = options()) => openBrowserTokenSocket(new URL('wss://example.test/'), ApiToken.from(TOKEN), opts, () => socket);

describe('browser token socket adapter', () => {
  it('sends one 80-byte token-only msgpack binary frame to the browser endpoint', async () => {
    const socket = new FakeSocket(); let address = ''; let protocols: string[] = [];
    const pending = openBrowserTokenSocket(new URL('wss://example.test/'), ApiToken.from(TOKEN), options(), (url, offered) => { address = url; protocols = offered; return socket; });
    expect(address).toBe('wss://example.test/shamir/v1/ws/browser'); expect(protocols).toEqual(['shamir-v1']); expect(socket.binaryType).toBe('arraybuffer');
    socket.readyState = 1; socket.emit('open'); const adapter = await pending;
    expect(socket.sent).toHaveLength(1); expect(socket.sent[0]?.byteLength).toBe(80);
    expect(decode(socket.sent[0] ?? new Uint8Array())).toEqual({ token: TOKEN });
    expect(adapter).toBeDefined();
  });
  it('rejects remote plaintext before constructing a socket', async () => {
    const factory = vi.fn(() => new FakeSocket());
    await expect(openBrowserTokenSocket(new URL('ws://example.test'), ApiToken.from(TOKEN), options(), factory)).rejects.toMatchObject({ code: 'invalid_request' });
    expect(factory).not.toHaveBeenCalled();
  });
  it('buffers early messages and replays an early terminal close without peer reason', async () => {
    const socket = new FakeSocket(); const pending = openFake(socket); socket.readyState = 1; socket.emit('open');
    const adapter = await pending; socket.emit('message', { data: new Uint8Array([1, 2]).buffer });
    const onMessage = vi.fn(); adapter.onMessage(onMessage); expect(onMessage).toHaveBeenCalledWith(new Uint8Array([1, 2]));
    socket.close(1000); const onClose = vi.fn(); adapter.onClose(onClose);
    expect(onClose).toHaveBeenCalledWith({ code: 1000, reason: '', wasClean: true });
    expect(() => adapter.send(new Uint8Array([1]))).toThrow();
  });
  it('rejects a mismatched negotiated protocol before sending credentials', async () => {
    const socket = new FakeSocket(); socket.protocol = 'other'; const pending = openFake(socket); socket.readyState = 1; socket.emit('open');
    await expect(pending).rejects.toMatchObject({ code: 'protocol_error' }); expect(socket.sent).toHaveLength(0);
  });
  it('bounds queued messages and resolves repeated close calls together', async () => {
    const socket = new FakeSocket(); const pending = openFake(socket); socket.readyState = 1; socket.emit('open'); const adapter = await pending;
    for (let i = 0; i < 65; i++) socket.emit('message', { data: new Uint8Array([i]).buffer });
    const closed = vi.fn(); adapter.onClose(closed);
    const first = adapter.close(); const second = adapter.close(); expect(first).toBe(second); socket.close(); await first;
    expect(closed).toHaveBeenCalledTimes(1);
  });
  it('handles timeout and abort before opening', async () => {
    const socket = new FakeSocket(); await expect(openFake(socket, options(1))).rejects.toMatchObject({ code: 'client_timeout' });
    const controller = new AbortController(); const waiting = openFake(new FakeSocket(), options(1000, controller.signal)); controller.abort();
    await expect(waiting).rejects.toMatchObject({ code: 'client_aborted' });
  });
});
