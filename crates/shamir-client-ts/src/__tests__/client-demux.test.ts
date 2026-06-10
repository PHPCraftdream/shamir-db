/**
 * Unit tests for ShamirClient rid-demux (M3).
 *
 * Tests the new concurrent multiplexed request/response model:
 *   - responses may arrive in any order (keyed by rid)
 *   - per-rid error frames reject only the matching request
 *   - rid-less frames go to onUnroutedFrame without breaking pending
 *   - socket close rejects all pending requests
 *
 * Uses a fake Socket/Platform — no live server required.
 */

import { describe, it, expect, vi } from 'vitest';
import type { Socket, Platform } from '../core/platform.js';
import { encode } from '../core/framing.js';
import { WsFramer } from '../core/framing.js';
import { ShamirClient } from '../core/client.js';

// ─── helpers ─────────────────────────────────────────────────────────────────

/**
 * Minimal fake Socket that lets tests push inbound frames and observe sent
 * frames. `messageHandler` and `closeHandler` are wired by WsFramer's
 * constructor via onMessage / onClose.
 */
class FakeSocket implements Socket {
  sent: Uint8Array[] = [];
  messageHandler: ((data: Uint8Array) => void) = () => {};
  closeHandler: ((err?: Error) => void) = () => {};
  private _closed = false;

  send(data: Uint8Array): void {
    if (this._closed) throw new Error('connection closed');
    this.sent.push(data);
  }

  onMessage(handler: (data: Uint8Array) => void): void {
    this.messageHandler = handler;
  }

  onClose(handler: (err?: Error) => void): void {
    this.closeHandler = handler;
  }

  close(): Promise<void> {
    this._closed = true;
    this.closeHandler();
    return Promise.resolve();
  }

  /** Push a raw frame (4-byte length prefix + body) as if received from server. */
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

  /** Simulate connection close with optional error. */
  simulateClose(err?: Error): void {
    this._closed = true;
    this.closeHandler(err);
  }
}

/**
 * Encode a server response envelope as the server would send it.
 * `res` is already msgpack-encoded (inner DbResponse bytes).
 */
function serverSuccessFrame(rid: number, dbResponse: object): Uint8Array {
  const resBytes = encode(dbResponse);
  return encode({ rid, res: resBytes });
}

function serverErrorFrame(rid: number, errorMsg: string): Uint8Array {
  return encode({ rid, error: errorMsg });
}

function serverFatalFrame(errorMsg: string): Uint8Array {
  return encode({ error: errorMsg });
}

/** Build a minimal fake Platform (no real crypto needed for these tests). */
function fakePlatform(): Platform {
  return {
    hmacSha256: (_k, _d) => new Uint8Array(32),
    sha256: (_d) => new Uint8Array(32),
    randomBytes: (n) => new Uint8Array(n),
    timingSafeEqual: (a, b) => {
      if (a.length !== b.length) return false;
      let diff = 0;
      for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
      return diff === 0;
    },
    argon2id: async () => new Uint8Array(32),
    openSocket: async () => { throw new Error('not used'); },
  };
}

/**
 * Construct a ShamirClient by bypassing `connect` (which requires a live
 * server). We reach into the private constructor via a cast to `unknown`.
 *
 * The framer is wired to `socket` before we call the constructor so that
 * the readLoop starts with the fake socket in place.
 */
function buildTestClient(socket: FakeSocket): ShamirClient {
  const platform = fakePlatform();
  const framer = new WsFramer(socket);
  const sessionId = new Uint8Array(32);
  const serverPubKey = new Uint8Array(32);
  const expiresAtNs = BigInt(0);

  // ShamirClient constructor is private; use the static factory bypass.
  // We replicate what `connect` does after the handshake: call `new ShamirClient(...)`.
  // Since the constructor is private in TS, cast through unknown.
  type ClientCtor = {
    new (
      platform: Platform,
      framer: WsFramer,
      sessionId: Uint8Array,
      serverPubKey: Uint8Array,
      expiresAtNs: bigint,
    ): ShamirClient;
  };
  const Ctor = ShamirClient as unknown as ClientCtor;
  return new Ctor(platform, framer, sessionId, serverPubKey, expiresAtNs);
}

// ─── tests ────────────────────────────────────────────────────────────────────

