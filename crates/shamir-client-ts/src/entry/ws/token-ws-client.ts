import { decodeTokenFrame, decodeTokenPayload, decodeTokenPush, encodeTokenPayload } from '../core/protocol/token/ws.js';
import type { TokenRequestTransport } from '../core/protocol/token/transport.js';
import type { TokenHttpRequest, TokenRequestOptions } from '../core/protocol/token/options.js';
import type { NodeTokenWsConnectOptions, BrowserTokenWsConnectOptions } from '../core/types/connection.js';
import type { TokenSocket, TokenPlatform, TokenSocketOptions } from '../platform/token/types.js';
import { TokenTransportError } from '../core/protocol/token/errors.js';
import { TokenWsLifecycle, type TokenWsRequest } from './token-lifecycle.js';
import { createTokenOperations, type TokenHttpClient } from './token-client.js';
import { TokenHttpDb } from './token-db.js';
import type { TransactionOptions, TransactionResult } from '../core/visibility/types.js';
import type { IsolationLevel, TransactionInfo } from '../core/types/write/batch.js';
import type { ReadQuery } from '../core/types/read/query.js';
import { CursorIterator } from '../core/client/cursor-iterator.js';
import { SubscriptionRouter } from '../core/client/subscription-router.js';
import type { SubscriptionEvent } from '../core/client/subscription-router.js';
import { Batch } from '../core/builders/write/batch.js';
import { Query } from '../core/builders/read/query.js';
import type { WireValue } from '../core/types/write/write.js';
import { ShamirDbError } from '../core/support/errors.js';
import { normalizeTokenHttpOptions } from '../core/protocol/token/options.js';
import { normalizeVisibility, normalizeWait, position, validateTransactionVisibility } from '../core/visibility/positions.js';

