import { request as nodeRequest } from 'node:http';
import { request as tlsRequest } from 'node:https';
import type { IncomingMessage, RequestOptions } from 'node:http';
import type { LoginHttp } from '../core.js';
import { LoginError } from '../errors.js';

const MAX_BODY_BYTES = 4096;

type LoginResponse = {
  status: number;
  contentType: string | null;
  retryAfter: string | null;
  body: Uint8Array;
};

export const nodeLoginHttp: LoginHttp = {
  request(url, body, signal, timeout) {
    return new Promise<LoginResponse>((resolve, reject) => {
      let settled = false;
      let responseComplete = false;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let req: ReturnType<typeof nodeRequest> | undefined;

      const finish = (error?: LoginError, result?: LoginResponse): void => {
        if (settled) return;
        settled = true;
        if (timer !== undefined) clearTimeout(timer);
        signal.removeEventListener('abort', abort);
        if (error) reject(error);
        else if (result) resolve(result);
      };

      const abort = (): void => {
        const error = new LoginError(signal.aborted ? 'client_aborted' : 'client_timeout');
        finish(error);
        req?.destroy();
      };

      if (signal.aborted) {
        finish(new LoginError('client_aborted'));
        return;
      }

      const onResponse = (response: IncomingMessage): void => {
        const chunks: Uint8Array[] = [];
        let size = 0;
        const status = response.statusCode ?? 0;
        const rejectResponse = (error: LoginError): void => {
          finish(error);
          response.destroy();
          req?.destroy();
        };

        response.on('error', () => finish(new LoginError('network_error')));
        response.on('aborted', () => finish(new LoginError('network_error')));

        if (status >= 300 && status < 400) {
          rejectResponse(new LoginError('protocol_error', undefined, status));
          return;
        }
        if (response.headers['content-encoding'] !== undefined) {
          rejectResponse(new LoginError('protocol_error', undefined, status));
          return;
        }
        const contentLength = response.headers['content-length'];
        if (contentLength !== undefined) {
          const values = Array.isArray(contentLength) ? contentLength : [contentLength];
          if (values.some((value) => /^\s*\d+\s*$/.test(value) && Number(value) > MAX_BODY_BYTES)) {
            rejectResponse(new LoginError('http_body_too_large', undefined, status));
            return;
          }
        }

        response.on('data', (chunk: Buffer | string) => {
          if (settled) return;
          const bytes = typeof chunk === 'string' ? Buffer.from(chunk) : chunk;
          if (size + bytes.length > MAX_BODY_BYTES) {
            rejectResponse(new LoginError('http_body_too_large', undefined, status));
            return;
          }
          chunks.push(new Uint8Array(bytes));
          size += bytes.length;
        });
        response.on('end', () => {
          if (settled) return;
          responseComplete = true;
          const resultBody = new Uint8Array(size);
          let offset = 0;
          for (const chunk of chunks) {
            resultBody.set(chunk, offset);
            offset += chunk.length;
            chunk.fill(0);
          }
          finish(undefined, {
            status,
            contentType: typeof response.headers['content-type'] === 'string' ? response.headers['content-type'] : null,
            retryAfter: typeof response.headers['retry-after'] === 'string' ? response.headers['retry-after'] : null,
            body: resultBody,
          });
        });
        response.on('aborted', () => finish(new LoginError('network_error')));
        response.on('error', () => finish(new LoginError('network_error')));
        response.on('close', () => {
          if (!responseComplete) finish(new LoginError('network_error'));
        });
      };

      const options: RequestOptions = {
        method: 'POST',
        headers: {
          'Content-Type': 'application/msgpack',
          'Content-Length': String(body.length),
          Accept: 'application/msgpack',
          Connection: 'close',
        },
        agent: false,
      };

      try {
        if (url.protocol === 'https:') {
          req = tlsRequest(url, {
            ...options,
            minVersion: 'TLSv1.3',
            ALPNProtocols: ['http/1.1'],
          }, onResponse);
        } else if (url.protocol === 'http:') {
          req = nodeRequest(url, options, onResponse);
        } else {
          finish(new LoginError('protocol_error'));
          return;
        }

        req.on('error', () => finish(new LoginError('network_error')));
        req.on('close', () => {
          if (!settled && !responseComplete) finish(new LoginError('network_error'));
        });
        signal.addEventListener('abort', abort, { once: true });
        if (signal.aborted) {
          abort();
          return;
        }
        timer = timeout ? setTimeout(() => {
          finish(new LoginError('client_timeout'));
          req?.destroy();
        }, timeout) : undefined;
        req.end(body);
      } catch {
        finish(new LoginError('network_error'));
        req?.destroy();
      }
    });
  },
};
