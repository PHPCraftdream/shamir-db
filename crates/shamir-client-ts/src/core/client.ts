/**
 * ShamirClient — platform-agnostic core.
 *
 * Constructed with a `Platform`; all platform specifics (crypto, sockets)
 * are delegated there. No `node:crypto`, `ws`, or WebCrypto imports here.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Platform } from './platform.js';
import type { ConnectOptions, ResumeOptions } from './types/index.js';
import type { BatchResponse, TransactionInfo } from './types/batch.js';
import { WsFramer, encode, decode } from './framing.js';
import { runHandshake } from './protocol.js';
import { signCanonical } from './hmac.js';
import { Db } from './db.js';
import { SubscriptionRouter } from './subscription-router.js';
import type { PushEnvelope } from './types/subscribe.js';

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

/** Pending request slot awaiting a server response. */
interface PendingRequest {
  resolve: (v: Record<string, unknown>) => void;
  reject: (e: Error) => void;
}

export class ShamirClient {
  private readonly platform: Platform;
  private readonly framer: WsFramer;
  private readonly _sessionId: Uint8Array;
  private readonly _serverPubKey: Uint8Array;
  private readonly _expiresAtNs: bigint;
  private readonly _resumptionTicket: Uint8Array | undefined;
  private readonly _resumptionExpiresAtNs: bigint | undefined;
  private readonly subscriptionRouter = new SubscriptionRouter();
  private nextRequestId = 1;

  /**
   * In-flight requests keyed by `rid`. Concurrent callers each get their own
   * slot; the readLoop resolves/rejects them in completion order (matching the
   * server's multiplexed response order — not send order).
   */
  private readonly pending = new Map<number, PendingRequest>();

  /**
   * Optional hook called for frames that carry neither a known `rid` nor a
   * fatal (rid-less) error. Designed as the entry point for future server-push
   * / subscription frames. By default such frames are silently dropped.
   */
  onUnroutedFrame?: (frame: Record<string, unknown>) => void;

