/**
 * Unit tests for runHandshake() — the 4-message SCRAM-Argon2id handshake.
 *
 * No live server, no real crypto. We drive a real `WsFramer` wrapping a
 * `FakeSocket`, and inject a deterministic `FakePlatform`.
 *
 * rmp_serde POSITIONAL ARRAY wire formats (documented in protocol.ts):
 *   Challenge = [salt(16), memory_kb, time, parallelism,
 *                argon2_version(0x13), server_nonce(32)]
 *   AuthOk    = [server_signature, server_pub_key, identity_sig,
 *                session_id(32), expires_at_ns,
 *                resumption_ticket?, resumption_expires_at_ns?,
 *                server_query_version?]
 */

import { describe, it, expect } from 'vitest';
import type { Socket, Platform, Argon2Params } from '../platform.js';
import { WsFramer, encode, decode } from '../framing.js';
import { runHandshake } from '../protocol.js';
import {
  buildAuthMessage,
  ARGON2_VERSION_13,
  BINDING_MODE_TLS_NO_EXPORT,
  SUPPORTED_VERSION,
} from '../scram.js';

// ─── FakeSocket (mirrors client-demux.test.ts) ──────────────────────────────

class FakeSocket implements Socket {
  sent: Uint8Array[] = [];
  messageHandler: (_data: Uint8Array) => void = () => {};
  closeHandler: (_err?: Error) => void = () => {};
  private _closed = false;

  send(data: Uint8Array): void {
    if (this._closed) throw new Error('connection closed');
    this.sent.push(data);
  }
  onMessage(h: (data: Uint8Array) => void): void {
    this.messageHandler = h;
  }
  onClose(h: (err?: Error) => void): void {
    this.closeHandler = h;
  }
  close(): Promise<void> {
    this._closed = true;
    this.closeHandler();
    return Promise.resolve();
  }

  /** Push a length-prefixed frame as if received from the server. */
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
    this._closed = true;
    this.closeHandler(err);
  }
}

// ─── FakePlatform ───────────────────────────────────────────────────────────
//
// MAKING verifyServerSignature PASS
// ---------------------------------
// verifyServerSignature (scram.ts) computes:
//     expected = platform.hmacSha256(serverKey, authMessage)
// and compares it (timingSafeEqual) to the server_signature in AuthOk.
//
// `serverKey` is produced inside computeClientProof as:
//     saltedPassword = platform.argon2id(password, salt, kdf)
//     serverKey      = platform.hmacSha256(saltedPassword, "Server Key")
//
// Therefore the test must compute `serverKey` EXACTLY the same way the
// protocol does, then set AuthOk.server_signature = hmacSha256(serverKey,
// authMessage). For that to be reproducible, the fake platform's crypto
// primitives must be DETERMINISTIC functions of their inputs (not random).
//
// Approach: a tiny non-cryptographic but deterministic HMAC/SHA/argon2
// stand-in. It does NOT need to be secure — only stable end-to-end so the
// two sides of the comparison agree. `authMessage` is rebuilt with the real
// buildAuthMessage() using the SAME values the test put in the challenge, so
// it is byte-identical to what protocol.ts builds internally.

/**
 * FNV-1a hash folded into a u32 (deterministic, non-crypto).
 */
function fnv1a32(bytes: Uint8Array): number {
  let h = 0x811c9dc5 >>> 0;
  for (let i = 0; i < bytes.length; i++) {
    h = Math.imul(h ^ bytes[i], 0x01000193) >>> 0;
  }
  return h >>> 0;
}

/** Write big-endian u32 into a Uint8Array at offset (no DataView, to avoid
 *  subarray/detached byteOffset pitfalls). */
function wrU32(b: Uint8Array, off: number, v: number): void {
  b[off] = (v >>> 24) & 0xff;
  b[off + 1] = (v >>> 16) & 0xff;
  b[off + 2] = (v >>> 8) & 0xff;
  b[off + 3] = v & 0xff;
}

/**
 * Deterministic "HMAC-SHA256" stand-in: NOT secure, but a stable 32-byte
 * digest of (key || data). Both protocol.ts and the test call this with
 * identical inputs, so outputs agree.
 */
