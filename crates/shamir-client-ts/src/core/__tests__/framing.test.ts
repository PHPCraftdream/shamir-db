/**
 * Unit tests for framing.ts:
 *   - promoteWideInts / encode / decode (the #216 useBigInt64 boundary)
 *   - WsFramer (length-prefix framing, recv queueing, close propagation)
 *
 * Uses a fake Socket — no live server required.
 */

import { describe, it, expect } from 'vitest';
import { decode as rawDecode } from '@msgpack/msgpack';
import { encode, decode, WsFramer } from '../framing.js';
import type { Socket } from '../platform.js';

// ── fake Socket ──────────────────────────────────────────────────────────────

/**
 * Minimal fake Socket: records sent buffers, lets a test push inbound frames
 * and simulate close. WsFramer wires `messageHandler` / `closeHandler` through
 * onMessage / onClose in its constructor.
 */
class FakeSocket implements Socket {
  sent: Uint8Array[] = [];
  messageHandler: (data: Uint8Array) => void = () => {};
  closeHandler: (err?: Error) => void = () => {};
  closed = false;

  send(data: Uint8Array): void {
    if (this.closed) throw new Error('connection closed');
    this.sent.push(data);
  }
  onMessage(handler: (data: Uint8Array) => void): void {
    this.messageHandler = handler;
  }
  onClose(handler: (err?: Error) => void): void {
    this.closeHandler = handler;
  }
  close(): Promise<void> {
    this.closed = true;
    return Promise.resolve();
  }
  /** Push a raw WS BINARY message (already including the 4-byte length prefix). */
  pushRaw(bytes: Uint8Array): void {
    this.messageHandler(bytes);
  }
  /** Push a body, wrapping it in the [u32_be length][payload] framing. */
  pushFrame(body: Uint8Array): void {
    const buf = new Uint8Array(4 + body.length);
    const len = body.length >>> 0;
    buf[0] = (len >>> 24) & 0xff;
    buf[1] = (len >>> 16) & 0xff;
    buf[2] = (len >>> 8) & 0xff;
    buf[3] = len & 0xff;
    buf.set(body, 4);
    this.messageHandler(buf);
  }
  simulateClose(err?: Error): void {
    this.closed = true;
    this.closeHandler(err);
  }
}

// ── encode / promoteWideInts ─────────────────────────────────────────────────

const U32_MAX = 0xffff_ffff;

/** First byte of a msgpack value tells us the wire type. */
function firstByte(buf: Uint8Array): number {
  return buf[0];
}

describe('encode — promoteWideInts integer boundary (#216 regression)', () => {
  it('encodes an integer ≤ u32::MAX as a msgpack int, not float64', () => {
    // 0xCB is float64; any int code is < 0xCB for these. A small uint stays uint.
    const buf = encode(U32_MAX);
    // uint32 marker is 0xCE; must NOT be float64 (0xCB) or float32 (0xCA).
    expect(firstByte(buf)).not.toBe(0xcb);
    expect(decode(buf)).toBe(U32_MAX);
  });

  it('encodes an integer > u32::MAX as msgpack uint64 (0xCF), not float64', () => {
    const big = U32_MAX + 1; // 2^32, a plain JS number, integer
    const buf = encode(big);
    expect(firstByte(buf)).toBe(0xcf); // uint64 — this is the #216 fix
    // decode returns bigint under useBigInt64
    expect(decode(buf)).toBe(BigInt(big));
  });

  it('encodes a realistic ms timestamp (≈1.78e12) as uint64, not float64', () => {
    const ts = 1782271121599; // > 2^32, integer — the value that broke #216
    const buf = encode(ts);
    expect(firstByte(buf)).toBe(0xcf);
    expect(decode(buf)).toBe(BigInt(ts));
  });

  it('encodes a most-negative-range integer < i32::MIN as a signed int, not float64', () => {
    const neg = -0x8000_0000 - 1; // below i32::MIN
    const buf = encode(neg);
    expect(firstByte(buf)).not.toBe(0xcb); // not float64
    expect(decode(buf)).toBe(BigInt(neg));
  });

  it('leaves a genuine non-integer float as float64 (no promotion)', () => {
    const buf = encode(3.5);
    expect(firstByte(buf)).toBe(0xcb); // float64 — untouched
    expect(decode(buf)).toBe(3.5);
  });

  it('leaves a large non-integer float (> U32_MAX) as float64', () => {
    const v = U32_MAX + 1.5; // > U32_MAX and genuinely fractional (Number.isInteger → false)
    const buf = encode(v);
    expect(firstByte(buf)).toBe(0xcb);
    expect(decode(buf)).toBe(v);
  });

  it('passes a genuine bigint straight through as uint64', () => {
    const v = 9007199254740993n; // > 2^53, only representable as bigint
    const buf = encode(v);
    expect(firstByte(buf)).toBe(0xcf);
    expect(decode(buf)).toBe(v);
  });

  it('keeps Uint8Array payloads byte-identical (msgpack bin, no recursion)', () => {
    const payload = new Uint8Array([0, 1, 254, 255, 42]);
    const buf = encode({ data: payload });
    const back = rawDecode(buf) as { data: Uint8Array };
    expect(back.data).toBeInstanceOf(Uint8Array);
    expect(Array.from(back.data)).toEqual([0, 1, 254, 255, 42]);
  });

  it('promotes wide integers nested inside arrays', () => {
    const buf = encode([1, U32_MAX + 1, 2]);
    const back = decode(buf) as unknown[];
    expect(back[0]).toBe(1);
    expect(back[1]).toBe(BigInt(U32_MAX + 1));
    expect(back[2]).toBe(2);
  });

  it('promotes wide integers nested inside objects', () => {
    const buf = encode({ small: 7, big: 1782271121599, nested: { v: U32_MAX + 5 } });
    const back = decode(buf) as {
      small: number;
      big: bigint;
      nested: { v: bigint };
    };
    expect(back.small).toBe(7);
    expect(back.big).toBe(1782271121599n);
    expect(back.nested.v).toBe(BigInt(U32_MAX + 5));
  });

  it('leaves strings, booleans and null untouched', () => {
    const buf = encode({ s: 'hello', t: true, f: false, n: null });
    expect(decode(buf)).toEqual({ s: 'hello', t: true, f: false, n: null });
  });
});

