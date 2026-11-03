import { performLogin, type LoginHttp } from '../core.js';
import type { LoginOptions, LoginResult } from '../types.js';
import { nodeLoginCrypto } from './node-crypto.js';
import { nodeLoginHttp } from './node-http.js';
import { connectTokenNode } from '../../platform/connect/node-connect.js';

export function loginNode(
  options: LoginOptions,
  http: LoginHttp = nodeLoginHttp,
): Promise<LoginResult> {
  const ca = options && typeof options === 'object' && options.ca instanceof Uint8Array
    ? new Uint8Array(options.ca)
    : options?.ca;
  return performLogin(options, nodeLoginCrypto, http, (token, url, channel, signal, requestTimeoutMs) => {
    return connectTokenNode({
      auth: 'token',
      token,
      url: url.href,
      channel,
      ...(ca === undefined ? {} : { tls: { ca } }),
      ...(signal === undefined ? {} : { signal }),
      ...(requestTimeoutMs === undefined ? {} : { requestTimeoutMs }),
    });
  });
}
