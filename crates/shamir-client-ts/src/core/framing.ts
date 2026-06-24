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

import { encode as rawEncode, decode as rawDecode } from '@msgpack/msgpack';
import type { Socket } from './platform.js';

/** Largest integer that `useBigInt64` still encodes as a msgpack int (uint32). */
const U32_MAX = 0xffff_ffff;
/** Most-negative integer that stays a msgpack int under `useBigInt64` (i32 min). */
const I32_MIN = -0x8000_0000;

/**
 * Recursively promote integer `number`s that fall OUTSIDE the 32-bit range to
 * `bigint`.
 *
 * Why: `@msgpack/msgpack` with `useBigInt64: true` encodes a plain integer
 * `number` whose magnitude is ≥ 2³² as a msgpack **float64**, not a 64-bit int
 * (it reserves int64/uint64 for actual `bigint`). The server then rejects it
 * ("invalid type: floating point, expected u64") for u64 fields like
 * `as_of_timestamp`, `as_of_version`, etc. Promoting these to `bigint` makes
 * the encoder emit uint64/int64. Values ≤ 2³²-1, non-integers (real floats),
 * strings, `Uint8Array` (msgpack bin), `bigint`, booleans and null are left
 * untouched.
 */
function promoteWideInts(value: unknown): unknown {
  if (typeof value === 'number') {
    return Number.isInteger(value) && (value > U32_MAX || value < I32_MIN)
      ? BigInt(value)
      : value;
  }
  // Leave binary payloads (records_idmsgpack, $fn bytes, …) byte-identical.
  if (value instanceof Uint8Array) return value;
  if (Array.isArray(value)) return value.map(promoteWideInts);
  if (value !== null && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      out[k] = promoteWideInts(v);
    }
    return out;
  }
  return value;
}

/**
 * Wrapper around `@msgpack/msgpack` encode that enables BigInt-as-uint64
 * encoding (`useBigInt64: true`) for genuine `bigint` values (e.g. the
 * fxhash principal_id), AND promotes wide integer `number`s to `bigint`
 * first (see {@link promoteWideInts}) so 64-bit-range integers like
 * timestamps / versions are emitted as msgpack uint64, not float64.
 */
export function encode(value: unknown): Uint8Array {
  return rawEncode(promoteWideInts(value), { useBigInt64: true });
}

/**
 * Wrapper around `@msgpack/msgpack` decode that enables BigInt for int64/uint64
 * values (`useBigInt64: true`).  Without this, any 64-bit integer in the
 * response (e.g. principal_id, owner id) is silently truncated to a JS Number,
 * losing precision for values > 2^53.
 */
export function decode(buffer: Uint8Array): unknown {
  return rawDecode(buffer, { useBigInt64: true });
}

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
