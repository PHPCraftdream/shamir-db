import { randomBytes, createHash, createHmac, timingSafeEqual } from 'node:crypto';
import type { Argon2Params } from '../../core/support/platform.js';
import { NodePlatform } from '../../platform/node.js';
import type { LoginCrypto } from '../core.js';

export const nodeLoginCrypto: LoginCrypto = {
  randomBytes(n) {
    return new Uint8Array(randomBytes(n));
  },
  async sha256(data) {
    return new Uint8Array(createHash('sha256').update(data).digest());
  },
  async hmac(key, data) {
    return new Uint8Array(createHmac('sha256', key).update(data).digest());
  },
  equal(a, b) {
    return a.length === b.length && timingSafeEqual(a, b);
  },
  argon2id(password, salt, params: Argon2Params) {
    return NodePlatform.argon2id(password, salt, params);
  },
};
