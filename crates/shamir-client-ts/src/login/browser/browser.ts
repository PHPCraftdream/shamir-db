import { performLogin, type LoginHttp } from '../core.js';
import type { LoginOptions, LoginResult } from '../types.js';
import { LoginError } from '../errors.js';
import { browserLoginCrypto } from './browser-crypto.js';
import { browserLoginHttp } from './browser-http.js';
import { connectTokenBrowser } from '../../platform/connect/browser-connect.js';

export function loginBrowser(
  options: LoginOptions,
  http: LoginHttp = browserLoginHttp,
): Promise<LoginResult> {
  if (options && typeof options === 'object' && options.ca !== undefined) return Promise.reject(new LoginError('invalid_request'));
  return performLogin(options, browserLoginCrypto, http, (token, url, channel, signal, requestTimeoutMs) => {
    return connectTokenBrowser({
      auth: 'token',
      token,
      url: url.href,
      channel,
      ...(signal === undefined ? {} : { signal }),
      ...(requestTimeoutMs === undefined ? {} : { requestTimeoutMs }),
    });
  });
}
