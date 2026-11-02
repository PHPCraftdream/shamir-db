import { validateTokenBatchResponse } from '../token-response.js';
import { decodeTokenFrame, decodeTokenPayload, decodeTokenPush, encodeTokenPayload } from '../../core/protocol/token/ws.js';
import type { TokenRequestTransport } from '../../core/protocol/token/transport.js';
import type { TokenHttpRequest, TokenRequestOptions } from '../../core/protocol/token/options.js';
import type { NodeTokenWsConnectOptions, BrowserTokenWsConnectOptions } from '../../core/types/connection.js';
import type { TokenSocket, TokenPlatform, TokenSocketOptions } from '../../platform/token/types.js';
import { TokenTransportError } from '../../core/protocol/token/errors.js';
import { isBrowserLoopback } from '../../platform/token/socket-browser.js';
import { TokenWsLifecycle, type TokenWsRequest } from './token-lifecycle.js';
import { createTokenOperations, validateBatch, type TokenHttpClient } from '../token-client.js';
import { TokenHttpDb } from '../token-db.js';
import type { TransactionOptions, TransactionResult } from '../../core/visibility/types.js';
import type { IsolationLevel, TransactionInfo } from '../../core/types/write/batch.js';
import type { ReadQuery } from '../../core/types/read/query.js';
import { TokenCursor } from './token-cursor.js';
import { SubscriptionRouter } from '../../core/client/subscription-router.js';
import type { SubscriptionEvent } from '../../core/client/subscription-router.js';
import { Batch } from '../../core/builders/write/batch.js';
import { unsubscribeOp } from '../../core/builders/stream/subscribe.js';
import { Query } from '../../core/builders/read/query.js';
import type { WireValue } from '../../core/types/write/write.js';
import { ShamirDbError } from '../../core/support/errors.js';
import { normalizeTokenHttpOptions } from '../../core/protocol/token/options.js';
import { normalizeVisibility, normalizeWait, position, validateTransactionVisibility } from '../../core/visibility/positions.js';

