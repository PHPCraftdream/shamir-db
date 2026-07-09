/**
 * Unit tests for ShamirClient — covers the two audit-fix areas in
 * `client.ts` (audit 2026-07-06 §1.2):
 *
 *   1. The `execute` / `tx_begin` / `tx_execute` request envelopes must
 *      carry `query_version: CURRENT_QUERY_LANG_VERSION` (was a hardcoded
 *      stale `1` while the client already emits v2-only wire features).
 *   2. `ShamirClient.resume()` must read `server_query_version` from the
 *      resume_ok response and propagate it into the new client (was always
 *      silently downgraded to `0`).
 *
 * No live server, no real crypto. Drives `resume()` against a fake WS
 * socket so we get a real `ShamirClient` instance with a working readLoop,
 * then captures the bytes the client sends for `execute`/`txBegin`/
 * `txExecute` and decodes the inner DbRequest to assert on `query_version`.
 *
 * resume_ok wire shape (positional msgpack ARRAY — mirrors
 * `crates/shamir-server/src/connection/wire.rs:ResumeOkWire`, emitted via
 * `rmp_serde::to_vec`):
 *   [0]: session_id (bytes, 32)
 *   [1]: expires_at_ns (u64)
 *   [2]: resumption_ticket (bytes; empty when none)
 *   [3]: resumption_expires_at_ns (u64; 0 when none)
 *   [4]: server_query_version (u8; 0 when none)
 */

import { describe, it, expect, afterEach, vi } from 'vitest';
import type { Socket, Platform } from '../platform.js';
import { encode, decode } from '../framing.js';
import { ShamirClient } from '../client.js';
import type { ResumeOptions } from '../types/index.js';
import { CURRENT_QUERY_LANG_VERSION } from '../scram.js';
import { ShamirDbError, ShamirTimeoutError } from '../errors.js';

// Track opened clients so afterEach can close them — otherwise the persistent
// readLoop keeps the socket handle alive and vitest hangs on exit.
const openedClients: ShamirClient[] = [];
afterEach(async () => {
  while (openedClients.length > 0) {
    const c = openedClients.pop()!;
    try {
      await c.close();
    } catch {
      /* socket may already be closed */
    }
  }
});

// ─── FakeSocket ─────────────────────────────────────────────────────────────
//
// Mirrors protocol.test.ts's FakeSocket but with DEFERRED frame delivery:
// frames pushed BEFORE a message handler is registered are buffered and
// flushed the instant `onMessage` is called (i.e. when the WsFramer is
// constructed inside resume()/connect()). This eliminates the race where a
// test pushes a server response before the framer exists.

class FakeSocket implements Socket {
  sent: Uint8Array[] = [];
  private messageHandler: (_data: Uint8Array) => void = () => {};
  closeHandler: (_err?: Error) => void = () => {};
  /** Frames pushed before onMessage(); flushed on registration. */
  private pending: Uint8Array[] = [];
  /** True once a real handler has been registered via onMessage(). */
  private _handlerRegistered = false;
  private _closed = false;

  send(data: Uint8Array): void {
    if (this._closed) throw new Error('connection closed');
    this.sent.push(data);
  }
  onMessage(h: (data: Uint8Array) => void): void {
    this.messageHandler = h;
    this._handlerRegistered = true;
    // Flush any frames pushed before we registered.
    const queued = this.pending;
    this.pending = [];
    for (const f of queued) this.messageHandler(f);
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
    // Deliver now if a handler is registered, else buffer for onMessage().
    if (this._handlerRegistered) {
      this.messageHandler(buf);
    } else {
      this.pending.push(buf);
    }
  }
}

/**
 * Build a Platform whose openSocket returns (and captures) a FakeSocket.
 * The socket is also returned so the test can push frames / inspect sends.
 */
function platformWithSocket(): { platform: Platform; socket: FakeSocket } {
  const socket = new FakeSocket();
  const platform: Platform = {
    hmacSha256: () => new Uint8Array(32),
    sha256: () => new Uint8Array(32),
    randomBytes: (n) => {
      const out = new Uint8Array(n);
      for (let i = 0; i < n; i++) out[i] = (i * 31 + 7) & 0xff;
      return out;
    },
    timingSafeEqual: (a, b) => a.length === b.length,
    // Not used by resume(); stub for type completeness.
    argon2id: async () => new Uint8Array(32),
    openSocket: async () => socket,
  };
  return { platform, socket };
}

function makeResumeOpts(): ResumeOptions {
  return {
    host: '127.0.0.1',
    port: 9999,
    ticket: new Uint8Array([1, 2, 3, 4]),
    serverPubKey: new Uint8Array(32).fill(0xab),
  };
}

/**
 * Build a resume_ok frame body as the positional msgpack ARRAY the server
 * actually sends (`rmp_serde::to_vec(&ResumeOkWire)` — a plain struct with no
 * `#[serde(rename_all)]` serialises as a positional array in declaration
 * order, NOT a named map). Mirrors how protocol.test.ts mocks `auth_ok`.
 * `serverQueryVersion` controls the advertised version (defaults to 2).
 */
