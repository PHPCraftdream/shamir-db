import { performLogin, type LoginHttp } from '../core.js';
import type { LoginOptions, LoginResult } from '../types.js';
import { nodeLoginCrypto } from './node-crypto.js';
import { nodeLoginHttp } from './node-http.js';
import { connectTokenNode } from '../../platform/connect/node-connect.js';

export function loginNode(
  options: LoginOptions,
  http: LoginHttp = nodeLoginHttp,
): Promise<LoginResult> {
  return performLogin(options, nodeLoginCrypto, http, (token, url, channel, signal, requestTimeoutMs) => {
    return connectTokenNode({
      auth: 'token',
      token,
      url: url.href,
      channel,
      ...(signal === undefined ? {} : { signal }),
      ...(requestTimeoutMs === undefined ? {} : { requestTimeoutMs }),
    });
  });
}
