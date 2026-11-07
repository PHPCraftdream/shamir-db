import type { LoginHttp } from '../core.js';
import { LoginError } from '../errors.js';

interface Reader {
  read(): Promise<{ done: boolean; value?: Uint8Array }>;
  cancel(reason?: unknown): Promise<void>;
  releaseLock?(): void;
}

interface ResponseLike {
  status: number;
  headers: { get(name: string): string | null };
  body: { getReader(): Reader; cancel?(): Promise<void> } | null;
}

interface Init {
  method: 'POST';
  headers: Readonly<Record<string, string>>;
  body: Uint8Array;
  credentials: 'omit';
  redirect: 'error';
  signal: AbortSignal;
}

type FetchLike = (url: string, init: Init) => Promise<ResponseLike>;

function fetcher(): FetchLike {
  const f = (globalThis as unknown as { fetch?: FetchLike }).fetch;
  if (typeof f !== 'function') throw new LoginError('network_error');
  return f.bind(globalThis);
}

function cancelWithoutWaiting(cancel: () => Promise<void> | void): void {
  try { void Promise.resolve(cancel()).catch(() => {}); } catch { /* Best effort only. */ }
}

export const browserLoginHttp: LoginHttp = {
  async request(url, body, signal, timeout) {
    if (signal.aborted) throw new LoginError('client_aborted');

    const controller = new AbortController();
    let timerFired = false;
    let reader: Reader | undefined;
    let response: ResponseLike | undefined;
    let settled = false;
    let completed = false;
    let chunks: Uint8Array[] = [];
    let size = 0;
    let rejectAbort: (() => void) | undefined;
    const abortWait = new Promise<never>((_, reject) => {
      rejectAbort = () => reject(new LoginError(
        signal.aborted ? 'client_aborted' : timerFired ? 'client_timeout' : 'network_error',
      ));
    });
    const wipeChunks = (): void => { for (const chunk of chunks) chunk.fill(0); chunks = []; };
    const abort = () => {
      controller.abort();
      rejectAbort?.();
      const activeReader = reader;
      const activeBody = response?.body;
      if (activeReader) cancelWithoutWaiting(() => activeReader.cancel());
      else if (activeBody) cancelWithoutWaiting(() => activeBody.cancel?.());
    };
    const onAbort = () => abort();
    signal.addEventListener('abort', onAbort, { once: true });
    const timer = timeout > 0 ? setTimeout(() => { timerFired = true; abort(); }, timeout) : undefined;

    const cancelLateResponse = (pending: Promise<ResponseLike>) => {
      void pending.then((late) => {
        if (settled || controller.signal.aborted) {
          try {
            const lateReader = late.body?.getReader();
            if (lateReader) {
              cancelWithoutWaiting(() => lateReader.cancel());
              try { lateReader.releaseLock?.(); } catch { /* Best effort only. */ }
            } else {
              const lateBody = late.body;
              if (lateBody) cancelWithoutWaiting(() => lateBody.cancel?.());
            }
          } catch { /* Best effort only. */ }
        }
      }, () => {});
    };

    try {
      if (signal.aborted) throw new LoginError('client_aborted');
      const pending = Promise.resolve().then(() => fetcher()(url.href, {
        method: 'POST',
        headers: { 'Content-Type': 'application/msgpack', Accept: 'application/msgpack' },
        body,
        credentials: 'omit',
        redirect: 'error',
        signal: controller.signal,
      }));
      cancelLateResponse(pending);
      response = await Promise.race([pending, abortWait]);
      const status = response.status;
      if (status >= 300 && status < 400) throw new LoginError('protocol_error', undefined, status);
      if (response.headers.get('content-encoding') !== null) {
        throw new LoginError('protocol_error', undefined, status);
      }
      const contentLength = response.headers.get('content-length');
      if (contentLength !== null && /^\d+$/.test(contentLength) && Number(contentLength) > 4096) {
        throw new LoginError('http_body_too_large', undefined, status);
      }

      reader = response.body?.getReader();
      const activeReader = reader;
      if (activeReader) {
        for (;;) {
          const read = Promise.resolve().then(() => activeReader.read()).then((item) => {
            if (settled || controller.signal.aborted) item.value?.fill(0);
            return item;
          });
          const item = await Promise.race([read, abortWait]);
          if (settled || controller.signal.aborted) { item.value?.fill(0); throw new LoginError(signal.aborted ? 'client_aborted' : timerFired ? 'client_timeout' : 'network_error'); }
          if (item.done) { item.value?.fill(0); break; }
          if (!item.value) continue;
          size += item.value.byteLength;
          if (size > 4096) {
            item.value.fill(0);
            cancelWithoutWaiting(() => activeReader.cancel());
            throw new LoginError('http_body_too_large', undefined, status);
          }
          chunks.push(item.value);
        }
      }

      const bytes = new Uint8Array(size);
      let offset = 0;
      try {
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        const contentType = response.headers.get('content-type');
        const retryAfter = response.headers.get('retry-after');
        wipeChunks();
        completed = true;
        return { status, contentType, retryAfter, body: bytes };
      } catch (error) {
        bytes.fill(0);
        throw error;
      }
    } catch (error) {
      if (error instanceof LoginError) throw error;
      throw new LoginError(signal.aborted ? 'client_aborted' : timerFired ? 'client_timeout' : 'network_error');
    } finally {
      settled = true;
      wipeChunks();
      signal.removeEventListener('abort', onAbort);
      if (timer !== undefined) clearTimeout(timer);
      const activeReader = reader;
      const activeBody = response?.body;
      if (activeReader) {
        if (!completed) cancelWithoutWaiting(() => activeReader.cancel());
        try { activeReader.releaseLock?.(); } catch { /* Best effort only. */ }
      } else if (activeBody && !completed) {
        cancelWithoutWaiting(() => activeBody.cancel?.());
      }
    }
  },
};
