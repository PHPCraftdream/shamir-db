import { encode as encodeWire } from '@msgpack/msgpack';
import { LoginError } from './errors.js';
import type { LoginScope } from './types.js';
import { validateKdfParams } from '../core/protocol/scram.js';

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const DOMAIN = encoder.encode('SHAMIR-HTTP-LOGIN-v1');
const DATABASE_SYNTAX = /^[A-Za-z0-9_-]+$/;

export interface Scope {
  label: string;
  mode: 'read_only' | 'read_write';
  channels: 'rest' | 'ws' | 'both';
  databases: readonly string[] | null;
}

export interface Challenge {
  security: number;
  listenerId: Uint8Array;
  handshakeId: Uint8Array;
  salt: Uint8Array;
  serverNonce: Uint8Array;
  memoryKb: number;
  time: number;
  parallelism: number;
}

function invalidRequest(): never {
  throw new LoginError('invalid_request');
}

function protocolError(): never {
  throw new LoginError('protocol_error');
}

function concat(parts: readonly Uint8Array[]): Uint8Array {
  const length = parts.reduce((total, part) => total + part.length, 0);
  const output = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    output.set(part, offset);
    offset += part.length;
  }
  return output;
}

function uintBytes(value: number | bigint, width: number): Uint8Array {
  let integer: bigint;
  try {
    integer = typeof value === 'bigint' ? value : BigInt(value);
  } catch {
    return protocolError();
  }
  if (integer < 0n || integer >= 1n << BigInt(width * 8)) return protocolError();
  const bytes = new Uint8Array(width);
  for (let index = width - 1; index >= 0; index--) {
    bytes[index] = Number(integer & 0xffn);
    integer >>= 8n;
  }
  return bytes;
}

function validText(value: unknown, minimum: number, maximum: number): Uint8Array {
  if (typeof value !== 'string') return invalidRequest();
  const bytes = encoder.encode(value);
  if (bytes.length < minimum || bytes.length > maximum) return invalidRequest();
  try {
    if (decoder.decode(bytes) !== value) return invalidRequest();
  } catch {
    return invalidRequest();
  }
  return bytes;
}

export function canonicalUser(value: string): string {
  // Partial UsernameCaseMapped compatibility: width-map only characters whose
  // compatibility decomposition is explicitly width-related. This uses the
  // runtime Unicode tables, not a pinned Unicode 15.1 PRECIS implementation.
  const widthMapped = value.replace(/[\u3000\uFF01-\uFFEE]/gu, (character) => {
    const codePoint = character.codePointAt(0)!;
    if (codePoint === 0x3000 || (codePoint >= 0xff01 && codePoint <= 0xffee)) {
      return character.normalize('NFKC');
    }
    return character;
  });
  // Normalize contiguous halfwidth kana together so dakuten/handakuten compose.
  const mappedWidth = widthMapped.replace(/[\uFF61-\uFF9F]+/gu, (run) => run.normalize('NFKC'));
  // RFC 8265 order: the class check follows width mapping and precedes case mapping and NFC.
  checkIdentifierClass(mappedWidth);
  // Per code point, so the context-sensitive final-sigma rule never applies.
  const mapped = Array.from(mappedWidth, (character) => character.toLowerCase()).join('').normalize('NFC');
  const bytes = validText(mapped, 1, 255);
  checkIdentifierClass(mapped);
  return decoder.decode(bytes);
}

const IDENTIFIER_LETTER_DIGIT = /^[\p{Ll}\p{Lu}\p{Lo}\p{Lm}\p{Mn}\p{Mc}\p{Nd}]$/u;
// Conjoining jamo, RFC 5892 exceptions, right-to-left scripts (bidi rule unsupported) and ignorables.
const IDENTIFIER_FORBIDDEN =
  /[\u1100-\u11FF\uA960-\uA97F\uD7B0-\uD7FF\u302E\u302F\u3031-\u3035\u303B\u0590-\u08FF\uFB1D-\uFDFF\uFE70-\uFEFF\u{10800}-\u{10FFF}\u{1E800}-\u{1EFFF}]|\p{Default_Ignorable_Code_Point}/u;

// Partial UsernameCaseMapped compatibility, not full PRECIS: the runtime Unicode tables are not pinned
// and right-to-left names are rejected instead of following the bidi rule. Both fail safe.
function checkIdentifierClass(text: string): void {
  for (const character of text) {
    const codePoint = character.codePointAt(0)!;
    if (codePoint < 0x80) {
      if (codePoint < 0x21 || codePoint > 0x7e) return invalidRequest();
      continue;
    }
    if (codePoint === 0x3007) continue;
    if (
      !IDENTIFIER_LETTER_DIGIT.test(character) ||
      character.normalize('NFKC') !== character ||
      IDENTIFIER_FORBIDDEN.test(character)
    ) {
      return invalidRequest();
    }
  }
}

