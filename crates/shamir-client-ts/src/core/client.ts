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
import {
  DEFAULT_REQUEST_TIMEOUT_MS,
  DEFAULT_CONNECT_TIMEOUT_MS,
} from './types/connection.js';
import type { BatchResponse, TransactionInfo } from './types/batch.js';
import type { WireValue } from './types/write.js';
import { ShamirDbError, ShamirTimeoutError } from './errors.js';
import { WsFramer, encode, decode } from './framing.js';
import {
  runHandshake,
  asBytes,
  RESUME_OK_SESSION_ID,
  RESUME_OK_EXPIRES_AT_NS,
  RESUME_OK_RESUMPTION_TICKET,
  RESUME_OK_RESUMPTION_EXPIRES_AT_NS,
  RESUME_OK_SERVER_QUERY_VERSION,
} from './protocol.js';
import { CURRENT_QUERY_LANG_VERSION } from './scram.js';
import { signCanonical } from './hmac.js';
import { Db } from './db.js';
import { SubscriptionRouter } from './subscription-router.js';
import type { PushEnvelope } from './types/subscribe.js';
import { InternerCacheRegistry } from './field-map.js';
import type { InternerDelta } from './field-map.js';
import {
  collectFieldNames,
  encodeRecordIdMsgpack,
  qvHasFnMarker,
  deinternResponse,
} from './interner-ops.js';

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
  /**
   * Per-request timeout handle (Finding 2.2). Cleared when the response
   * arrives / the request is rejected. `undefined` when timeouts are disabled
   * (`requestTimeoutMs === 0`).
   */
  timer?: ReturnType<typeof setTimeout>;
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
  /** Per-`(db, repo)` field-name ↔ id cache (Stage 5-wire interner). */
  private readonly _internerCache = new InternerCacheRegistry();
  /**
   * Max query-language version advertised by the server in auth_ok / resume_ok.
   * 0 = pre-v2 server (no id-keyed write path). Set once at connect/resume.
   */
  private readonly _serverQueryVersion: number;
  /**
   * Per-request deadline in ms (Finding 2.2); `0` disables it. A pending
   * request with no response within this budget rejects with a
   * {@link ShamirTimeoutError}.
   */
  private readonly _requestTimeoutMs: number;
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
    serverQueryVersion?: number,
    requestTimeoutMs?: number,
  ) {
    this.platform = platform;
    this.framer = framer;
    this._sessionId = sessionId;
    this._serverPubKey = serverPubKey;
    this._expiresAtNs = expiresAtNs;
    this._resumptionTicket = resumptionTicket;
    this._resumptionExpiresAtNs = resumptionExpiresAtNs;
    this._serverQueryVersion = serverQueryVersion ?? 0;
    this._requestTimeoutMs = requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;

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
    const socket = await withConnectTimeout(
      platform.openSocket(url, {
        rejectUnauthorized: opts.tls?.rejectUnauthorized ?? true,
        origin,
      }),
      opts.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS,
    );
    const framer = new WsFramer(socket);

    try {
      const {
        sessionId,
        serverPubKey,
        expiresAtNs,
        resumptionTicket,
        resumptionExpiresAtNs,
        serverQueryVersion,
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
        serverQueryVersion,
        opts.requestTimeoutMs,
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
    const socket = await withConnectTimeout(
      platform.openSocket(url, {
        rejectUnauthorized: opts.tls?.rejectUnauthorized ?? true,
        origin,
      }),
      opts.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS,
    );
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

      // Server responds with `ResumeOkWire` (crates/shamir-server/src/
      // connection/wire.rs:80-92): a plain struct with NO `#[serde(rename_all)]`,
      // emitted via `rmp_serde::to_vec` (not `to_vec_named`) → a POSITIONAL
      // msgpack ARRAY in declaration order:
      //   [0] session_id (bytes, 32)
      //   [1] expires_at_ns (u64)
      //   [2] resumption_ticket (bytes; empty Vec when no ticket)
      //   [3] resumption_expires_at_ns (u64; 0 when no ticket)
      //   [4] server_query_version (u8; 0 when absent)
      // This mirrors how `runHandshake` decodes `auth_ok` positionally.
      //
      // On resume REJECTION the server actually shuts the connection down
      // (handshake.rs:602-606), so the client observes a socket close rather
      // than an error frame. The array-vs-error-map guard below is kept
      // defensive (and mirrors runHandshake's auth_ok error-map check): a
      // map with an `error` string is still surfaced as a thrown rejection.
      const rawBytes = await framer.recv();
      const resp = decode(rawBytes) as unknown[] | { error?: string };

      if (!Array.isArray(resp)) {
        const errMap = resp as { error?: string };
        if (typeof errMap.error === 'string') {
          throw new Error(`resume rejected: ${errMap.error}`);
        }
        throw new Error('resume_ok: unexpected non-array response');
      }

      const sessionId = asBytes(
        resp[RESUME_OK_SESSION_ID],
        'resume response: session_id',
      );
      if (sessionId.length !== 32) {
        throw new Error('resume response: session_id must be 32 bytes');
      }

      const expiresAtNs = BigInt(
        resp[RESUME_OK_EXPIRES_AT_NS] as number | bigint,
      );

      // Optional trailing fields. The server always emits indices 2 and 3
      // (even when no ticket is issued: empty Vec and 0u64 respectively), but
      // a pre-v2 server that omits them yields a shorter array — read them
      // defensively, matching runHandshake's handling of the analogous
      // auth_ok trailing fields.
      // An empty/absent resumption_ticket means "no ticket" → undefined.
      const ticketRaw = resp[RESUME_OK_RESUMPTION_TICKET];
      const resumptionTicket =
        ticketRaw instanceof Uint8Array && ticketRaw.length > 0
          ? ticketRaw
          : undefined;
      const resumptionExpiresRaw = resp[RESUME_OK_RESUMPTION_EXPIRES_AT_NS];
      const resumptionExpiresAtNs =
        resumptionTicket !== undefined &&
        resumptionExpiresRaw !== undefined &&
        resumptionExpiresRaw !== null
          ? BigInt(resumptionExpiresRaw as number | bigint)
          : undefined;

      // Max query-language version the server supports (u8, default 0).
      // Absent/malformed → 0 (a pre-v2 server omitting the field is a valid
      // legacy case), mirroring runHandshake's handling of the analogous
      // `auth_ok.server_query_version` positional field (index 7 there).
      const serverQueryVersion =
        typeof resp[RESUME_OK_SERVER_QUERY_VERSION] === 'number'
          ? resp[RESUME_OK_SERVER_QUERY_VERSION]
          : 0;

      return new ShamirClient(
        platform,
        framer,
        sessionId,
        opts.serverPubKey,
        expiresAtNs,
        resumptionTicket,
        resumptionExpiresAtNs,
        serverQueryVersion,
        opts.requestTimeoutMs,
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
          const waiter = this.takePending(rid);
          if (waiter) {
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
        const waiter = this.takePending(rid);
        if (waiter) {
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
              // Finding 2.1: surface a typed ShamirDbError carrying the
              // server's `code` + a `retryable` classification, instead of an
              // opaque interpolated-string Error the caller has to regex.
              waiter.reject(
                new ShamirDbError(
                  (dbResponse.code as string) ?? 'unknown',
                  (dbResponse.message as string) ?? '',
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

  /**
   * Remove and return the pending slot for `rid`, clearing its timeout timer
   * (Finding 2.2) so a settled request never fires a spurious timeout.
   */
  private takePending(rid: number): PendingRequest | undefined {
    const waiter = this.pending.get(rid);
    if (waiter) {
      this.pending.delete(rid);
      if (waiter.timer !== undefined) clearTimeout(waiter.timer);
    }
    return waiter;
  }

  /** Reject every pending request with `err`, clearing timers and the map. */
  private rejectAll(err: Error): void {
    for (const waiter of this.pending.values()) {
      if (waiter.timer !== undefined) clearTimeout(waiter.timer);
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
      const slot: PendingRequest = { resolve, reject };
      this.pending.set(rid, slot);

      // Finding 2.2: arm a per-request deadline. Without this a Ping /
      // TxCommit / CreateScramUser (none bounded by the server's
      // `max_execution_time_secs`) or a lost response id would hang the caller
      // forever. On fire, reject with a typed, retryable timeout and drop the
      // slot; a late response then routes to nothing (harmless).
      if (this._requestTimeoutMs > 0) {
        slot.timer = setTimeout(() => {
          if (this.pending.get(rid) === slot) {
            this.pending.delete(rid);
            reject(new ShamirTimeoutError('request', this._requestTimeoutMs));
          }
        }, this._requestTimeoutMs);
        // Do not keep the Node event loop alive solely for this timer.
        (slot.timer as { unref?: () => void }).unref?.();
      }

      let envelope: Uint8Array;
      try {
        // Outer request envelope. `req` is opaque msgpack bytes (serde_bytes)
        // carrying the internally-tagged DbRequest (tag = "op").
        envelope = encode({ sid: this._sessionId, rid, req: encode(req) });
      } catch (err) {
        this.takePending(rid);
        reject(err instanceof Error ? err : new Error(String(err)));
        return;
      }

      try {
        this.framer.send(envelope);
      } catch (err) {
        // Socket already closed — remove the slot and reject immediately.
        this.takePending(rid);
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
   *
   * Stage 5-wire interner integration (ambient sync):
   * - Attaches `interner_epochs` from the client's cached epochs for `db`,
   *   so the server knows which entries to include in its delta.
   * - After receiving the response, merges any `interner_delta` entries into
   *   the per-repo FieldMap caches.
   */
  async execute(db: string, batch: object): Promise<BatchResponse> {
    // Attach ambient interner epochs so the server returns only the delta.
    const epochs = this._internerCache.allEpochs(db);
    const enrichedBatch: Record<string, unknown> =
      typeof batch === 'object' && batch !== null
        ? { ...(batch as Record<string, unknown>) }
        : { ...(batch as object) };
    if (Object.keys(epochs).length > 0) {
      // Convert bigint epochs to numbers for msgpack serialisation. The
      // epoch values are gap-free high-water marks and will fit in a JS
      // safe integer for any realistic number of fields (2^53 fields would
      // require a universe-sized interner). The framing encoder
      // (@msgpack/msgpack) does NOT encode BigInt by default — it throws
      // "Unrecognized object: [object BigInt]" — so we narrow to Number
      // here; the server reads u64 and accepts the msgpack uint just fine.
      const wireEpochs: Record<string, number> = {};
      for (const [repo, e] of Object.entries(epochs)) {
        wireEpochs[repo] = Number(e);
      }
      // Merge with any epochs already present (e.g. from executeWithTouch
      // which may include epoch-0 cold repos for delta bootstrapping).
      const existing = enrichedBatch['interner_epochs'] as
        | Record<string, number>
        | undefined;
      enrichedBatch['interner_epochs'] = existing
        ? { ...existing, ...wireEpochs }
        : wireEpochs;
    }

    const r = await this.sendDbRequest({
      op: 'execute',
      query_version: CURRENT_QUERY_LANG_VERSION,
      db,
      batch: enrichedBatch,
    });
    // DbResponse::Batch is `{ kind: "batch", response: BatchResponse }` —
    // unwrap the envelope so callers get the BatchResponse directly.
    if (r.kind === 'batch' && r.response !== undefined) {
      const response = r.response as BatchResponse;
      // Merge any interner_delta from the response into the local cache.
      this.mergeInternerDelta(db, response);
      return response;
    }
    throw new Error(
      `unexpected DbResponse kind for execute: ${(r.kind as string) ?? 'missing'}`,
    );
  }

  /**
   * Merge `response.interner_delta` into the per-repo FieldMap caches for
   * `db`. Called after every successful `execute` response.
   *
   * Monotonic: the FieldMap's `applyDelta` rejects stale epochs, so
   * out-of-order or duplicate deltas are harmless.
   */
  private mergeInternerDelta(db: string, response: BatchResponse): void {
    const delta = response.interner_delta;
    if (delta === undefined || delta === null) return;
    for (const [repo, repoDelta] of Object.entries(delta)) {
      const fm = this._internerCache.getOrCreate(db, repo);
      const normalised: InternerDelta = {
        epoch: BigInt(repoDelta.epoch),
        entries: repoDelta.entries.map(([id, name]) => [BigInt(id), name]),
      };
      fm.applyDelta(normalised);
    }
  }

  /**
   * Register field `names` against the server's interner for `(db, repo)`
   * and merge the returned `(name, id)` mappings into the local cache.
   *
   * Returns the resolved `{ name → id }` map in input order. Idempotent:
   * the server returns existing ids for already-interned names.
   *
   * Short-circuits when every name is already cached (no roundtrip).
   */
  async touchFields(
    db: string,
    repo: string,
    names: string[],
  ): Promise<Map<string, bigint>> {
    const fm = this._internerCache.getOrCreate(db, repo);
    const missing = fm.missingNames(names);

    if (missing.length > 0) {
      // Send an interner_touch op for the unknown names.
      const alias = '_ic_touch';
      const batch = {
        id: '_ic_touch',
        queries: {
          [alias]: { interner_touch: repo, names: missing },
        },
      };
      const resp = await this.execute(db, batch);
      // Parse the touch payload: server returns
      // `{ "interner_touch": "<repo>", "epoch": <u64>, "mappings": [[name, id], ...] }`
      const result = resp.results[alias];
      if (result !== undefined && result.records.length > 0) {
        const rec = result.records[0] as Record<string, unknown>;
        const epoch = rec['epoch'];
        const mappings = rec['mappings'];
        if (Array.isArray(mappings)) {
          for (const pair of mappings as unknown[]) {
            if (Array.isArray(pair) && pair.length === 2) {
              const name = pair[0] as string;
              const id = BigInt(pair[1] as number | bigint);
              fm.insertEntry(name, id);
            }
          }
        }
        if (epoch !== undefined && epoch !== null) {
          const bigEpoch = BigInt(epoch as number | bigint);
          if (bigEpoch > fm.epoch()) {
            // applyDelta with just the epoch bump
            fm.applyDelta({ epoch: bigEpoch, entries: [] });
          }
        }
      }
    }

    // Return resolved map for all requested names (both cached + freshly minted).
    const result = new Map<string, bigint>();
    for (const name of names) {
      const id = fm.getId(name);
      if (id !== undefined) {
        result.set(name, id);
      }
    }
    return result;
  }

  /**
   * Access the interner cache registry. Allows callers to read cached
   * field name ↔ id mappings without a roundtrip (cache-hit path only).
   */
  get internerCache(): InternerCacheRegistry {
    return this._internerCache;
  }

  /**
   * Max query-language version advertised by the server. `0` means pre-v2.
   */
  serverQueryVersion(): number {
    return this._serverQueryVersion;
  }

  /**
   * Execute a batch with automatic client-side field interning (Stage 5-wire).
   *
   * 1. Collects field names from INSERT/SET/UPDATE ops, grouped by repo.
   * 2. Touches unknown field names per repo (populates the FieldMap cache).
   * 3. On v2+ servers: encodes fully-literal INSERT records into id-keyed
   *    msgpack (`records_idmsgpack`); removes them from `values`. Records
   *    with `$fn` markers stay on `values` (server-side eval). Sets
   *    `result_encoding = "id"` so the server returns id-keyed rows.
   * 4. Calls `execute()`.
   * 5. De-interns any id-keyed result rows back to name-keyed objects.
   *
   * Mirrors the Rust `Client::execute_with_touch`.
   */
  async executeWithTouch(db: string, batch: object): Promise<BatchResponse> {
    const batchObj = batch as Record<string, unknown>;
    const queries = batchObj['queries'] as
      | Record<string, Record<string, unknown>>
      | undefined;
    if (!queries) {
      return this.execute(db, batch);
    }

    // Collect field names per repo (write ops only).
    const perRepo = new Map<string, string[]>();
    // Collect ALL repos referenced by any data op (read + write) for
    // de-interning the response.
    const allRepos = new Set<string>();

    for (const entry of Object.values(queries)) {
      collectFieldNames(entry, perRepo);
      // Extract repo from table-ref for all data ops.
      const repo = extractRepo(entry);
      if (repo !== undefined) {
        allRepos.add(repo);
      }
    }

    // Touch each repo's unknown fields. If touch fails (e.g. non-admin user
    // lacks permission for interner_touch), fall back to plain execute — the
    // id-on-wire path requires a warm cache so we skip it entirely.
    let touchOk = true;
    for (const [repo, names] of perRepo) {
      const unique = [...new Set(names)].sort();
      if (unique.length > 0) {
        try {
          await this.touchFields(db, repo, unique);
        } catch {
          touchOk = false;
          break;
        }
      }
    }

    if (!touchOk) {
      // Touch failed — fall back to plain execute (name-keyed, no id-on-wire).
      return this.execute(db, batch);
    }

    // Ensure a FieldMap exists for every referenced repo. Explicitly attach
    // interner_epochs (including epoch-0 cold repos) so the server returns
    // deltas even for repos the client has never seen before.
    const wireEpochs: Record<string, number> = {};
    for (const repo of allRepos) {
      const fm = this._internerCache.getOrCreate(db, repo);
      wireEpochs[repo] = Number(fm.epoch());
    }
    // Also include any already-warm repos from prior requests.
    const cached = this._internerCache.allEpochs(db);
    for (const [repo, e] of Object.entries(cached)) {
      if (!(repo in wireEpochs)) {
        wireEpochs[repo] = Number(e);
      }
    }
    if (Object.keys(wireEpochs).length > 0) {
      batchObj['interner_epochs'] = wireEpochs;
    }

    // v2 id-keyed write path.
    if (this._serverQueryVersion >= 2) {
      for (const entry of Object.values(queries)) {
        if ('insert_into' in entry && Array.isArray(entry['values'])) {
          const tableRef = entry['insert_into'];
          const repo = tableRefToRepo(tableRef);
          const fm = this._internerCache.getOrCreate(db, repo);
          const values = entry['values'] as WireValue[];
          const remaining: WireValue[] = [];
          const idmsgpackList: Uint8Array[] = [];

          for (const qv of values) {
            if (qvHasFnMarker(qv)) {
              remaining.push(qv);
            } else {
              const bytes = encodeRecordIdMsgpack(
                qv as Record<string, WireValue>,
                fm,
              );
              idmsgpackList.push(bytes);
            }
          }

          entry['values'] = remaining;
          if (idmsgpackList.length > 0) {
            entry['records_idmsgpack'] = idmsgpackList;
          }
        }
      }
      // Request id-keyed result rows only when no query-ref or sub-batch
      // dependencies exist — those rely on server-side intermediate results
      // staying name-keyed for path resolution ($param, queryRef).
      if (!batchHasRefs(queries)) {
        batchObj['result_encoding'] = 'id';
      }
    }

    const response = await this.execute(db, batch);

    // De-intern id-keyed result rows (no-op when result_encoding was not set).
    const repos = [...allRepos];
    return deinternResponse(this._internerCache, db, response, repos);
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
      query_version: CURRENT_QUERY_LANG_VERSION,
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
      query_version: CURRENT_QUERY_LANG_VERSION,
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

// ── Helpers (module-private) ──────────────────────────────────────────────

/**
 * Race an in-flight `openSocket` promise against a connect deadline (Finding
 * 2.2). If the socket resolves after the timeout already fired, it is closed so
 * no descriptor leaks. `timeoutMs <= 0` disables the bound (awaits the socket
 * directly).
 */
async function withConnectTimeout(
  socketPromise: Promise<import('./platform.js').Socket>,
  timeoutMs: number,
): Promise<import('./platform.js').Socket> {
  if (timeoutMs <= 0) return socketPromise;

  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(
      () => reject(new ShamirTimeoutError('connect', timeoutMs)),
      timeoutMs,
    );
    (timer as { unref?: () => void }).unref?.();
  });

  try {
    return await Promise.race([socketPromise, timeout]);
  } catch (e) {
    // If the socket eventually opens after we timed out, close it to avoid a
    // leaked connection.
    void socketPromise.then((s) => void s.close()).catch(() => {});
    throw e;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

/** Extract the repo string from a table-ref wire value. */
function tableRefToRepo(tableRef: unknown): string {
  if (Array.isArray(tableRef) && tableRef.length >= 1) {
    return String(tableRef[0]);
  }
  // Bare string = default repo "main".
  return 'main';
}

/**
 * Extract the repo from any data-op entry (INSERT/UPDATE/SET/DELETE/SELECT).
 * Returns `undefined` for non-data ops.
 */
function extractRepo(entry: Record<string, unknown>): string | undefined {
  if ('insert_into' in entry) return tableRefToRepo(entry['insert_into']);
  if ('update' in entry) return tableRefToRepo(entry['update']);
  if ('set' in entry && !('key' in entry)) return undefined; // not a SetOp
  if ('set' in entry && 'key' in entry) return tableRefToRepo(entry['set']);
  if ('delete_from' in entry) return tableRefToRepo(entry['delete_from']);
  if ('from' in entry) {
    // ReadQuery: `from` is a TableRefWire — either a bare table name
    // (default repo "main"), a `[repo, table]` pair, OR a legacy form
    // `{ from: table, repo?: string }`. Resolve the repo from whichever
    // form is present so the interner cache is keyed correctly.
    const fromVal = entry['from'];
    if (Array.isArray(fromVal)) {
      return tableRefToRepo(fromVal);
    }
    const repo = entry['repo'];
    return typeof repo === 'string' ? repo : 'main';
  }
  return undefined;
}

/**
 * Detect if any query entry contains query-ref (`$query_ref`) or sub-batch
 * (`batch`) dependencies. When present, the server resolves intermediate
 * results by name-keyed field paths — requesting `result_encoding = 'id'`
 * would break those resolutions.
 */
function batchHasRefs(
  queries: Record<string, Record<string, unknown>>,
): boolean {
  for (const entry of Object.values(queries)) {
    if ('batch' in entry) return true;
    if (hasQueryRef(entry)) return true;
  }
  return false;
}

/** Recursively check if a value contains `{ "$query": ... }` or `{ "$param": ... }`. */
function hasQueryRef(val: unknown): boolean {
  if (val === null || val === undefined) return false;
  if (typeof val !== 'object') return false;
  if (Array.isArray(val)) return val.some(hasQueryRef);
  const obj = val as Record<string, unknown>;
  if ('$query' in obj || '$param' in obj) return true;
  return Object.values(obj).some(hasQueryRef);
}
