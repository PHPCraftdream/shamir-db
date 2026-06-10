/**
 * Unit tests for ShamirClient.resume() — fast reconnection via resumption
 * ticket (M5c).
 *
 * Uses a fake Socket/Platform — no live server required.
 */

import { describe, it, expect } from 'vitest';
import type { Socket, Platform } from '../core/platform.js';
import { encode, decode } from '../core/framing.js';
import { ShamirClient } from '../core/client.js';
import type { ResumeOptions } from '../core/types/index.js';

// ─── helpers ─────────────────────────────────────────────────────────────────

/**
 * A fake socket that, when it receives a `send()`, immediately echoes a
 * pre-configured server response frame back via `onMessage`. This avoids any
 * timing dependency: WsFramer registers its onMessage handler in its
 * constructor (before `resume()` calls `framer.send()`), so the handler is
 * always in place when the auto-reply fires.
 */
class AutoReplySocket implements Socket {
  /** Raw bytes to push back as a framed message when send() is called. */
  private replyBody: Uint8Array;
  sent: Uint8Array[] = [];
  private messageHandler: ((data: Uint8Array) => void) = () => {};
  private closeHandler: ((err?: Error) => void) = () => {};
  private _closed = false;

  constructor(replyBody: Uint8Array) {
    this.replyBody = replyBody;
  }

  send(data: Uint8Array): void {
    if (this._closed) throw new Error('connection closed');
    this.sent.push(data);
    // Immediately push the pre-configured response frame.
    // WsFramer already registered onMessage before framer.send() was called.
    this.pushFrame(this.replyBody);
  }

  onMessage(handler: (data: Uint8Array) => void): void {
    this.messageHandler = handler;
  }

  onClose(handler: (err?: Error) => void): void {
    this.closeHandler = handler;
  }

  close(): Promise<void> {
    if (!this._closed) {
      this._closed = true;
      this.closeHandler();
    }
    return Promise.resolve();
  }

  /** Push raw body as a length-prefixed frame (mirrors WsFramer send format). */
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
}

/** Build a minimal fake Platform. `socketFactory` produces the socket to use. */
function fakePlatform(socketFactory: () => Socket): Platform {
  return {
    hmacSha256: (_k, _d) => new Uint8Array(32),
    sha256: (_d) => new Uint8Array(32),
    randomBytes: (n) => new Uint8Array(n).fill(0xab),
    timingSafeEqual: (a, b) => {
      if (a.length !== b.length) return false;
      let diff = 0;
      for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
      return diff === 0;
    },
    argon2id: async () => new Uint8Array(32),
    openSocket: async () => socketFactory(),
  };
}

function makeResumeOpts(
  ticket: Uint8Array,
  serverPubKey?: Uint8Array,
): ResumeOptions {
  return {
    host: '127.0.0.1',
    port: 1234,
    ticket,
    serverPubKey: serverPubKey ?? new Uint8Array(32).fill(0x07),
    tls: { rejectUnauthorized: false },
  };
}

/** Build a length-prefixed server response frame (what AutoReplySocket stores). */
function serverResponseFrame(fields: Record<string, unknown>): Uint8Array {
  return encode(fields);
}

// ─── tests ────────────────────────────────────────────────────────────────────