describe('ShamirClient rid-demux (unit)', () => {
  /**
   * (a) Two concurrent requests; responses arrive in REVERSE order (rid=2 first,
   *     then rid=1). Both promises must resolve with the correct response bodies.
   */
  it('concurrent requests resolve with correct bodies when responses arrive out of order', async () => {
    const socket = new FakeSocket();
    const client = buildTestClient(socket);

    // Start two requests concurrently — neither awaited yet.
    const p1 = client.ping(); // will be rid=1 (nextRequestId starts at 1)
    const p2 = client.ping(); // will be rid=2

    // Yield to the microtask queue so both framer.send() calls have fired
    // and the pending map has been populated.
    await Promise.resolve();

    // Push rid=2 response first (reverse order).
    socket.pushFrame(serverSuccessFrame(2, { kind: 'pong' }));
    // Then rid=1.
    socket.pushFrame(serverSuccessFrame(1, { kind: 'pong' }));

    const [r1, r2] = await Promise.all([p1, p2]);
    expect(r1).toEqual({ kind: 'pong' });
    expect(r2).toEqual({ kind: 'pong' });
  });

  /**
   * Verify that out-of-order responses carry through even when response bodies differ.
   */
  it('out-of-order responses carry distinct payloads to the correct callers', async () => {
    const socket = new FakeSocket();
    const client = buildTestClient(socket);

    const p1 = client.ping(); // rid=1
    const p2 = client.ping(); // rid=2

    await Promise.resolve();

    // Deliver rid=2 with payload A and rid=1 with payload B.
    socket.pushFrame(serverSuccessFrame(2, { kind: 'batch', response: { id: 'B' } }));
    socket.pushFrame(serverSuccessFrame(1, { kind: 'batch', response: { id: 'A' } }));

    const [r1, r2] = await Promise.all([p1, p2]);
    // r1 came from rid=1 → payload B (sent last but destined for p1)
    expect((r1 as { kind: string; response: { id: string } }).response.id).toBe('A');
    // r2 came from rid=2 → payload A (sent first but destined for p2)
    expect((r2 as { kind: string; response: { id: string } }).response.id).toBe('B');
  });

  /**
   * (b) Error frame with rid rejects only the matching pending request; the
   *     other concurrent request continues and resolves normally.
   */
  it('error frame with rid rejects only that request; sibling request still resolves', async () => {
    const socket = new FakeSocket();
    const client = buildTestClient(socket);

    const p1 = client.ping(); // rid=1
    const p2 = client.ping(); // rid=2

    await Promise.resolve();

    // rid=1 gets a protocol error.
    socket.pushFrame(serverErrorFrame(1, 'not authorised'));
    // rid=2 gets a normal response.
    socket.pushFrame(serverSuccessFrame(2, { kind: 'pong' }));

    await expect(p1).rejects.toThrow('protocol error: not authorised');
    await expect(p2).resolves.toEqual({ kind: 'pong' });
  });

  /**
   * (c) A frame without rid calls onUnroutedFrame and does not disturb any
   *     pending requests.
   */
  it('frame without rid calls onUnroutedFrame and does not affect pending requests', async () => {
    const socket = new FakeSocket();
    const client = buildTestClient(socket);

    const unrouted: Array<Record<string, unknown>> = [];
    client.onUnroutedFrame = (f) => unrouted.push(f);

    const p1 = client.ping(); // rid=1

    await Promise.resolve();

    // Push a server-push frame (no rid, no error — a subscription event).
    const pushFrame = encode({ kind: 'subscription_event', topic: 'updates' });
    socket.pushFrame(pushFrame);

    // Now resolve the actual request.
    socket.pushFrame(serverSuccessFrame(1, { kind: 'pong' }));

    await expect(p1).resolves.toEqual({ kind: 'pong' });
    expect(unrouted.length).toBe(1);
    expect(unrouted[0].kind).toBe('subscription_event');
    expect(unrouted[0].topic).toBe('updates');
  });

  /**
   * (d) Socket close rejects all pending requests.
   */
  it('socket close rejects all pending requests with the close error', async () => {
    const socket = new FakeSocket();
    const client = buildTestClient(socket);

    const p1 = client.ping(); // rid=1
    const p2 = client.ping(); // rid=2

    await Promise.resolve();

    const closeErr = new Error('connection reset by peer');
    socket.simulateClose(closeErr);

    await expect(p1).rejects.toThrow('connection reset by peer');
    await expect(p2).rejects.toThrow('connection reset by peer');
  });

  /**
   * Sending after socket close rejects immediately (framer.send throws).
   */
  it('send after socket close rejects immediately', async () => {
    const socket = new FakeSocket();
    const client = buildTestClient(socket);

    socket.simulateClose();

    await expect(client.ping()).rejects.toThrow('connection closed');
  });

  /**
   * Fatal (rid-less) error frame rejects ALL pending requests and stops the loop.
   */
  it('fatal protocol error (no rid) rejects all pending requests', async () => {
    const socket = new FakeSocket();
    const client = buildTestClient(socket);

    const p1 = client.ping(); // rid=1
    const p2 = client.ping(); // rid=2

    await Promise.resolve();

    socket.pushFrame(serverFatalFrame('server fatal: shutting down'));

    await expect(p1).rejects.toThrow('protocol error: server fatal: shutting down');
    await expect(p2).rejects.toThrow('protocol error: server fatal: shutting down');
  });

  /**
   * DB-layer error (`kind: "error"` in the inner DbResponse) rejects only
   * the matching request with the db error message.
   */
  it('db-layer error in inner DbResponse rejects the request with db error message', async () => {
    const socket = new FakeSocket();
    const client = buildTestClient(socket);

    const p1 = client.ping();

    await Promise.resolve();

    socket.pushFrame(
      serverSuccessFrame(1, {
        kind: 'error',
        code: 'NotFound',
        message: 'table does not exist',
      }),
    );

    await expect(p1).rejects.toThrow('db error [NotFound]: table does not exist');
  });
});
