/**
 * Node.js entry point.
 *
 * Wires ShamirClient (core) with NodePlatform.
 * `connect(opts)` is the public factory that Node consumers call.
 */

import { ShamirClient } from './core/client.js';
import { NodePlatform } from './platform/node.js';
import type { ConnectOptions, ResumeOptions } from './core/types/index.js';

/**
 * Open an authenticated ShamirDB connection from Node.js.
 * Uses `ws` for the socket and `node:crypto` for HMAC/SHA256.
 */
export async function connect(opts: ConnectOptions): Promise<ShamirClient> {
  return ShamirClient.connect(NodePlatform, opts);
}

/**
 * Fast reconnection from Node.js using a resumption ticket from a previous
 * session. Skips the full SCRAM handshake.
 */
export async function resume(opts: ResumeOptions): Promise<ShamirClient> {
  return ShamirClient.resume(NodePlatform, opts);
}

export { ShamirClient };
// Typed error surface (Finding 2.1 / 2.2): callers branch on `.code` /
// `.retryable` instead of regexing message strings.
export {
  ShamirDbError,
  ShamirTimeoutError,
  isRetryableCode,
  RETRYABLE_ERROR_CODES,
} from './core/errors.js';
export type { TxOpened, ScramUserCreated } from './core/client.js';
export { SubscriptionRouter } from './core/subscription-router.js';
export type { SubscriptionEvent } from './core/subscription-router.js';
export { SubscriptionHandle } from './core/subscription-handle.js';
export type { ResumeOptions };
export type { Db, Tx } from './core/db.js';

// Builders: per-domain namespace objects (filter / select / write / ddl /
// admin) + Query / Batch / call —
// `import { filter, write, ddl, Query, Batch } from '@shamir/client'`.
export * from './core/builders/index.js';
// The wire type model (platform-agnostic core).
export type * from './core/types/index.js';
