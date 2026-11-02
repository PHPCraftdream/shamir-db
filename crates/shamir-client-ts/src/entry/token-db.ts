import { Batch } from '../core/builders/write/batch.js';
import { Query } from '../core/builders/read/query.js';
import type { BatchOpInput, BatchRequest, BatchResponse, QueryResult } from '../core/types/write/batch.js';
import type { WireValue } from '../core/types/write/write.js';
import type { TokenHttpClient } from './token-client.js';
import type { VisibilityState, VisibilityWait, TransactionOptions, TransactionResult } from '../core/visibility/types.js';
import { TokenTransportError } from '../core/protocol/token/errors.js';
import { CursorIterator } from '../core/client/cursor-iterator.js';
import type { ReadQuery } from '../core/types/read/query.js';
import type { SubscriptionEvent } from '../core/client/subscription-router.js';

interface Buildable { build(): object; }
export interface TokenTransactionContext {
  run(op: object): Promise<QueryResult>;
  rows(op: object): Promise<Array<Record<string, WireValue>>>;
  query(table: string): Query;
  withRepo(repo: string, table: string): Query;
  batch(id?: WireValue): Batch;
}
export interface TokenLiveSubscription extends AsyncIterable<SubscriptionEvent> {
  [Symbol.asyncIterator](): AsyncIterableIterator<SubscriptionEvent>;
  readonly subId: number;
  next(): Promise<SubscriptionEvent | undefined>;
  unsubscribe(): Promise<void>;
}
export interface TokenLiveResult {
  response: BatchResponse;
  subs: Record<string, TokenLiveSubscription>;
}

interface TokenDbClient {
  execute(db: string, batch: object, wait?: VisibilityWait): Promise<BatchResponse>;
  waitForVisibility(db: string, repo: string, version: bigint, wait: VisibilityWait): Promise<VisibilityState>;
  getDdlOpStatus(db: string, repo: string, table: string, opId: string): ReturnType<TokenHttpClient['getDdlOpStatus']>;
}

export class TokenHttpDb {
  constructor(private readonly client: TokenDbClient, readonly name: string) {}

  async run(op: object): Promise<QueryResult> {
    const built = typeof (op as Partial<Buildable>).build === 'function' ? (op as Buildable).build() : op;
    const batch = Batch.create().add('_', built as BatchOpInput).build();
    const response = await this.client.execute(this.name, batch);
    const result = response.results['_'];
    if (result === undefined) throw new Error("server returned no result for operation alias '_'");
    return result;
  }

  async rows(op: object): Promise<Array<Record<string, WireValue>>> { return (await this.run(op)).records; }
  query(table: string): Query { return Query.from(table).bindCtx({ exec: (batch) => this.client.execute(this.name, batch) }); }
  withRepo(repo: string, table: string): Query { return Query.withRepo(repo, table).bindCtx({ exec: (batch) => this.client.execute(this.name, batch) }); }
  batch(id?: WireValue): Batch { return Batch.create(id).bindCtx({ exec: (batch) => this.client.execute(this.name, batch) }); }
  execute(batch: BatchRequest): Promise<BatchResponse> { return this.client.execute(this.name, batch); }
  waitForVisibility(repo: string, version: bigint, wait: VisibilityWait): Promise<VisibilityState> { return this.client.waitForVisibility(this.name, repo, version, wait); }
  getDdlOpStatus(repo: string, table: string, opId: string) { return this.client.getDdlOpStatus(this.name, repo, table, opId); }
  cursor(_query: ReadQuery | { build(): ReadQuery }, _pageSize: number): CursorIterator { throw unsupportedRest(); }
  tx<T>(_fn: (tx: TokenTransactionContext) => Promise<T>, _opts?: TransactionOptions): Promise<TransactionResult<T>> { return Promise.reject(unsupportedRest()); }
  runLive(_batch: Batch): Promise<TokenLiveResult> { return Promise.reject(unsupportedRest()); }
}

function unsupportedRest(): TokenTransportError {
  return new TokenTransportError('not_supported_over_rest', 'This operation is not supported over REST');
}