export interface TokenWsSocketFactory { (url: URL, token: NodeTokenWsConnectOptions['token'], signal: AbortSignal, timeoutMs: number): Promise<TokenSocket>; }
interface LocalSubscriptionEvent extends SubscriptionEvent {}
class TokenSubscription implements AsyncIterableIterator<LocalSubscriptionEvent> {
  private queue: LocalSubscriptionEvent[] = []; private waiters: Array<(result: LocalSubscriptionEvent | undefined) => void> = []; private done = false;
  constructor(readonly subId: number, private readonly router: SubscriptionRouter, private readonly lifecycle: TokenWsLifecycle, private readonly sendUnsubscribe: () => Promise<unknown>) { this.router.register(subId, (event) => this.push(event)); }
  private push(event: LocalSubscriptionEvent): void { if (this.done) return; if (event.kind === 'closed') { this.terminate(); return; } const waiter = this.waiters.shift(); if (waiter) waiter(event); else if (this.queue.length < 256) this.queue.push(event); }
  next(): Promise<LocalSubscriptionEvent | undefined> { const event = this.queue.shift(); if (event) return Promise.resolve(event); if (this.done) return Promise.resolve(undefined); return new Promise((resolve) => this.waiters.push(resolve)); }
  async unsubscribe(): Promise<void> { if (this.done) return; try { await this.sendUnsubscribe(); } finally { this.terminate(); } }
  terminate(): void { if (this.done) return; this.done = true; this.queue = []; this.router.unregister(this.subId); this.lifecycle.unregisterSubscription(this.subId); for (const waiter of this.waiters) waiter(undefined); this.waiters = []; }
  [Symbol.asyncIterator](): AsyncIterableIterator<LocalSubscriptionEvent> { return { next: async () => { const value = await this.next(); return value === undefined ? { done: true, value: undefined } : { done: false, value }; } }; }
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
  readonly expiresAtNs: bigint;
  readonly channel = 'ws' as const;
  readonly sid: Uint8Array;
  private closed = false;
  constructor(base: TokenHttpClient, lifecycle: TokenWsLifecycle, platform: TokenPlatform) { this.base = base; this.lifecycle = lifecycle; this.platform = platform; this.sid = lifecycle.sid; this.expiresAtNs = lifecycle.expiresAtNs; }
  ping() { return this.base.ping(); }
  execute(db: string, batch: object, wait?: Parameters<TokenHttpClient['execute']>[2]) { return this.base.execute(db, batch, wait); }
  executeWithTouch(db: string, batch: object) { return this.execute(db, batch); }
  waitForVisibility(...args: Parameters<TokenHttpClient['waitForVisibility']>) { return this.base.waitForVisibility(...args); }
  getDdlOpStatus(...args: Parameters<TokenHttpClient['getDdlOpStatus']>) { return this.base.getDdlOpStatus(...args); }
  db(name: string) { return new TokenWsDb(this, name); }
  private request(request: TokenWsRequest, options: TokenRequestOptions = {}) { return this.lifecycle.request(request, options); }
  txBegin(db: string, repo: string, isolation?: IsolationLevel): Promise<{ tx_handle: number; snapshot_version: number | bigint; isolation: string }> {
    return this.request({ op: 'tx_begin', query_version: 4, db, repo, ...(isolation ? { isolation } : {}) }).then((r) => {
      if (r.kind !== 'tx_opened' || !Number.isSafeInteger(r.tx_handle) || Number(r.tx_handle) < 0 || !['snapshot','serializable'].includes(String(r.isolation))) throw new TokenTransportError('protocol_error', 'Malformed transaction open response');
      try { return { tx_handle: r.tx_handle as number, snapshot_version: position(r.snapshot_version), isolation: r.isolation as string }; } catch { throw new TokenTransportError('protocol_error', 'Malformed transaction open response'); }
    });
  }
  async txExecute(db: string, handle: number, batch: object): Promise<import('../core/types/write/batch.js').BatchResponse> {
    const r = await this.request({ op: 'tx_execute', query_version: 4, db, tx_handle: handle, batch: batch as never });
    if (r.kind !== 'tx_batch' || !isRecord(r.response)) throw new TokenTransportError('protocol_error', 'Malformed transaction batch response');
    return validateTokenBatchResponse(r.response);
  }
  async txCommit(db: string, handle: number, wait?: import('../core/visibility/types.js').VisibilityWait): Promise<TransactionInfo> {
    const safeWait = wait === undefined ? undefined : normalizeWait(wait);
    const r = await this.request({ op: 'tx_commit', query_version: 4, db, tx_handle: handle, ...(safeWait ? { visibility_wait: safeWait } : {}) } as never);
    if (r.kind !== 'tx_committed' || !isRecord(r.transaction)) throw new TokenTransportError('protocol_error', 'Malformed transaction commit response');
    const tx = r.transaction;
    if (!(typeof tx.tx_id === 'number' && Number.isSafeInteger(tx.tx_id) && tx.tx_id >= 0 || typeof tx.tx_id === 'bigint' && tx.tx_id >= 0n) || !['committed','aborted','indeterminate'].includes(String(tx.status)) || typeof tx.materialized !== 'boolean') throw new TokenTransportError('protocol_error', 'Malformed transaction result');
    try { validateTransactionVisibility(tx as unknown as TransactionInfo); } catch { throw new TokenTransportError('protocol_error', 'Malformed transaction visibility'); }
    return tx as unknown as TransactionInfo;
  }
  txRollback(db: string, handle: number): Promise<void> { return this.request({ op: 'tx_rollback', db, tx_handle: handle } as never).then((r) => { if (r.kind !== 'tx_rolled_back') throw new TokenTransportError('protocol_error', 'Malformed transaction rollback response'); }); }
  cursor(db: string, query: ReadQuery | { build(): ReadQuery }, pageSize: number): CursorIterator { const cursor = new CursorIterator((request) => this.request(request as never), db, query, pageSize); this.lifecycle.registerCursor(cursor); return cursor; }
  async runTransaction<T>(db: string, fn: (tx: TokenTx) => Promise<T>, opts?: TransactionOptions): Promise<TransactionResult<T>> {
    const opened = await this.txBegin(db, opts?.repo ?? 'main', opts?.isolation); let commitAttempted = false;
    try { const value = await fn(new TokenTx(this, db, opened.tx_handle)); commitAttempted = true; const transaction = await this.txCommit(db, opened.tx_handle, opts?.visibilityWait); if (transaction.status === 'aborted') throw Object.assign(new ShamirDbError(transaction.reason ?? 'tx_error', 'Transaction aborted'), { transaction }); if (transaction.status === 'indeterminate') throw Object.assign(new ShamirDbError(transaction.reason ?? 'wal_requires_reopen', 'Transaction outcome indeterminate; reconcile before retrying'), { transaction }); const visibility = transaction.visibility; if (!visibility) throw new TokenTransportError('protocol_error', 'Missing transaction visibility'); return { status: 'committed', state: visibility.state, value, transaction, visibility } as TransactionResult<T>; }
    finally { if (!commitAttempted) await this.txRollback(db, opened.tx_handle).catch(() => undefined); }
  }
  async runLive(db: string, batch: Batch): Promise<{ response: import('../core/types/write/batch.js').BatchResponse; subs: Record<string, TokenSubscription> }> {
    const response = await this.execute(db, batch.build()); const subs: Record<string, TokenSubscription> = {};
    for (const [alias, result] of Object.entries(response.results)) { const value = result.value; if (isRecord(value) && typeof value.sub === 'number' && Number.isSafeInteger(value.sub)) { const id = value.sub; const sub = new TokenSubscription(id, this.router, this.lifecycle, () => this.execute(db, Batch.create().add('_unsub', { unsubscribe: id }).build())); subs[alias] = sub; this.lifecycle.registerSubscription(id, sub, (push) => this.router.route(push)); } }
    return { response, subs };
  }
  async close(): Promise<void> { if (this.closed) return; this.closed = true; await this.lifecycle.close(); await this.base.close(); await this.platform.close(); }
}
export class TokenWsDb extends TokenHttpDb {
  constructor(private readonly wsClient: TokenWsClientImpl, name: string) { super(wsClient, name); }
  cursor(query: ReadQuery | { build(): ReadQuery }, pageSize: number): CursorIterator { return this.wsClient.cursor(this.name, query, pageSize); }
  tx<T>(fn: (tx: TokenTx) => Promise<T>, opts?: TransactionOptions): Promise<TransactionResult<T>> { return this.wsClient.runTransaction(this.name, fn, opts); }
  runLive(batch: Batch): ReturnType<TokenWsClientImpl['runLive']> { return this.wsClient.runLive(this.name, batch); }
}
export function createTokenWsClient<T extends NodeTokenWsConnectOptions | BrowserTokenWsConnectOptions>(options: T, platformFactory: (options: T) => TokenPlatform, socketFactory?: TokenWsSocketFactory): Promise<TokenWsClientImpl> {
  const httpOptions = normalizeTokenHttpOptions({ auth: 'token', channel: 'http', url: `http${options.url.startsWith('wss:') ? 's' : ''}://${new URL(options.url).host}/`, token: options.token, requestTimeoutMs: options.requestTimeoutMs, connectTimeoutMs: options.connectTimeoutMs, signal: options.signal });
  const secure = options.url.startsWith('wss:'); const wsUrl = new URL(httpOptions.url); wsUrl.protocol = secure ? 'wss:' : 'ws:';
  const platform = platformFactory(options);
  const socketOptions = (signal: AbortSignal, connectTimeoutMs: number): TokenSocketOptions => ({ ...('tls' in options && options.tls ? { tls: options.tls } : {}), ...('origin' in options && options.origin ? { origin: options.origin } : {}), signal, connectTimeoutMs });
  const open: TokenWsSocketFactory = socketFactory ? (url, token, signal, timeout) => socketFactory(url, token, socketOptions(signal, timeout)) : (url, token, signal, timeout) => platform.openTokenSocket(url, token, socketOptions(signal, timeout));
  return TokenWsLifecycle.connect(wsUrl, options.token, options.signal, httpOptions.connectTimeoutMs, httpOptions.requestTimeoutMs, open, !('tls' in options || 'origin' in options)).then((lifecycle) => {
    const operations = createTokenOperations({ auth: 'token', channel: 'http', url: httpOptions.url.href, token: options.token, requestTimeoutMs: httpOptions.requestTimeoutMs, connectTimeoutMs: httpOptions.connectTimeoutMs, signal: options.signal }, new WsOperationsTransport(lifecycle, platform), true);
    return new TokenWsClientImpl(operations, lifecycle, platform);
  });
}
export type TokenWsEnvelope = ReturnType<typeof decodeTokenFrame>;
export function decodeTokenWsPayload(bytes: Uint8Array): unknown { return decodeTokenPayload(bytes); }
export function encodeTokenWsPayload(value: unknown): Uint8Array { return encodeTokenPayload(value); }
export type TokenWsPush = ReturnType<typeof decodeTokenPush>;