function resumeOkFrame(opts?: { serverQueryVersion?: number }): Uint8Array {
  return encode([
    new Uint8Array(32).fill(0x01),
    BigInt('1830000000000000000'),
    new Uint8Array([10, 20, 30, 40]),
    BigInt('9999999999999999999'),
    opts?.serverQueryVersion ?? 2,
  ]);
}

/**
 * Decode the LAST frame on `socket.sent` as a request envelope and return
 * the inner DbRequest (the `req` field is itself msgpack bytes).
 *
 * `socket.sent` holds the raw bytes passed to `Socket.send()`, which (per
 * WsFramer.send) is the 4-byte length prefix + the msgpack envelope body.
 * We strip the prefix, decode the outer envelope `{ sid, rid, req }`, then
 * decode the inner `req` bytes (msgpack bytes of the DbRequest).
 */
function decodeLastRequest(socket: FakeSocket): Record<string, unknown> {
  const frame = socket.sent[socket.sent.length - 1];
  // Strip the 4-byte big-endian length prefix that WsFramer.send prepends.
  const envelopeBytes = frame.subarray(4);
  const outer = decode(envelopeBytes) as Record<string, unknown>;
  const reqBytes = outer['req'] as Uint8Array;
  return decode(reqBytes) as Record<string, unknown>;
}

/**
 * Push a fake server response for the given rid. The readLoop routes by rid
 * and decodes `res` (msgpack bytes) as the DbResponse.
 */
function pushDbResponse(
  socket: FakeSocket,
  rid: number,
  dbResponse: Record<string, unknown>,
): void {
  socket.pushFrame(encode({ rid, res: encode(dbResponse) }));
}

// ─── tests ──────────────────────────────────────────────────────────────────

describe('ShamirClient.resume (unit, fake WS socket)', () => {
  it('propagates server_query_version from resume_ok into the new client', async () => {
    const { platform, socket } = platformWithSocket();
    // Push BEFORE awaiting resume(); the FakeSocket buffers until the framer
    // registers its onMessage handler inside resume().
    socket.pushFrame(resumeOkFrame({ serverQueryVersion: 2 }));

    const client = await ShamirClient.resume(platform, makeResumeOpts());
    openedClients.push(client);

    expect(client.serverQueryVersion()).toBe(2);
  });

  it('defaults server_query_version to 0 when the field is absent (legacy server)', async () => {
    const { platform, socket } = platformWithSocket();
    // Pre-v2 server omits server_query_version: a 4-element array (no index 4).
    // The client must default index 4 to 0.
    socket.pushFrame(
      encode([
        new Uint8Array(32).fill(0x07),
        BigInt('1830000000000000000'),
      ]),
    );

    const client = await ShamirClient.resume(platform, makeResumeOpts());
    openedClients.push(client);

    expect(client.serverQueryVersion()).toBe(0);
  });
});

describe('ShamirClient request envelopes carry CURRENT_QUERY_LANG_VERSION', () => {
  /**
   * Resume a client (v2 server) to obtain a real instance with a working
   * readLoop, then exercise the request path under test.
   */
  async function resumeV2Client(): Promise<{
    client: ShamirClient;
    socket: FakeSocket;
  }> {
    const { platform, socket } = platformWithSocket();
    socket.pushFrame(resumeOkFrame({ serverQueryVersion: 2 }));
    const client = await ShamirClient.resume(platform, makeResumeOpts());
    openedClients.push(client);
    return { client, socket };
  }

  it('execute() sends query_version = CURRENT_QUERY_LANG_VERSION (not the stale 1)', async () => {
    const { client, socket } = await resumeV2Client();
    // Push the response that execute() expects BEFORE the call — rids start
    // at 1, and execute() resolves on DbResponse::Batch. By now the framer's
    // handler is registered so pushFrame delivers immediately.
    pushDbResponse(socket, 1, {
      kind: 'batch',
      response: {
        id: 1,
        results: {},
        execution_plan: [],
        execution_time_us: 0,
      },
    });

    await client.execute('mydb', { id: 1, queries: {} });

    const req = decodeLastRequest(socket);
    expect(req['op']).toBe('execute');
    expect(req['query_version']).toBe(CURRENT_QUERY_LANG_VERSION);
    // Sanity: assert against the constant, not a magic number, so the test
    // does not silently go stale if the constant changes. AND assert it is
    // not the old hardcoded 1.
    expect(req['query_version']).not.toBe(1);
  });

  it('txBegin() sends query_version = CURRENT_QUERY_LANG_VERSION (not the stale 1)', async () => {
    const { client, socket } = await resumeV2Client();
    pushDbResponse(socket, 1, {
      kind: 'tx_opened',
      tx_handle: 42,
      snapshot_version: 7,
      isolation: 'snapshot',
    });

    await client.txBegin('mydb', 'main');

    const req = decodeLastRequest(socket);
    expect(req['op']).toBe('tx_begin');
    expect(req['query_version']).toBe(CURRENT_QUERY_LANG_VERSION);
    expect(req['query_version']).not.toBe(1);
  });

  it('txExecute() sends query_version = CURRENT_QUERY_LANG_VERSION (not the stale 1)', async () => {
    const { client, socket } = await resumeV2Client();
    pushDbResponse(socket, 1, {
      kind: 'tx_batch',
      response: {
        id: 1,
        results: {},
        execution_plan: [],
        execution_time_us: 0,
      },
    });

    await client.txExecute('mydb', 99, { id: 1, queries: {} });

    const req = decodeLastRequest(socket);
    expect(req['op']).toBe('tx_execute');
    expect(req['query_version']).toBe(CURRENT_QUERY_LANG_VERSION);
    expect(req['query_version']).not.toBe(1);
  });
});

