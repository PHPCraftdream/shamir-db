/** Opaque API bearer token; raw credentials never become object properties. */
const tokenValues = new WeakMap<ApiToken, string>();
const NODE_INSPECT = Symbol.for('nodejs.util.inspect.custom');

const BASE64URL = /^[A-Za-z0-9_-]+$/;

function canonicalBase64Url(value: string, length: number, unusedBits: number): boolean {
  if (value.length !== length || !BASE64URL.test(value)) return false;
  const last = value.charCodeAt(value.length - 1);
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  const index = alphabet.indexOf(String.fromCharCode(last));
  return index >= 0 && (index & ((1 << unusedBits) - 1)) === 0;
}

function validToken(value: string): boolean {
  return value.length === 71 && value.startsWith('shk1.') && value[27] === '.' &&
    canonicalBase64Url(value.slice(5, 27), 22, 4) &&
    canonicalBase64Url(value.slice(28), 43, 2);
}

/** Opaque, redacting handle for a canonical `shk1` API token. */
export class ApiToken {
  private constructor(value: string) {
    tokenValues.set(this, value);
  }

  static from(value: string): ApiToken {
    if (typeof value !== 'string' || !validToken(value)) throw new TypeError('Malformed API token');
    return new ApiToken(value);
  }

  toString(): string { return '[REDACTED]'; }
  toJSON(): string { return '[REDACTED]'; }
  toLocaleString(): string { return '[REDACTED]'; }
  [Symbol.toPrimitive](_hint: string): string { return '[REDACTED]'; }
  [NODE_INSPECT](): string { return 'ApiToken [REDACTED]'; }
}

/** Internal transport accessor; deliberately not re-exported by package entries. */
export function tokenValueForTransport(token: ApiToken): string {
  const value = tokenValues.get(token);
  if (value === undefined) throw new TypeError('Invalid API token handle');
  return value;
}
