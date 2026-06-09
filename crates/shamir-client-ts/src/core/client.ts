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
import type { BatchResponse, TransactionInfo } from './types/batch.js';
import { WsFramer, encode, decode } from './framing.js';
import { runHandshake } from './protocol.js';
import { signCanonical } from './hmac.js';
import { Db } from './db.js';

/** Result of {@link ShamirClient.txBegin} (`DbResponse::TxOpened`). */
export interface TxOpened {
  /** Opaque handle for subsequent txExecute/txCommit/txRollback. */
  tx_handle: number;
  /** MVCC version the transaction's snapshot reads at. */
  snapshot_version: number;
  /** Effective isolation (`"snapshot"` | `"serializable"`). */
  isolation: string;
}

/** Result of {@link ShamirClient.createScramUser} (`DbResponse::UserCreated`). */
export interface ScramUserCreated {
  /** Echoed user name (post-normalisation). */
  name: string;
  /** Stable 16-byte user id assigned by the directory. */
  user_id: Uint8Array;
}

export class ShamirClient {
  private readonly platform: Platform;
  private readonly framer: WsFramer;
  private readonly _sessionId: Uint8Array;
  private readonly _serverPubKey: Uint8Array;
  private readonly _expiresAtNs: bigint;
  private nextRequestId = 1;
  /** Serialises wire round-trips (see {@link sendDbRequest}). */
  private sendQueue: Promise<unknown> = Promise.resolve();

