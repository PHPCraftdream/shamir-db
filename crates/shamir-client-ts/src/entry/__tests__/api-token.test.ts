import { describe, expect, it } from 'vitest';
import { inspect } from 'node:util';
import { ApiToken, tokenValueForTransport } from '../api-token.js';

const canonicalToken = (): string => `shk1.${'A'.repeat(21)}A.${'A'.repeat(42)}A`;

function rejected(value: unknown): TypeError {
  try {
    ApiToken.from(value as string);
  } catch (error) {
    expect(error).toBeInstanceOf(TypeError);
    return error as TypeError;
  }
  throw new Error('expected malformed token to be rejected');
}

describe('ApiToken', () => {
  it('accepts only the canonical shk1 length, prefix, alphabet, and unpadded shape', () => {
    const value = canonicalToken();
    expect(value).toHaveLength(71);
    expect(ApiToken.from(value)).toBeInstanceOf(ApiToken);
    for (const bad of [
      value.slice(1), `x${value.slice(1)}`, `${value}=`, value.replace('.', '/'),
      value.slice(0, 26) + 'B' + value.slice(27),
      value.slice(0, 70) + 'B',
    ]) expect(() => ApiToken.from(bad)).toThrow(TypeError);
  });

  it('rejects noncanonical id and secret trailing bits', () => {
    const good = canonicalToken();
    const badId = `${good.slice(0, 26)}B${good.slice(27)}`;
    const badSecret = `${good.slice(0, 70)}B`;
    expect(() => ApiToken.from(badId)).toThrow(TypeError);
    expect(() => ApiToken.from(badSecret)).toThrow(TypeError);
  });

  it('rejects huge and non-string runtime inputs without leaking their contents', () => {
    const secret = canonicalToken();
    const huge = `shk1.${'A'.repeat(100_000)}.${secret}`;
    const hugeError = rejected(huge);
    expect(`${hugeError.message}\n${hugeError.stack ?? ''}`).not.toContain(secret);
    expect(`${hugeError.message}\n${hugeError.stack ?? ''}`).not.toContain(huge);
    const nonString = { credential: secret };
    const objectError = rejected(nonString);
    expect(`${objectError.message}\n${objectError.stack ?? ''}`).not.toContain(secret);
  });

  it('redacts coercion and inspection without exposing credential via own keys', () => {
    const value = canonicalToken();
    const token = ApiToken.from(value);
    expect(String(token)).toBe('[REDACTED]');
    expect(`${token}`).toBe('[REDACTED]');
    expect(JSON.stringify(token)).toBe('"[REDACTED]"');
    expect(token.toLocaleString()).toBe('[REDACTED]');
    expect(token[Symbol.toPrimitive]('string')).toBe('[REDACTED]');
    expect(inspect(token)).toBe('ApiToken [REDACTED]');
    expect(Object.keys(token)).toEqual([]);
    expect(Object.getOwnPropertySymbols(token)).toEqual([]);
    expect(Reflect.ownKeys(token)).toEqual([]);
    expect([String(token), `${token}`, JSON.stringify(token), token.toLocaleString(), inspect(token)].join('|')).not.toContain(value);
  });

  it('provides the original canonical value only through the internal transport accessor', () => {
    const value = canonicalToken();
    expect(tokenValueForTransport(ApiToken.from(value))).toBe(value);
    const forged = Object.create(ApiToken.prototype) as ApiToken;
    const error = (() => {
      try { tokenValueForTransport(forged); }
      catch (caught) { return caught; }
      throw new Error('expected forged handle rejection');
    })();
    expect(error).toBeInstanceOf(TypeError);
    expect((error as TypeError).message).not.toContain(value);
    expect((error as TypeError).stack ?? '').not.toContain(value);
  });
});
