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

export interface TokenPlatform {
  requestHttp(url: URL, token: import('../../../entry/api-token.js').ApiToken, init: TokenHttpRequestInit): Promise<TokenHttpResponse>;
  close(): Promise<void>;
}

export const DEFAULT_TOKEN_RESPONSE_BYTES = 16 * 1024 * 1024;