function fakeHmac(key: Uint8Array, data: Uint8Array): Uint8Array {
  // Always copy inputs into a fresh buffer so subarray views / detached
  // buffers can't bite us.
  const cat = new Uint8Array(key.length + data.length);
  cat.set(key, 0);
  cat.set(data, key.length);

  // Seed from two FNV passes with different init constants.
  let s0 = fnv1a32(cat);
  let s1 = fnv1a32(cat);
  // perturb s1 by hashing a slightly different view
  const tag = new Uint8Array(1);
  tag[0] = 0x5a;
  const cat2 = new Uint8Array(cat.length + 1);
  cat2.set(cat, 0);
  cat2.set(tag, cat.length);
  s1 = fnv1a32(cat2);

  const out = new Uint8Array(32);
  for (let i = 0; i < 8; i++) {
    s0 = (Math.imul(s0, 0x01000193) ^ (i + 0x70)) >>> 0;
    s1 = (Math.imul(s1, 0x01000193) ^ (i + 0xe0)) >>> 0;
    wrU32(out, i * 4, (s0 ^ s1) >>> 0);
  }
  return out;
}

/** Deterministic 32-byte "salted password" stand-in for argon2id. */
function fakeArgon2(password: Uint8Array, salt: Uint8Array, p: Argon2Params): Uint8Array {
  const tag = new Uint8Array(3);
  tag[0] = p.memoryKb & 0xff;
  tag[1] = p.time & 0xff;
  tag[2] = p.parallelism & 0xff;
  const cat = new Uint8Array(password.length + salt.length + tag.length);
  cat.set(password, 0);
  cat.set(salt, password.length);
  cat.set(tag, password.length + salt.length);
  return fakeHmac(new TextEncoder().encode('argon2'), cat);
}

function makeFakePlatform(): Platform {
  // NOTE: argon2id is declared async on the Platform interface, but the body
  // here is purely synchronous. We deliberately do NOT mark the method async
  // so a sync-cast in computeServerSignature gets the raw Uint8Array back.
  // The protocol awaits the result; awaiting a non-thenable just yields it.
  const platform: Platform = {
    hmacSha256: (k, d) => fakeHmac(k, d),
    sha256: (d) => fakeHmac(new Uint8Array(0), d),
    randomBytes: (n) => {
      // Deterministic but content-varied nonce so it isn't all zeros.
      const out = new Uint8Array(n);
      for (let i = 0; i < n; i++) out[i] = (i * 31 + 7) & 0xff;
      return out;
    },
    timingSafeEqual: (a, b) => {
      if (a.length !== b.length) return false;
      let diff = 0;
      for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
      return diff === 0;
    },
    argon2id: ((pw: Uint8Array, salt: Uint8Array, p: Argon2Params) =>
      fakeArgon2(pw, salt, p)) as unknown as Platform['argon2id'],
    openSocket: async () => {
      throw new Error('not used');
    },
  };
  return platform;
}

// ─── wire-frame builders ────────────────────────────────────────────────────

/** 16-byte salt used in every test. */
const SALT = new Uint8Array(16).fill(0xa5);
/** 32-byte server nonce used in every test. */
const SERVER_NONCE = new Uint8Array(32).fill(0x5a);
/** Valid, in-bounds KDF params. */
const KDF = {
  memoryKb: 65536,
  time: 3,
  parallelism: 1,
  argon2Version: ARGON2_VERSION_13,
};

/** Build a valid Challenge frame body (msgpack array). */
function challengeFrame(overrides: Partial<{
  salt: Uint8Array;
  memoryKb: number;
  time: number;
  parallelism: number;
  argon2Version: number;
  serverNonce: Uint8Array;
}> = {}): Uint8Array {
  return encode([
    overrides.salt ?? SALT,
    overrides.memoryKb ?? KDF.memoryKb,
    overrides.time ?? KDF.time,
    overrides.parallelism ?? KDF.parallelism,
    overrides.argon2Version ?? KDF.argon2Version,
    overrides.serverNonce ?? SERVER_NONCE,
  ]);
}

/**
 * Compute the server_signature the fake platform's verifyServerSignature
 * will accept, by mirroring computeClientProof's key schedule.
 *
 * Mirrors scram.ts::computeClientProof:
 *   saltedPassword = argon2id(password, salt, kdf)
 *   serverKey      = hmacSha256(saltedPassword, "Server Key")
 *   serverSig      = hmacSha256(serverKey, authMessage)
 *
 * `authMessage` is rebuilt with the real buildAuthMessage() using the SAME
 * values the test put in the challenge, so it is byte-identical to what
 * protocol.ts builds internally.
 */