function byteCompare(left: string, right: string): number {
  const a = encoder.encode(left);
  const b = encoder.encode(right);
  for (let index = 0; index < Math.min(a.length, b.length); index++) {
    if (a[index] !== b[index]) return a[index] - b[index];
  }
  return a.length - b.length;
}

export function freezeScope(input: LoginScope = {}): Scope {
  if (!input || typeof input !== 'object' || Array.isArray(input) ||
      Object.keys(input).some((key) => !['label', 'mode', 'channels', 'databases'].includes(key))) {
    return invalidRequest();
  }
  if (input.label !== undefined && typeof input.label !== 'string') return invalidRequest();
  if (input.mode !== undefined && input.mode !== 'read_only' && input.mode !== 'read_write') return invalidRequest();
  if (input.channels !== undefined && input.channels !== 'rest' && input.channels !== 'ws' && input.channels !== 'both') return invalidRequest();
  const label = input.label ?? 'http-login';
  const labelBytes = validText(label, 1, 128);
  if (/[\u0000-\u001f\u007f]/u.test(label)) return invalidRequest();

  const suppliedDatabases = input.databases === undefined ? null : input.databases;
  if (suppliedDatabases !== null &&
      (!Array.isArray(suppliedDatabases) || suppliedDatabases.length < 1 || suppliedDatabases.length > 4)) {
    return invalidRequest();
  }
  const databases = suppliedDatabases === null ? null : [...suppliedDatabases];
  let scopeLength = 5 + labelBytes.length;
  let previous: string | undefined;
  for (const database of databases ?? []) {
    const bytes = validText(database, 1, 255);
    if (database !== database.normalize('NFC') || !DATABASE_SYNTAX.test(database) ||
        (previous !== undefined && byteCompare(previous, database) >= 0)) {
      return invalidRequest();
    }
    previous = database;
    scopeLength += 2 + bytes.length;
  }
  if (scopeLength > 512) return invalidRequest();

  const mode = input.mode ?? 'read_only';
  const channels = input.channels ?? 'rest';
  if (mode !== 'read_only' && mode !== 'read_write') return invalidRequest();
  if (channels !== 'rest' && channels !== 'ws' && channels !== 'both') return invalidRequest();
  return Object.freeze({
    label,
    mode,
    channels,
    databases: databases === null ? null : Object.freeze(databases),
  });
}

export function encodeScope(scope: Scope): Uint8Array {
  const label = encoder.encode(scope.label);
  const parts: Uint8Array[] = [
    uintBytes(label.length, 2),
    label,
    Uint8Array.of(scope.mode === 'read_only' ? 0 : 1),
    Uint8Array.of(scope.channels === 'rest' ? 0 : scope.channels === 'ws' ? 1 : 2),
    Uint8Array.of(scope.databases === null ? 255 : scope.databases.length),
  ];
  for (const database of scope.databases ?? []) {
    const bytes = encoder.encode(database);
    parts.push(uintBytes(bytes.length, 2), bytes);
  }
  return concat(parts);
}

function checkedUint(value: number, maximum: number): boolean {
  return Number.isSafeInteger(value) && value >= 0 && value <= maximum;
}

export function buildLoginTranscript(
  user: string,
  clientNonce: Uint8Array,
  challenge: Challenge,
  kdfVersion: number,
  scope: Scope,
): Uint8Array {
  const userBytes = validText(user, 1, 255);
  const scopeBytes = encodeScope(freezeScope(scope));
  try {
    validateKdfParams({ memoryKb: challenge.memoryKb, time: challenge.time, parallelism: challenge.parallelism, argon2Version: 19 });
  } catch {
    return protocolError();
  }
  if (canonicalUser(user) !== user || clientNonce.length !== 32 ||
      challenge.listenerId.length !== 16 || challenge.handshakeId.length !== 32 ||
      challenge.salt.length !== 16 || challenge.serverNonce.length !== 32 ||
      clientNonce.every((byte) => byte === 0) || challenge.serverNonce.every((byte) => byte === 0) ||
      clientNonce.every((byte, index) => byte === challenge.serverNonce[index]) ||
      !checkedUint(challenge.security, 2) ||
      !checkedUint(kdfVersion, 0xff) || kdfVersion !== 19 || scopeBytes.length < 6 || scopeBytes.length > 512) {
    return protocolError();
  }
  return concat([
    DOMAIN,
    Uint8Array.of(1, 3, challenge.security),
    challenge.listenerId,
    challenge.handshakeId,
    uintBytes(userBytes.length, 2),
    userBytes,
    clientNonce,
    challenge.serverNonce,
    challenge.salt,
    uintBytes(challenge.memoryKb, 4),
    uintBytes(challenge.time, 4),
    uintBytes(challenge.parallelism, 4),
    Uint8Array.of(kdfVersion),
    scopeBytes,
  ]);
}

