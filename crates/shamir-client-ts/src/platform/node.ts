/**
 * NodePlatform — thin Node.js adapter (~70 lines).
 *
 * Wraps:
 *   - node:crypto   → hmacSha256 / sha256 / randomBytes / timingSafeEqual
 *   - argon2-browser → argon2id (works in Node via WASM)
 *   - ws            → openSocket
 */

import { createHash, createHmac, timingSafeEqual, randomBytes } from 'node:crypto';
import WebSocket from 'ws';
import argon2 from 'argon2-browser';
import type { Platform, Socket, Argon2Params } from '../core/platform.js';
import { ARGON2_VERSION_13 } from '../core/scram.js';

class NodeSocket implements Socket {
  private readonly ws: WebSocket;
  private readonly messageHandlers: Array<(data: Uint8Array) => void> = [];
  private closeHandlers: Array<(err?: Error) => void> = [];
  private closeFired = false;

  constructor(ws: WebSocket) {
    this.ws = ws;
    ws.binaryType = 'nodebuffer';
    ws.on('message', (data: Buffer, isBinary: boolean) => {
      if (!isBinary) return;
      const bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
      for (const h of this.messageHandlers) h(bytes);
    });
    ws.on('close', () => {
      if (this.closeFired) return;
      this.closeFired = true;
      for (const h of this.closeHandlers) h(undefined);
    });
    ws.on('error', (err: Error) => {
      if (this.closeFired) return;
      this.closeFired = true;
      for (const h of this.closeHandlers) h(err);
    });
  }

  send(data: Uint8Array): void {
    this.ws.send(data, { binary: true });
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
      this.ws.once('close', () => resolve());
      this.ws.close(1000);
    });
  }
}

export const NodePlatform: Platform = {
  hmacSha256(key: Uint8Array, data: Uint8Array): Uint8Array {
    return new Uint8Array(createHmac('sha256', key).update(data).digest());
  },

  sha256(data: Uint8Array): Uint8Array {
    return new Uint8Array(createHash('sha256').update(data).digest());
  },

  randomBytes(n: number): Uint8Array {
    return new Uint8Array(randomBytes(n));
  },

  timingSafeEqual(a: Uint8Array, b: Uint8Array): boolean {
    if (a.length !== b.length) return false;
    return timingSafeEqual(a, b);
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
    opts: { rejectUnauthorized?: boolean; origin?: string },
  ): Promise<Socket> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url, {
        rejectUnauthorized: opts.rejectUnauthorized ?? true,
        origin: opts.origin,
      });
      const onOpen = () => {
        ws.off('error', onError);
        resolve(new NodeSocket(ws));
      };
      const onError = (err: Error) => {
        ws.off('open', onOpen);
        reject(err);
      };
      ws.once('open', onOpen);
      ws.once('error', onError);
    });
  },
};
