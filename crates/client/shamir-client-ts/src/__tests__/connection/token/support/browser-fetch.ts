import { request as httpRequest } from 'node:http';
import { request as httpsRequest, type RequestOptions as HttpsRequestOptions } from 'node:https';

/** An abort error shaped like the DOM one (the `lib` setting has no DOMException). */
function abortError(): Error {
  return Object.assign(new Error('The operation was aborted'), { name: 'AbortError' });
}

interface LoginFetchInit {
  method: 'POST';
  headers: Readonly<Record<string, string>>;
  body: Uint8Array;
  credentials: 'omit';
  redirect: 'error';
  signal: AbortSignal;
}

interface Reader {
  read(): Promise<{ done: boolean; value?: Uint8Array }>;
  cancel(reason?: unknown): Promise<void>;
  releaseLock(): void;
}

interface ResponseLike {
  status: number;
  headers: { get(name: string): string | null };
  body: { getReader(): Reader; cancel(): Promise<void> };
}

export interface BrowserFetchObservation {
  url: string;
  method: string;
  headers: Readonly<Record<string, string | string[] | undefined>>;
}

export function installBrowserFetch(
  onRequest?: (observation: BrowserFetchObservation) => void,
  origin = 'http://localhost',
): { restore(): void } {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'fetch');
  const fetchCompatible = async (input: string | URL, init: LoginFetchInit): Promise<ResponseLike> => {
    const url = new URL(input);
    if ((url.protocol !== 'http:' && url.protocol !== 'https:') || init.redirect !== 'error' || init.credentials !== 'omit') {
      throw new TypeError('Unsupported browser login fetch request');
    }
    if (init.signal.aborted) throw abortError();
    const headers: Record<string, string> = { ...init.headers, Origin: origin };
    onRequest?.({ url: url.href, method: init.method, headers: { ...headers } });

    return await new Promise<ResponseLike>((resolve, reject) => {
      let settled = false;
      let received = 0;
      const chunks: Buffer[] = [];
      let incomingResponse: import('node:http').IncomingMessage | undefined;
      const requestOptions: HttpsRequestOptions = {
        protocol: url.protocol,
        hostname: url.hostname,
        port: url.port || undefined,
        path: `${url.pathname}${url.search}`,
        method: init.method,
        headers: { ...headers, 'Content-Length': Buffer.byteLength(init.body) },
      };
      const request = (url.protocol === 'https:' ? httpsRequest : httpRequest)(requestOptions, (incoming) => {
        incomingResponse = incoming;
        incoming.on('data', (value: Buffer | string) => {
          if (settled) return;
          const chunk = typeof value === 'string' ? Buffer.from(value) : value;
          received += chunk.length;
          if (received > 4096) {
            finish(new Error('HTTP response exceeds 4096 bytes'));
            return;
          }
          chunks.push(Buffer.from(chunk));
        });
        incoming.once('aborted', () => finish(new Error('HTTP response aborted')));
        incoming.once('error', (error) => finish(error));
        incoming.once('close', () => {
          if (!incoming.complete) finish(new Error('HTTP response closed before completion'));
        });
        incoming.once('end', () => {
          if (settled) return;
          const buffer = Buffer.concat(chunks, received);
          chunks.forEach((chunk) => chunk.fill(0));
          chunks.length = 0;
          finish(undefined, {
            status: incoming.statusCode ?? 0,
            headers: { get: (name) => {
              const value = incoming.headers[name.toLowerCase()];
              return Array.isArray(value) ? value.join(', ') : value ?? null;
            } },
            body: makeBody(buffer),
          });
        });
      });
      const timer = setTimeout(() => finish(new Error('HTTP request deadline exceeded')), 10_000);
      const cleanup = (): void => {
        clearTimeout(timer);
        init.signal.removeEventListener('abort', abort);
      };
      const finish = (error?: Error, response?: ResponseLike): void => {
        if (settled) return;
        settled = true;
        cleanup();
        if (error) {
          chunks.forEach((chunk) => chunk.fill(0));
          chunks.length = 0;
          request.destroy();
          incomingResponse?.destroy();
          reject(error);
        } else if (response) {
          resolve(response);
        }
      };
      const abort = (): void => finish(abortError());
      request.once('error', (error) => finish(error));
      request.once('close', () => {
        if (!incomingResponse?.complete) finish(new Error('HTTP request closed before response completed'));
      });
      init.signal.addEventListener('abort', abort, { once: true });
      if (init.signal.aborted) abort();
      else request.end(Buffer.from(init.body));
    });
  };

  Object.defineProperty(globalThis, 'fetch', { configurable: true, enumerable: previous?.enumerable ?? true, writable: true, value: fetchCompatible });
  return {
    restore(): void {
      if (previous) Object.defineProperty(globalThis, 'fetch', previous);
      else Reflect.deleteProperty(globalThis, 'fetch');
    },
  };
}

function makeBody(buffer: Buffer): ResponseLike['body'] {
  let consumed = false;
  let released = false;
  const zero = (): void => {
    buffer.fill(0);
  };
  return {
    getReader(): Reader {
      return {
        async read() {
          if (released) throw new TypeError('Reader lock has been released');
          if (consumed) return { done: true };
          consumed = true;
          const value = new Uint8Array(buffer);
          zero();
          return { done: false, value };
        },
        async cancel() { consumed = true; zero(); },
        releaseLock() { released = true; },
      };
    },
    async cancel() { consumed = true; zero(); },
  };
}

export async function preloadBrowserCrypto(): Promise<void> {
  const platform = (await import('../../../../platform/browser.js')).BrowserPlatform;
  const password = new Uint8Array(32).fill(0x70);
  const salt = new Uint8Array(16).fill(0x73);
  let result: Uint8Array | undefined;
  try {
    result = await platform.argon2id(password, salt, { memoryKb: 19456, time: 2, parallelism: 1 });
  } finally {
    password.fill(0);
    salt.fill(0);
    result?.fill(0);
  }
}
