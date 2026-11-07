import type { TokenHttpClient } from '../token-client.js';
import type { TokenHttpDb, TokenTransactionContext, TokenLiveResult } from '../token-db.js';
import type { ReadQuery } from '../../core/types/read/query.js';
import type { TokenCursor } from './token-cursor.js';
import type { TransactionOptions, TransactionResult } from '../../core/visibility/types.js';
import type { Batch } from '../../core/builders/write/batch.js';

export interface TokenWsDb extends TokenHttpDb {
  cursor(query: ReadQuery | { build(): ReadQuery }, pageSize: number): TokenCursor;
  tx<T>(fn: (tx: TokenTransactionContext) => Promise<T>, opts?: TransactionOptions): Promise<TransactionResult<T>>;
  runLive(batch: Batch): Promise<TokenLiveResult>;
}

export interface TokenWsClient extends Omit<TokenHttpClient, 'channel' | 'sid' | 'expiresAtNs' | 'db'> {
  readonly channel: 'ws';
  readonly sid: Uint8Array;
  readonly expiresAtNs: bigint;
  db(name: string): TokenWsDb;
}
export type PublicTokenClient = TokenHttpClient | TokenWsClient;
