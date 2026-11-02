import type { TokenHttpRequest, TokenRequestOptions } from './options.js';
import type { TokenTransportError } from './errors.js';

export type TokenResponseMap = Record<string, unknown>;

export interface TokenRequestTransport {
  readonly channel: 'http' | 'ws';
  request(request: TokenHttpRequest, options?: TokenRequestOptions): Promise<TokenResponseMap>;
  close(): Promise<void>;
}

export type TokenFailure = TokenTransportError;