export interface TokenWsSocketFactory { (url: URL, token: NodeTokenWsConnectOptions['token'], options: TokenSocketOptions): Promise<TokenSocket>; }
type LocalSubscriptionEvent = SubscriptionEvent;
const MAX_SUBSCRIPTIONS = 128;
const terminalSubscriptionError = (): TokenTransportError => new TokenTransportError('connection_closed', 'Subscription closed');
class TokenSubscription implements AsyncIterable<LocalSubscriptionEvent> {
  private queue: LocalSubscriptionEvent[] = []; private waiters: Array<(result: LocalSubscriptionEvent | undefined) => void> = []; private done = false; private unsubscribePromise: Promise<void> | undefined; private lifecycleUnsubscribe: () => void = () => undefined;
  constructor(readonly subId: number, private readonly router: SubscriptionRouter, private readonly lifecycle: TokenWsLifecycle, private readonly sendUnsubscribe: () => Promise<unknown>) { this.router.register(subId, (event) => this.push(event)); this.lifecycleUnsubscribe = this.lifecycle.onTerminal(() => this.terminate()); }
  private push(event: LocalSubscriptionEvent): void { if (this.done) return; if (event.kind === 'closed' || event.kind === 'slow_consumer') { this.terminate(); return; } const waiter = this.waiters.shift(); if (waiter) waiter(event); else if (this.queue.length < 256) this.queue.push(event); else this.terminate(); }
  next(): Promise<LocalSubscriptionEvent | undefined> { const event = this.queue.shift(); if (event) return Promise.resolve(event); if (this.done) return Promise.resolve(undefined); return new Promise((resolve) => this.waiters.push(resolve)); }
  unsubscribe(): Promise<void> { if (this.unsubscribePromise) return this.unsubscribePromise; if (this.done) return Promise.resolve(); this.lifecycle.unregisterSubscription(this.subId); this.unsubscribePromise = (async () => { try { await this.sendUnsubscribe(); } finally { this.terminate(); } })(); return this.unsubscribePromise; }
  terminate(): void { if (this.done) return; this.done = true; this.queue = []; this.router.unregister(this.subId); this.lifecycle.unregisterSubscription(this.subId); this.lifecycleUnsubscribe(); for (const waiter of this.waiters) waiter(undefined); this.waiters = []; }
  private iterator: AsyncIterableIterator<LocalSubscriptionEvent> | undefined;
  [Symbol.asyncIterator](): AsyncIterableIterator<LocalSubscriptionEvent> { if (!this.iterator) this.iterator = { next: async () => { const value = await this.next(); return value === undefined ? { done: true, value: undefined } : { done: false, value }; }, return: async () => { this.terminate(); return { done: true, value: undefined }; }, [Symbol.asyncIterator]() { return this; } }; return this.iterator; }
}
class TokenTx {
  constructor(private readonly client: TokenWsClientImpl, private readonly dbName: string, private readonly txHandle: number) {}
  private exec(batch: object) { return this.client.txExecute(this.dbName, this.txHandle, batch); }
  async run(op: object) { const built = typeof (op as { build?: unknown }).build === 'function' ? (op as { build(): object }).build() : op; const response = await this.exec(Batch.create().add('_', built as never).build()); const result = response.results._; if (!result) throw new TokenTransportError('protocol_error', 'Missing transaction query result'); return result; }
  async rows(op: object): Promise<Array<Record<string, WireValue>>> { return (await this.run(op)).records; }
  query(table: string) { return Query.from(table).bindCtx({ exec: (batch) => this.exec(batch) }); }
  withRepo(repo: string, table: string) { return Query.withRepo(repo, table).bindCtx({ exec: (batch) => this.exec(batch) }); }
  batch(id?: WireValue) { return Batch.create(id).bindCtx({ exec: (batch) => this.exec(batch) }); }
}
class WsOperationsTransport implements TokenRequestTransport {
  readonly channel = 'ws' as const;
  constructor(private readonly lifecycle: TokenWsLifecycle, private readonly platform: TokenPlatform) {}
  request(request: TokenHttpRequest, options?: TokenRequestOptions) { return this.lifecycle.request(request, options); }
  close(): Promise<void> { return this.platform.close(); }
}
function isRecord(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) && !(value instanceof Uint8Array); }
export class TokenWsClientImpl {
  private readonly router = new SubscriptionRouter();
  private base: TokenHttpClient;
  private readonly lifecycle: TokenWsLifecycle;
  private readonly platform: TokenPlatform;
  private lifecycleUnsubscribe: () => void = () => undefined;
  readonly terminalController = new AbortController();
  private readonly transactionHandles = new Map<number, string>();
  readonly expiresAtNs: bigint;
  readonly channel = 'ws' as const;
  readonly sid: Uint8Array;
  private closed = false;
  constructor(base: TokenHttpClient, lifecycle: TokenWsLifecycle, platform: TokenPlatform) { this.base = base; this.lifecycle = lifecycle; this.platform = platform; this.sid = lifecycle.sid; this.expiresAtNs = lifecycle.expiresAtNs; this.lifecycleUnsubscribe = lifecycle.onTerminal((error) => { this.terminalController.abort(); this.transactionHandles.clear(); /* Server owns rollback after terminal close. */ }); }
  ping() { return this.base.ping(); }
  execute(db: string, batch: object, wait?: Parameters<TokenHttpClient['execute']>[2]) { return this.base.execute(db, batch, wait); }
  executeWithTouch(db: string, batch: object) { return this.execute(db, batch); }
  waitForVisibility(...args: Parameters<TokenHttpClient['waitForVisibility']>) { return this.base.waitForVisibility(...args); }
  getDdlOpStatus(...args: Parameters<TokenHttpClient['getDdlOpStatus']>) { return this.base.getDdlOpStatus(...args); }
  db(name: string) { return new TokenWsDb(this, name); }
  private request(request: TokenWsRequest, options: TokenRequestOptions = {}) { return this.lifecycle.request(request, options); }
  txBegin(db: string, repo: string, isolation?: Exclude<IsolationLevel, 'pessimistic'>): Promise<{ tx_handle: number; snapshot_version: number | bigint; isolation: string }> {
    return this.request({ op: 'tx_begin', query_version: 4, db, repo, ...(isolation ? { isolation } : {}) }).then((r) => {
      if (r.kind !== 'tx_opened' || !Number.isSafeInteger(r.tx_handle) || Number(r.tx_handle) < 0 || !['snapshot','serializable'].includes(String(r.isolation))) throw new TokenTransportError('protocol_error', 'Malformed transaction open response');
      try { const opened = { tx_handle: r.tx_handle as number, snapshot_version: position(r.snapshot_version), isolation: r.isolation as string }; this.transactionHandles.set(opened.tx_handle, db); return opened; } catch { throw new TokenTransportError('protocol_error', 'Malformed transaction open response'); }
    });
  }
  async txExecute(db: string, handle: number, batch: object): Promise<import('../../core/types/write/batch.js').BatchResponse> {
    if (this.terminalController.signal.aborted || !this.transactionHandles.has(handle)) throw new TokenTransportError('connection_closed', 'Token transaction is closed');
    validateBatch(batch, true);
    const r = await this.request({ op: 'tx_execute', query_version: 4, db, tx_handle: handle, batch: batch as never });
    if (r.kind !== 'tx_batch' || !isRecord(r.response)) throw new TokenTransportError('protocol_error', 'Malformed transaction batch response');
    return validateTokenBatchResponse(r.response);
  }
  async txCommit(db: string, handle: number, wait?: import('../../core/visibility/types.js').VisibilityWait): Promise<TransactionInfo> {
    if (this.terminalController.signal.aborted || !this.transactionHandles.has(handle)) throw new TokenTransportError('connection_closed', 'Token transaction is closed');
    const safeWait = wait === undefined ? undefined : normalizeWait(wait);
    this.transactionHandles.delete(handle);
    const r = await this.request({ op: 'tx_commit', query_version: 4, db, tx_handle: handle, ...(safeWait ? { visibility_wait: safeWait } : {}) } as never);
    this.transactionHandles.delete(handle);
    if (r.kind !== 'tx_committed' || !isRecord(r.transaction)) throw new TokenTransportError('protocol_error', 'Malformed transaction commit response');
    const tx = r.transaction;
    if (!(typeof tx.tx_id === 'number' && Number.isSafeInteger(tx.tx_id) && tx.tx_id >= 0 || typeof tx.tx_id === 'bigint' && tx.tx_id >= 0n) || !['committed','aborted','indeterminate'].includes(String(tx.status)) || typeof tx.materialized !== 'boolean') throw new TokenTransportError('protocol_error', 'Malformed transaction result');
    try { validateTransactionVisibility(tx as unknown as TransactionInfo); } catch { throw new TokenTransportError('protocol_error', 'Malformed transaction visibility'); }
    return tx as unknown as TransactionInfo;
  }
  txRollback(db: string, handle: number): Promise<void> { if (this.terminalController.signal.aborted || !this.transactionHandles.has(handle)) return Promise.reject(new TokenTransportError('connection_closed', 'Token transaction is closed')); return this.request({ op: 'tx_rollback', db, tx_handle: handle } as never).then((r) => { if (r.kind !== 'tx_rolled_back') throw new TokenTransportError('protocol_error', 'Malformed transaction rollback response'); this.transactionHandles.delete(handle); }); }
  cursor(db: string, query: ReadQuery | { build(): ReadQuery }, pageSize: number): TokenCursor { const cursor = new TokenCursor((request) => this.request(request as never), db, query, pageSize); cursor.attachTerminalSignal(this.lifecycle.terminalSignal); this.lifecycle.registerCursor(cursor); return cursor; }
  async runTransaction<T>(db: string, fn: (tx: TokenTx) => Promise<T>, opts?: TransactionOptions): Promise<TransactionResult<T>> {
    if (opts?.isolation === 'pessimistic') throw new TokenTransportError('invalid_request', 'Pessimistic transactions are not supported over WebSocket');
    const opened = await this.txBegin(db, opts?.repo ?? 'main', opts?.isolation); let commitAttempted = false;
    const onTerminal = (): void => { this.terminalController.signal.removeEventListener('abort', onTerminal); };
    this.terminalController.signal.addEventListener('abort', onTerminal, { once: true });
    try { const value = await fn(new TokenTx(this, db, opened.tx_handle)); if (this.terminalController.signal.aborted) throw new TokenTransportError('connection_closed', 'Token transaction is closed'); commitAttempted = true; const transaction = await this.txCommit(db, opened.tx_handle, opts?.visibilityWait); if (transaction.status !== 'committed') throw Object.assign(new ShamirDbError(transaction.reason ?? (transaction.status === 'indeterminate' ? 'wal_requires_reopen' : 'tx_error'), transaction.status === 'indeterminate' ? 'Transaction outcome indeterminate; reconcile before retrying' : 'Transaction aborted'), { transaction }); const visibility = transaction.visibility; if (!visibility) throw new TokenTransportError('protocol_error', 'Missing transaction visibility'); if (visibility.state === 'visible') return { status: 'committed', state: 'visible', value, transaction, visibility }; return { status: 'committed', state: 'pending', value, transaction, visibility }; }
    catch (error) { if (!commitAttempted && !this.terminalController.signal.aborted) await this.txRollback(db, opened.tx_handle).catch(() => undefined); throw error; }
    finally { this.terminalController.signal.removeEventListener('abort', onTerminal); }
  }
  async runLive(db: string, batch: Batch): Promise<{ response: import('../../core/types/write/batch.js').BatchResponse; subs: Record<string, TokenSubscription> }> {
    const response = await this.execute(db, batch.build()); const subs: Record<string, TokenSubscription> = {};
    const grants = Object.entries(response.results).flatMap(([alias, result]) => {
      const value = result.value;
      return isRecord(value) && typeof value.sub === 'number' && Number.isSafeInteger(value.sub) ? [{ alias, id: value.sub }] : [];
    });
    const created: TokenSubscription[] = [];
    try {
      for (const { alias, id } of grants) {
        const sub = new TokenSubscription(id, this.router, this.lifecycle, () => this.execute(db, Batch.create().add('_unsub', unsubscribeOp(id)).build()));
        created.push(sub);
        this.lifecycle.registerSubscription(id, sub, (push) => this.router.route(push));
        subs[alias] = sub;
      }
    } catch (error) {
      for (const { id } of grants) this.lifecycle.unregisterSubscription(id);
      for (const sub of created) sub.terminate();
      await Promise.all(grants.map(({ id }) => this.execute(db, Batch.create().add('_unsub', unsubscribeOp(id)).build()).catch(() => undefined)));
      throw error;
    }
    return { response, subs };
  }
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    try {
      if (!this.terminalController.signal.aborted) {
        for (const handle of [...this.transactionHandles.keys()]) {
          const db = this.transactionHandles.get(handle);
          if (db) await this.txRollback(db, handle).catch(() => undefined);
        }
      }
    } finally {
      this.terminalController.abort();
      try { await this.lifecycle.close(); } finally {
        this.lifecycleUnsubscribe();
        this.transactionHandles.clear();
        await this.base.close();
      }
    }
  }
}
export class TokenWsDb extends TokenHttpDb {
  constructor(private readonly wsClient: TokenWsClientImpl, name: string) { super(wsClient, name); }
  cursor(query: ReadQuery | { build(): ReadQuery }, pageSize: number): TokenCursor { return this.wsClient.cursor(this.name, query, pageSize); }
  tx<T>(fn: (tx: TokenTx) => Promise<T>, opts?: TransactionOptions): Promise<TransactionResult<T>> { return this.wsClient.runTransaction(this.name, fn, opts); }
  runLive(batch: Batch): ReturnType<TokenWsClientImpl['runLive']> { return this.wsClient.runLive(this.name, batch); }
}
export async function createTokenWsClient<T extends NodeTokenWsConnectOptions | BrowserTokenWsConnectOptions>(options: T, platformFactory: (options: T) => TokenPlatform, socketFactory?: TokenWsSocketFactory): Promise<TokenWsClientImpl> {
  const rawUrl = options.url;
  if (typeof rawUrl !== 'string' || rawUrl.trim() !== rawUrl || !/^wss?:\/\/[^/?#\\\\\s]+\/?$/i.test(rawUrl)) return Promise.reject(new TypeError('Invalid token WebSocket URL'));
  let parsed: URL; try { parsed = new URL(rawUrl); } catch { return Promise.reject(new TypeError('Invalid token WebSocket URL')); }
  if ((parsed.protocol.toLowerCase() !== 'ws:' && parsed.protocol.toLowerCase() !== 'wss:') || !parsed.hostname || parsed.username || parsed.password || parsed.search || parsed.hash || (parsed.pathname !== '/' && parsed.pathname !== '')) return Promise.reject(new TypeError('Token WebSocket URL must be an origin root'));
  const secure = parsed.protocol.toLowerCase() === 'wss:';
  const normalizedOriginUrl = new URL(`${secure ? 'https' : 'http'}://${parsed.host}/`);
  const httpOptions = normalizeTokenHttpOptions({ auth: 'token', channel: 'http', url: normalizedOriginUrl.href, token: options.token, requestTimeoutMs: options.requestTimeoutMs, connectTimeoutMs: options.connectTimeoutMs, signal: options.signal });
  const wsUrl = new URL(`${secure ? 'wss' : 'ws'}://${parsed.host}/`);
  const platform = platformFactory(options);
  if (platform.runtime === 'browser' && parsed.protocol.toLowerCase() === 'ws:' && !isBrowserLoopback(parsed.hostname)) {
    return Promise.resolve(platform.close()).catch(() => undefined).then(() => Promise.reject(new TokenTransportError('invalid_request', 'Token WebSocket connection failed', { phase: 'connect' })));
  }
  const socketOptions = (signal: AbortSignal, connectTimeoutMs: number): TokenSocketOptions => ({ ...('tls' in options && options.tls ? { tls: options.tls } : {}), ...('origin' in options && options.origin ? { origin: options.origin } : {}), signal, connectTimeoutMs });
  const open: TokenWsSocketFactory = socketFactory ? (url, token, opts) => socketFactory(url, token, opts) : (url, token, opts) => platform.openTokenSocket(url, token, opts);
  return TokenWsLifecycle.connect(wsUrl, options.token, options.signal, httpOptions.connectTimeoutMs, httpOptions.requestTimeoutMs, (url, token, signal, timeout) => open(url, token, socketOptions(signal, timeout)), platform.runtime === 'browser').then((lifecycle) => {
    const operations = createTokenOperations({ auth: 'token', channel: 'http', url: httpOptions.url.href, token: options.token, requestTimeoutMs: httpOptions.requestTimeoutMs, connectTimeoutMs: httpOptions.connectTimeoutMs, signal: options.signal }, new WsOperationsTransport(lifecycle, platform), true);
    return new TokenWsClientImpl(operations, lifecycle, platform);
  }).catch(async (error: unknown) => {
    try { await platform.close(); } catch { /* preserve the connection failure */ }
    throw error;
  });
}
export type TokenWsEnvelope = ReturnType<typeof decodeTokenFrame>;
export function decodeTokenWsPayload(bytes: Uint8Array): unknown { return decodeTokenPayload(bytes); }
export function encodeTokenWsPayload(value: unknown): Uint8Array { return encodeTokenPayload(value); }
export type TokenWsPush = ReturnType<typeof decodeTokenPush>;
