/**
 * WebSocket binary framing + msgpack encode/decode helpers.
 *
 * The framing protocol: every WS BINARY message = [u32_be length][payload].
 * The inner length MUST equal the WS message body length minus 4.
 *
 * @msgpack/msgpack is platform-agnostic (pure JS, works in Node and browser)
 * so it is imported directly — no Platform delegation needed here.
 *
 * PLATFORM-AGNOSTIC: no `node:crypto`, `ws`, or WebCrypto here.
 */

import { encode, decode } from '@msgpack/msgpack';
import type { Socket } from './platform.js';

export { encode, decode };

/** Default post-auth frame ceiling — matches MAX_FRAME_SIZE_DEFAULT (16 MiB). */
export const MAX_FRAME_SIZE_DEFAULT = 16 * 1024 * 1024;

/**
 * A promise-based duplex over a `Socket` that speaks the
 * `[u32_be length][payload]` framing. Inbound frames are queued so a
 * caller can `recv()` them one at a time in request/response order.
 */
export class WsFramer {
  private readonly socket: Socket;
  private readonly inbox: Uint8Array[] = [];
  private readonly waiters: Array<{
    resolve: (frame: Uint8Array) => void;
    reject: (err: Error) => void;
  }> = [];
  private closed = false;
  private closeErr: Error | null = null;

  constructor(socket: Socket) {
    this.socket = socket;

    socket.onMessage((data: Uint8Array) => {
      try {
        this.onBinary(data);
      } catch (e) {
        this.fail(e instanceof Error ? e : new Error(String(e)));
      }
    });

    socket.onClose((err?: Error) => {
      this.fail(err ?? new Error('connection closed'));
    });
  }

  private onBinary(bytes: Uint8Array): void {
    if (bytes.length < 4) {
      throw new Error(`frame too short: ${bytes.length} bytes`);
    }
    const declared =
      ((bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3]) >>> 0;
    const body = bytes.subarray(4);
    if (declared !== body.length) {
      throw new Error(
        `length prefix mismatch: declared=${declared} actual=${body.length}`,
      );
    }
    // Copy out of the shared buffer so later messages cannot alias it.
    const frame = body.slice();
    const waiter = this.waiters.shift();
    if (waiter) {
      waiter.resolve(frame);
    } else {
      this.inbox.push(frame);
    }
  }

  private fail(err: Error): void {
    if (this.closed) return;
    this.closed = true;
    this.closeErr = err;
    while (this.waiters.length > 0) {
      this.waiters.shift()!.reject(err);
    }
  }

  /** Send one frame (`[u32_be length][payload]` inside a WS BINARY message). */
  send(payload: Uint8Array): void {
    if (this.closed) {
      throw this.closeErr ?? new Error('connection closed');
    }
    const buf = new Uint8Array(4 + payload.length);
    const len = payload.length >>> 0;
    buf[0] = (len >>> 24) & 0xff;
    buf[1] = (len >>> 16) & 0xff;
    buf[2] = (len >>> 8) & 0xff;
    buf[3] = len & 0xff;
    buf.set(payload, 4);
    this.socket.send(buf);
  }

  /** Receive the next inbound frame payload (length prefix stripped). */
  recv(): Promise<Uint8Array> {
    const queued = this.inbox.shift();
    if (queued) return Promise.resolve(queued);
    if (this.closed) {
      return Promise.reject(this.closeErr ?? new Error('connection closed'));
    }
    return new Promise((resolve, reject) => {
      this.waiters.push({ resolve, reject });
    });
  }

  /** Close the underlying socket. Idempotent. */
  close(): Promise<void> {
    return this.socket.close();
  }
}
