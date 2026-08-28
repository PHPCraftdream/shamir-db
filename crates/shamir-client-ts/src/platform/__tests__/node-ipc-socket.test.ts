/**
 * Unit tests for `NodeIpcSocket` (platform/node.ts) — the length-prefixed
 * frame reassembler over a raw `node:net` byte stream.
 *
 * Unlike WebSocket (message-framed for free), a `net.Socket` delivers an
 * unbounded byte stream: a `'data'` event can carry a partial frame,
 * several frames back-to-back, or a frame split arbitrarily across two
 * events. These tests drive that reassembly directly against a fake
 * `net.Socket` (a bare `EventEmitter` + the handful of methods
 * `NodeIpcSocket` actually calls) — no real OS socket, so chunk
 * boundaries are exactly what each test specifies, not whatever the
 * kernel happens to coalesce.
 */

import { EventEmitter } from 'node:events';
import type { Socket as NetSocket } from 'node:net';
import { describe, it, expect, vi } from 'vitest';
import { NodeIpcSocket } from '../node.js';

class FakeNetSocket extends EventEmitter {
  written: Buffer[] = [];
  destroyed = false;

  write(data: Uint8Array): boolean {
    this.written.push(Buffer.from(data));
    return true;
  }

  end(): void {
    this.emit('close');
  }
}

/** `[u32_be length][payload]` — the same framing `WsFramer` expects. */
function frame(payload: Uint8Array): Buffer {
  const buf = Buffer.alloc(4 + payload.length);
  buf.writeUInt32BE(payload.length, 0);
  Buffer.from(payload).copy(buf, 4);
  return buf;
}

/** `NodeIpcSocket` delivers plain `Uint8Array`s; `frame()` builds `Buffer`s
 * for convenient chunking/concatenation. Both are byte-identical but
 * `toEqual` treats `Buffer` (a `Uint8Array` subclass with extra
 * prototype machinery) and a plain `Uint8Array` as structurally
 * different — compare via plain number arrays instead. */
function bytes(x: Uint8Array): number[] {
  return Array.from(x);
}

describe('NodeIpcSocket frame reassembly', () => {
  it('delivers one full frame received in a single chunk', () => {
    const net = new FakeNetSocket();
    const sock = new NodeIpcSocket(net as unknown as NetSocket);
    const onMessage = vi.fn();
    sock.onMessage(onMessage);

    const payload = new Uint8Array([1, 2, 3, 4]);
    net.emit('data', frame(payload));

    expect(onMessage).toHaveBeenCalledTimes(1);
    // Delivered WITH the length prefix — matches what WsFramer.onBinary expects.
    expect(bytes(onMessage.mock.calls[0][0])).toEqual(bytes(frame(payload)));
  });

  it('reassembles a frame split across two chunks', () => {
    const net = new FakeNetSocket();
    const sock = new NodeIpcSocket(net as unknown as NetSocket);
    const onMessage = vi.fn();
    sock.onMessage(onMessage);

    const payload = new Uint8Array([9, 8, 7, 6, 5]);
    const full = frame(payload);
    // Split mid-payload, and even mid-length-prefix, to prove both cases work.
    net.emit('data', full.subarray(0, 2)); // half of the length prefix
    expect(onMessage).not.toHaveBeenCalled();
    net.emit('data', full.subarray(2, 6)); // rest of prefix + first payload byte
    expect(onMessage).not.toHaveBeenCalled();
    net.emit('data', full.subarray(6)); // remaining payload bytes
    expect(onMessage).toHaveBeenCalledTimes(1);
    expect(bytes(onMessage.mock.calls[0][0])).toEqual(bytes(full));
  });

  it('drains multiple frames delivered in one chunk', () => {
    const net = new FakeNetSocket();
    const sock = new NodeIpcSocket(net as unknown as NetSocket);
    const onMessage = vi.fn();
    sock.onMessage(onMessage);

    const f1 = frame(new Uint8Array([1]));
    const f2 = frame(new Uint8Array([2, 2]));
    const f3 = frame(new Uint8Array([3, 3, 3]));
    net.emit('data', Buffer.concat([f1, f2, f3]));

    expect(onMessage).toHaveBeenCalledTimes(3);
    expect(bytes(onMessage.mock.calls[0][0])).toEqual(bytes(f1));
    expect(bytes(onMessage.mock.calls[1][0])).toEqual(bytes(f2));
    expect(bytes(onMessage.mock.calls[2][0])).toEqual(bytes(f3));
  });

  it('leaves a trailing partial frame buffered for the next chunk', () => {
    const net = new FakeNetSocket();
    const sock = new NodeIpcSocket(net as unknown as NetSocket);
    const onMessage = vi.fn();
    sock.onMessage(onMessage);

    const f1 = frame(new Uint8Array([1, 1]));
    const f2 = frame(new Uint8Array([2, 2, 2]));
    // f1 whole, plus the first 3 bytes of f2's 7-byte frame.
    net.emit('data', Buffer.concat([f1, f2.subarray(0, 3)]));
    expect(onMessage).toHaveBeenCalledTimes(1);
    expect(bytes(onMessage.mock.calls[0][0])).toEqual(bytes(f1));

    net.emit('data', f2.subarray(3));
    expect(onMessage).toHaveBeenCalledTimes(2);
    expect(bytes(onMessage.mock.calls[1][0])).toEqual(bytes(f2));
  });

  it('send() writes raw bytes to the underlying net.Socket', () => {
    const net = new FakeNetSocket();
    const sock = new NodeIpcSocket(net as unknown as NetSocket);
    const payload = new Uint8Array([5, 6, 7]);
    sock.send(payload);
    expect(net.written).toHaveLength(1);
    expect(net.written[0]).toEqual(Buffer.from(payload));
  });

  it('fires onClose exactly once on a close event', () => {
    const net = new FakeNetSocket();
    const sock = new NodeIpcSocket(net as unknown as NetSocket);
    const onClose = vi.fn();
    sock.onClose(onClose);

    net.emit('close');
    net.emit('error', new Error('should be ignored — close already fired'));

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onClose.mock.calls[0][0]).toBeUndefined();
  });

  it('fires onClose with the error on an error event', () => {
    const net = new FakeNetSocket();
    const sock = new NodeIpcSocket(net as unknown as NetSocket);
    const onClose = vi.fn();
    sock.onClose(onClose);

    const err = new Error('boom');
    net.emit('error', err);

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onClose.mock.calls[0][0]).toBe(err);
  });
});
