import type { ApiToken } from '../../entry/api-token.js';

export interface TokenHttpRequestInit {
  method: 'GET' | 'POST';
  headers: Readonly<Record<string, string>>;
  body?: Uint8Array;
  signal: AbortSignal;
  redirect: 'error';
  credentials: 'omit';
}

export interface TokenHttpResponse {
  status: number;
  headers: Readonly<{ get(name: string): string | null }>;
  body: Uint8Array;
}

export interface TokenSocketCloseEvent { code: number; reason: string; wasClean: boolean; error?: Error; }
export interface TokenSocket {
  send(data: Uint8Array): void;
  onMessage(handler: (data: Uint8Array) => void): void;
  onClose(handler: (event: TokenSocketCloseEvent) => void): void;
  close(code?: number, reason?: string): Promise<void>;
}
export interface TokenSocketOptions { tls?: import('../../core/types/connection.js').TokenTlsOptions; origin?: string; signal: AbortSignal; connectTimeoutMs: number; }

export interface TokenPlatform {
  requestHttp(url: URL, token: ApiToken, init: TokenHttpRequestInit): Promise<TokenHttpResponse>;
  openTokenSocket(url: URL, token: ApiToken, opts: TokenSocketOptions): Promise<TokenSocket>;
  close(): Promise<void>;
}

export const DEFAULT_TOKEN_RESPONSE_BYTES = 16 * 1024 * 1024;
