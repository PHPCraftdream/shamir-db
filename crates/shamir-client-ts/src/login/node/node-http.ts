import { request as nodeRequest } from 'node:http';
import { request as tlsRequest } from 'node:https';
import type { ClientRequest, IncomingMessage, RequestOptions } from 'node:http';
import type { RequestOptions as HttpsRequestOptions } from 'node:https';
import type { ConnectionOptions } from 'node:tls';
import type { LoginHttp } from '../core.js';
import { LoginError } from '../errors.js';

const MAX_BODY_BYTES = 4096;
type LoginResponse = { status: number; contentType: string | null; retryAfter: string | null; body: Uint8Array };
type Requester = (url: URL, options: RequestOptions | HttpsRequestOptions, callback: (response: IncomingMessage) => void) => ClientRequest;

export function createNodeLoginHttp(requestHttp: Requester = nodeRequest, requestTls: Requester = tlsRequest): LoginHttp {
  return {
    request(url, body, signal, timeout, ca) {
      return new Promise<LoginResponse>((resolve, reject) => {
        let settled = false;
        let responseComplete = false;
        let timer: ReturnType<typeof setTimeout> | undefined;
        let req: ClientRequest | undefined;
        const chunks: Uint8Array[] = [];
        const wipeChunks = (): void => { for (const chunk of chunks) chunk.fill(0); chunks.length = 0; };
        const finish = (error?: LoginError, result?: LoginResponse): void => {
          if (settled) { if (result) result.body.fill(0); return; }
          settled = true;
          if (error) wipeChunks();
          if (timer !== undefined) clearTimeout(timer);
          signal.removeEventListener('abort', abort);
          if (error) reject(error); else if (result) resolve(result);
        };
        const abort = (): void => { finish(new LoginError(signal.aborted ? 'client_aborted' : 'client_timeout')); req?.destroy(); };
        if (signal.aborted) { finish(new LoginError('client_aborted')); return; }

        const onResponse = (response: IncomingMessage): void => {
          let size = 0;
          const status = response.statusCode ?? 0;
          response.on('data', (chunk: Buffer | string) => {
            if (typeof chunk === 'string') {
              const bytes = Buffer.from(chunk);
              if (settled) { bytes.fill(0); return; }
              if (size + bytes.length > MAX_BODY_BYTES) { bytes.fill(0); rejectResponse(new LoginError('http_body_too_large', undefined, status)); return; }
              chunks.push(bytes); size += bytes.length; return;
            }
            const bytes = new Uint8Array(chunk);
            chunk.fill(0);
            if (settled) { bytes.fill(0); return; }
            if (size + bytes.length > MAX_BODY_BYTES) { bytes.fill(0); rejectResponse(new LoginError('http_body_too_large', undefined, status)); return; }
            chunks.push(bytes); size += bytes.length;
          });
          response.on('error', () => finish(new LoginError('network_error')));
          response.on('aborted', () => finish(new LoginError('network_error')));
          response.on('close', () => { if (!responseComplete) finish(new LoginError('network_error')); });
          const rejectResponse = (error: LoginError): void => { finish(error); response.destroy(); req?.destroy(); };
          if (settled) { response.destroy(); return; }
          if (status >= 300 && status < 400) { rejectResponse(new LoginError('protocol_error', undefined, status)); return; }
          if (response.headers['content-encoding'] !== undefined) { rejectResponse(new LoginError('protocol_error', undefined, status)); return; }
          const contentLength = response.headers['content-length'];
          if (contentLength !== undefined && (Array.isArray(contentLength) ? contentLength : [contentLength]).some(value => /^\s*\d+\s*$/.test(value) && Number(value) > MAX_BODY_BYTES)) {
            rejectResponse(new LoginError('http_body_too_large', undefined, status)); return;
          }
          response.on('end', () => {
            if (settled) return;
            responseComplete = true;
            const resultBody = new Uint8Array(size);
            let offset = 0;
            try {
              for (const chunk of chunks) { resultBody.set(chunk, offset); offset += chunk.length; }
              const result = { status, contentType: typeof response.headers['content-type'] === 'string' ? response.headers['content-type'] : null,
                retryAfter: typeof response.headers['retry-after'] === 'string' ? response.headers['retry-after'] : null, body: resultBody };
              wipeChunks(); finish(undefined, result);
            } catch { resultBody.fill(0); rejectResponse(new LoginError('network_error')); }
          });
        };

        const options: RequestOptions = { method: 'POST', headers: { 'Content-Type': 'application/msgpack', 'Content-Length': String(body.length), Accept: 'application/msgpack', Connection: 'close' }, agent: false };
        try {
          if (url.protocol === 'https:') {
            const tlsOptions: HttpsRequestOptions & Pick<ConnectionOptions, 'ALPNProtocols'> = {
              ...options, minVersion: 'TLSv1.3', ALPNProtocols: ['http/1.1'], rejectUnauthorized: true,
              ...(ca === undefined ? {} : { ca: typeof ca === 'string' ? ca : Buffer.from(ca) }),
            };
            req = requestTls(url, tlsOptions, onResponse);
          } else if (url.protocol === 'http:') req = requestHttp(url, options, onResponse);
          else { finish(new LoginError('protocol_error')); return; }
          req.on('error', () => finish(new LoginError('network_error')));
          if (settled) { req.destroy(); return; }
          req.on('close', () => { if (!settled && !responseComplete) finish(new LoginError('network_error')); });
          signal.addEventListener('abort', abort, { once: true });
          if (signal.aborted) { abort(); return; }
          timer = timeout ? setTimeout(() => { finish(new LoginError('client_timeout')); req?.destroy(); }, timeout) : undefined;
          req.end(body);
        } catch { finish(new LoginError('network_error')); req?.destroy(); }
      });
    },
  };
}

export const nodeLoginHttp: LoginHttp = createNodeLoginHttp();
