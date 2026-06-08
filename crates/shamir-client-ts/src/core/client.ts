/**
 * ShamirClient — platform-agnostic core.
 *
 * Constructed with a `Platform`; all platform specifics (crypto, sockets)
 * are delegated there. No `node:crypto`, `ws`, or WebCrypto imports here.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Platform } from './platform.js';
import type { ConnectOptions } from './types/index.js';
import { WsFramer, encode, decode } from './framing.js';
import { runHandshake } from './protocol.js';

export class ShamirClient {
  private readonly framer: WsFramer;
  private readonly _sessionId: Uint8Array;
  private readonly _serverPubKey: Uint8Array;
  private readonly _expiresAtNs: bigint;
  private nextRequestId = 1;

  private constructor(
    framer: WsFramer,
    sessionId: Uint8Array,
    serverPubKey: Uint8Array,
    expiresAtNs: bigint,
  ) {
    this.framer = framer;
    this._sessionId = sessionId;
    this._serverPubKey = serverPubKey;
    this._expiresAtNs = expiresAtNs;
  }

  /**
   * Open a WS connection, run the SCRAM handshake, and return an
   * authenticated client. Platform provides crypto + socket.
   */
  static async connect(
    platform: Platform,
    opts: ConnectOptions,
  ): Promise<ShamirClient> {
    const origin = opts.origin ?? `https://${opts.host}`;
    const url = `wss://${opts.host}:${opts.port}/shamir/v1/browser`;
    const socket = await platform.openSocket(url, {
      rejectUnauthorized: opts.tls?.rejectUnauthorized ?? true,
      origin,
    });
    const framer = new WsFramer(socket);

    try {
      const { sessionId, serverPubKey, expiresAtNs } = await runHandshake(
        platform,
        framer,
        opts.username,
        opts.password,
      );
      return new ShamirClient(framer, sessionId, serverPubKey, expiresAtNs);
    } catch (e) {
      await framer.close();
      throw e;
    }
  }

  /** 32-byte session id assigned by the server. */
  sessionId(): Uint8Array {
    return this._sessionId;
  }

  /** Raw 32-byte Ed25519 server public key from `auth_ok`. */
  serverPubKeyPin(): Uint8Array {
    return this._serverPubKey;
  }

  /** Absolute session expiry (unix nanoseconds). */
  expiresAtNs(): bigint {
    return this._expiresAtNs;
  }

  /**
   * Health-check ping — zero DB cost. Returns the decoded `DbResponse`.
   * Uses `DbRequest::Ping` (internally-tagged enum with `op: "ping"`).
   */
  async ping(): Promise<object> {
    const rid = this.nextRequestId++;
    const reqBody = encode({ op: 'ping' });
    const envelope = encode({
      sid: this._sessionId,
      rid,
      req: reqBody,
    });
    this.framer.send(envelope);

    const respBytes = await this.framer.recv();
    const resp = decode(respBytes) as {
      rid?: number;
      res?: Uint8Array;
      error?: string;
    };
    if (typeof resp.error === 'string') {
      throw new Error(`protocol error: ${resp.error}`);
    }
    if (!(resp.res instanceof Uint8Array)) {
      throw new Error('response envelope missing `res` bytes');
    }
    return decode(resp.res) as object;
  }

  /**
   * Execute a BatchRequest against `db`. Returns the decoded `DbResponse`.
   * Throws on transport, protocol, or DB-layer errors.
   */
  async execute(db: string, batch: object): Promise<object> {
    const rid = this.nextRequestId++;
    // Inner DB request — internally-tagged enum (tag = "op").
    const reqBody = encode({
      op: 'execute',
      query_version: 1,
      db,
      batch,
    });
    // Outer request envelope. `req` is opaque msgpack bytes (serde_bytes).
    const envelope = encode({
      sid: this._sessionId,
      rid,
      req: reqBody,
    });
    this.framer.send(envelope);

    const respBytes = await this.framer.recv();
    const resp = decode(respBytes) as {
      rid?: number;
      res?: Uint8Array;
      error?: string;
    };

    if (typeof resp.error === 'string') {
      throw new Error(`protocol error: ${resp.error}`);
    }
    if (resp.rid !== undefined && resp.rid !== rid) {
      throw new Error(`request id mismatch: sent ${rid}, got ${resp.rid}`);
    }
    if (!(resp.res instanceof Uint8Array)) {
      throw new Error('response envelope missing `res` bytes');
    }
    const dbResponse = decode(resp.res) as {
      kind?: string;
      code?: string;
      message?: string;
    };
    if (dbResponse.kind === 'error') {
      throw new Error(
        `db error [${dbResponse.code ?? 'unknown'}]: ${dbResponse.message ?? ''}`,
      );
    }
    return dbResponse;
  }

  /** Close the WS (normal closure). Idempotent. */
  async close(): Promise<void> {
    await this.framer.close();
  }
}
