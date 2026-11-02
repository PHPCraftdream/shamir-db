/**
 * Browser entry point.
 *
 * Wires ShamirClient (core) with BrowserPlatform.
 * `connect(opts)` is the public factory that browser/bundler consumers call.
 */

import { ShamirClient } from './core/client/client.js';
import { BrowserPlatform } from './platform/browser.js';
import type { ScramConnectOptions, BrowserConnectOptions } from './core/types/index.js';
import { connectTokenBrowser } from './platform/browser.js';
import type { PublicBrowserTokenClient } from './platform/connect/browser-connect.js';
import { TokenTransportError } from './core/protocol/token/errors.js';

/**
 * Open an authenticated ShamirDB connection from a browser.
 * Uses native WebSocket and WebCrypto / argon2-browser.
 */
export function connect(opts: ScramConnectOptions): Promise<ShamirClient>;
export function connect(opts: BrowserConnectOptions): Promise<PublicBrowserTokenClient>;
export async function connect(opts: ScramConnectOptions | BrowserConnectOptions): Promise<ShamirClient | PublicBrowserTokenClient> {
  if (!opts || typeof opts !== 'object') throw new TokenTransportError('invalid_request', 'Invalid connection options');
  if ('auth' in opts) {
    if (opts.auth !== 'token' || !('token' in opts) || !('url' in opts)) throw new TokenTransportError('invalid_request', 'Invalid token connection options');
    return connectTokenBrowser(opts);
  }
  if ('token' in opts) throw new TokenTransportError('invalid_request', 'Token authentication must be explicit');
  return ShamirClient.connect(BrowserPlatform, opts);
}

export { ApiToken } from './entry/api-token.js';
export { TokenTransportError } from './core/protocol/token/errors.js';
export type { TokenTransportErrorCode, TokenTransportErrorOptions } from './core/protocol/token/errors.js';
export type { TokenConnectOptions, BrowserConnectOptions, ScramConnectOptions, BrowserTokenHttpConnectOptions, BrowserTokenWsConnectOptions } from './core/types/connection.js';
export { VisibilityError } from './core/visibility/errors.js';
export type { TransactionResult, TransactionOptions } from './core/visibility/types.js';
export {
  extractReceipts,
  reconcileReceipts,
  executeWithReceipts,
  decodeChangesSince,
} from './core/receipts/index.js';
export type {
  AliasReceipt,
  ChangesSinceEvent,
  ChangesSinceRecordChange,
  ChangesSinceResult,
  RepoObservation,
} from './core/receipts/index.js';
export { ShamirClient };
export {
  ShamirDbError,
  ShamirTimeoutError,
  isRetryableCode,
  isVersionConflict,
  RETRYABLE_ERROR_CODES,
} from './core/support/errors.js';
export type { TxOpened, ScramUserCreated } from './core/client/client.js';
export type { Db, Tx } from './core/client/db.js';

// Builders: per-domain namespace objects (filter / select / write / ddl /
// admin) + Query / Batch / call —
// `import { filter, write, ddl, Query, Batch } from '@shamir/client/browser'`.
export * from './core/builders/index.js';
// The wire type model (platform-agnostic core).
export type * from './core/types/index.js';