  private constructor(
    platform: Platform,
    framer: WsFramer,
    sessionId: Uint8Array,
    serverPubKey: Uint8Array,
    expiresAtNs: bigint,
  ) {
    this.platform = platform;
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
      return new ShamirClient(
        platform,
        framer,
        sessionId,
        serverPubKey,
        expiresAtNs,
      );
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
   * Hex HMAC-SHA256 tag over `canonical`, keyed by this session's derived
   * HMAC key (`SHA256("shamir-db hmac key v1\0" || session_id)`). The
   * "did-you-mean-it" intent tag the server requires on destructive
   * `drop_*` / migration ops. Pair with the `hmac.canonical*` builders.
   */
  hmacTagHex(canonical: Uint8Array): string {
    return signCanonical(this.platform, this._sessionId, canonical);
  }

  /**
   * Round-trip one `DbRequest` and return the decoded `DbResponse` object.
   *
   * Calls are SERIALISED: `WsFramer` delivers responses FIFO (it does not
   * match by `rid`), so two overlapping round-trips would cross-resolve.
   * Each call is chained after the previous one's completion — concurrent
   * callers (e.g. `Promise.all([db.run(a), db.run(b)])`) simply queue and run
   * one at a time over the wire. The chain advances on both success and
   * failure so one rejected request never wedges the queue.
   */
  private sendDbRequest(req: object): Promise<Record<string, unknown>> {
    const run = this.sendQueue.then(() => this.doSendDbRequest(req));
    this.sendQueue = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  /** The actual single round-trip; serialised by {@link sendDbRequest}. */
  private async doSendDbRequest(
    req: object,
  ): Promise<Record<string, unknown>> {
    const rid = this.nextRequestId++;
    // Outer request envelope. `req` is opaque msgpack bytes (serde_bytes)
    // carrying the internally-tagged DbRequest (tag = "op").
    const envelope = encode({ sid: this._sessionId, rid, req: encode(req) });
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
    const dbResponse = decode(resp.res) as Record<string, unknown>;
    if (dbResponse.kind === 'error') {
      throw new Error(
        `db error [${(dbResponse.code as string) ?? 'unknown'}]: ${
          (dbResponse.message as string) ?? ''
        }`,
      );
    }
    return dbResponse;
  }

  /**
   * Health-check ping — zero DB cost. Returns the decoded `DbResponse`
   * (`{ kind: "pong" }`). Uses `DbRequest::Ping`.
   */
  async ping(): Promise<object> {
    return this.sendDbRequest({ op: 'ping' });
  }

  /**
   * Execute a BatchRequest against `db`. Returns the unwrapped
   * {@link BatchResponse} (the server's `DbResponse::Batch.response`), so
   * callers read `.results` / `.execution_plan` / `.transaction` directly —
   * matching the napi binding's ergonomics. Throws on transport, protocol,
   * or DB-layer (`kind:"error"`) failures.
   */
  async execute(db: string, batch: object): Promise<BatchResponse> {
    const r = await this.sendDbRequest({
      op: 'execute',
      query_version: 1,
      db,
      batch,
    });
    // DbResponse::Batch is `{ kind: "batch", response: BatchResponse }` —
    // unwrap the envelope so callers get the BatchResponse directly.
    if (r.kind === 'batch' && r.response !== undefined) {
      return r.response as BatchResponse;
    }
    throw new Error(
      `unexpected DbResponse kind for execute: ${(r.kind as string) ?? 'missing'}`,
    );
  }

  // ── Interactive (multi-call) transactions ─────────────────────────

  /**
   * Open an interactive transaction scoped to a single `repo`. Returns the
   * minted handle + the snapshot version it reads at. `isolation` is
   * `"snapshot"` (default) | `"serializable"`.
   */
  async txBegin(
    db: string,
    repo: string,
    isolation?: 'snapshot' | 'serializable',
  ): Promise<TxOpened> {
    const r = await this.sendDbRequest({
      op: 'tx_begin',
      query_version: 1,
      db,
      repo,
      ...(isolation !== undefined ? { isolation } : {}),
    });
    if (r.kind !== 'tx_opened') {
      throw new Error(`unexpected DbResponse kind for tx_begin: ${r.kind}`);
    }
    return {
      tx_handle: r.tx_handle as number,
      snapshot_version: r.snapshot_version as number,
      isolation: r.isolation as string,
    };
  }

  /**
   * Run a batch inside an open interactive transaction. State accumulates in
   * the parked transaction (no commit). Returns the {@link BatchResponse}
   * (its `.transaction` stays null — there is no per-call commit outcome yet).
   */
  async txExecute(
    db: string,
    txHandle: number,
    batch: object,
  ): Promise<BatchResponse> {
    const r = await this.sendDbRequest({
      op: 'tx_execute',
      query_version: 1,
      db,
      tx_handle: txHandle,
      batch,
    });
    if (r.kind === 'tx_batch' && r.response !== undefined) {
      return r.response as BatchResponse;
    }
    throw new Error(`unexpected DbResponse kind for tx_execute: ${r.kind}`);
  }

  /**
   * Commit an open interactive transaction (runs the full commit pipeline).
   * Returns the {@link TransactionInfo} (committed or aborted).
   */
  async txCommit(db: string, txHandle: number): Promise<TransactionInfo> {
    const r = await this.sendDbRequest({
      op: 'tx_commit',
      db,
      tx_handle: txHandle,
    });
    if (r.kind === 'tx_committed' && r.transaction !== undefined) {
      return r.transaction as TransactionInfo;
    }
    throw new Error(`unexpected DbResponse kind for tx_commit: ${r.kind}`);
  }

  /** Roll back (abort) an open interactive transaction. */
  async txRollback(db: string, txHandle: number): Promise<void> {
    const r = await this.sendDbRequest({
      op: 'tx_rollback',
      db,
      tx_handle: txHandle,
    });
    if (r.kind !== 'tx_rolled_back') {
      throw new Error(`unexpected DbResponse kind for tx_rollback: ${r.kind}`);
    }
  }

  // ── SCRAM user provisioning ───────────────────────────────────────

  /**
   * Create a SCRAM-authenticatable user (one that can log in over the wire).
   * Requires a superuser session. The server runs Argon2id with its KDF
   * defaults and writes the durable user record. `roles: ["superuser"]`
   * grants admin powers; other strings are app-defined.
   */
  async createScramUser(
    name: string,
    password: string,
    roles: string[] = [],
  ): Promise<ScramUserCreated> {
    const r = await this.sendDbRequest({
      op: 'create_scram_user',
      name,
      password,
      roles,
    });
    if (r.kind === 'user_created') {
      return { name: r.name as string, user_id: r.user_id as Uint8Array };
    }
    throw new Error(
      `unexpected DbResponse kind for create_scram_user: ${r.kind}`,
    );
  }

  /**
   * Return a bound {@link Db} handle for `name`. Subsequent calls via
   * the handle (`db.run(...)`, `db.query(...)`, `db.batch(...)`, etc.)
   * automatically thread the client and database name — no re-threading.
   */
  db(name: string): Db {
    return new Db(this, name);
  }

  /** Close the WS (normal closure). Idempotent. */
  async close(): Promise<void> {
    await this.framer.close();
  }
}