function computeServerSignature(
  platform: Platform,
  username: string,
  password: string,
  clientNonce: Uint8Array,
  salt: Uint8Array,
  kdf: typeof KDF,
  serverNonce: Uint8Array,
): Uint8Array {
  const normalizedUser = username.normalize('NFC');
  const authMessage = buildAuthMessage(normalizedUser, clientNonce, {
    serverNonce,
    salt,
    kdf,
  });
  // Our fake platform's argon2id is synchronous under the hood (the Platform
  // interface marks it async, but the impl returns immediately). Call it
  // through a sync cast so this helper stays synchronous.
  const syncArgon = platform.argon2id as unknown as (
    pw: Uint8Array, s: Uint8Array, p: Argon2Params,
  ) => Uint8Array;
  const salted = syncArgon(
    new TextEncoder().encode(password),
    salt,
    { memoryKb: kdf.memoryKb, time: kdf.time, parallelism: kdf.parallelism },
  );
  const serverKey = platform.hmacSha256(
    salted,
    new TextEncoder().encode('Server Key'),
  );
  return platform.hmacSha256(serverKey, authMessage);
}

/** Extract the client_nonce the protocol put in its auth_init frame. */
function readAuthInit(socket: FakeSocket): {
  user: string;
  client_nonce: Uint8Array;
  binding_mode: number;
  version: number;
} {
  // socket.sent[0] is the full length-prefixed frame; payload starts at byte 4.
  const frame = socket.sent[0];
  const payload = frame.subarray(4);
  return decode(payload) as {
    user: string;
    client_nonce: Uint8Array;
    binding_mode: number;
    version: number;
  };
}

// ─── tests ──────────────────────────────────────────────────────────────────

