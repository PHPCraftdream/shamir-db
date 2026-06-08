/**
 * BrowserPlatform — thin browser adapter (~70 lines).
 *
 * Wraps:
 *   - @noble/hashes (sync)           → hmacSha256 / sha256
 *   - crypto.getRandomValues         → randomBytes
 *   - @noble/ciphers/utils equalBytes → timingSafeEqual
 *   - argon2-browser (WASM)          → argon2id
 *   - native WebSocket               → openSocket
 *
 * `@noble/hashes` gives synchronous SHA-256/HMAC (WebCrypto is async-only),
 * matching the sync `Platform` interface. Both Node and browser paths are
 * fully functional.
 */

import { equalBytes } from '@noble/ciphers/utils';
import { sha256 as nobleSha256 } from '@noble/hashes/sha256';
import { hmac } from '@noble/hashes/hmac';
import argon2 from 'argon2-browser';
import type { Platform, Socket, Argon2Params } from '../core/platform.js';
import { ARGON2_VERSION_13 } from '../core/scram.js';

class BrowserSocket implements Socket {
  private readonly ws: WebSocket;
  private readonly messageHandlers: Array<(data: Uint8Array) => void> = [];
  private closeHandlers: Array<(err?: Error) => void> = [];

  constructor(ws: WebSocket) {
    this.ws = ws;
    ws.binaryType = 'arraybuffer';
    ws.addEventListener('message', (ev: MessageEvent) => {
      const bytes = new Uint8Array(ev.data as ArrayBuffer);
      for (const h of this.messageHandlers) h(bytes);
    });
    ws.addEventListener('close', () => {
      for (const h of this.closeHandlers) h(undefined);
    });
    ws.addEventListener('error', () => {
      for (const h of this.closeHandlers) h(new Error('WebSocket error'));
    });
  }

  send(data: Uint8Array): void {
    this.ws.send(data);
  }

  onMessage(handler: (data: Uint8Array) => void): void {
    this.messageHandlers.push(handler);
  }

  onClose(handler: (err?: Error) => void): void {
    this.closeHandlers.push(handler);
  }

  close(): Promise<void> {
    return new Promise((resolve) => {
      if (
        this.ws.readyState === WebSocket.CLOSED ||
        this.ws.readyState === WebSocket.CLOSING
      ) {
        resolve();
        return;
      }
      this.ws.addEventListener('close', () => resolve(), { once: true });
      this.ws.close(1000);
    });
  }
}

export const BrowserPlatform: Platform = {
  hmacSha256(key: Uint8Array, data: Uint8Array): Uint8Array {
    return hmac(nobleSha256, key, data);
  },

  sha256(data: Uint8Array): Uint8Array {
    return nobleSha256(data);
  },

  randomBytes(n: number): Uint8Array {
    const buf = new Uint8Array(n);
    // `crypto` global is available in browsers and Node 19+.
    crypto.getRandomValues(buf);
    return buf;
  },

  timingSafeEqual(a: Uint8Array, b: Uint8Array): boolean {
    if (a.length !== b.length) return false;
    return equalBytes(a, b);
  },

  async argon2id(
    password: Uint8Array,
    salt: Uint8Array,
    p: Argon2Params,
  ): Promise<Uint8Array> {
    // argon2-browser types omit `version` but the runtime accepts it.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const result = await (argon2.hash as any)({
      pass: password,
      salt,
      time: p.time,
      mem: p.memoryKb,
      parallelism: p.parallelism,
      hashLen: 32,
      type: argon2.ArgonType.Argon2id,
      version: ARGON2_VERSION_13,
    });
    return new Uint8Array(result.hash as Uint8Array);
  },

  async openSocket(
    url: string,
    _opts: { rejectUnauthorized?: boolean; origin?: string },
  ): Promise<Socket> {
    return new Promise((resolve, reject) => {
      // Browser WebSocket: TLS policy is enforced by the browser; no
      // rejectUnauthorized option (self-signed certs must be trusted at OS level).
      const ws = new WebSocket(url);
      ws.binaryType = 'arraybuffer';
      const onOpen = () => {
        ws.removeEventListener('error', onError);
        resolve(new BrowserSocket(ws));
      };
      const onError = () => {
        ws.removeEventListener('open', onOpen);
        reject(new Error(`WebSocket failed to connect to ${url}`));
      };
      ws.addEventListener('open', onOpen, { once: true });
      ws.addEventListener('error', onError, { once: true });
    });
  },
};

// Re-export equalBytes for tests.
export { equalBytes };