// ── decode ───────────────────────────────────────────────────────────────────

describe('decode — useBigInt64', () => {
  it('round-trips a uint64 > 2^53 without precision loss', () => {
    const v = 18446744073709551610n; // near u64::MAX
    expect(decode(encode(v))).toBe(v);
  });
});

// ── WsFramer ─────────────────────────────────────────────────────────────────

describe('WsFramer.send', () => {
  it('prefixes the payload with a big-endian u32 length', () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    framer.send(new Uint8Array([0xaa, 0xbb, 0xcc]));
    expect(sock.sent).toHaveLength(1);
    const buf = sock.sent[0];
    expect(buf.length).toBe(7);
    // length prefix = 3, big-endian
    expect([buf[0], buf[1], buf[2], buf[3]]).toEqual([0, 0, 0, 3]);
    expect([buf[4], buf[5], buf[6]]).toEqual([0xaa, 0xbb, 0xcc]);
  });

  it('encodes a length > 255 across the correct prefix bytes', () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    framer.send(new Uint8Array(300));
    const buf = sock.sent[0];
    expect([buf[0], buf[1], buf[2], buf[3]]).toEqual([0, 0, 0x01, 0x2c]); // 300 = 0x012c
  });

  it('throws when sending after close', () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    sock.simulateClose(new Error('boom'));
    expect(() => framer.send(new Uint8Array([1]))).toThrow('boom');
  });
});

describe('WsFramer.recv', () => {
  it('returns a queued frame body with the length prefix stripped', async () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    sock.pushFrame(new Uint8Array([1, 2, 3, 4]));
    const frame = await framer.recv();
    expect(Array.from(frame)).toEqual([1, 2, 3, 4]);
  });

  it('resolves a recv() that was awaited before the frame arrived', async () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    const pending = framer.recv();
    sock.pushFrame(new Uint8Array([9, 9]));
    expect(Array.from(await pending)).toEqual([9, 9]);
  });

  it('delivers frames in FIFO order across queued and awaited recvs', async () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    sock.pushFrame(new Uint8Array([1]));
    sock.pushFrame(new Uint8Array([2]));
    expect(Array.from(await framer.recv())).toEqual([1]);
    expect(Array.from(await framer.recv())).toEqual([2]);
  });

  it('copies the body out of the shared buffer (no aliasing of later frames)', async () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    sock.pushFrame(new Uint8Array([7, 8]));
    const frame = await framer.recv();
    // mutating the captured frame must not affect a fresh decode
    frame[0] = 0;
    expect(frame[0]).toBe(0); // proves it is our own copy, detached from the socket buffer
  });

  it('rejects a frame shorter than 4 bytes via the socket error path', () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    sock.pushRaw(new Uint8Array([1, 2, 3])); // < 4 → throws inside onBinary → fail()
    return expect(framer.recv()).rejects.toThrow(/frame too short/);
  });

  it('rejects on a length-prefix / body-length mismatch', () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    // declares length 10 but body is only 2 bytes
    sock.pushRaw(new Uint8Array([0, 0, 0, 10, 0xaa, 0xbb]));
    return expect(framer.recv()).rejects.toThrow(/length prefix mismatch/);
  });

  it('rejects a pending recv() when the socket closes with an error', () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    const pending = framer.recv();
    sock.simulateClose(new Error('disconnected'));
    return expect(pending).rejects.toThrow('disconnected');
  });

  it('rejects a recv() issued after the socket already closed', () => {
    const sock = new FakeSocket();
    const framer = new WsFramer(sock);
    sock.simulateClose();
    return expect(framer.recv()).rejects.toThrow(/connection closed/);
  });
});