describe('runHandshake (unit, fake WsFramer + fake Platform)', () => {
  const USERNAME = 'alice';
  const PASSWORD = 'correct horse battery staple';

  /**
   * Push both server frames (challenge + auth_ok) up-front BEFORE awaiting
   * runHandshake. The framer queues inbound frames, so this is safe and
   * simpler than microtask-stepping.
   *
   * Returns the FakeSocket so the test can inspect sent frames afterwards.
   */
  function buildHandshake(opts: {
    challengeBody?: Uint8Array;
    authOkBody: Uint8Array;
  }): { socket: FakeSocket; framer: WsFramer; platform: Platform; p: Promise<unknown> } {
    const socket = new FakeSocket();
    const framer = new WsFramer(socket);
    const platform = makeFakePlatform();

    socket.pushFrame(opts.challengeBody ?? challengeFrame());
    socket.pushFrame(opts.authOkBody);

    const p = runHandshake(platform, framer, USERNAME, PASSWORD);
    return { socket, framer, platform, p };
  }

  // ─── happy path ────────────────────────────────────────────────────────

  it('happy path: returns HandshakeResult with correct fields', async () => {
    const platform = makeFakePlatform();
    const socket = new FakeSocket();
    const framer = new WsFramer(socket);

    // The protocol generates client_nonce from platform.randomBytes; since
    // our fake is deterministic we can predict it.
    const expectedClientNonce = platform.randomBytes(32);
    // Reset randomBytes state by making a fresh platform for the actual run:
    const runPlatform = makeFakePlatform();

    // Pre-compute the server signature using the SAME client_nonce the
    // protocol will emit (the fake randomBytes is deterministic).
    const serverSig = computeServerSignature(
      runPlatform,
      USERNAME,
      PASSWORD,
      expectedClientNonce,
      SALT,
      KDF,
      SERVER_NONCE,
    );

    const sessionId = new Uint8Array(32).fill(0x01);
    const serverPubKey = new Uint8Array(32).fill(0x02);
    const expiresAtNs = BigInt('1830000000000000000');

    socket.pushFrame(challengeFrame());
    socket.pushFrame(
      encode([serverSig, serverPubKey, new Uint8Array(64), sessionId, expiresAtNs]),
    );

    const result = await runHandshake(runPlatform, framer, USERNAME, PASSWORD);

    expect(result.sessionId).toEqual(sessionId);
    expect(result.serverPubKey).toEqual(serverPubKey);
    expect(result.expiresAtNs).toBe(expiresAtNs);
    expect(typeof result.expiresAtNs).toBe('bigint');
    // No index-7 → default 0.
    expect(result.serverQueryVersion).toBe(0);
    // No resumption fields → undefined.
    expect(result.resumptionTicket).toBeUndefined();
    expect(result.resumptionExpiresAtNs).toBeUndefined();
  });

  it('happy path: reads serverQueryVersion from positional index 7 when present', async () => {
    const platform = makeFakePlatform();
    const socket = new FakeSocket();
    const framer = new WsFramer(socket);

    const expectedClientNonce = platform.randomBytes(32);
    const runPlatform = makeFakePlatform();
    const serverSig = computeServerSignature(
      runPlatform,
      USERNAME,
      PASSWORD,
      expectedClientNonce,
      SALT,
      KDF,
      SERVER_NONCE,
    );

    const sessionId = new Uint8Array(32).fill(0x03);
    const serverPubKey = new Uint8Array(32).fill(0x04);
    const expiresAtNs = BigInt('9999999999999999999');

    socket.pushFrame(challengeFrame());
    // 8-element array: index 7 = server_query_version.
    socket.pushFrame(
      encode([
        serverSig,
        serverPubKey,
        new Uint8Array(64),
        sessionId,
        expiresAtNs,
        undefined,
        undefined,
        2, // serverQueryVersion
      ]),
    );

    const result = await runHandshake(runPlatform, framer, USERNAME, PASSWORD);
    expect(result.serverQueryVersion).toBe(2);
    expect(result.sessionId).toEqual(sessionId);
  });

  it('happy path: surfaces resumption ticket + expires when present', async () => {
    const platform = makeFakePlatform();
    const socket = new FakeSocket();
    const framer = new WsFramer(socket);

    const expectedClientNonce = platform.randomBytes(32);
    const runPlatform = makeFakePlatform();
    const serverSig = computeServerSignature(
      runPlatform,
      USERNAME,
      PASSWORD,
      expectedClientNonce,
      SALT,
      KDF,
      SERVER_NONCE,
    );

    const sessionId = new Uint8Array(32).fill(0x05);
    const serverPubKey = new Uint8Array(32).fill(0x06);
    const expiresAtNs = BigInt('1234567890123456789');
    const ticket = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);
    const ticketExpires = BigInt('9999999999999999999');

    socket.pushFrame(challengeFrame());
    socket.pushFrame(
      encode([
        serverSig,
        serverPubKey,
        new Uint8Array(64),
        sessionId,
        expiresAtNs,
        ticket,
        ticketExpires,
      ]),
    );

    const result = await runHandshake(runPlatform, framer, USERNAME, PASSWORD);
    expect(result.resumptionTicket).toEqual(ticket);
    expect(result.resumptionExpiresAtNs).toBe(ticketExpires);
    expect(typeof result.resumptionExpiresAtNs).toBe('bigint');
    // index 7 absent → 0.
    expect(result.serverQueryVersion).toBe(0);
  });

  // ─── auth_init frame contents ──────────────────────────────────────────

  it('auth_init (first sent frame) carries NFC user, client_nonce, binding_mode, version', async () => {
    const platform = makeFakePlatform();
    const socket = new FakeSocket();
    const framer = new WsFramer(socket);

    const expectedClientNonce = platform.randomBytes(32);
    const runPlatform = makeFakePlatform();
    const serverSig = computeServerSignature(
      runPlatform,
      USERNAME,
      PASSWORD,
      expectedClientNonce,
      SALT,
      KDF,
      SERVER_NONCE,
    );

    socket.pushFrame(challengeFrame());
    socket.pushFrame(
      encode([serverSig, new Uint8Array(32), new Uint8Array(64), new Uint8Array(32), BigInt(1)]),
    );

    await runHandshake(runPlatform, framer, USERNAME, PASSWORD);

    const init = readAuthInit(socket);
    expect(init.user).toBe(USERNAME.normalize('NFC'));
    expect(init.client_nonce).toBeInstanceOf(Uint8Array);
    expect(init.client_nonce.length).toBe(32);
    expect(init.binding_mode).toBe(BINDING_MODE_TLS_NO_EXPORT);
    expect(init.version).toBe(SUPPORTED_VERSION);
  });

  // ─── challenge validation error paths ──────────────────────────────────

  it('rejects when memory_kb exceeds 262144', async () => {
    const { p } = buildHandshake({
      challengeBody: challengeFrame({ memoryKb: 262145 }),
      authOkBody: encode([new Uint8Array(32), new Uint8Array(32), new Uint8Array(64), new Uint8Array(32), BigInt(1)]),
    });
    await expect(p).rejects.toThrow(/memory_kb 262145 exceeds limit/);
  });

  it('rejects when time exceeds 8', async () => {
    const { p } = buildHandshake({
      challengeBody: challengeFrame({ time: 9 }),
      authOkBody: encode([new Uint8Array(32), new Uint8Array(32), new Uint8Array(64), new Uint8Array(32), BigInt(1)]),
    });
    await expect(p).rejects.toThrow(/challenge time 9 exceeds limit/);
  });

  it('rejects when parallelism exceeds 8', async () => {
    const { p } = buildHandshake({
      challengeBody: challengeFrame({ parallelism: 9 }),
      authOkBody: encode([new Uint8Array(32), new Uint8Array(32), new Uint8Array(64), new Uint8Array(32), BigInt(1)]),
    });
    await expect(p).rejects.toThrow(/challenge parallelism 9 exceeds limit/);
  });

  it('rejects when argon2_version is not 0x13', async () => {
    const { p } = buildHandshake({
      challengeBody: challengeFrame({ argon2Version: 0x10 }),
      authOkBody: encode([new Uint8Array(32), new Uint8Array(32), new Uint8Array(64), new Uint8Array(32), BigInt(1)]),
    });
    await expect(p).rejects.toThrow(/argon2_version must be 0x13, got 16/);
  });

  it('rejects when salt length is not 16', async () => {
    const { p } = buildHandshake({
      challengeBody: challengeFrame({ salt: new Uint8Array(15) }),
      authOkBody: encode([new Uint8Array(32), new Uint8Array(32), new Uint8Array(64), new Uint8Array(32), BigInt(1)]),
    });
    await expect(p).rejects.toThrow(/challenge salt must be 16 bytes/);
  });

  it('rejects when server_nonce length is not 32', async () => {
    const { p } = buildHandshake({
      challengeBody: challengeFrame({ serverNonce: new Uint8Array(31) }),
      authOkBody: encode([new Uint8Array(32), new Uint8Array(32), new Uint8Array(64), new Uint8Array(32), BigInt(1)]),
    });
    await expect(p).rejects.toThrow(/challenge server_nonce must be 32 bytes/);
  });

  // ─── auth_ok validation error paths ────────────────────────────────────

  it('rejects when verifyServerSignature fails (MITM)', async () => {
    // Craft a valid-looking AuthOk but with a signature that will NOT match.
    const bogusSig = new Uint8Array(32).fill(0xde);
    const { p } = buildHandshake({
      authOkBody: encode([
        bogusSig,
        new Uint8Array(32),
        new Uint8Array(64),
        new Uint8Array(32),
        BigInt(1),
      ]),
    });
    await expect(p).rejects.toThrow(/server signature verification failed \(MITM\?\)/);
  });

  it('rejects when session_id length is not 32', async () => {
    // Signature must verify first; use the deterministic platform.
    const platform = makeFakePlatform();
    const socket = new FakeSocket();
    const framer = new WsFramer(socket);

    const expectedClientNonce = platform.randomBytes(32);
    const runPlatform = makeFakePlatform();
    const serverSig = computeServerSignature(
      runPlatform,
      USERNAME,
      PASSWORD,
      expectedClientNonce,
      SALT,
      KDF,
      SERVER_NONCE,
    );

    socket.pushFrame(challengeFrame());
    socket.pushFrame(
      encode([
        serverSig,
        new Uint8Array(32),
        new Uint8Array(64),
        new Uint8Array(16), // wrong length
        BigInt(1),
      ]),
    );

    await expect(
      runHandshake(runPlatform, framer, USERNAME, PASSWORD),
    ).rejects.toThrow(/auth_ok\.session_id must be 32 bytes/);
  });

  it('rejects when msg4 is an error map instead of an array', async () => {
    const { p } = buildHandshake({
      authOkBody: encode({ error: 'bad creds' }),
    });
    await expect(p).rejects.toThrow(/authentication failed: bad creds/);
  });
});