// ─── Finding 2.1: typed error surface (ShamirDbError) ────────────────────────

describe('ShamirClient — typed DbResponse::Error surface (Finding 2.1)', () => {
  async function resumeV2Client(): Promise<{
    client: ShamirClient;
    socket: FakeSocket;
  }> {
    const { platform, socket } = platformWithSocket();
    socket.pushFrame(resumeOkFrame({ serverQueryVersion: 2 }));
    const client = await ShamirClient.resume(platform, makeResumeOpts());
    openedClients.push(client);
    return { client, socket };
  }

  it('rejects with a typed ShamirDbError carrying code + retryable=false for a fatal code', async () => {
    const { client, socket } = await resumeV2Client();
    // Server returns DbResponse::Error { kind:"error", code, message }.
    pushDbResponse(socket, 1, {
      kind: 'error',
      code: 'validation',
      message: 'unknown alias',
    });

    const err = await client
      .execute('mydb', { id: 1, queries: {} })
      .then(
        () => {
          throw new Error('expected rejection');
        },
        (e) => e,
      );

    expect(err).toBeInstanceOf(ShamirDbError);
    expect(err.code).toBe('validation');
    expect(err.retryable).toBe(false);
    // Message continuity: still carries the [code]: message form.
    expect(err.message).toContain('[validation]');
    expect(err.message).toContain('unknown alias');
  });

  it('classifies transient codes as retryable=true', async () => {
    const { client, socket } = await resumeV2Client();
    pushDbResponse(socket, 1, {
      kind: 'error',
      code: 'tx_conflict',
      message: 'write-write conflict',
    });

    const err = await client
      .execute('mydb', { id: 1, queries: {} })
      .then(
        () => {
          throw new Error('expected rejection');
        },
        (e) => e,
      );

    expect(err).toBeInstanceOf(ShamirDbError);
    expect(err.code).toBe('tx_conflict');
    expect(err.retryable).toBe(true);
  });
});

// ─── Finding 2.2: request / connect timeouts ─────────────────────────────────

describe('ShamirClient — request/connect timeouts (Finding 2.2)', () => {
  it('rejects a never-answered request with a retryable ShamirTimeoutError', async () => {
    vi.useFakeTimers();
    try {
      const { platform, socket } = platformWithSocket();
      socket.pushFrame(resumeOkFrame({ serverQueryVersion: 2 }));
      const client = await ShamirClient.resume(platform, {
        ...makeResumeOpts(),
        requestTimeoutMs: 50,
      });
      openedClients.push(client);

      // ping() never gets a response — the client-side deadline must fire.
      const p = client.ping();
      // Attach a catch synchronously so the eventual rejection is not
      // "unhandled" while we advance timers.
      const settled = p.then(
        () => {
          throw new Error('expected timeout rejection');
        },
        (e) => e,
      );

      await vi.advanceTimersByTimeAsync(60);
      const err = await settled;

      expect(err).toBeInstanceOf(ShamirTimeoutError);
      expect(err.code).toBe('client_timeout');
      expect(err.retryable).toBe(true);
      expect(err.phase).toBe('request');
    } finally {
      vi.useRealTimers();
    }
  });

  it('rejects a hanging connect with a ShamirTimeoutError(phase=connect)', async () => {
    vi.useFakeTimers();
    try {
      // openSocket never resolves → the connect-timeout race must reject.
      const platform: Platform = {
        hmacSha256: () => new Uint8Array(32),
        sha256: () => new Uint8Array(32),
        randomBytes: (n) => new Uint8Array(n),
        timingSafeEqual: (a, b) => a.length === b.length,
        argon2id: async () => new Uint8Array(32),
        openSocket: () => new Promise<Socket>(() => {}), // never resolves
      };

      const attempt = ShamirClient.resume(platform, {
        ...makeResumeOpts(),
        connectTimeoutMs: 40,
      }).then(
        () => {
          throw new Error('expected connect timeout');
        },
        (e) => e,
      );

      await vi.advanceTimersByTimeAsync(50);
      const err = await attempt;

      expect(err).toBeInstanceOf(ShamirTimeoutError);
      expect(err.phase).toBe('connect');
      expect(err.retryable).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});
