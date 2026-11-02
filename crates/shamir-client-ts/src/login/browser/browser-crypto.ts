import type { Argon2Params } from '../../core/support/platform.js';
import { BrowserPlatform } from '../../platform/browser.js';
import type { LoginCrypto } from '../core.js';

interface SubtleLike {
  digest(algorithm: string, data: Uint8Array | ArrayBuffer): Promise<ArrayBuffer>;
  importKey(
    format: 'raw',
    keyData: Uint8Array | ArrayBuffer,
    algorithm: { name: string; hash: string },
    extractable: boolean,
    keyUsages: readonly string[],
  ): Promise<unknown>;
  sign(algorithm: string | { name: string }, key: unknown, data: Uint8Array | ArrayBuffer): Promise<ArrayBuffer>;
}

interface CryptoLike {
  getRandomValues<T extends ArrayBufferView>(array: T): T;
  subtle: SubtleLike;
}

function webCrypto(): CryptoLike {
  const value = (globalThis as unknown as { crypto?: CryptoLike }).crypto;
  if (!value?.subtle || typeof value.getRandomValues !== 'function') {
    throw new Error('WebCrypto unavailable');
  }
  return value;
}

export const browserLoginCrypto: LoginCrypto = {
  randomBytes(n) {
    const bytes = new Uint8Array(n);
    webCrypto().getRandomValues(bytes);
    return bytes;
  },
  async sha256(data) {
    return new Uint8Array(await webCrypto().subtle.digest('SHA-256', data));
  },
  async hmac(keyBytes, data) {
    const subtle = webCrypto().subtle;
    const key = await subtle.importKey('raw', keyBytes, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
    return new Uint8Array(await subtle.sign('HMAC', key, data));
  },
  equal(a, b) {
    return a.length === b.length && BrowserPlatform.timingSafeEqual(a, b);
  },
  argon2id(password, salt, params: Argon2Params) {
    return BrowserPlatform.argon2id(password, salt, params);
  },
};
