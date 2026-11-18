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
import { isMigrationFloat } from '../builders/admin/schema-migration/model/float.js';
import type { Socket } from '../support/platform.js';

/** Largest integer that `useBigInt64` still encodes as a msgpack int (uint32). */
const U32_MAX = 0xffff_ffff;
/** Most-negative integer that stays a msgpack int under `useBigInt64` (i32 min). */
const I32_MIN = -0x8000_0000;

/** Shared encoder options — the only BigInt semantics the wire speaks. */
const ENCODE_OPTS = { useBigInt64: true } as const;

/**
 * Carries walk state through one `promoteWideInts` traversal.
 *
 * The walk reproduces the PRIOR byte behavior exactly: every key — including
 * a literal `"__proto__"` — is applied with plain assignment (`out[k] = v`).
 * For `"__proto__"` that historically mutated the object's prototype instead
 * of creating an own key (the key silently vanished from the encoded map),
 * and that is what the marker-free path must keep emitting. `protoSeen`
 * records whether such a key was encountered; when the same request also
 * contains an explicit `MigrationFloat`, `encode` reruns the (already
 * promoted) tree with `restoreProto` set, so those keys are created as
 * ordinary own enumerable properties via defineProperty in natural key
 * order, and `encodeWithFloats` emits their canonical map-key bytes.
 */
interface WalkState {
  found: boolean;
  protoSeen: boolean;
  restoreProto: boolean;
}

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
function promoteWideInts(value: unknown, state: WalkState): unknown {
  if (isMigrationFloat(value)) {
    state.found = true;
    return value;
  }
  if (typeof value === 'number') {
    return Number.isInteger(value) && (value > U32_MAX || value < I32_MIN)
      ? BigInt(value)
      : value;
  }
  // Leave binary payloads (records_idmsgpack, $fn bytes, …) byte-identical.
  if (value instanceof Uint8Array) return value;
  if (Array.isArray(value)) return value.map((v) => promoteWideInts(v, state));
  if (value !== null && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      const promoted = promoteWideInts(v, state);
      if (k === '__proto__') {
        // Plain assignment IS the legacy behavior: object values mutate the
        // prototype (the key vanishes from the own-key map), non-object
        // values are ignored entirely. Recorded so `encode` knows a narrow
        // restore rerun is needed when a marker is also present.
        state.protoSeen = true;
        if (state.restoreProto) {
          Object.defineProperty(out, '__proto__', {
            value: promoted,
            writable: true,
            enumerable: true,
            configurable: true,
          });
          continue;
        }
      }
      out[k] = promoted;
    }
    return out;
  }
  return value;
}

/** MessagePack fixed-format width ranges for array and map headers. */
const FIXED_ARR_MAX = 15;
const FIXED_MAP_MAX = 15;
const U16_ARR_MAX = 0xffff;

function writeHeader(
  count: number,
  fixedBase: number,
  u16Code: number,
  u32Code: number,
  fixedMax: number,
): Uint8Array {
  const out = new Uint8Array(count <= fixedMax ? 1 : count <= U16_ARR_MAX ? 3 : 5);
  if (count <= fixedMax) {
    out[0] = fixedBase | count;
  } else if (count <= U16_ARR_MAX) {
    out[0] = u16Code;
    out[1] = (count >>> 8) & 0xff;
    out[2] = count & 0xff;
  } else {
    out[0] = u32Code;
    out[1] = (count >>> 24) & 0xff;
    out[2] = (count >>> 16) & 0xff;
    out[3] = (count >>> 8) & 0xff;
    out[4] = count & 0xff;
  }
  return out;
}

/**
 * Recursively encode a shape that contains at least one `MigrationFloat`
 * marker. Scalars are delegated to `rawEncode` with the same options as the
 * normal path (preserving `useBigInt64` semantics), markers become a raw
 * msgpack float64 (0xCB + 8 big-endian IEEE-754 bits), and containers are
 * written with proper fixed/u16/u32 headers. No msgpack extension reaches
 * the wire — the server only ever sees plain MessagePack types.
 *
 * Bytes are accumulated as `Uint8Array` chunks (one per header/scalar/marker)
 * and concatenated exactly once by the caller, so a 16 MiB text/binary payload
 * never materialises as a per-byte `number[]`.
 */
function encodeWithFloats(value: unknown, chunks: Uint8Array[]): void {
  if (isMigrationFloat(value)) {
    const bytes = new Uint8Array(9);
    bytes[0] = 0xcb;
    new DataView(bytes.buffer).setFloat64(1, value.value, false); // big-endian, bit-exact (-0 kept)
    chunks.push(bytes);
    return;
  }
  if (Array.isArray(value)) {
    chunks.push(writeHeader(value.length, 0x90, 0xdc, 0xdd, FIXED_ARR_MAX));
    for (const item of value) encodeWithFloats(item, chunks);
    return;
  }
  if (value !== null && typeof value === 'object' && !(value instanceof Uint8Array)) {
    const entries = Object.entries(value as Record<string, unknown>);
    chunks.push(writeHeader(entries.length, 0x80, 0xde, 0xdf, FIXED_MAP_MAX));
    for (const [k, v] of entries) {
      // Keys in these DTOs are plain strings — encode directly, no promotion.
      chunks.push(rawEncode(k, ENCODE_OPTS));
      encodeWithFloats(v, chunks);
    }
    return;
  }
  // Scalar (or Uint8Array bin payload): shared options keep BigInt semantics.
  chunks.push(rawEncode(value, ENCODE_OPTS));
}

/**
 * Wrapper around `@msgpack/msgpack` encode that enables BigInt-as-uint64
 * encoding (`useBigInt64: true`) for genuine `bigint` values (e.g. the
 * fxhash principal_id), AND promotes wide integer `number`s to `bigint`
 * first (see {@link promoteWideInts}) so 64-bit-range integers like
 * timestamps / versions are emitted as msgpack uint64, not float64.
 */
export function encode(value: unknown): Uint8Array {
  const state: WalkState = { found: false, protoSeen: false, restoreProto: false };
  const promoted = promoteWideInts(value, state);
  if (!state.found) {
    // Marker-free path: the promoted tree keeps the legacy assignment
    // semantics exactly (own `__proto__` keys mutate the prototype and stay
    // absent from the own-key map) — byte-identical to prior behavior, with
    // no second walk.
    return rawEncode(promoted, ENCODE_OPTS);
  }
  let encoded: unknown = promoted;
  if (state.protoSeen) {
    // Marked branch WITH a `__proto__` key: rerun the ORIGINAL value tree
    // (promotion is idempotent). The first legacy walk already dropped its own
    // `__proto__` key, so it cannot be restored from `promoted`; rerunning on
    // the original makes those keys ordinary own enumerable properties via
    // defineProperty, in natural key order. This is the only case that pays
    // for a second walk.
    encoded = promoteWideInts(value, {
      found: true,
      protoSeen: true,
      restoreProto: true,
    });
  }
  // Marked branch without `__proto__`: no second walk — `encodeWithFloats`
  // emits the canonical query-value map key bytes for constructor-like keys.
  const chunks: Uint8Array[] = [];
  encodeWithFloats(encoded, chunks);
  let total = 0;
  for (const chunk of chunks) total += chunk.length;
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
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
