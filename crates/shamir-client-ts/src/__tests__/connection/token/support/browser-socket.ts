import WebSocket from 'ws';
import { decode } from '../../../../core/protocol/framing.js';

export interface BrowserSocketObservation {
  pathname: string;
  protocol: string;
  hasAuthorization: boolean;
  unexpectedStatus?: number;
  close?: { code: number; reason: string };
  firstFrame?: { binary: boolean; tokenOnly: boolean; tokenLength: number; canonical: boolean; byteLength: number };
  waitForClose(timeoutMs: number): Promise<{ code: number; reason: string }>;
}

export interface BrowserSocketObserver {
  sockets: BrowserSocketObservation[];
  restore(): void;
}

function recordFirstFrame(observation: BrowserSocketObservation, data: Parameters<WebSocket['send']>[0]): void {
  if (observation.firstFrame) return;
  let bytes: Uint8Array | undefined;
  if (data instanceof Uint8Array) bytes = new Uint8Array(data);
  else if (data instanceof ArrayBuffer) bytes = new Uint8Array(data);
  if (!bytes) {
    observation.firstFrame = { binary: false, tokenOnly: false, tokenLength: 0, canonical: false, byteLength: 0 };
    return;
  }

  let tokenOnly = false;
  let tokenLength = 0;
  let canonical = false;
  try {
    const value = decode(bytes);
    if (typeof value === 'object' && value !== null && 'token' in value) {
      const token = (value as { token?: unknown }).token;
      if (typeof token === 'string') {
        tokenOnly = Object.keys(value).length === 1;
        tokenLength = token.length;
        canonical = /^shk1\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}$/.test(token);
      }
    }
  } catch { /* malformed first frame is recorded as invalid */ }
  observation.firstFrame = { binary: true, tokenOnly, tokenLength, canonical, byteLength: bytes.byteLength };
}

export function installBrowserSocket(origin: string, ca?: string): BrowserSocketObserver {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'WebSocket');
  const sockets: BrowserSocketObservation[] = [];
  class InstrumentedSocket extends WebSocket {
    private observation: BrowserSocketObservation;

    send(data: Parameters<WebSocket['send']>[0], cb?: (err?: Error) => void): void;
    send(data: Parameters<WebSocket['send']>[0], options: { mask?: boolean; binary?: boolean; compress?: boolean; fin?: boolean }, cb?: (err?: Error) => void): void;
    send(data: Parameters<WebSocket['send']>[0], optionsOrCallback?: { mask?: boolean; binary?: boolean; compress?: boolean; fin?: boolean } | ((err?: Error) => void), cb?: (err?: Error) => void): void {
      if (this.observation) recordFirstFrame(this.observation, data);
      if (typeof optionsOrCallback === 'function') super.send(data, optionsOrCallback);
      else if (optionsOrCallback) super.send(data, optionsOrCallback, cb);
      else super.send(data, cb);
    }

    constructor(address: string | URL, protocols?: string | string[]) {
      const observation: BrowserSocketObservation = {
        pathname: '',
        protocol: '',
        hasAuthorization: false,
        waitForClose(timeoutMs) {
          if (observation.close) return Promise.resolve(observation.close);
          return new Promise((resolve, reject) => {
            const onClose = (code: number, reason: Buffer): void => {
              clearTimeout(timer);
              thisSocket.removeListener('close', onClose);
              observation.close = { code, reason: reason.toString() };
              resolve(observation.close);
            };
            const timer = setTimeout(() => {
              thisSocket.removeListener('close', onClose);
              reject(new Error('browser WebSocket close deadline exceeded'));
            }, timeoutMs);
            thisSocket.once('close', onClose);
          });
        },
      };
      let thisSocket: InstrumentedSocket;
      super(address, protocols, {
        origin,
        headers: { Host: 'localhost' },
        perMessageDeflate: false,
        maxPayload: 16 * 1024 * 1024,
        ...(ca ? { ca } : {}),
        finishRequest: (request) => {
          observation.hasAuthorization = request.hasHeader('authorization');
          request.end();
        },
      });
      thisSocket = this;
      this.observation = observation;
      observation.pathname = new URL(this.url).pathname;
      this.on('error', () => undefined);
      this.on('open', () => { observation.protocol = this.protocol; });
      this.on('unexpected-response', (_request, response) => {
        observation.unexpectedStatus = response.statusCode;
        response.resume();
        this.terminate();
      });
      this.on('close', (code, reason) => { observation.close = { code, reason: reason.toString() }; });
      sockets.push(observation);
    }
  }
  Object.defineProperty(globalThis, 'WebSocket', { configurable: true, writable: true, value: InstrumentedSocket });
  return {
    sockets,
    restore() {
      if (previous) Object.defineProperty(globalThis, 'WebSocket', previous);
      else Reflect.deleteProperty(globalThis, 'WebSocket');
    },
  };
}