describe('ShamirClient.resume()', () => {
  it('sends ticket + client_nonce + binding_mode=2 as the first frame', async () => {
    const sessionId = new Uint8Array(32).fill(0x01);
    const ticket = new Uint8Array(64).fill(0xcc);
    const socket = new AutoReplySocket(
      serverResponseFrame({
        session_id: sessionId,
        expires_at_ns: 9_999_999_999,
      }),
    );
    const platform = fakePlatform(() => socket);

    const client = await ShamirClient.resume(platform, makeResumeOpts(ticket));
    await client.close();

    // One send from resume(): the resume request.
    expect(socket.sent.length).toBe(1);

    // Decode the sent frame — WsFramer added a 4-byte length prefix.
    const body = socket.sent[0].slice(4); // strip length prefix
    const msg = decode(body) as Record<string, unknown>;

    expect(msg.binding_mode).toBe(2);
    expect(msg.ticket).toBeInstanceOf(Uint8Array);
    expect((msg.ticket as Uint8Array).length).toBe(ticket.length);
    expect(msg.client_nonce).toBeInstanceOf(Uint8Array);
    expect((msg.client_nonce as Uint8Array).length).toBe(32);
  });

  it('returns a client with the session_id from the resume response', async () => {
    const sessionId = new Uint8Array(32).fill(0x42);
    const ticket = new Uint8Array(32).fill(0xdd);
    const socket = new AutoReplySocket(
      serverResponseFrame({
        session_id: sessionId,
        expires_at_ns: 1_000_000_000,
      }),
    );
    const client = await ShamirClient.resume(
      fakePlatform(() => socket),
      makeResumeOpts(ticket),
    );
    expect(client.sessionId()).toEqual(sessionId);
    await client.close();
  });

  it('stores resumption_ticket and resumption_expires_at_ns when provided', async () => {
    const sessionId = new Uint8Array(32).fill(0x01);
    const newTicket = new Uint8Array(64).fill(0xee);
    // msgpack cannot encode BigInt; use a number. resume() wraps it in BigInt().
    const newExpiryNum = 5_000_000_000;
    const ticket = new Uint8Array(32).fill(0x11);
    const socket = new AutoReplySocket(
      serverResponseFrame({
        session_id: sessionId,
        expires_at_ns: 1_000_000_000,
        resumption_ticket: newTicket,
        resumption_expires_at_ns: newExpiryNum,
      }),
    );
    const client = await ShamirClient.resume(
      fakePlatform(() => socket),
      makeResumeOpts(ticket),
    );
    expect(client.resumptionTicket()).toEqual(newTicket);
    expect(client.resumptionExpiresAtNs()).toBe(BigInt(newExpiryNum));
    await client.close();
  });

  it('returns undefined for resumption getters when server omits ticket', async () => {
    const sessionId = new Uint8Array(32).fill(0x01);
    const ticket = new Uint8Array(32).fill(0x22);
    const socket = new AutoReplySocket(
      serverResponseFrame({
        session_id: sessionId,
        expires_at_ns: 1_000_000_000,
      }),
    );
    const client = await ShamirClient.resume(
      fakePlatform(() => socket),
      makeResumeOpts(ticket),
    );
    expect(client.resumptionTicket()).toBeUndefined();
    expect(client.resumptionExpiresAtNs()).toBeUndefined();
    await client.close();
  });

  it('throws and closes socket when server sends an error', async () => {
    const ticket = new Uint8Array(32).fill(0x33);
    const socket = new AutoReplySocket(
      serverResponseFrame({ error: 'ticket_expired' }),
    );
    await expect(
      ShamirClient.resume(fakePlatform(() => socket), makeResumeOpts(ticket)),
    ).rejects.toThrow('ticket_expired');
  });

  it('throws when session_id is not 32 bytes', async () => {
    const ticket = new Uint8Array(32).fill(0x44);
    const socket = new AutoReplySocket(
      serverResponseFrame({
        session_id: new Uint8Array(16),
        expires_at_ns: 1_000_000_000,
      }),
    );
    await expect(
      ShamirClient.resume(fakePlatform(() => socket), makeResumeOpts(ticket)),
    ).rejects.toThrow('session_id must be 32 bytes');
  });

  it('preserves serverPubKey from opts', async () => {
    const sessionId = new Uint8Array(32).fill(0x01);
    const serverPubKey = new Uint8Array(32).fill(0xfe);
    const ticket = new Uint8Array(32).fill(0x55);
    const socket = new AutoReplySocket(
      serverResponseFrame({
        session_id: sessionId,
        expires_at_ns: 1_000_000_000,
      }),
    );
    const client = await ShamirClient.resume(
      fakePlatform(() => socket),
      makeResumeOpts(ticket, serverPubKey),
    );
    expect(client.serverPubKeyPin()).toEqual(serverPubKey);
    await client.close();
  });
});

describe('ShamirClient resumption getters (via constructor)', () => {
  it('resumptionTicket() and resumptionExpiresAtNs() return undefined when not provided', async () => {
    // Reach into the private constructor via a cast — the same technique used
    // in client-demux.test.ts.
    const { WsFramer } = await import('../core/framing.js');

    // Minimal socket that does nothing (we only need the constructor).
    const noopSocket: Socket = {
      send: () => {},
      onMessage: () => {},
      onClose: () => {},
      close: async () => {},
    };
    const platform: Platform = {
      hmacSha256: () => new Uint8Array(32),
      sha256: () => new Uint8Array(32),
      randomBytes: (n) => new Uint8Array(n),
      timingSafeEqual: () => true,
      argon2id: async () => new Uint8Array(32),
      openSocket: async () => noopSocket,
    };
    const framer = new WsFramer(noopSocket);

    type ClientCtor = {
      new (
        platform: Platform,
        framer: InstanceType<typeof WsFramer>,
        sessionId: Uint8Array,
        serverPubKey: Uint8Array,
        expiresAtNs: bigint,
        resumptionTicket?: Uint8Array,
        resumptionExpiresAtNs?: bigint,
      ): ShamirClient;
    };
    const Ctor = ShamirClient as unknown as ClientCtor;
    const client = new Ctor(
      platform,
      framer,
      new Uint8Array(32),
      new Uint8Array(32),
      0n,
    );
    expect(client.resumptionTicket()).toBeUndefined();
    expect(client.resumptionExpiresAtNs()).toBeUndefined();
    // Don't close — noopSocket.close() is fine but the closeHandler fires and
    // that's a no-op with empty waiters.
  });

  it('resumptionTicket() and resumptionExpiresAtNs() return values when provided', async () => {
    const { WsFramer } = await import('../core/framing.js');
    const noopSocket: Socket = {
      send: () => {},
      onMessage: () => {},
      onClose: () => {},
      close: async () => {},
    };
    const platform: Platform = {
      hmacSha256: () => new Uint8Array(32),
      sha256: () => new Uint8Array(32),
      randomBytes: (n) => new Uint8Array(n),
      timingSafeEqual: () => true,
      argon2id: async () => new Uint8Array(32),
      openSocket: async () => noopSocket,
    };
    const framer = new WsFramer(noopSocket);

    type ClientCtor = {
      new (
        platform: Platform,
        framer: InstanceType<typeof WsFramer>,
        sessionId: Uint8Array,
        serverPubKey: Uint8Array,
        expiresAtNs: bigint,
        resumptionTicket?: Uint8Array,
        resumptionExpiresAtNs?: bigint,
      ): ShamirClient;
    };
    const Ctor = ShamirClient as unknown as ClientCtor;
    const ticket = new Uint8Array(64).fill(0xab);
    const expiry = 1_234_567_890n;
    const client = new Ctor(
      platform,
      framer,
      new Uint8Array(32),
      new Uint8Array(32),
      0n,
      ticket,
      expiry,
    );
    expect(client.resumptionTicket()).toEqual(ticket);
    expect(client.resumptionExpiresAtNs()).toBe(expiry);
  });
});