class BoundedReader {
  private offset = 0;
  private depth = 0;

  constructor(private readonly bytes: Uint8Array) {}

  private take(length: number): Uint8Array {
    if (!Number.isSafeInteger(length) || length < 0 || length > this.bytes.length - this.offset) {
      return protocolError();
    }
    const result = this.bytes.subarray(this.offset, this.offset + length);
    this.offset += length;
    return result;
  }

  private unsigned(width: number): number | bigint {
    if (width === 8) {
      let value = 0n;
      for (const byte of this.take(width)) value = (value << 8n) | BigInt(byte);
      return value;
    }
    let value = 0;
    for (const byte of this.take(width)) value = value * 256 + byte;
    if (!Number.isSafeInteger(value)) return protocolError();
    return value;
  }

  private text(length: number): string {
    try {
      return decoder.decode(this.take(length));
    } catch {
      return protocolError();
    }
  }

  private value(): unknown {
    if (++this.depth > 32) return protocolError();
    try {
      const marker = this.take(1)[0];
      if (marker <= 0x7f) return marker;
      if (marker >= 0xe0) return protocolError();
      if ((marker & 0xe0) === 0xa0) return this.text(marker & 0x1f);
      if ((marker & 0xf0) === 0x90) return this.array(marker & 0x0f);
      if ((marker & 0xf0) === 0x80) return this.map(marker & 0x0f);
      if (marker === 0xc0) return null;
      if (marker === 0xc2) return false;
      if (marker === 0xc3) return true;
      switch (marker) {
        case 0xc4: return new Uint8Array(this.take(this.unsigned(1) as number));
        case 0xc5: return new Uint8Array(this.take(this.unsigned(2) as number));
        case 0xc6: return new Uint8Array(this.take(this.unsigned(4) as number));
        case 0xcc: return this.unsigned(1);
        case 0xcd: return this.unsigned(2);
        case 0xce: return this.unsigned(4);
        case 0xcf: return this.unsigned(8);
        case 0xd9: return this.text(this.unsigned(1) as number);
        case 0xda: return this.text(this.unsigned(2) as number);
        case 0xdb: return this.text(this.unsigned(4) as number);
        case 0xdc: return this.array(this.unsigned(2) as number);
        case 0xdd: return this.array(this.unsigned(4) as number);
        case 0xde: return this.map(this.unsigned(2) as number);
        case 0xdf: return this.map(this.unsigned(4) as number);
        default: return protocolError();
      }
    } finally {
      this.depth--;
    }
  }

  private array(length: number): unknown[] {
    if (length > 4096) return protocolError();
    const values: unknown[] = [];
    for (let index = 0; index < length; index++) values.push(this.value());
    return values;
  }

  private map(length: number): Record<string, unknown> {
    if (length > 4096) return protocolError();
    const result: Record<string, unknown> = Object.create(null) as Record<string, unknown>;
    for (let index = 0; index < length; index++) {
      const key = this.value();
      if (typeof key !== 'string' || Object.hasOwn(result, key)) return protocolError();
      result[key] = this.value();
    }
    return result;
  }

  decode(): Record<string, unknown> {
    const value = this.value();
    if (!value || typeof value !== 'object' || value instanceof Uint8Array || Array.isArray(value) ||
        this.offset !== this.bytes.length) {
      return protocolError();
    }
    return value as Record<string, unknown>;
  }
}

export function decodeMessage(bytes: Uint8Array): Record<string, unknown> {
  if (!(bytes instanceof Uint8Array) || bytes.length > 4096) return protocolError();
  try {
    return new BoundedReader(bytes).decode();
  } catch {
    return protocolError();
  }
}

export function encodeMessage(value: unknown): Uint8Array {
  try {
    return encodeWire(value, { useBigInt64: true });
  } catch {
    return protocolError();
  }
}