  private constructor(
    platform: Platform,
    framer: WsFramer,
    sessionId: Uint8Array,
    serverPubKey: Uint8Array,
    expiresAtNs: bigint,
    resumptionTicket?: Uint8Array,
    resumptionExpiresAtNs?: bigint,
  ) {
    this.platform = platform;
    this.framer = framer;
    this._sessionId = sessionId;
    this._serverPubKey = serverPubKey;
    this._expiresAtNs = expiresAtNs;
    this._resumptionTicket = resumptionTicket;
    this._resumptionExpiresAtNs = resumptionExpiresAtNs;

    // Start the persistent read loop immediately. The loop is the sole consumer
    // of framer.recv(); it routes each incoming frame to the matching pending
    // slot by rid.
    void this.readLoop();
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
      const {
        sessionId,
        serverPubKey,
        expiresAtNs,
        resumptionTicket,
        resumptionExpiresAtNs,
      } = await runHandshake(
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
        resumptionTicket,
        resumptionExpiresAtNs,
      );
    } catch (e) {
      await framer.close();
      throw e;
    }
  }

  /**
   * Fast reconnection using a resumption ticket from a previous session.
   *
   * Opens a new WS connection and sends a resume frame (ticket + fresh nonce)
   * instead of running the full SCRAM handshake. The server validates the
   * ticket and issues a new session. Resolves with a fully authenticated
   * {@link ShamirClient} on success.
   *
   * Throws (and closes the socket) if the server rejects the ticket or the
   * response is malformed.
   */
  static async resume(
    platform: Platform,
    opts: ResumeOptions,
  ): Promise<ShamirClient> {
    const origin = opts.origin ?? `https://${opts.host}`;
    const url = `wss://${opts.host}:${opts.port}/shamir/v1/browser`;
    const socket = await platform.openSocket(url, {
      rejectUnauthorized: opts.tls?.rejectUnauthorized ?? true,
      origin,
    });
    const framer = new WsFramer(socket);

    try {
      const clientNonce = platform.randomBytes(32);

      // Wire: { ticket, client_nonce, binding_mode: 0x02 }
      framer.send(
        encode({
          ticket: opts.ticket,
          client_nonce: clientNonce,
          binding_mode: 0x02,
        }),
      );

      // Server responds: { session_id, expires_at_ns, resumption_ticket?,
      //                    resumption_expires_at_ns? }
      const rawBytes = await framer.recv();
      const resp = decode(rawBytes) as Record<string, unknown>;

      if (typeof resp.error === 'string') {
        throw new Error(`resume rejected: ${resp.error}`);
      }

      const sessionId = resp.session_id;
      if (!(sessionId instanceof Uint8Array) || sessionId.length !== 32) {
        throw new Error('resume response: session_id must be 32 bytes');
      }

      const expiresAtNs = BigInt(
        resp.expires_at_ns as number | bigint,
      );

      const resumptionTicket =
        resp.resumption_ticket instanceof Uint8Array
          ? resp.resumption_ticket
          : undefined;
      const resumptionExpiresAtNs =
        resp.resumption_expires_at_ns !== undefined &&
        resp.resumption_expires_at_ns !== null
          ? BigInt(resp.resumption_expires_at_ns as number | bigint)
          : undefined;

      return new ShamirClient(
        platform,
        framer,
        sessionId,
        opts.serverPubKey,
        expiresAtNs,
        resumptionTicket,
        resumptionExpiresAtNs,
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
   * Resumption ticket issued by the server, if any. Present only when the
   * server returned one (either at `connect()` or `resume()` time).
   * Pass to {@link ShamirClient.resume} for fast reconnection.
   */
  resumptionTicket(): Uint8Array | undefined {
    return this._resumptionTicket;
  }

  /**
   * Absolute expiry of the resumption ticket (unix nanoseconds), or
   * `undefined` if no ticket was issued.
   */
  resumptionExpiresAtNs(): bigint | undefined {
    return this._resumptionExpiresAtNs;
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
   * Persistent read loop — the sole consumer of `framer.recv()`.
   *
   * Routing rules for each decoded envelope:
   *   • `error` field present AND `rid` present → reject that pending request.
   *   • `error` field present AND `rid` absent → fatal protocol error; reject
   *     all pending requests and stop the loop.
   *   • `res` bytes present AND `rid` present → resolve that pending request.
   *   • Neither routable → call `onUnroutedFrame` (server-push / subscription
   *     frames); default is silent drop.
   *
   * On socket close (`framer.recv()` rejects): every pending request is
   * rejected with the close error and the loop terminates.
   */
  private async readLoop(): Promise<void> {
    for (;;) {
      let frameBytes: Uint8Array;
      try {
        frameBytes = await this.framer.recv();
      } catch (err) {
        // Socket closed or errored — reject all pending and stop.
        const closeErr =
          err instanceof Error ? err : new Error(String(err));
        this.rejectAll(closeErr);
        return;
      }

      let frame: Record<string, unknown>;
      try {
        frame = decode(frameBytes) as Record<string, unknown>;
      } catch (err) {
        // Malformed frame — treat as fatal.
        const parseErr =
          err instanceof Error ? err : new Error(String(err));
        this.rejectAll(parseErr);
        return;
      }

      const rid = typeof frame.rid === 'number' ? frame.rid : undefined;
      const errorMsg =
        typeof frame.error === 'string' ? frame.error : undefined;

      if (errorMsg !== undefined) {
        if (rid !== undefined) {
          // Request-scoped protocol error — reject only this request.
          const waiter = this.pending.get(rid);
          if (waiter) {
            this.pending.delete(rid);
            waiter.reject(new Error(`protocol error: ${errorMsg}`));
          }
          // If rid is unknown, the server sent an unsolicited error frame —
          // fall through to onUnroutedFrame so it isn't silently lost.
          else {
            this.onUnroutedFrame?.(frame);
          }
        } else {
          // Fatal protocol error — no rid to route to; tear down everything.
          this.rejectAll(new Error(`protocol error: ${errorMsg}`));
          return;
        }
        continue;
      }

      if (rid !== undefined) {
        const waiter = this.pending.get(rid);
        if (waiter) {
          this.pending.delete(rid);
          if (frame.res instanceof Uint8Array) {
            let dbResponse: Record<string, unknown>;
            try {
              dbResponse = decode(frame.res) as Record<string, unknown>;
            } catch (err) {
              waiter.reject(
                err instanceof Error ? err : new Error(String(err)),
              );
              continue;
            }
            if (dbResponse.kind === 'error') {
              waiter.reject(
                new Error(
                  `db error [${(dbResponse.code as string) ?? 'unknown'}]: ${
                    (dbResponse.message as string) ?? ''
                  }`,
                ),
              );
            } else {
              waiter.resolve(dbResponse);
            }
          } else {
            waiter.reject(
              new Error('response envelope missing `res` bytes'),
            );
          }
          continue;
        }
        // rid present but not in pending — unrouted (e.g. late/stale push).
      }

      // No rid, no error — server-push or subscription frame.
      if (typeof frame.push === 'string') {
        this.subscriptionRouter.route(frame as unknown as PushEnvelope);
        continue;
      }
      this.onUnroutedFrame?.(frame);
    }
  }

  /** Reject every pending request with `err` and clear the map. */
  private rejectAll(err: Error): void {
    for (const waiter of this.pending.values()) {
      waiter.reject(err);
    }
    this.pending.clear();
  }

  /**
   * Round-trip one `DbRequest` and return the decoded `DbResponse` object.
   *
   * Calls are CONCURRENT: each request is assigned a unique `rid` and
   * registered in the pending map before the frame is sent (eliminating the
   * race between a fast server response and registration). The persistent
   * `readLoop` demultiplexes responses by `rid`, so multiple overlapping
   * round-trips (`Promise.all([db.run(a), db.run(b)])`) proceed in parallel
   * and resolve in server-completion order — not send order.
   *
   * On send failure the pending slot is removed immediately and the returned
   * promise rejects with the send error.
   */
  private sendDbRequest(req: object): Promise<Record<string, unknown>> {
    const rid = this.nextRequestId++;

    return new Promise<Record<string, unknown>>((resolve, reject) => {
      // Register BEFORE sending to avoid missing a fast response.
      this.pending.set(rid, { resolve, reject });

      let envelope: Uint8Array;
      try {
        // Outer request envelope. `req` is opaque msgpack bytes (serde_bytes)
        // carrying the internally-tagged DbRequest (tag = "op").
        envelope = encode({ sid: this._sessionId, rid, req: encode(req) });
      } catch (err) {
        this.pending.delete(rid);
        reject(err instanceof Error ? err : new Error(String(err)));
        return;
      }

      try {
        this.framer.send(envelope);
      } catch (err) {
        // Socket already closed — remove the slot and reject immediately.
        this.pending.delete(rid);
        reject(err instanceof Error ? err : new Error(String(err)));
      }
    });
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

  /** Access the subscription router for registering/unregistering push handlers. */
  get subscriptions(): SubscriptionRouter {
    return this.subscriptionRouter;
  }

  /** Close the WS (normal closure). Idempotent. */
  async close(): Promise<void> {
    await this.framer.close();
  }
}
