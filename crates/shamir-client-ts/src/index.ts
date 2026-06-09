/**
 * Node.js entry point.
 *
 * Wires ShamirClient (core) with NodePlatform.
 * `connect(opts)` is the public factory that Node consumers call.
 */

import { ShamirClient } from './core/client.js';
import { NodePlatform } from './platform/node.js';
import type { ConnectOptions } from './core/types/index.js';

/**
 * Open an authenticated ShamirDB connection from Node.js.
 * Uses `ws` for the socket and `node:crypto` for HMAC/SHA256.
 */
export async function connect(opts: ConnectOptions): Promise<ShamirClient> {
  return ShamirClient.connect(NodePlatform, opts);
}

export { ShamirClient };
export type { TxOpened, ScramUserCreated } from './core/client.js';
export type { Db, Tx } from './core/db.js';

// All builders (filter/select/write/ddl/admin/query/batch/call) as FLAT named
// exports — `import { eq, insert, createTable, call, Query, Batch } from '@shamir/client'`.
export * from './core/builders/index.js';
// The wire type model (platform-agnostic core).
export type * from './core/types/index.js';
