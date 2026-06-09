/**
 * Browser entry point.
 *
 * Wires ShamirClient (core) with BrowserPlatform.
 * `connect(opts)` is the public factory that browser/bundler consumers call.
 */

import { ShamirClient } from './core/client.js';
import { BrowserPlatform } from './platform/browser.js';
import type { ConnectOptions } from './core/types/index.js';

/**
 * Open an authenticated ShamirDB connection from a browser.
 * Uses native WebSocket and WebCrypto / argon2-browser.
 */
export async function connect(opts: ConnectOptions): Promise<ShamirClient> {
  return ShamirClient.connect(BrowserPlatform, opts);
}

export { ShamirClient };
export type { TxOpened, ScramUserCreated } from './core/client.js';
export type { Db } from './core/db.js';

// All builders (filter/select/write/ddl/admin/query/batch/call) as FLAT named
// exports — `import { eq, insert, createTable, call, Query, Batch } from '@shamir/client/browser'`.
export * from './core/builders/index.js';
// The wire type model (platform-agnostic core).
export type * from './core/types/index.js';
