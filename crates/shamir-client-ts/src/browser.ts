/**
 * Browser entry point.
 *
 * Wires ShamirClient (core) with BrowserPlatform.
 * `connect(opts)` is the public factory that browser/bundler consumers call.
 */

import { ShamirClient } from './core/client.js';
import { BrowserPlatform } from './platform/browser.js';
import type { ConnectOptions } from './core/types.js';

/**
 * Open an authenticated ShamirDB connection from a browser.
 * Uses native WebSocket and WebCrypto / argon2-browser.
 */
export async function connect(opts: ConnectOptions): Promise<ShamirClient> {
  return ShamirClient.connect(BrowserPlatform, opts);
}

export { ShamirClient };
export type { ConnectOptions };
